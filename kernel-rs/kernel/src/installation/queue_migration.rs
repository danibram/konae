use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Deployment;

const BACKUP_ROOT: &str = ".konae-queue-migration";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub calls_rows: usize,
    pub mail_rows: usize,
    pub affected_apps: Vec<String>,
    pub consumers_paused: bool,
    pub blockers: Vec<String>,
    pub backup: Option<String>,
    pub mail_key_fingerprint: Option<String>,
    pub notes: Vec<String>,
}

/// Reads installed releases without upgrading app settings during preflight.
pub(crate) fn deployments(data_root: &Path, public_origin: &str) -> Result<Vec<Deployment>> {
    let apps = crate::layout::apps(data_root);
    if !apps.is_dir() {
        return Ok(Vec::new());
    }
    let mut deployments = Vec::new();
    for entry in std::fs::read_dir(apps)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let path = crate::layout::record(data_root, &id);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        };
        let installed: crate::Installed = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not a Deployment record.", path.display()))?;
        let settings = crate::Settings::read(data_root, &id)?;
        ensure!(
            settings.id == id,
            "App folder {id} declares a different app ID."
        );
        deployments.push(settings.deployment(installed, public_origin));
    }
    deployments.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(deployments)
}

pub fn preflight(data_root: &Path, deployments: &[Deployment]) -> Result<Preflight> {
    let calls_path = data_root.join(crate::calls::CALLS_DATABASE);
    let mut calls_rows = 0;
    let mut affected_apps = Vec::new();
    let mut consumers_paused = true;
    let mut blockers = Vec::new();
    let mut notes = Vec::new();
    if calls_path.is_file() {
        let db =
            Connection::open_with_flags(&calls_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if table_exists(&db, "calls")? {
            calls_rows = db.query_row("SELECT count(*) FROM calls", [], |row| {
                row.get::<_, usize>(0)
            })?;
            let mut stmt =
                db.prepare("SELECT DISTINCT deployment FROM calls ORDER BY deployment")?;
            affected_apps = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for app in &affected_apps {
                let Some(deployment) = deployments.iter().find(|deployment| &deployment.id == app)
                else {
                    blockers.push(format!("Legacy Calls references app {app}, which this Kernel does not serve. Keep the source row and restore that app's previous release before retrying."));
                    consumers_paused = false;
                    continue;
                };
                consumers_paused &= crate::maintenance::pause(data_root, app).is_some();
                if let Err(error) = crate::calls::preflight_legacy_calls(data_root, deployment) {
                    blockers.push(format!("App {app}: {error:#}"));
                }
            }
        }
    }

    let mail_path = data_root.join(crate::mail::MAIL_DATABASE);
    let mut mail_rows = 0;
    let mut key_fingerprint = None;
    if mail_path.is_file() {
        let db =
            Connection::open_with_flags(&mail_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let user_tables: usize = db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            user_tables == 0 || table_exists(&db, "jobs")?,
            "mail.sqlite has tables but no supported jobs queue. Preserve it and use the Kernel release that owns its format."
        );
        if table_exists(&db, "jobs")? {
            let columns = [
                "id",
                "purpose",
                "recipient",
                "scope",
                "token_hash",
                "link_nonce",
                "link_sealed",
                "state",
                "attempts",
                "next_attempt_at",
                "created_at",
                "updated_at",
            ];
            for column in columns {
                ensure!(
                    has_column(&db, "jobs", column)?,
                    "mail.sqlite uses an unsupported jobs format (missing {column}). Preserve the backup and run the matching previous Kernel to drain or upgrade this queue."
                );
            }
            mail_rows = db.query_row("SELECT count(*) FROM jobs", [], |row| {
                row.get::<_, usize>(0)
            })?;
            if db.query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE link_sealed IS NOT NULL)",
                [],
                |row| row.get::<_, bool>(0),
            )? {
                let secret = migration_secret(data_root)?;
                crate::mail::validate_migration_key(&db, &secret)?;
                key_fingerprint =
                    Some(hex::encode(Sha256::digest(secret.as_bytes()))[..12].to_owned());
            } else {
                notes.push("mail.sqlite has no sealed links; no mail key was needed".to_owned());
            }
        }
    }
    Ok(Preflight {
        calls_rows,
        mail_rows,
        affected_apps,
        consumers_paused,
        blockers,
        backup: None,
        mail_key_fingerprint: key_fingerprint,
        notes,
    })
}

pub fn migrate(data_root: &Path, deployments: &[Deployment]) -> Result<Preflight> {
    let mut report = preflight(data_root, deployments)?;
    ensure!(
        report.blockers.is_empty(),
        "Queue preflight found blockers: {}",
        report.blockers.join("; ")
    );
    ensure!(
        report.consumers_paused,
        "Stop queue consumers and put every affected app into maintenance, then rerun preflight and migration."
    );
    let backup = create_backup(data_root)?;
    report.backup = Some(backup.display().to_string());
    let mut converted = 0usize;
    for app in &report.affected_apps {
        let deployment = deployments
            .iter()
            .find(|deployment| &deployment.id == app)
            .context("Preflight app disappeared")?;
        converted += crate::calls::migrate_legacy_calls_explicit(data_root, deployment)
            .with_context(|| format!("Queue migration stopped. The backup remains at {}. Correct the reported source issue, then rerun preflight and migration.", backup.display()))?;
    }
    ensure!(
        converted == report.calls_rows,
        "Converted {converted} of {} legacy Calls rows. The backup remains at {}. Re-run preflight before any retry.",
        report.calls_rows,
        backup.display()
    );
    report.notes.push(format!(
        "Converted {converted} Calls rows. mail.sqlite was validated and retained in place."
    ));
    Ok(report)
}

