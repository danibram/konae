use crate::{Installed, Settings, installation::Installation, layout};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path};

fn optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid {}", path.display()))?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn claim(addresses: &mut HashMap<String, String>, key: String, id: &str) -> Result<()> {
    if let Some(other) = addresses.insert(key.clone(), id.to_owned()) {
        ensure!(id == other, "Apps {other} and {id} both claim {key}.");
    }
    Ok(())
}

pub(super) fn check_materialized(root: &Path, files: &[super::ManifestFile]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for file in files {
        ensure!(seen.insert(&file.path), "Duplicate path {} in the installation copy.", file.path);
        let (size, digest) = super::checksum_file(&root.join(&file.path))?;
        ensure!(size == file.size && digest == file.sha256,
            "Staged file {} differs from the selected copy.", file.path);
    }
    Ok(())
}

pub(super) fn check_cut(root: &Path, installation: &Installation) -> Result<()> {
    (installation.check_catalog)(root)?;
    let apps = layout::apps(root);
    if !apps.try_exists()? {
        return Ok(());
    }
    let mut hosts = HashMap::new();
    let mut paths = HashMap::new();
    for entry in fs::read_dir(apps)? {
        let name = entry?.file_name();
        let id = name.to_str().filter(|id| crate::runtime::valid_app_id(id)).context("Invalid app folder name.")?;
        let private = layout::private(root, id);
        let installed: Option<Installed> = optional_json(&layout::record(root, id))?;
        let settings = layout::settings(root, id).try_exists()?.then(|| Settings::read(root, id)).transpose()?;
        if let Some(settings) = &settings {
            ensure!(settings.id == id, "App folder {id} holds settings for another app.");
        }
        if let Some(record) = installed {
            let settings = settings.context("Published app has no settings.")?;
            let dist = layout::dist(root, id);
            ensure!(dist.join("index.html").is_file(), "Published app {id} has no dist/index.html.");
            let released = record.released.as_ref().or(settings.released.as_ref());
            if let Some(digest) = released.and_then(|r| r.artifact_digest.as_ref()) {
                ensure!(crate::folder::version_of(&crate::folder::read_dist(&dist)?) == *digest,
                    "App {id} has different bytes from its released artifact.");
            }
            claim(&mut hosts, record.hostname.to_ascii_lowercase(), id)?;
        } else {
            ensure!(settings.and_then(|s| s.released).is_none(), "App {id} has released settings but no deployment record.");
        }
        if let Some(record) = optional_json::<crate::hostname::Record>(&private.join(crate::hostname::RECORD_FILE))? {
            for previous in record.previous {
                claim(&mut hosts, previous.to_ascii_lowercase(), id)?;
            }
        }
        if let Some(record) = optional_json::<crate::domain::Record>(&private.join(crate::domain::RECORD_FILE))? {
            claim(&mut hosts, record.hostname.to_ascii_lowercase(), id)?;
        }
        if let Some(record) = optional_json::<crate::path::Record>(&private.join(crate::path::RECORD_FILE))?
            && record.enabled {
            claim(&mut paths, id.to_owned(), id)?;
            if let Some(name) = record.name {
                claim(&mut paths, name, id)?;
            }
        }
        check_files(root, id).with_context(|| format!("Invalid file store for app {id}"))?;
    }
    Ok(())
}

fn check_files(root: &Path, id: &str) -> Result<()> {
    let files = layout::files(root, id);
    if !files.try_exists()? {
        return Ok(());
    }
    let index: Vec<Value> = optional_json(&files.join("index.json"))?.unwrap_or_default();
    let mut indexed = std::collections::HashSet::new();
    let mut accounts = None;
    for file in &index {
        let key = file["id"].as_str().filter(|v| crate::valid_file_id(v)).context("Invalid indexed file ID.")?;
        ensure!(indexed.insert(key), "Duplicate indexed file {key}.");
        let blob = files.join(format!("{key}.blob"));
        ensure!(Some(fs::metadata(&blob)?.len()) == file["size"].as_u64(), "Indexed size differs for file {key}.");
        if let Some(digest) = file.get("sha256") {
            ensure!(digest.as_str() == Some(&super::checksum_file(&blob)?.1), "Indexed SHA-256 differs for file {key}.");
        }
        if !file["ownerId"].is_null() {
            let owner = crate::file_owner(file).context("Invalid file owner.")?;
            if accounts.is_none() {
                accounts = Some(Connection::open_with_flags(layout::identity(root, id), OpenFlags::SQLITE_OPEN_READ_ONLY)?);
            }
            let exists: bool = accounts.as_ref().unwrap().query_row(
                "SELECT EXISTS(SELECT 1 FROM app_users WHERE id=?1)", [owner], |r| r.get(0))?;
            ensure!(exists, "File {key} has no matching app user.");
        }
    }
    for entry in fs::read_dir(files)? {
        let name = entry?.file_name();
        if let Some(key) = name.to_str().and_then(|s| s.strip_suffix(".blob")) {
            ensure!(indexed.contains(key), "File {key} is absent from the index.");
        }
    }
    Ok(())
}
