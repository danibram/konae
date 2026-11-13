//! `storage-migrate` and `artifacts-recover` move an installation from the
//! older S3 settings to one bucket, and back.
//!
//! Both read the older settings from a protected file in the format of
//! `podman --env-file`, and run the same parsers the Kernel starts with. The
//! destination of `storage-migrate` is the environment the command runs in,
//! which is the environment the Kernel will start with, so the store it
//! records in `files-store.json` is the store the Kernel computes.
//!
//! Neither command deletes a source object.

use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    Installed, copies, layout, list_s3_keys, read_s3_bytes,
    file_store::{self, FileStore, LocalFolder, S3Settings, S3Store, Vars},
};

const NOTHING_SWITCHED: &str = "files-store.json was not changed.";

/// The settings a file holds, one `NAME=value` a line. Blank lines and lines
/// that start with `#` are skipped. The file must be a regular file that only
/// its owner can read, because it holds keys.
pub fn read_settings_file(path: &Path) -> Result<HashMap<String, String>> {
    let metadata = std::fs::symlink_metadata(path).with_context(|| format!("Could not read {}.", path.display()))?;
    ensure!(metadata.is_file(), "{} must be a regular file, not a link or a folder.", path.display());
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "{} can be read by other users. Run chmod 600 on it first.",
        path.display()
    );
    parse_settings(&std::fs::read_to_string(path).with_context(|| format!("Could not read {}.", path.display()))?)
        .with_context(|| format!("{} is not a settings file.", path.display()))
}

fn parse_settings(text: &str) -> Result<HashMap<String, String>> {
    let mut settings = HashMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        // Only the name: the value is never echoed, since it may be a key.
        let (name, value) = line.split_once('=').with_context(|| format!("Line {} has no NAME=value.", number + 1))?;
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Line {} does not start with a setting name.",
            number + 1
        );
        settings.insert(name.to_owned(), value.to_owned());
    }
    Ok(settings)
}

/// The prefix of copies in a bucket, from `KONAE_COPY_S3=1` or the older
/// `KONAE_COPY_S3_PREFIX`, with the bucket and keys that reach it.
fn copy_location(data_root: &Path, get: Vars<'_>) -> Result<Option<S3Settings>> {
    let bucket = S3Settings::copies(get)?;
    let Some(config) = copies::CopyConfig::from_vars(data_root, bucket.is_some(), get)? else { return Ok(None) };
    let prefix = config.targets.iter().find_map(|target| match target {
        copies::Target::S3 { prefix } => Some(prefix.clone()),
        copies::Target::Directory(_) => None,
    });
    Ok(match (bucket, prefix) {
        (Some(bucket), Some(prefix)) => Some(S3Settings { prefix, ..bucket }),
        _ => None,
    })
}

fn store(settings: Option<&S3Settings>, data_root: &Path) -> Arc<dyn FileStore> {
    match settings {
        Some(settings) => Arc::new(S3Store::new(settings.clone())),
        None => Arc::new(LocalFolder::new(data_root)),
    }
}

fn where_copies_go(settings: &S3Settings) -> String {
    format!("{} {}/{}", settings.endpoint, settings.bucket, settings.prefix)
}

/// What `storage-migrate` found and did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub deployments: usize,
    /// Indexed files the destination holds now, read back with their SHA-256.
    pub files: usize,
    /// Of `files`, those an earlier run had already copied.
    pub already: usize,
    pub bytes: u64,
    /// Files of this installation's apps in the destination that no index
    /// names, left for `files-reconcile`.
    pub unindexed: usize,
}

async fn digest(mut blob: file_store::Blob) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let read = blob.reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        length += read as u64;
    }
    Ok((length, hex::encode(hasher.finalize())))
}

/// Refuses a destination that holds files of an app this data directory
/// does not have. They belong to another installation or to a store nobody
/// cleaned, and `files-reconcile` would delete them once the Kernel ran on
/// that store. Files of this installation's own apps that no index names
/// are deleted files, as a rollback finds in the old bucket; they are counted
/// for `files-reconcile`.
async fn unindexed_in_destination(data_root: &Path, target: &dyn FileStore) -> Result<usize> {
    let apps: HashSet<String> = LocalFolder::new(data_root).deployments().await?.into_iter().collect();
    let mut foreign = Vec::new();
    let mut unindexed = 0;
    for deployment in target.deployments().await? {
        let objects = target.objects(&deployment).await?;
        if !apps.contains(&deployment) {
            foreign.extend(objects.into_iter().map(|object| format!("{deployment}/{}", object.id)));
            continue;
        }
        let indexed: HashSet<String> =
            file_store::indexed_files(data_root, &deployment).await?.unwrap_or_default().into_iter().map(|(id, _)| id).collect();
        unindexed += objects.iter().filter(|object| !indexed.contains(&object.id)).count();
    }
    ensure!(
        foreign.is_empty(),
        "The destination already holds {} files of apps this installation does not have, such as {}. Choose an empty bucket or prefix. {NOTHING_SWITCHED}",
        foreign.len(),
        foreign[0]
    );
    Ok(unindexed)
}

