use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::json;

use crate::installation::Installation;

#[derive(Serialize)]
struct Policy {
    binary: &'static str,
    scope: &'static str,
    component: &'static str,
    kind: &'static str,
    presence: &'static str,
    writer: &'static str,
    capture: &'static str,
    restore: &'static str,
    test: &'static str,
}

fn policies(profile: &str) -> Result<Vec<Policy>> {
    let mut rows = Vec::new();
    for line in include_str!("inventory.tsv").lines().skip(1) {
        let fields: Vec<_> = line.split('\t').collect();
        let [binary, scope, component, kind, presence, writer, capture, restore, test] = fields[..] else {
            bail!("Invalid inventory row: {line}");
        };
        ensure!(
            fields.iter().all(|field| !field.is_empty()),
            "Incomplete inventory row: {line}"
        );
        ensure!(
            matches!(binary, "both" | "kernel" | "konae"),
            "Unknown inventory profile: {binary}"
        );
        if binary == "both" || binary == profile {
            rows.push(Policy {
                binary,
                scope,
                component,
                kind,
                presence,
                writer,
                capture,
                restore,
                test,
            });
        }
    }
    Ok(rows)
}

fn check_declarations(rows: &[Policy], kind: &str, declared: BTreeSet<&str>) -> Result<()> {
    let covered = rows
        .iter()
        .filter(|row| row.kind == kind)
        .map(|row| row.component)
        .collect();
    if let Some(name) = declared.difference(&covered).next() {
        bail!("No inventory policy for declared {kind} {name}.");
    }
    if let Some(name) = covered.difference(&declared).next() {
        bail!("Inventory policy names undeclared {kind} {name}.");
    }
    Ok(())
}

fn check_registry(rows: &[Policy], installation: &Installation) -> Result<()> {
    for account in &installation.accounts {
        ensure!(
            installation.holds(account.file),
            "No installation snapshot declared for account store {}.",
            account.file
        );
    }
    check_declarations(
        rows,
        "root-db",
        installation.databases.iter().map(|db| db.file).collect(),
    )?;
    check_declarations(
        rows,
        "app-db",
        crate::copies::DEPLOYMENT_DATABASES.into_iter().collect(),
    )?;
    check_declarations(
        rows,
        "module",
        crate::modules::registry()["modules"]
            .as_object()
            .context("Missing module registry")?
            .keys()
            .map(String::as_str)
            .collect(),
    )
}

fn matches_name(pattern: &str, name: &str) -> bool {
    if pattern == ".{uuid}.import-{uuid}" {
        return name
            .strip_prefix('.')
            .and_then(|n| n.split_once(".import-"))
            .is_some_and(|(blob, operation)| crate::valid_file_id(blob) && crate::valid_file_id(operation));
    }
    let Some((prefix, rest)) = pattern.split_once('{') else {
        return name == pattern;
    };
    let Some((token, suffix)) = rest.split_once('}') else {
        return false;
    };
    let Some(id) = name.strip_prefix(prefix).and_then(|rest| rest.strip_suffix(suffix)) else {
        return false;
    };
    match token {
        "app" => crate::runtime::valid_app_id(id),
        "uuid" => crate::valid_file_id(id),
        "simple" => id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "backup" => crate::safe_relative_path(id) && !id.contains("..") && !id.chars().any(char::is_control),
        _ => false,
    }
}

fn matches(row: &Policy, name: &str) -> bool {
    row.component.split('|').any(|pattern| matches_name(pattern, name))
}

fn database(kind: &str) -> bool {
    matches!(kind, "root-db" | "app-db" | "sqlite" | "scratch-db")
}

fn auxiliary(scope: &[&Policy], name: &str, entries: &BTreeMap<String, fs::FileType>) -> Option<&'static str> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if let Some(base) = name.strip_suffix(suffix)
            && entries.get(base).is_some_and(fs::FileType::is_file)
            && scope.iter().any(|row| database(row.kind) && matches(row, base))
        {
            return Some("SQLite -wal|-shm|-journal");
        }
    }
    let (base, suffix) = name.strip_prefix('.')?.rsplit_once(".tmp-")?;
    (crate::valid_file_id(suffix) && scope.iter().any(|row| row.kind == "file" && matches(row, base)))
        .then_some("atomic sibling")
}

struct Scan<'a> {
    rows: &'a [Policy],
    counts: BTreeMap<String, usize>,
}

