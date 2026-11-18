use super::{CopyKey, ManifestFile, Source, check_sealed, parse_copy_id, read_manifest, source_for, verified_bytes};
use crate::{DatabaseBackup, import::{HOLDS_DATA, HOLDS_SOURCES}, safe_relative_path};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, io::ErrorKind, path::Path};

// A manifest lists files. This zero-byte entry preserves a data snapshot with all modules off.
const EMPTY: &str = ".empty-data";

fn check_id(id: &str) -> Result<()> {
    ensure!(safe_relative_path(id) && !id.contains('/') && !id.contains("..") && !id.chars().any(char::is_control), "Invalid backup ID.");
    Ok(())
}

fn scope(record: &DatabaseBackup) -> String {
    if record.holds.is_empty() { return "database".into(); }
    if record.holds.iter().all(|s| matches!(s.as_str(), HOLDS_DATA | HOLDS_SOURCES)) {
        return record.holds.join("+");
    }
    "unknown".into()
}

fn paths(record: &DatabaseBackup, id: &str) -> Result<Vec<(String, bool)>> {
    ensure!(record.id == id, "Backup record ID differs from its filename.");
    if record.holds.is_empty() {
        let flat = format!("{id}.sqlite");
        ensure!(record.key == flat, "Invalid database backup key.");
        return Ok(vec![(flat, true)]);
    }
    ensure!(record.key == id, "Invalid internal backup key.");
    let mut files = Vec::new();
    for part in &record.holds {
        match part.as_str() {
            HOLDS_DATA => {
                for name in ["database.sqlite", "identity.sqlite", "files.tar", crate::usage::USAGE_DATABASE] {
                    files.push((format!("{id}/{name}"), false));
                }
            }
            HOLDS_SOURCES => files.push((format!("{id}/sources.json"), true)),
            _ => bail!("Unknown internal backup scope."),
        }
    }
    ensure!(record.holds.len() == record.holds.iter().collect::<std::collections::HashSet<_>>().len(), "Duplicate internal backup scope.");
    Ok(files)
}

#[derive(Debug)]
struct Missing(String);
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for Missing {}

// Older records specify a total across optional components. Only the import writer's
// names count toward it. Files beside or below them never enter a copy.
fn contents(record: &DatabaseBackup, id: &str, size: impl Fn(&str) -> Result<Option<u64>>) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut total = 0u64;
    for (path, required) in paths(record, id)? {
        match size(&path)? {
            Some(bytes) => {
                ensure!(bytes > 0, "Empty backup component {path}.");
                total = total.checked_add(bytes).context("Invalid internal backup size.")?;
                files.push(path);
            }
            None if required => return Err(Missing(format!("Missing {path}.")).into()),
            None => {}
        }
    }
    if total < record.size_bytes {
        return Err(Missing(format!("Found {total} of {} recorded bytes.", record.size_bytes)).into());
    }
    ensure!(total == record.size_bytes, "Found {total} bytes, expected {}.", record.size_bytes);
    Ok(files)
}

fn regular_size(path: &Path) -> Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(m.is_file(), "Expected a regular backup file.");
            Ok(Some(m.len()))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn disk_record(dir: &Path, id: &str) -> Result<(DatabaseBackup, Vec<String>, Vec<u8>)> {
    let name = format!("{id}.json");
    check_id(id)?;
    ensure!(fs::symlink_metadata(dir)?.is_dir(), "Expected a backup directory.");
    let path = dir.join(name);
    regular_size(&path)?.context("Backup record missing.")?;
    let bytes = fs::read(path)?;
    let record: DatabaseBackup = serde_json::from_slice(&bytes).context("Unreadable backup record.")?;
    let files = (|| {
        if !record.holds.is_empty() {
            let metadata = fs::symlink_metadata(dir.join(id))?;
            ensure!(metadata.is_dir(), "Expected a backup payload directory.");
        }
        contents(&record, id, |path| regular_size(&dir.join(path)))
    })().with_context(|| format!("Backup {id} ({})", scope(&record)))?;
    Ok((record, files, bytes))
}

pub(crate) fn checked_record(dir: &Path, id: &str) -> Result<DatabaseBackup> {
    disk_record(dir, id).map(|v| v.0).map_err(|e| anyhow::anyhow!("{e:#}"))
}

#[cfg(test)]
pub(super) fn copy_records(source: &Path, target: &Path) -> Result<()> {
    stage_records(source, target, |from, to| Ok(fs::copy(from, to)?))
}

pub(super) fn stage_records(source: &Path, target: &Path, mut copy: impl FnMut(&Path, &Path) -> Result<u64>) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let name = entry?.file_name();
        if Path::new(&name).extension().is_none_or(|ext| ext != "json") { continue; }
        let id = name.to_str().context("Backup record filename must be UTF-8.")?.strip_suffix(".json").unwrap();
        let (backup, files, _) = disk_record(source, id).with_context(|| format!("Internal backup {id}"))?;
        if files.is_empty() {
            fs::create_dir(target.join(id))?;
            fs::write(target.join(id).join(EMPTY), [])?;
        }
        let mut copied = 0u64;
        for path in files {
            let to = target.join(&path);
            fs::create_dir_all(to.parent().unwrap())?;
            copied = copied.checked_add(copy(&source.join(path), &to)?).context("Invalid internal backup size.")?;
        }
        ensure!(copied == backup.size_bytes, "Backup {id} ({}) changed during the copy.", scope(&backup));
        copy(&source.join(&name), &target.join(name))?;
    }
    Ok(())
}