/// Copies every indexed file from `source` to `target` and reads each back.
/// A file `target` already holds with the same bytes counts as copied, so a
/// run that stopped halfway can run again. With `dry_run` it writes nothing
/// and checks that each source file exists with its indexed size.
pub async fn copy_files(data_root: &Path, source: &dyn FileStore, target: &dyn FileStore, dry_run: bool) -> Result<Moved> {
    let unindexed = unindexed_in_destination(data_root, target).await?;
    let scratch = data_root.join("transfers");
    if !dry_run {
        tokio::fs::create_dir_all(&scratch).await?;
        // A run that was killed leaves its temporary copy behind.
        let mut leftovers = tokio::fs::read_dir(&scratch).await?;
        while let Some(entry) = leftovers.next_entry().await? {
            if entry.file_name().to_string_lossy().starts_with(".storage-migrate-") {
                tokio::fs::remove_file(entry.path()).await?;
            }
        }
    }
    let mut moved = Moved { unindexed, ..Moved::default() };
    for deployment in LocalFolder::new(data_root).deployments().await? {
        let Some(files) = file_store::indexed_files(data_root, &deployment).await? else { continue };
        moved.deployments += 1;
        for (id, size) in files {
            // Read the destination's copy to its end before opening the source,
            // so no response body waits unread while another one is read.
            let existing = match target.get(&deployment, &id, None).await? {
                Some(existing) => Some(digest(existing).await?),
                None => None,
            };
            let blob = source
                .get(&deployment, &id, None)
                .await?
                .with_context(|| format!("File {id} of Deployment {deployment} is in the index but not in the source. {NOTHING_SWITCHED}"))?;
            ensure!(
                blob.length == size,
                "File {id} of Deployment {deployment} holds {} bytes in the source, but its index says {size}. {NOTHING_SWITCHED}",
                blob.length
            );
            if let Some(theirs) = existing {
                ensure!(
                    theirs == digest(blob).await?,
                    "The destination already holds file {id} of Deployment {deployment} with other bytes. {NOTHING_SWITCHED}"
                );
                moved.already += 1;
            } else if !dry_run {
                let temp = scratch.join(format!(".storage-migrate-{}", uuid::Uuid::new_v4()));
                let copied = async {
                    let (length, sha256) = file_store::save_blob(blob, &temp).await?;
                    ensure!(length == size, "File {id} of Deployment {deployment} read {length} bytes, but its index says {size}. {NOTHING_SWITCHED}");
                    target.put(&deployment, &id, &temp).await?;
                    let stored = target
                        .get(&deployment, &id, None)
                        .await?
                        .with_context(|| format!("File {id} of Deployment {deployment} did not arrive. {NOTHING_SWITCHED}"))?;
                    ensure!(
                        digest(stored).await? == (length, sha256),
                        "File {id} of Deployment {deployment} reads back with another SHA-256. {NOTHING_SWITCHED}"
                    );
                    Ok(())
                }
                .await;
                let _ = tokio::fs::remove_file(&temp).await;
                copied?;
            }
            moved.files += 1;
            moved.bytes += size;
        }
    }
    Ok(moved)
}

/// The copies and the revocation record stay where they are. The command
/// refuses a destination that would look for them anywhere else, or with
/// another secret, and opens them with the destination's keys before it
/// switches.
async fn check_copies(
    source: Option<&S3Settings>,
    target: Option<&S3Settings>,
    source_key: Option<&copies::CopyKey>,
    target_key: Option<&copies::CopyKey>,
    accept_unsigned: bool,
) -> Result<Option<copies::CheckedCopies>> {
    let (source, target) = match (source, target) {
        (None, None) => return Ok(None),
        (None, Some(target)) => {
            let client = file_store::s3_client(target);
            let held = crate::list_s3_keys(&client, &target.bucket, &target.prefix)
                .await
                .with_context(|| format!("Could not list {}. {NOTHING_SWITCHED}", where_copies_go(target)))?;
            ensure!(
                held.is_empty(),
                "--from names no S3 copy target, but {} already holds copies or a revocation record. Give --from every setting the Kernel last started with, KONAE_DATA_BUCKET and KONAE_COPY_S3_PREFIX included, so the command can check that record before it switches. {NOTHING_SWITCHED}",
                where_copies_go(target)
            );
            return Ok(None);
        }
        (Some(source), None) => bail!(
            "The settings in --from keep copies and the revocation record in {}, but this environment sends no copy to S3. Set KONAE_COPY_S3=1, so that suspensions and revoked keys keep reaching that record. {NOTHING_SWITCHED}",
            where_copies_go(source)
        ),
        (Some(source), Some(target)) => (source, target),
    };
    ensure!(
        (&source.endpoint, &source.bucket, &source.prefix) == (&target.endpoint, &target.bucket, &target.prefix),
        "Copies are in {}, but this environment sends them to {}. storage-migrate does not move copies or the revocation record. Use the bucket that holds them, with copies under {}. {NOTHING_SWITCHED}",
        where_copies_go(source),
        where_copies_go(target),
        copies::S3_PREFIX
    );
    ensure!(
        source_key.map(copies::CopyKey::fingerprint) == target_key.map(copies::CopyKey::fingerprint),
        "KONAE_COPY_SECRET in this environment differs from the one in --from, so the copies already made would not open. {NOTHING_SWITCHED}"
    );
    let checked = copies::check_s3(file_store::s3_client(target), &target.bucket, &target.prefix, target_key, accept_unsigned, NOTHING_SWITCHED).await?;
    Ok(Some(checked))
}