pub fn rollback(data_root: &Path, backup: &Path) -> Result<()> {
    let backup = if backup.is_absolute() {
        backup.to_path_buf()
    } else {
        data_root.join(backup)
    };
    ensure!(
        backup.starts_with(data_root.join(BACKUP_ROOT)),
        "Rollback backup must be inside {}.",
        data_root.join(BACKUP_ROOT).display()
    );
    let manifest: Vec<(String, Option<String>)> =
        serde_json::from_slice(&std::fs::read(backup.join("manifest.json"))?)?;
    // Validate the whole backup before replacing any live database.
    for (relative, expected) in &manifest {
        ensure!(
            !relative.is_empty()
                && Path::new(relative)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "Backup entry {relative} is not a relative data path."
        );
        if let Some(expected) = expected {
            ensure!(
                checksum(&backup.join(relative))? == *expected,
                "Backup file {relative} failed its SHA-256 check; rollback stopped."
            );
        }
    }
    for (relative, expected) in manifest {
        let target = data_root.join(&relative);
        match expected {
            Some(_) => {
                let source = backup.join(&relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(source, target)?;
            }
            None => match std::fs::remove_file(target) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into());
                }
                _ => {}
            },
        }
    }
    Ok(())
}

fn create_backup(data_root: &Path) -> Result<PathBuf> {
    let parent = data_root.join(BACKUP_ROOT);
    std::fs::create_dir_all(&parent)?;
    let backup = parent.join(format!("backup-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&backup)?;
    let mut files = Vec::new();
    for name in ["mail.sqlite", "calls.sqlite"] {
        for suffix in ["", "-wal", "-shm"] {
            let relative = format!("{name}{suffix}");
            record_backup_file(data_root, &backup, &relative, &mut files)?;
        }
    }
    let apps = crate::layout::apps(data_root);
    if apps.is_dir() {
        for entry in std::fs::read_dir(apps)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            for name in [
                "data/database.sqlite",
                "data/database.sqlite-wal",
                "data/database.sqlite-shm",
            ] {
                let relative = format!("apps/{}/{}", entry.file_name().to_string_lossy(), name);
                record_backup_file(data_root, &backup, &relative, &mut files)?;
            }
        }
    }
    std::fs::write(
        backup.join("manifest.json"),
        serde_json::to_vec_pretty(&files)?,
    )?;
    Ok(backup)
}

fn record_backup_file(
    root: &Path,
    backup: &Path,
    relative: &str,
    manifest: &mut Vec<(String, Option<String>)>,
) -> Result<()> {
    let source = root.join(relative);
    if !source.is_file() {
        manifest.push((relative.to_owned(), None));
        return Ok(());
    }
    let target = backup.join(relative);
    std::fs::create_dir_all(target.parent().context("Backup path has no parent")?)?;
    std::fs::copy(&source, &target)?;
    manifest.push((relative.to_owned(), Some(checksum(&target)?)));
    Ok(())
}

fn checksum(path: &Path) -> Result<String> {
    crate::copies::checksum_file(path).map(|(_, sha256)| sha256)
}

fn table_exists(db: &Connection, name: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
}

fn has_column(db: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = db.prepare("SELECT name FROM pragma_table_info(?1)")?;
    Ok(stmt
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column))
}

fn migration_secret(data_root: &Path) -> Result<String> {
    for name in ["KONAE_MAIL_SECRET", "KONAE_SETUP_KEY"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Ok(value);
        }
    }
    std::fs::read_to_string(data_root.join(crate::operator::MAIL_KEY_FILE))
        .with_context(|| "mail.sqlite contains encrypted links, but no KONAE_MAIL_SECRET, KONAE_SETUP_KEY or installation mail-key is readable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_backup_is_rejected_before_any_database_is_replaced() {
        let root =
            std::env::temp_dir().join(format!("konae-queue-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mail.sqlite"), b"mail before").unwrap();
        std::fs::write(root.join("calls.sqlite"), b"calls before").unwrap();
        let backup = create_backup(&root).unwrap();
        std::fs::write(root.join("mail.sqlite"), b"mail after").unwrap();
        std::fs::write(root.join("calls.sqlite"), b"calls after").unwrap();
        std::fs::write(backup.join("calls.sqlite"), b"corrupt").unwrap();

        assert!(
            rollback(&root, &backup)
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
        assert_eq!(
            std::fs::read(root.join("mail.sqlite")).unwrap(),
            b"mail after"
        );
        assert_eq!(
            std::fs::read(root.join("calls.sqlite")).unwrap(),
            b"calls after"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