impl Scan<'_> {
    fn visit(&mut self, root: &Path, relative: &Path, scope: &str) -> Result<()> {
        let policies: Vec<_> = self.rows.iter().filter(|row| row.scope == scope).collect();
        let mut entries = BTreeMap::new();
        for entry in fs::read_dir(root.join(relative))? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| anyhow::anyhow!("Invalid inventory name: {name:?}"))?;
            entries.insert(name, entry.file_type()?);
        }
        for row in policies.iter().filter(|row| row.presence == "required") {
            ensure!(
                entries.contains_key(row.component),
                "Missing required component {:?}",
                relative.join(row.component)
            );
        }
        for (name, ty) in &entries {
            let path = relative.join(name);
            if ty.is_file()
                && let Some(kind) = auxiliary(&policies, name, &entries)
            {
                *self.counts.entry(format!("auxiliary/{kind}")).or_default() += 1;
                continue;
            }
            let (row, pattern) = policies
                .iter()
                .find_map(|row| {
                    let directory = row.kind == "tree" || row.kind.starts_with("dir:");
                    if (directory && ty.is_dir()) || (!directory && ty.is_file()) {
                        row.component
                            .split('|')
                            .find(|pattern| matches_name(pattern, name))
                            .map(|pattern| (*row, pattern))
                    } else {
                        None
                    }
                })
                .with_context(|| format!("No inventory policy for {path:?} with this file type"))?;
            if scope == "backups" {
                let id = if ty.is_dir() {
                    name
                } else {
                    name.rsplit_once('.').unwrap().0
                };
                ensure!(
                    entries.get(&format!("{id}.json")).is_some_and(|t| t.is_file()),
                    "Missing internal backup record for {path:?}"
                );
                ensure!(
                    entries.get(id).is_some_and(|t| t.is_dir())
                        || entries.get(&format!("{id}.sqlite")).is_some_and(|t| t.is_file()),
                    "Missing internal backup payload for {path:?}"
                );
            }
            *self.counts.entry(format!("{scope}/{pattern}")).or_default() += 1;
            if let Some(child) = row.kind.strip_prefix("dir:") {
                self.visit(root, &path, child)?;
            }
        }
        Ok(())
    }
}

fn inspect(rows: &[Policy], root: &Path) -> Result<BTreeMap<String, usize>> {
    ensure!(
        fs::symlink_metadata(root)?.is_dir(),
        "Expected an installation data directory"
    );
    let mut scan = Scan {
        rows,
        counts: BTreeMap::new(),
    };
    scan.visit(root, Path::new(""), "root")?;
    Ok(scan.counts)
}

pub fn command(profile: &str, installation: &Installation) -> Result<bool> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("inventory") {
        return Ok(false);
    }
    let rows = policies(profile)?;
    check_registry(&rows, installation)?;
    let mut report = json!({"catalogVersion": 1, "binary": profile, "version": env!("CARGO_PKG_VERSION")});
    match (args.next().as_deref(), args.next()) {
        (Some("--policy"), None) => {
            report["policies"] = serde_json::to_value(rows)?;
            report["operations"] = json!({
                "installationCopy": {"scope": "All app folders and this binary's installation stores", "recover": "restore --from <copy repository>"},
                "appExport": {"scope": "One published app's portable folder and data", "recover": "project_import or kernel import"},
                "internalBackup": {"scope": "The selected database, data or sources inside one app", "recover": "database_execute restore-backup"}
            });
        }
        (None, None) => {
            let root = std::env::var_os("KONAE_DATA_DIR").unwrap_or_else(|| "/data".into());
            report["components"] = serde_json::to_value(
                inspect(&rows, Path::new(&root))
                    .with_context(|| format!("Cannot certify the {profile} store inventory"))?,
            )?;
        }
        _ => bail!("Usage: {profile} inventory [--policy]"),
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_store_or_module_without_policy_fails_even_on_an_empty_volume() {
        let installation = Installation::kernel();
        let rows = policies("kernel").unwrap();
        check_registry(&rows, &installation).unwrap();
        for name in ["calls.sqlite", "identity.sqlite", "page"] {
            let mut omitted = policies("kernel").unwrap();
            omitted.retain(|row| row.component != name);
            let error = check_registry(&omitted, &installation).unwrap_err().to_string();
            assert!(error.contains("No inventory policy") && error.contains(name), "{error}");
        }
        let mut next_release = installation;
        next_release.accounts.push(crate::installation::AccountDatabase {
            kind: "next",
            scope: "next",
            file: "new.sqlite",
        });
        assert!(check_registry(&rows, &next_release)
            .unwrap_err()
            .to_string()
            .contains("account store new.sqlite"));
        next_release.databases.push(crate::installation::RootDatabase {
            file: "new.sqlite",
            sign_out: &[],
        });
        assert!(check_registry(&rows, &next_release)
            .unwrap_err()
            .to_string()
            .contains("new.sqlite"));
    }
}