pub(super) fn warn_incomplete(root: &Path, copy: &str) -> Result<()> {
    let apps = crate::layout::apps(root);
    if !apps.exists() { return Ok(()); }
    for app in fs::read_dir(apps)? {
        let app = app?;
        let dir = app.path().join(".konae/backups");
        if !dir.is_dir() { continue; }
        for entry in fs::read_dir(&dir)? {
            let name = entry?.file_name();
            if let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json"))
                && let Err(e) = disk_record(&dir, id) {
                eprintln!("Copy {copy}, app {}, internal history {id}: {e:#}. Restore continues with the current app.", app.file_name().to_string_lossy());
            }
        }
    }
    Ok(())
}

fn missing(error: &anyhow::Error) -> bool {
    use aws_sdk_s3::{error::SdkError, operation::get_object::GetObjectError};
    error.is::<Missing>()
        || error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == ErrorKind::NotFound)
        || error.downcast_ref::<SdkError<GetObjectError>>().and_then(|e| e.as_service_error()).is_some_and(|e| e.is_no_such_key())
}

// Open only entries named by the manifest. Follow S3 home references under
// the same signature and checksum rules as restore.
pub(super) async fn audit(source: &Source, copy: &str, key: Option<&CopyKey>) -> Result<(Value, bool)> {
    ensure!(parse_copy_id(copy).is_some(), "Invalid copy ID.");
    let manifest = read_manifest(source, copy).await?.map_err(anyhow::Error::msg)?;
    ensure!(manifest.copy_id == copy, "Manifest belongs to another copy.");
    ensure!(!(matches!(source, Source::S3 { .. }) && key.is_some() && manifest.encryption.is_none()), "Expected a signed and encrypted S3 copy.");
    let key = key.map(CopyKey::files);
    check_sealed(copy, &manifest, key)?;
    let mut index = HashMap::new();
    for file in &manifest.files {
        ensure!(safe_relative_path(&file.path), "Invalid manifest path.");
        ensure!(file.home_copy.as_ref().is_none_or(|home| parse_copy_id(home).is_some()), "Invalid home copy ID.");
        ensure!(index.insert(file.path.as_str(), file).is_none(), "Duplicate manifest entry.");
    }

    let mut rows = Vec::new();
    let mut complete = true;
    for file in &manifest.files {
        let parts: Vec<_> = file.path.split('/').collect();
        let ["apps", app, ".konae", "backups", name] = parts.as_slice() else { continue };
        let Some(id) = name.strip_suffix(".json") else { continue };
        let mut held = String::from("unknown");
        let result: Result<()> = async {
            check_id(id)?;
            let bytes = verified_bytes(source, copy, &manifest, file, key).await?;
            let record: DatabaseBackup = serde_json::from_slice(&bytes).context("Unreadable backup record.")?;
            held = scope(&record);
            let dir = format!("apps/{app}/.konae/backups/");
            let lookup = |p: &str| -> Option<&ManifestFile> { index.get(format!("{dir}{p}").as_str()).copied() };
            let mut files = contents(&record, id, |p| Ok(lookup(p).map(|f| f.size)))?;
            if files.is_empty() { files.push(format!("{id}/{EMPTY}")); }
            for path in files {
                let component = lookup(&path).ok_or_else(|| Missing(format!("Missing {path}.")))?;
                let bytes = verified_bytes(source, copy, &manifest, component, key).await?;
                ensure!(!path.ends_with(EMPTY) || bytes.is_empty(), "Invalid empty data marker.");
            }
            Ok(())
        }.await;
        let status = match &result {
            Ok(()) => "recovered",
            Err(e) if missing(e) => "missing",
            Err(_) => "unchecked",
        };
        complete &= result.is_ok();
        let mut row = json!({"app":app, "backup":id, "scope":held, "status":status});
        if let Err(e) = result { row["reason"] = json!(e.to_string()); }
        rows.push(row);
    }
    Ok((json!({"copy":copy, "currentApp":"unchecked", "internalBackups":rows}), complete))
}

pub(crate) async fn operator_command() -> Result<bool> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("verify-history") { return Ok(false); }
    let usage = "verify-history --from <directory or s3://bucket/prefix> [--copy <copy ID>]";
    let (from, selected) = match &args[1..] {
        [flag, from] if flag == "--from" => (from, None),
        [flag, from, select, copy] if flag == "--from" && select == "--copy" => (from, Some(copy)),
        _ => bail!("{usage}"),
    };
    let source = source_for(from).await?;
    let key = CopyKey::from_env()?;
    let copies = match selected {
        Some(copy) => vec![copy.clone()],
        None => source.complete_copies().await?,
    };
    ensure!(!copies.is_empty(), "No copy manifests found.");
    let mut complete = true;
    for copy in copies {
        let (report, ok) = match audit(&source, &copy, key.as_ref()).await {
            Ok(result) => result,
            Err(e) => (json!({"copy":copy, "status":"unchecked", "currentApp":"unchecked", "error":e.to_string()}), false),
        };
        println!("{report}");
        complete &= ok;
    }
    ensure!(complete, "Some internal backup contents are missing or could not be verified.");
    Ok(true)
}