fn usage() -> String {
    format!(
        "Usage: {program} storage-migrate --from <settings file> [--dry-run]\n       {program} artifacts-recover --from <settings file> [--dry-run]",
        program = crate::program()
    )
}

fn arguments(args: &[String]) -> Result<(PathBuf, bool)> {
    let mut from = None;
    let mut dry_run = false;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--from" => from = Some(PathBuf::from(rest.next().with_context(usage)?)),
            "--dry-run" => dry_run = true,
            _ => bail!("{}", usage()),
        }
    }
    Ok((from.with_context(usage)?, dry_run))
}

pub async fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_root = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    match args.first().map(String::as_str) {
        Some("storage-migrate") => {
            let (from, dry_run) = arguments(&args[1..])?;
            // A preflight reads only, so it may run beside a serving Kernel.
            let _stopped = if dry_run { None } else { Some(file_store::hold_while_stopped(&data_root, "storage-migrate")?) };
            let settings = read_settings_file(&from)?;
            let raw = |name: &str| settings.get(name).cloned();
            let get = |name: &str| raw(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
            let source = Side {
                files: S3Settings::files(&get)?,
                copies: copy_location(&data_root, &get)?,
                key: copies::CopyKey::from_vars(&raw)?,
            };
            let target = Side {
                files: S3Settings::from_env()?,
                copies: copy_location(&data_root, &file_store::env_var)?,
                key: copies::CopyKey::from_env()?,
            };
            let report = migrate(&data_root, &source, &target, copies::accept_unsigned_from_env(), dry_run).await?;
            println!("{report}");
            Ok(true)
        }
        Some("artifacts-recover") => {
            let (from, dry_run) = arguments(&args[1..])?;
            let settings = read_settings_file(&from)?;
            let get = |name: &str| settings.get(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
            let recovered = recover_artifacts(&data_root, &get, dry_run).await?;
            println!("{recovered}");
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// One side of a migration: where files and copies go, and the copy secret.
pub struct Side {
    pub files: Option<S3Settings>,
    pub copies: Option<S3Settings>,
    pub key: Option<copies::CopyKey>,
}

pub async fn migrate(data_root: &Path, source: &Side, target: &Side, accept_unsigned: bool, dry_run: bool) -> Result<String> {
    let recorded = file_store::read_marker(data_root).await?.unwrap_or_else(|| file_store::describe(None));
    let wanted = file_store::describe(target.files.as_ref());
    let named = file_store::describe(source.files.as_ref());
    if recorded == wanted {
        return Ok(format!("The files are already in {}. Nothing to move.", file_store::spoken(&wanted)));
    }
    ensure!(
        recorded == named,
        "files-store.json says the files are in {}, but --from names {}. Give --from the settings the Kernel last started with. {NOTHING_SWITCHED}",
        file_store::spoken(&recorded),
        file_store::spoken(&named)
    );
    let checked = check_copies(source.copies.as_ref(), target.copies.as_ref(), source.key.as_ref(), target.key.as_ref(), accept_unsigned).await?;
    let from = store(source.files.as_ref(), data_root);
    let to = store(target.files.as_ref(), data_root);
    let moved = copy_files(data_root, from.as_ref(), to.as_ref(), dry_run).await?;
    let unindexed = match moved.unindexed {
        0 => String::new(),
        count => format!(" The destination also holds {count} files of these apps that no index names, deleted ones; files-reconcile removes them once the Kernel runs on it."),
    };
    let copies = match checked {
        Some(checked) => format!(
            " Opened the manifests of {} copies and {} with this environment's keys and secret; they stay where they are.",
            checked.copies,
            match checked.entries {
                Some(entries) => format!("a revocation record of {entries} entries"),
                None => "no revocation record".to_owned(),
            }
        ),
        None => String::new(),
    };
    if dry_run {
        return Ok(format!(
            "Preflight: {} files ({} bytes) of {} Deployments to copy from {} to {}, {} of them already there.{unindexed}{copies} Nothing was written.",
            moved.files,
            moved.bytes,
            moved.deployments,
            file_store::spoken(&named),
            file_store::spoken(&wanted),
            moved.already
        ));
    }
    file_store::write_marker(data_root, &wanted).await?;
    Ok(format!(
        "Copied {} files ({} bytes) of {} Deployments to {}, {} of them already there, each read back with the same SHA-256, and recorded it in files-store.json.{unindexed}{copies} The source still holds every file. Start the Kernel with this environment.",
        moved.files,
        moved.bytes,
        moved.deployments,
        file_store::spoken(&wanted),
        moved.already
    ))
}

/// Brings back into `dist/` every file the older artifact bucket holds for
/// the live release of each app and the folder lacks. The Kernel no longer
/// reads that bucket, so a file that only lived there would stop being
/// served. It compares every app before it writes, writes only missing
/// files, never deletes, and skips an app that publishes during the run, so
/// it may run beside a serving Kernel.
pub async fn recover_artifacts(data_root: &Path, get: Vars<'_>, dry_run: bool) -> Result<String> {
    ensure!(
        !file_store::unified(get)?,
        "--from must hold the older settings, with KONAE_ARTIFACT_BUCKET, not KONAE_S3_BUCKET."
    );
    let settings = S3Settings::copies(get)?.context("--from names no KONAE_S3_ENDPOINT, so there is no artifact bucket to read.")?;
    let bucket = get("KONAE_ARTIFACT_BUCKET").unwrap_or_else(|| "konae-apps".into());
    let client = file_store::s3_client(&settings);
    let mut lines = Vec::new();
    let mut differing = Vec::new();
    // Compares every app first, so a differing file stops the run before
    // anything is written.
    let mut writes = Vec::new();
    let mut checked = 0usize;
    let mut entries = tokio::fs::read_dir(layout::apps(data_root)).await?;
    let mut ids = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if let Some(id) = entry.file_name().to_str() {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    for id in ids {
        let Some(version) = live_version(data_root, &id).await? else { continue };
        let prefix = format!("deployments/{id}/{version}/");
        let keys = list_s3_keys(&client, &bucket, &prefix).await.with_context(|| format!("Could not list {bucket}/{prefix}."))?;
        let dist = layout::dist(data_root, &id);
        let mut files = 0;
        let mut absent = Vec::new();
        // A key that ends in `/` is a folder marker some tools write, not a file.
        for key in keys.iter().filter(|key| !key.ends_with('/')) {
            let path = &key[prefix.len()..];
            ensure!(crate::safe_relative_path(path), "{bucket}/{key} is not a path inside dist/.");
            let local = dist.join(path);
            let remote = read_s3_bytes(&client, &bucket, key).await.with_context(|| format!("Could not read {bucket}/{key}."))?;
            files += 1;
            match tokio::fs::read(&local).await {
                Ok(present) if present == remote => {}
                Ok(_) => differing.push(format!("{id}/dist/{path}")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => absent.push((local, path.to_owned(), remote)),
                Err(error) => return Err(error).with_context(|| format!("Could not read {}.", local.display())),
            }
        }
        checked += files;
        lines.push((id, version, files, absent.len()));
        writes.push(absent);
    }
    let report = |written: bool| {
        lines
            .iter()
            .map(|(id, version, files, absent)| {
                let now = if *absent > 0 && written { " and now written" } else { "" };
                format!("{id} release {version}: {files} files in the bucket, {absent} missing from dist/{now}.")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ensure!(
        differing.is_empty(),
        "{}\ndist/ holds other bytes than the bucket for {} files, such as {}. Nothing was written; compare them before upgrading.",
        report(false),
        differing.len(),
        differing[0]
    );
    let missing: usize = writes.iter().map(Vec::len).sum();
    if !dry_run {
        for ((id, version, _, _), absent) in lines.iter().zip(writes) {
            if absent.is_empty() { continue; }
            // A publish while the command compared would make these files
            // belong to a release that is no longer live.
            ensure!(
                live_version(data_root, id).await?.as_deref() == Some(version.as_str()),
                "{id} published a new release while artifacts-recover ran, so its missing files were not written. Run it again."
            );
            for (local, path, remote) in absent {
                let parent = local.parent().context("A dist file has no folder.")?;
                tokio::fs::create_dir_all(parent).await?;
                crate::write_atomic(&local, &remote).await?;
                ensure!(tokio::fs::read(&local).await? == remote, "{id}/dist/{path} reads back with other bytes.");
            }
        }
    }
    Ok(format!(
        "{}\n{checked} files compared with {bucket}; {missing} {}.",
        report(!dry_run),
        if dry_run { "would be written. Nothing was written" } else { "written into dist/ and read back" }
    ))
}

/// The live release of one app, or `None` when the app has no Deployment record.
async fn live_version(data_root: &Path, id: &str) -> Result<Option<String>> {
    let path = layout::record(data_root, id);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Could not read {}.", path.display())),
    };
    let installed: Installed = serde_json::from_slice(&bytes).with_context(|| format!("{} is not a Deployment record.", path.display()))?;
    Ok(Some(installed.artifact_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned().filter(|value| !value.is_empty())
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("konae-storage-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(layout::apps(&root)).unwrap();
        root
    }

    async fn add_file(root: &Path, store: &dyn FileStore, app: &str, id: &str, bytes: &[u8]) {
        let index = layout::files(root, app).join("index.json");
        tokio::fs::create_dir_all(index.parent().unwrap()).await.unwrap();
        let mut entries: Vec<serde_json::Value> =
            tokio::fs::read(&index).await.ok().map(|b| serde_json::from_slice(&b).unwrap()).unwrap_or_default();
        entries.push(serde_json::json!({"id": id, "size": bytes.len()}));
        tokio::fs::write(&index, serde_json::to_vec(&entries).unwrap()).await.unwrap();
        let temp = root.join(format!("{id}.tmp"));
        tokio::fs::write(&temp, bytes).await.unwrap();
        store.put(app, id, &temp).await.unwrap();
        tokio::fs::remove_file(temp).await.unwrap();
    }

    fn nix_root() -> bool {
        std::process::Command::new("id").arg("-u").output().map(|out| out.stdout.starts_with(b"0\n")).unwrap_or(false)
    }

    fn id(n: u8) -> String {
        format!("00000000-0000-0000-0000-{n:012}")
    }

    #[test]
    fn settings_files_read_like_podman_env_files() {
        let parsed = parse_settings("# comment\n\nKONAE_S3_BUCKET=konae\r\nKONAE_COPY_SECRET= spaced value \n").unwrap();
        assert_eq!(parsed["KONAE_S3_BUCKET"], "konae");
        assert_eq!(parsed["KONAE_COPY_SECRET"], " spaced value ");
        let error = parse_settings("KONAE_S3_SECRET_KEY\n").unwrap_err().to_string();
        assert!(error.contains("Line 1"), "{error}");
    }

    #[test]
    fn a_settings_file_others_can_read_is_refused() {
        let root = temp_root();
        let path = root.join("legacy.env");
        std::fs::write(&path, "KONAE_DATA_BUCKET=konae-data\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_settings_file(&path).unwrap_err().to_string().contains("chmod 600"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_settings_file(&path).unwrap()["KONAE_DATA_BUCKET"], "konae-data");
    }

    #[test]
    fn one_bucket_refuses_every_older_setting_and_partial_settings() {
        let base = [("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "http://s3"), ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s")];
        for legacy in ["KONAE_DATA_BUCKET", "KONAE_ARTIFACT_BUCKET", "KONAE_COPY_S3_PREFIX", "KONAE_FILES_S3_PREFIX", "KONAE_FILES_S3_ENDPOINT"] {
            let mut pairs = base.to_vec();
            pairs.push((legacy, "x"));
            let error = S3Settings::files(&vars(&pairs)).unwrap_err().to_string();
            assert!(error.contains(legacy), "{error}");
            assert!(S3Settings::copies(&vars(&pairs)).is_err());
        }
        let error = S3Settings::files(&vars(&[("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "http://s3")])).unwrap_err().to_string();
        assert!(error.contains("KONAE_S3_ACCESS_KEY"), "{error}");
        // 0.4.2 ignored these two without a bucket, so an older installation keeps starting.
        assert!(S3Settings::files(&vars(&[("KONAE_FILES_S3_REGION", "fr-par"), ("KONAE_FILES_S3_PATH_STYLE", "1")])).unwrap().is_none());
        let error = S3Settings::files(&vars(&[("KONAE_FILES_S3_PREFIX", "konae-files/")])).unwrap_err().to_string();
        assert!(error.contains("KONAE_FILES_S3_BUCKET is not"), "{error}");
        let root = temp_root();
        let error = copies::CopyConfig::from_vars(&root, true, &vars(&[("KONAE_COPY_S3", "1"), ("KONAE_S3_ENDPOINT", "http://s3")])).unwrap_err().to_string();
        assert!(error.contains("needs KONAE_S3_BUCKET"), "{error}");
        let error = S3Settings::files(&vars(&[("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "http://s3"), ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s"), ("KONAE_S3_PATH_STYLE", "yes")])).unwrap_err().to_string();
        assert!(error.contains("KONAE_S3_PATH_STYLE must be 1 or 0"), "{error}");
    }

    #[test]
    fn one_bucket_puts_files_and_copies_under_fixed_prefixes_with_one_key() {
        let unified = vars(&[
            ("KONAE_S3_BUCKET", "konae-data"), ("KONAE_S3_ENDPOINT", "https://s3.example"), ("KONAE_S3_REGION", "fr-par"),
            ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s"), ("KONAE_COPY_S3", "1"),
        ]);
        let files = S3Settings::files(&unified).unwrap().unwrap();
        assert_eq!((files.bucket.as_str(), files.prefix.as_str(), files.region.as_str()), ("konae-data", "files/", "fr-par"));
        let root = temp_root();
        let copies = copy_location(&root, &unified).unwrap().unwrap();
        assert_eq!((copies.bucket.as_str(), copies.prefix.as_str()), ("konae-data", "copies/"));
        assert_eq!((copies.access_key.as_str(), copies.endpoint.as_str()), (files.access_key.as_str(), files.endpoint.as_str()));
        // The older settings this replaces name the same copy location.
        let legacy = vars(&[
            ("KONAE_S3_ENDPOINT", "https://s3.example"), ("KONAE_S3_REGION", "fr-par"), ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s"),
            ("KONAE_DATA_BUCKET", "konae-data"), ("KONAE_COPY_S3_PREFIX", "copies/"), ("KONAE_ARTIFACT_BUCKET", "konae-apps"),
            ("KONAE_FILES_S3_BUCKET", "photos"), ("KONAE_FILES_S3_ENDPOINT", "https://s3.example"), ("KONAE_FILES_S3_PREFIX", ""),
            ("KONAE_FILES_S3_ACCESS_KEY", "fa"), ("KONAE_FILES_S3_SECRET_KEY", "fs"),
        ]);
        assert_eq!(copy_location(&root, &legacy).unwrap(), Some(copies));
        // An empty older prefix stays empty, as the Kernel records it.
        assert_eq!(S3Settings::files(&legacy).unwrap().unwrap().prefix, "");
    }

    #[tokio::test]
    async fn the_recorded_store_is_the_one_the_kernel_starts_with() {
        let root = temp_root();
        let unified = vars(&[("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "https://s3.example"), ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s")]);
        let legacy = vars(&[("KONAE_FILES_S3_BUCKET", "photos"), ("KONAE_FILES_S3_ENDPOINT", "https://s3.example"), ("KONAE_FILES_S3_ACCESS_KEY", "a"), ("KONAE_FILES_S3_SECRET_KEY", "s")]);
        for (from, to) in [(&legacy, &unified), (&unified, &legacy)] {
            let from = S3Settings::files(from).unwrap();
            let to = S3Settings::files(to).unwrap();
            // What a run writes once every file reads back.
            file_store::write_marker(&root, &file_store::describe(to.as_ref())).await.unwrap();
            file_store::Files::open(&root, file_store::FileLimits::default(), to.clone()).await.unwrap();
            assert!(file_store::Files::open(&root, file_store::FileLimits::default(), from.clone()).await.is_err());
            // A second run after the switch finds nothing to do.
            let side = |files| Side { files, copies: None, key: None };
            let report = migrate(&root, &side(from), &side(to), false, false).await.unwrap();
            assert!(report.contains("already in"), "{report}");
        }
    }

    #[tokio::test]
    async fn a_marker_that_does_not_match_from_switches_nothing() {
        let root = temp_root();
        let other = S3Settings::files(&vars(&[("KONAE_FILES_S3_BUCKET", "other"), ("KONAE_FILES_S3_ENDPOINT", "https://s3.example"), ("KONAE_FILES_S3_ACCESS_KEY", "a"), ("KONAE_FILES_S3_SECRET_KEY", "s")])).unwrap();
        let target = S3Settings::files(&vars(&[("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "https://s3.example"), ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s")])).unwrap();
        let side = |files| Side { files, copies: None, key: None };
        let error = migrate(&root, &side(other), &side(target), false, false).await.unwrap_err().to_string();
        assert!(error.contains("--from names"), "{error}");
        assert_eq!(file_store::read_marker(&root).await.unwrap(), None);
    }

    #[tokio::test]
    async fn copies_stay_where_they_are_with_the_same_secret() {
        let place = |bucket: &str, prefix: &str| S3Settings {
            endpoint: "https://s3.example".into(), region: "fr-par".into(), bucket: bucket.into(), prefix: prefix.into(),
            access_key: "a".into(), secret_key: "s".into(), path_style: true,
        };
        let error = check_copies(Some(&place("konae-data", "copies/")), None, None, None, false).await.unwrap_err().to_string();
        assert!(error.contains("KONAE_COPY_S3=1"), "{error}");
        let error = check_copies(Some(&place("konae-data", "backups/")), Some(&place("konae-data", "copies/")), None, None, false).await.unwrap_err().to_string();
        assert!(error.contains("does not move copies"), "{error}");
        let one = copies::CopyKey::derive(&"a".repeat(32));
        let two = copies::CopyKey::derive(&"b".repeat(32));
        let error = check_copies(Some(&place("konae-data", "copies/")), Some(&place("konae-data", "copies/")), Some(&one), Some(&two), false).await.unwrap_err().to_string();
        assert!(error.contains("KONAE_COPY_SECRET"), "{error}");
    }

    #[tokio::test]
    async fn a_from_without_copy_settings_cannot_skip_a_record_already_in_the_bucket() {
        let endpoint = file_store::tests::fake_s3().await;
        let target = S3Settings {
            endpoint, region: "us-east-1".into(), bucket: "konae".into(), prefix: copies::S3_PREFIX.into(),
            access_key: "konae".into(), secret_key: "konae-secret".into(), path_style: true,
        };
        assert!(check_copies(None, Some(&target), None, None, false).await.unwrap().is_none(), "an empty copies/ has nothing to check");
        file_store::s3_client(&target)
            .put_object().bucket("konae").key("copies/revocations/1.json").body(b"{}".to_vec().into())
            .send().await.unwrap();
        let error = check_copies(None, Some(&target), None, None, false).await.unwrap_err().to_string();
        assert!(error.contains("--from names no S3 copy target") && error.contains("KONAE_DATA_BUCKET"), "{error}");
    }

    #[tokio::test]
    async fn copying_files_verifies_each_and_can_run_again() {
        let root = temp_root();
        let elsewhere = temp_root();
        let source = LocalFolder::new(&root);
        let target = LocalFolder::new(&elsewhere);
        add_file(&root, &source, "app-a", &id(1), b"first").await;
        add_file(&root, &source, "app-a", &id(2), b"second").await;
        add_file(&root, &source, "app-b", &id(3), b"third").await;

        let preflight = copy_files(&root, &source, &target, true).await.unwrap();
        assert_eq!(preflight, Moved { deployments: 2, files: 3, already: 0, bytes: 16, unindexed: 0 });
        assert!(target.deployments().await.unwrap().is_empty(), "a preflight writes nothing");

        // A run that stopped after one file.
        let partial = root.join("partial");
        tokio::fs::write(&partial, b"first").await.unwrap();
        target.put("app-a", &id(1), &partial).await.unwrap();

        let moved = copy_files(&root, &source, &target, false).await.unwrap();
        assert_eq!(moved, Moved { deployments: 2, files: 3, already: 1, bytes: 16, unindexed: 0 });
        assert_eq!(file_store::read_all(&target, "app-b", &id(3)).await.unwrap().unwrap(), b"third");
        assert!(source.get("app-a", &id(2), None).await.unwrap().is_some(), "the source keeps its files");
        assert_eq!(copy_files(&root, &source, &target, false).await.unwrap().already, 3);

        // A killed run leaves its temporary copy; the next run removes it.
        let leftover = root.join("transfers").join(".storage-migrate-killed");
        tokio::fs::write(&leftover, b"patient photo").await.unwrap();
        copy_files(&root, &source, &target, false).await.unwrap();
        assert!(!leftover.exists());
    }

    #[tokio::test]
    async fn copying_files_refuses_missing_resized_conflicting_and_foreign_files() {
        let root = temp_root();
        let elsewhere = temp_root();
        let source = LocalFolder::new(&root);
        let target = LocalFolder::new(&elsewhere);
        add_file(&root, &source, "app-a", &id(1), b"first").await;

        let foreign = root.join("foreign");
        tokio::fs::write(&foreign, b"not ours").await.unwrap();
        target.put("app-z", &id(9), &foreign).await.unwrap();
        let error = copy_files(&root, &source, &target, true).await.unwrap_err().to_string();
        assert!(error.contains("apps this installation does not have"), "{error}");
        target.delete("app-z", &id(9)).await.unwrap();

        // A deleted file of one of its own apps is counted, not refused.
        target.put("app-a", &id(8), &foreign).await.unwrap();
        assert_eq!(copy_files(&root, &source, &target, true).await.unwrap().unindexed, 1);
        target.delete("app-a", &id(8)).await.unwrap();

        target.put("app-a", &id(1), &foreign).await.unwrap();
        let error = copy_files(&root, &source, &target, true).await.unwrap_err().to_string();
        assert!(error.contains("with other bytes"), "{error}");
        target.delete("app-a", &id(1)).await.unwrap();

        add_file(&root, &source, "app-a", &id(2), b"second").await;
        source.delete("app-a", &id(2)).await.unwrap();
        let error = copy_files(&root, &source, &target, true).await.unwrap_err().to_string();
        assert!(error.contains("not in the source"), "{error}");

        tokio::fs::write(&foreign, b"second, longer").await.unwrap();
        source.put("app-a", &id(2), &foreign).await.unwrap();
        let error = copy_files(&root, &source, &target, false).await.unwrap_err().to_string();
        assert!(error.contains("its index says 6"), "{error}");
    }

    #[tokio::test]
    async fn moves_files_from_one_bucket_to_another_and_keeps_the_source() {
        let endpoint = file_store::tests::fake_s3().await;
        let bucket = |bucket: &str, prefix: &str| S3Settings {
            endpoint: endpoint.clone(), region: "us-east-1".into(), bucket: bucket.into(), prefix: prefix.into(),
            access_key: "konae".into(), secret_key: "konae-secret".into(), path_style: true,
        };
        let root = temp_root();
        let old = bucket("photos", "konae-files/");
        let new = bucket("konae-data", file_store::FILES_PREFIX);
        let old_store = S3Store::new(old.clone());
        add_file(&root, &old_store, "app-a", &id(1), b"first").await;
        add_file(&root, &old_store, "app-a", &id(2), &vec![7u8; 150_000]).await;
        file_store::write_marker(&root, &file_store::describe(Some(&old))).await.unwrap();
        let copies_at = bucket("konae-data", copies::S3_PREFIX);
        let side = |files: &S3Settings| Side { files: Some(files.clone()), copies: Some(copies_at.clone()), key: None };

        let preflight = migrate(&root, &side(&old), &side(&new), false, true).await.unwrap();
        assert!(preflight.contains("Preflight: 2 files (150005 bytes) of 1 Deployments"), "{preflight}");
        assert!(preflight.contains("Opened the manifests of 0 copies and no revocation record"), "{preflight}");
        assert_eq!(file_store::read_marker(&root).await.unwrap(), Some(file_store::describe(Some(&old))));

        let report = migrate(&root, &side(&old), &side(&new), false, false).await.unwrap();
        assert!(report.contains("Copied 2 files"), "{report}");
        let new_store = S3Store::new(new.clone());
        assert_eq!(file_store::read_all(&new_store, "app-a", &id(2)).await.unwrap().unwrap(), vec![7u8; 150_000]);
        assert_eq!(old_store.list("app-a").await.unwrap().len(), 2, "the old bucket keeps its files");
        file_store::Files::open(&root, file_store::FileLimits::default(), Some(new.clone())).await.unwrap();

        // Back again, for a rollback after the new bucket took writes and a delete.
        add_file(&root, &new_store, "app-a", &id(3), b"written after the switch").await;
        let index = layout::files(&root, "app-a").join("index.json");
        let mut entries: Vec<serde_json::Value> = serde_json::from_slice(&tokio::fs::read(&index).await.unwrap()).unwrap();
        entries.retain(|entry| entry["id"] != id(1));
        tokio::fs::write(&index, serde_json::to_vec(&entries).unwrap()).await.unwrap();
        let back = migrate(&root, &side(&new), &side(&old), false, false).await.unwrap();
        assert!(back.contains("Copied 2 files") && back.contains("holds 1 files of these apps that no index names"), "{back}");
        assert_eq!(file_store::read_all(&old_store, "app-a", &id(3)).await.unwrap().unwrap(), b"written after the switch");
        file_store::Files::open(&root, file_store::FileLimits::default(), Some(old)).await.unwrap();
    }

    #[tokio::test]
    async fn artifacts_recover_writes_only_missing_dist_files() {
        let endpoint = file_store::tests::fake_s3().await;
        let get = vars(&[("KONAE_S3_ENDPOINT", endpoint.as_str()), ("KONAE_ARTIFACT_BUCKET", "konae-apps")]);
        let root = temp_root();
        let record = layout::record(&root, "app-a");
        tokio::fs::create_dir_all(record.parent().unwrap()).await.unwrap();
        tokio::fs::write(&record, br#"{"hostname":"a.localhost","createdAt":1,"actions":"{}","artifactVersion":"v1"}"#).await.unwrap();
        let dist = layout::dist(&root, "app-a");
        tokio::fs::create_dir_all(&dist).await.unwrap();
        tokio::fs::write(dist.join("index.html"), b"<p>live</p>").await.unwrap();
        let client = file_store::s3_client(&S3Settings::copies(&get).unwrap().unwrap());
        for (key, body) in [("v1/index.html", "<p>live</p>"), ("v1/assets/app.js", "run()"), ("v1/assets/", ""), ("v0/old.js", "stale")] {
            client.put_object().bucket("konae-apps").key(format!("deployments/app-a/{key}")).body(body.as_bytes().to_vec().into()).send().await.unwrap();
        }

        let dry = recover_artifacts(&root, &get, true).await.unwrap();
        assert!(dry.contains("2 files in the bucket, 1 missing from dist/"), "{dry}");
        assert!(!dist.join("assets/app.js").exists());
        let done = recover_artifacts(&root, &get, false).await.unwrap();
        assert!(done.contains("now written"), "{done}");
        assert_eq!(std::fs::read(dist.join("assets/app.js")).unwrap(), b"run()");
        assert!(!dist.join("old.js").exists(), "only the live release");

        // One file differs and another is missing: the run stops before it writes either.
        tokio::fs::write(dist.join("index.html"), b"<p>edited</p>").await.unwrap();
        tokio::fs::remove_file(dist.join("assets/app.js")).await.unwrap();
        let error = recover_artifacts(&root, &get, false).await.unwrap_err().to_string();
        assert!(error.contains("app-a/dist/index.html") && error.contains("Nothing was written"), "{error}");
        assert_eq!(std::fs::read(dist.join("index.html")).unwrap(), b"<p>edited</p>");
        assert!(!dist.join("assets/app.js").exists(), "nothing written when a file differs");

        // A record that cannot be read stops the run instead of skipping the app.
        tokio::fs::write(dist.join("index.html"), b"<p>live</p>").await.unwrap();
        std::fs::set_permissions(&record, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        let unreadable = recover_artifacts(&root, &get, true).await;
        std::fs::set_permissions(&record, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        if nix_root() { /* root reads any file */ } else {
            assert!(unreadable.unwrap_err().to_string().contains("deployment.json"));
        }
        let unified = vars(&[("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", endpoint.as_str())]);
        assert!(recover_artifacts(&root, &unified, true).await.unwrap_err().to_string().contains("older settings"));
    }
}
