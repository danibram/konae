use crate::{DAY_MS, KEEP_DAILY_DAYS, KEEP_RECENT, MANIFEST, Manifest, ManifestFile, PathScope,
    encode_manifest, parse_copy_id, reader::directory_file};
use anyhow::{Context, Result};
use anyhow::ensure;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, io::Read, path::Path, time::Duration};

pub fn vacuum_into(source: &Path, target: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
    Ok(())
}

fn manifest_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?.to_str().context("The copy contains a non-UTF-8 path.")?;
    Ok(relative.replace(std::path::MAIN_SEPARATOR, "/"))
}

pub fn list_files(root: &Path) -> Result<Vec<ManifestFile>> {
    fn walk(root: &Path, directory: &Path, files: &mut Vec<ManifestFile>) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(root, &path, files)?;
                continue;
            }
            ensure!(kind.is_file(), "Expected a regular file at {}.", path.display());
            let relative = manifest_path(root, &path)?;
            if relative == MANIFEST {
                continue;
            }
            let (size, sha256) = checksum_file(&path)?;
            files.push(ManifestFile { path: relative, size, sha256, home_copy: None });
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub fn checksum_file(path: &Path) -> Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

pub(crate) fn check_legacy_blobs(previous: &Manifest, legacy: &HashMap<String, String>) -> Result<()> {
    for file in &previous.files {
        if let Some(digest) = legacy.get(&file.path) {
            ensure!(digest == &file.sha256, "Indexed file {} does not match the SHA-256 in copy {}.", file.path, previous.copy_id);
        }
    }
    Ok(())
}

pub fn send_to_directory(staging: &Path, directory: &Path, id: &str, manifest: &Manifest, legacy: &HashMap<String, String>, scope: &dyn PathScope) -> Result<()> {
    let manifest_bytes = encode_manifest(scope, id, manifest)?;
    ensure!(manifest.files.iter().all(|file| file.home_copy.as_deref().is_none_or(|home| home == id)),
        "A directory copy must contain each file under its own copy ID.");
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Could not create {}", directory.display()))?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".partial-") {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    let partial_name = format!(".partial-{id}");
    let partial = directory.join(&partial_name);
    let previous = std::fs::read_dir(directory)?.filter_map(|entry| entry.ok()).filter_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        (parse_copy_id(&name).is_some() && entry.path().join(MANIFEST).is_file()).then_some(name)
    }).max();
    let previous_manifest = previous.as_deref().map(|copy| -> Result<Manifest> {
        Ok(serde_json::from_slice(&std::fs::read(directory_file(directory, copy, MANIFEST)?)?)?)
    }).transpose()?;
    if let Some(previous) = &previous_manifest { check_legacy_blobs(previous, legacy)?; }
    let previous_files = previous_manifest.into_iter().flat_map(|manifest| manifest.files)
        .map(|file| (file.path.clone(), file)).collect();
    let previous = previous.as_deref().map(|copy| (directory, copy));
    copy_tree_incremental(staging, staging, &partial, previous, &previous_files)?;
    for file in &manifest.files {
        let (size, digest) = directory_file(directory, &partial_name, &file.path).and_then(|path| checksum_file(&path))
            .with_context(|| format!("Could not verify {} in directory copy {id}", file.path))?;
        ensure!(size == file.size && digest == file.sha256, "File {} in directory copy {id} does not match its checksum.", file.path);
    }
    std::fs::write(partial.join(MANIFEST), manifest_bytes)?;
    std::fs::rename(&partial, directory.join(id))?;
    Ok(())
}

fn copy_tree_incremental(root: &Path, source: &Path, target: &Path, previous: Option<(&Path, &str)>, previous_files: &HashMap<String, ManifestFile>) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree_incremental(root, &from, &to, previous, previous_files)?;
            continue;
        }
        if source == root && entry.file_name() == MANIFEST { continue; }
        let rel = manifest_path(root, &from)?;
        let unchanged = match previous_files.get(&rel) {
            Some(expected) => checksum_file(&from)? == (expected.size, expected.sha256.clone()),
            None => false,
        };
        if let Some((directory, copy)) = previous.filter(|_| unchanged) {
            let candidate = directory_file(directory, copy, &rel)
                .with_context(|| format!("Could not reuse {rel} from directory copy {copy}"))?;
            if std::fs::hard_link(&candidate, &to).is_err() {
                std::fs::copy(&candidate, &to).with_context(|| format!("Could not copy unchanged file {rel}"))?;
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn prune_directory(directory: &Path, now: u64) -> Result<usize> {
    let mut complete = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if parse_copy_id(&name).is_some() && entry.path().join(MANIFEST).is_file() {
            complete.push(name);
        }
    }
    let expired = expired_copies(&complete, now);
    for name in &expired {
        std::fs::remove_dir_all(directory.join(name))?;
    }
    Ok(expired.len())
}

pub fn expired_copies(ids: &[String], now: u64) -> Vec<String> {
    let mut dated = ids
        .iter()
        .filter_map(|id| parse_copy_id(id).map(|at| (at, id)))
        .collect::<Vec<_>>();
    dated.sort_by(|a, b| b.0.cmp(&a.0));
    let mut days = HashSet::new();
    let mut expired = Vec::new();
    for (index, (at, id)) in dated.into_iter().enumerate() {
        let newest_of_day = days.insert(at / DAY_MS);
        if index < KEEP_RECENT {
            continue;
        }
        if newest_of_day && now.saturating_sub(at) <= KEEP_DAILY_DAYS * DAY_MS {
            continue;
        }
        expired.push(id.clone());
    }
    expired
}
