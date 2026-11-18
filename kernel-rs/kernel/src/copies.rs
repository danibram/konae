
pub(crate) mod history;
mod validation;
#[cfg(test)]
mod history_tests;

use crate::installation::Installation;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
#[cfg(test)]
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use sha2::{Digest, Sha256};
use crate::{AppState, database_lock, list_s3_keys, now_ms, read_s3_bytes, recovery};
pub use konae_backup::{checksum_file, civil_from_days, vacuum_into};
use konae_backup::{
    CopyReader, MANIFEST, Manifest, ManifestCache, ManifestFile, PathScope, S3CopyTarget, Source,
    check_sealed, copy_id, newest_authentic_copy, normalize_prefix, parse_copy_id, prune_directory, prune_s3_cached,
    read_manifest, send_to_directory, send_to_s3, verified_bytes,
};
#[cfg(test)]
use konae_backup::{DAY_MS, MAX_MANIFEST_BYTES, manifest_cache_key, encode_manifest, expired_copies, list_files, sealed_manifest};

impl PathScope for Installation {
    fn allows(&self, path: &str) -> bool { self.holds(path) || path.starts_with("apps/") }
}

pub(crate) const DEPLOYMENT_DATABASES: [&str; 4] = ["database.sqlite", "identity.sqlite", "usage.sqlite", crate::layout::SECRETS_DATABASE];
/// The prefix of copies in the installation's one bucket.
pub const S3_PREFIX: &str = "copies/";
const DEFAULT_INTERVAL_SECONDS: u64 = 3600;


#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Directory(PathBuf),
    S3 { prefix: String },
}

impl Target {
    fn kind(&self) -> &'static str {
        match self {
            Target::Directory(_) => "directory",
            Target::S3 { .. } => "s3",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CopyConfig {
    pub targets: Vec<Target>,
    pub interval: Duration,
}

impl CopyConfig {
    /// `s3_configured` says whether `KONAE_S3_BUCKET` names a bucket.
    pub fn from_env(data_root: &Path, s3_configured: bool) -> Result<Option<Self>> {
        // Refuses a short secret at startup rather than at the first copy.
        CopyKey::from_env()?;
        let get = crate::file_store::env_var;
        Self::parse(
            get("KONAE_COPY_DIR").as_deref(),
            get("KONAE_COPY_S3").as_deref(),
            get("KONAE_COPY_INTERVAL_SECONDS").as_deref(),
            data_root,
            s3_configured,
        )
    }

    fn parse(
        directory: Option<&str>,
        copy_s3: Option<&str>,
        interval: Option<&str>,
        data_root: &Path,
        s3_configured: bool,
    ) -> Result<Option<Self>> {
        let mut targets = Vec::new();
        if let Some(directory) = directory {
            let directory = std::path::absolute(directory)?;
            let data_root = std::path::absolute(data_root)?;
            ensure!(
                !directory.starts_with(&data_root) && !data_root.starts_with(&directory),
                "KONAE_COPY_DIR must be outside KONAE_DATA_DIR, on another disk."
            );
            targets.push(Target::Directory(directory));
        }
        match copy_s3 {
            None => {}
            Some("1") => {
                ensure!(s3_configured, "KONAE_COPY_S3=1 needs KONAE_S3_BUCKET.");
                targets.push(Target::S3 { prefix: S3_PREFIX.into() });
            }
            Some(_) => bail!("KONAE_COPY_S3 must be 1 when set."),
        }
        if targets.is_empty() {
            return Ok(None);
        }
        let seconds = match interval {
            Some(value) => value
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|seconds| *seconds > 0)
                .context("KONAE_COPY_INTERVAL_SECONDS must be a whole number of seconds above zero.")?,
            None => DEFAULT_INTERVAL_SECONDS,
        };
        Ok(Some(Self { targets, interval: Duration::from_secs(seconds) }))
    }
}

mod key;
pub use key::CopyKey;

mod health;
pub use health::CopyStatus;
pub(crate) use health::status_directory;

// ---------------------------------------------------------------------------
// Scheduled copy

pub fn spawn_schedule(state: AppState, config: CopyConfig) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            // run_copy logs and reports failures in health; the next tick tries again.
            let _ = run_copy(&state, &config.targets).await;
        }
    });
}

#[derive(Clone, Serialize, Deserialize)]
struct CopyState {
    files: Vec<ManifestFile>,
    #[serde(default)]
    s3_manifests: HashMap<String, Manifest>,
}

struct StagedCopy {
    manifest: Manifest,
    legacy_blobs: HashMap<String, String>,
}

/// Takes one copy and sends it to every target, then prunes old copies.
/// Returns the copy ID when every target succeeded.
pub async fn run_copy(state: &AppState, targets: &[Target]) -> Result<String> {
    let started = now_ms();
    let id = copy_id(started);
    for target in targets {
        state.copies.update(target.kind(), |report| {
            report.last_attempt_at = Some(started);
            report.running = true;
        }).await;
    }
    let staging_root = state.data_root.join(".copy-staging");
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    let staging = staging_root.join(&id);
    let mut cache = ManifestCache::new();
    let staged = stage_copy(state, &staging, started, &mut cache).await;
    let mut failures = Vec::new();
    let mut published = false;
    for target in targets {
        let kind = target.kind();
        let outcome = match &staged {
            Err(error_value) => Err(anyhow::anyhow!("the snapshot failed: {error_value:#}")),
            Ok(staged) => match recovery::start(target, state.s3.as_ref(), &state.data_bucket).await {
                Ok(()) => send(state, target, &staging, staged, &id, &mut cache).await,
                Err(error_value) => Err(error_value.context("the revocation record could not be started")),
            },
        };
        let result = match outcome {
            Ok(()) => {
                published = true;
                state.copies.update(kind, |report| {
                    report.last_cut_at = Some(started);
                    report.last_success_at = Some(now_ms());
                    report.last_copy_id = Some(id.clone());
                    report.last_error = None;
                }).await;
                prune(state, target, &id, &mut cache).await.context("The copy is complete but pruning failed")
            }
            Err(error) => Err(error),
        };
        let error = match result {
            Ok(pruned) => {
                println!("Copy {id} sent to the {kind} target; pruned {pruned} old copies.");
                None
            }
            Err(error) => {
                let message = format!("{error:#}");
                eprintln!("Copy {id} to the {kind} target failed: {message}");
                failures.push(format!("{kind}: {message}"));
                Some(message)
            }
        };
        state.copies.update(kind, |report| {
            report.running = false;
            report.last_error = error;
        }).await;
    }
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    if published && let Ok(staged) = staged {
        let saved = CopyState {
            files: staged.manifest.files,
            s3_manifests: cache.into_iter().filter_map(|(key, result)| result.ok().map(|manifest| (key, manifest))).collect(),
        };
        crate::write_atomic(&state.data_root.join(".copy-state.json"), &serde_json::to_vec(&saved)?).await?;
    }
    if failures.is_empty() { Ok(id) } else { bail!("{}", failures.join("; ")) }
}

async fn send(state: &AppState, target: &Target, staging: &Path, staged: &StagedCopy, id: &str, cache: &mut ManifestCache) -> Result<()> {
    match target {
        Target::Directory(directory) => {
            let (staging, directory, id, manifest) = (staging.to_owned(), directory.clone(), id.to_owned(), staged.manifest.clone());
            let legacy = staged.legacy_blobs.clone();
            let installation = state.installation.clone();
            tokio::task::spawn_blocking(move || send_to_directory(&staging, &directory, &id, &manifest, &legacy, installation.as_ref())).await?
        }
        Target::S3 { prefix } => {
            let client = state.s3.as_ref().context("S3 is not configured.")?;
            let target = S3CopyTarget { client, bucket: &state.data_bucket, prefix, key: state.copies.key().map(CopyKey::files) };
            send_to_s3(target, staging, &staged.manifest, id, cache, &staged.legacy_blobs, state.installation.as_ref()).await
        }
    }
}

async fn prune(state: &AppState, target: &Target, id: &str, cache: &mut ManifestCache) -> Result<usize> {
    match target {
        Target::Directory(directory) => {
            let directory = directory.clone();
            tokio::task::spawn_blocking(move || prune_directory(&directory, now_ms())).await?
        }
        Target::S3 { prefix } => {
            let s3 = state.s3.as_ref().context("S3 is not configured.")?;
            prune_s3_cached(s3, &state.data_bucket, prefix, id, now_ms(), cache).await
        }
    }
}

/// Builds the copy in `staging`. Each Deployment is snapshotted under its
/// data and identity locks, the same locks an action, an upload, a signup and
/// a publish take, so its databases, files and artifact agree with each other.
async fn stage_copy(state: &AppState, staging: &Path, created_at: u64, cache: &mut ManifestCache) -> Result<StagedCopy> {
    let prior = tokio::fs::read(state.data_root.join(".copy-state.json")).await.ok()
        .and_then(|bytes| serde_json::from_slice::<CopyState>(&bytes).ok());
    if let Some(prior) = prior.as_ref() {
        for (key, manifest) in &prior.s3_manifests {
            cache.insert(key.clone(), Ok(manifest.clone()));
        }
    }
    let previous_files: HashMap<_, _> = prior.iter().flat_map(|prior| &prior.files)
        .map(|file| (file.path.as_str(), file.sha256.as_str())).collect();
    tokio::fs::create_dir_all(crate::layout::apps(staging)).await?;
    for name in state.installation.databases.iter().map(|database| database.file) {
        let source = state.data_root.join(name);
        if source.exists() {
            let target = staging.join(name);
            tokio::task::spawn_blocking(move || vacuum_into(&source, &target)).await??;
        }
    }
    let mut indexed_files = Vec::new();
    let mut entries = tokio::fs::read_dir(crate::layout::apps(&state.data_root)).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let Some(deployment) = entry.file_name().to_str().map(str::to_owned) else { continue };
        // An editor save swaps app/ under the project lock.
        let _project = database_lock(state, &format!("project:{deployment}")).await;
        let _data = database_lock(state, &deployment).await;
        let _identity = database_lock(state, &format!("identity:{deployment}")).await;
        let (source, target) = (entry.path(), crate::layout::folder(staging, &deployment));
        tokio::task::spawn_blocking(move || copy_deployment(&source, &target))
            .await?
            .with_context(|| format!("Deployment {deployment}"))?;
        drop((_project, _identity));
        copy_file_bytes(state, &deployment, &crate::layout::files(staging, &deployment), &previous_files, &mut indexed_files)
            .await
            .with_context(|| format!("The files of Deployment {deployment}"))?;
    }
    let root = staging.to_owned();
    tokio::task::spawn_blocking(move || {
        let manifest = Manifest::capture(&root, created_at)?;
        let staged: HashMap<_, _> = manifest.files.iter().map(|file| (file.path.as_str(), file)).collect();
        let mut legacy_blobs = HashMap::new();
        for (path, size, digest, legacy) in indexed_files {
            let file = staged.get(path.as_str()).with_context(|| format!("Staging is missing indexed file {path}"))?;
            ensure!(file.size == size && file.sha256 == digest, "Indexed file {path} changed in staging.");
            if legacy { legacy_blobs.insert(path, digest); }
        }
        Ok(StagedCopy { manifest, legacy_blobs })
    })
    .await?
}

async fn copy_file_bytes(
    state: &AppState, deployment: &str, files: &Path, previous_files: &HashMap<&str, &str>,
    indexed_files: &mut Vec<(String, u64, String, bool)>,
) -> Result<()> {
    let index = match tokio::fs::read(files.join("index.json")).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let inventory: HashMap<_, _> = state.files.store.objects(deployment).await?
        .into_iter().map(|object| (object.id, object.size)).collect();
    let mut seen = HashSet::new();
    for entry in &index {
        let id = entry["id"].as_str().filter(|id| crate::valid_file_id(id)).context("The file index has an invalid ID.")?;
        ensure!(seen.insert(id), "The file index lists {id} twice.");
        let indexed_size = entry["size"].as_u64().with_context(|| format!("File {id} has no indexed size."))?;
        let stored_size = inventory.get(id).with_context(|| format!("Indexed file {id} is missing from the file store."))?;
        ensure!(*stored_size == indexed_size, "File {id} has {stored_size} bytes in the store. The index requires {indexed_size}.");
        let blob = state.files.store.get(deployment, id, None).await
            .with_context(|| format!("Could not read indexed file {id}"))?
            .with_context(|| format!("Indexed file {id} disappeared before it could be read."))?;
        let (size, digest) = crate::file_store::save_blob(blob, &files.join(format!("{id}.blob"))).await
            .with_context(|| format!("Could not stage indexed file {id}"))?;
        ensure!(size == indexed_size, "Read {size} bytes from file {id}. The index requires {indexed_size}.");
        let path = format!("apps/{deployment}/data/files/{id}.blob");
        let expected = match entry.get("sha256") {
            Some(value) => Some(value.as_str().with_context(|| format!("File {id} has an invalid indexed SHA-256."))?),
            None => previous_files.get(path.as_str()).copied(),
        };
        ensure!(expected.is_none_or(|expected| expected == digest), "File {id} does not match its recorded SHA-256.");
        indexed_files.push((path, size, digest, entry.get("sha256").is_none()));
    }
    Ok(())
}

fn copy_deployment(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let (from, to) = (entry.path(), target.join(&name));
        match &*name.to_string_lossy() {
            crate::layout::SETTINGS_FILE if kind.is_file() => {
                std::fs::copy(&from, &to)?;
            }
            "app" | "dist" if kind.is_dir() => copy_tree(&from, &to)?,
            "data" if kind.is_dir() => copy_data(&from, &to)?,
            ".konae" => {
                ensure!(kind.is_dir(), "The app's .konae path must be a directory.");
                copy_private(&from, &to)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn copy_data(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let (from, to) = (entry.path(), target.join(&name));
        match &*name.to_string_lossy() {
            database if kind.is_file() && DEPLOYMENT_DATABASES.contains(&database) => vacuum_into(&from, &to)?,
            "files" if kind.is_dir() => {
                std::fs::create_dir_all(&to)?;
                match std::fs::copy(from.join("index.json"), to.join("index.json")) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn copy_private(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let (from, to) = (entry.path(), target.join(&name));
        match &*name.to_string_lossy() {
            "deployment.json" | "sources.json" | crate::maintenance::MARKER_FILE | crate::domain::RECORD_FILE | crate::path::RECORD_FILE | crate::hostname::RECORD_FILE if kind.is_file() => {
                std::fs::copy(&from, &to)?;
            }
            "backups" => {
                ensure!(kind.is_dir(), "The app's backups path must be a directory.");
                history::copy_records(&from, &to)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') && name != ".konae" {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target.join(&name))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
async fn s3_object_sizes(s3: &S3Client, bucket: &str, prefix: &str) -> Result<HashMap<String, u64>> {
    let mut result = HashMap::new();
    let mut continuation = None;
    loop {
        let page = s3.list_objects_v2().bucket(bucket).prefix(prefix)
            .set_continuation_token(continuation.take()).send().await?;
        for object in page.contents() {
            if let (Some(key), Some(size)) = (object.key(), object.size()) {
                result.insert(key.to_owned(), u64::try_from(size)?);
            }
        }
        match page.next_continuation_token() {
            Some(token) if page.is_truncated() == Some(true) => continuation = Some(token.to_owned()),
            _ => break,
        }
    }
    Ok(result)
}

#[cfg(test)]
async fn prune_s3(s3: &S3Client, bucket: &str, prefix: &str, current: &str, now: u64) -> Result<usize> {
    prune_s3_cached(s3, bucket, prefix, current, now, &mut ManifestCache::new()).await
}

fn restore_usage() -> String {
    format!("Usage: {} restore --from <copy directory | s3://bucket/prefix> [--copy <copy ID>] [--require-revocation <entry SHA-256>]", crate::program())
}

/// Handles the `restore` command. Returns false for any other command.
pub async fn operator_command(installation: &Installation) -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("restore") {
        return Ok(false);
    }
    let mut from = None;
    let mut copy = None;
    let mut witness = None;
    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--from" => from = rest.next().cloned(),
            "--copy" => copy = Some(rest.next().with_context(restore_usage)?.clone()),
            "--require-revocation" => {
                let hash = rest.next().with_context(restore_usage)?;
                ensure!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "Pass the SHA-256 of the revocation entry.");
                witness = Some(hash.to_ascii_lowercase());
            }
            _ => bail!("{}", restore_usage()),
        }
    }
    let from = from.with_context(restore_usage)?;
    let destination = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let source = source_for(&from).await?;
    let summary = restore_with_receipt(
        &source, copy.as_deref(), &destination, CopyKey::from_env()?.as_ref(),
        installation, &status_directory(&destination), witness.as_deref(),
    ).await?;
    println!(
        "Restored copy {} into {}: {} Deployments, {} files, checksums verified. Signed out {} sessions and credentials and ended every open email link; owners, agents and app users sign in again.",
        summary.copy_id,
        destination.display(),
        summary.deployments,
        summary.files,
        summary.sessions_removed
    );
    match summary.reconciled {
        Ok(changed) => println!("Applied the revocation record: {changed} rows changed, so no account suspended or API key revoked after this copy is active again."),
        Err(reason) => println!(
            "The installation starts in maintenance: {reason} Run {program} reconcile --from <the same source> once the record is reachable, or {program} reconcile --without-record to reopen without it.", program = crate::program()
        ),
    }
    Ok(true)
}

async fn restore_with_receipt(source: &Source, copy: Option<&str>, destination: &Path, key: Option<&CopyKey>,
    installation: &Installation, reports: &Path, witness: Option<&str>) -> Result<RestoreSummary> {
    let summary = restore_with_witness(source, copy, destination, key, installation, witness).await?;
    if summary.reconciled.is_ok() {
        health::record_restore(source, reports, &summary.copy_id).await?;
    }
    Ok(summary)
}

/// Reads the revocation record next to the copies at `from`, for
/// The `reconcile` command. `None` when the source keeps no record.
pub async fn read_record(from: &str, key: Option<&CopyKey>) -> Result<Option<Vec<Vec<u8>>>> {
    let source = source_for(from).await?;
    let source = match source {
        Source::Directory(path) if path.join(MANIFEST).is_file() => Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned()),
        other => other,
    };
    read_source_record(&source, key).await
}

async fn source_for(from: &str) -> Result<Source> {
    Ok(match from.strip_prefix("s3://") {
        Some(location) => {
            let (bucket, prefix) = location.split_once('/').unwrap_or((location, ""));
            ensure!(!bucket.is_empty(), "{}", restore_usage());
            let (client, _) = crate::copy_bucket_from_env()?
                .context("Set KONAE_S3_BUCKET, KONAE_S3_ENDPOINT and its keys to restore from S3.")?;
            Source::S3 { client, bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) }
        }
        None => Source::Directory(PathBuf::from(from)),
    })
}

async fn read_source_record(source: &Source, key: Option<&CopyKey>) -> Result<Option<Vec<Vec<u8>>>> {
    let dir = recovery::LEDGER_DIR;
    let start = recovery::LEDGER_START;
    match source {
        Source::Directory(root) => {
            let folder = root.join(dir);
            if !folder.join(start).is_file() {
                return Ok(None);
            }
            check_record_start(&tokio::fs::read(folder.join(start)).await?)?;
            let mut files = Vec::new();
            for entry in std::fs::read_dir(&folder)? {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if name.ends_with(".json") && name != start && !name.starts_with('.') {
                    files.push(tokio::fs::read(folder.join(&name)).await?);
                }
            }
            Ok(Some(files))
        }
        Source::S3 { client, bucket, prefix } => {
            let base = format!("{prefix}{dir}/");
            let keys = list_s3_keys(client, bucket, &base).await?;
            if !keys.iter().any(|key| key[base.len()..] == *start) {
                return Ok(None);
            }
            check_record_start(&read_s3_bytes(client, bucket, &format!("{base}{start}")).await?)?;
            let mut files = Vec::new();
            for object in keys.iter().filter(|object| object.ends_with(".json") && object[base.len()..] != *start) {
                let bytes = read_s3_bytes(client, bucket, object).await?;
                if let Some(key) = key {
                    recovery::check_signed(key, &object[base.len()..], &bytes)?;
                }
                files.push(bytes);
            }
            Ok(Some(files))
        }
    }
}

fn check_record_start(bytes: &[u8]) -> Result<()> {
    let marker: Value = serde_json::from_slice(bytes).context("Invalid revocation record start marker.")?;
    ensure!(marker["version"] == 1, "Unsupported revocation record start marker.");
    Ok(())
}

struct RestoreSummary {
    copy_id: String,
    deployments: usize,
    files: usize,
    sessions_removed: usize,
    reconciled: std::result::Result<usize, String>,
}

#[cfg(test)]
async fn restore(
    source: &Source,
    copy: Option<&str>,
    destination: &Path,
    key: Option<&CopyKey>,
    installation: &Installation,
) -> Result<RestoreSummary> {
    restore_with_witness(source, copy, destination, key, installation, None).await
}

async fn restore_with_witness(
    source: &Source,
    copy: Option<&str>,
    destination: &Path,
    key: Option<&CopyKey>,
    installation: &Installation,
    witness: Option<&str>,
) -> Result<RestoreSummary> {
    let _volume = crate::file_store::hold_while_stopped(destination, "restore")?;
    ensure_empty(destination)?;
    let direct;
    let (source, copy) = match (source, copy) {
        (Source::Directory(path), None) if path.join(MANIFEST).is_file() => {
            let name = path.file_name().and_then(|name| name.to_str()).context("The copy directory has no name.")?;
            direct = Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned());
            (&direct, name.to_owned())
        }
        (source, Some(copy)) => (source, copy.to_owned()),
        (source, None) if matches!(source, Source::S3 { .. }) && key.is_some() => {
            let key = key.expect("checked by the guard");
            (source, newest_authentic_copy(source, key, &mut |line| println!("{line}")).await?)
        }
        (source, None) => {
            let latest = source.complete_copies().await?.pop();
            (source, latest.context("No complete copy found. A complete copy has a manifest.json.")?)
        }
    };
    let reader = CopyReader::open(source, &copy, key.map(CopyKey::files), installation).await.map_err(|error| {
        if error.is::<konae_backup::UnencryptedS3Copy>() {
            anyhow::anyhow!("{error} Name a copy this installation sent with --copy, for example from the copy IDs under the S3 prefix: {} restore --from <source> --copy <copy ID>. Nothing was installed.", crate::program())
        } else {
            error
        }
    })?;
    let manifest = reader.manifest();
    let staging = destination.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    reader.restore(&staging).await?;
    let staged = async {
        let root = staging.clone();
        let stores = installation.clone();
        let copy_id = copy.clone();
        let files = manifest.files.clone();
        let removed = tokio::task::spawn_blocking(move || -> Result<usize> {
            validation::check_materialized(&root, &files)?;
            history::warn_incomplete(&root, &copy_id)?;
            check_databases(&root, &stores)?;
            validation::check_cut(&root, &stores)?;
            invalidate_sessions(&root, &stores)
        })
        .await??;
        let record = read_source_record(source, key).await;
        let witness = witness.map(str::to_owned);
        let cut = manifest.created_at;
        let key_check = if matches!(source, Source::S3 { .. }) {
            key.map(|k| k.fingerprint().to_owned())
        } else { None };
        let root = staging.clone();
        let stores = installation.clone();
        let reconciled = tokio::task::spawn_blocking(move || match witness {
            Some(hash) => recovery::prove_revocation(&root, &stores, record, &hash, cut).map(Ok),
            None => recovery::apply_or_hold_for_key(&root, &stores, record, key_check.as_deref()),
        }).await??;
        Ok((removed, reconciled))
    }
    .await;
    let (sessions_removed, reconciled) = match staged {
        Ok(count) => count,
        Err(error_value) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error_value);
        }
    };
    let root = destination.to_owned();
    tokio::task::spawn_blocking(move || activate(&staging, &root, sync_path)).await??;
    let deployments = manifest
        .files
        .iter()
        .filter(|file| file.path.ends_with("/deployment.json"))
        .count();
    Ok(RestoreSummary { copy_id: copy, deployments, files: manifest.files.len(), sessions_removed, reconciled })
}

fn sync_path(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all().with_context(|| format!("Could not sync {}", path.display()))
}

fn sync_tree(path: &Path) -> Result<()> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
    }
    sync_path(path)
}

fn activate(staging: &Path, destination: &Path, sync_dir: impl Fn(&Path) -> Result<()>) -> Result<()> {
    sync_dir(destination)?;
    for entry in std::fs::read_dir(staging)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        ensure!(!target.try_exists()?, "Restore destination is occupied at {}.", target.display());
        sync_tree(&entry.path())?;
        std::fs::rename(entry.path(), target)?;
        sync_dir(destination)?;
    }
    sync_dir(staging)?;
    std::fs::remove_dir(staging)?;
    if let Err(error) = sync_dir(destination) {
        // The tree is durable. Restore the startup guard while the volume lock is still held.
        std::fs::create_dir(staging).context("Could not restore the activation marker after a failed sync")?;
        sync_dir(destination).context("Could not sync the restored activation marker")?;
        return Err(error);
    }
    Ok(())
}

pub(crate) fn check_activation(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        ensure!(!entry?.file_name().to_string_lossy().starts_with(".restore-"),
            "Installation restore is incomplete. Repeat restore into an empty data directory before starting the Kernel.");
    }
    Ok(())
}

fn ensure_empty(destination: &Path) -> Result<()> {
    for entry in std::fs::read_dir(destination)? {
        let entry = entry?;
        if entry.file_name() == crate::file_store::KERNEL_LOCK_FILE || entry.file_name() == "lost+found" {
            continue;
        }
        bail!("Restore refuses {}: it is not empty. Point KONAE_DATA_DIR at an empty directory.", destination.display());
    }
    Ok(())
}

fn sqlite_files(root: &Path, installation: &Installation) -> Result<Vec<PathBuf>> {
    let mut paths = installation.databases.iter().map(|database| root.join(database.file)).filter(|path| path.is_file()).collect::<Vec<_>>();
    if let Ok(entries) = std::fs::read_dir(crate::layout::apps(&root)) {
        for entry in entries {
            let data = entry?.path().join("data");
            for name in DEPLOYMENT_DATABASES {
                if data.join(name).is_file() {
                    paths.push(data.join(name));
                }
            }
        }
    }
    Ok(paths)
}

fn check_databases(root: &Path, installation: &Installation) -> Result<()> {
    for path in sqlite_files(root, installation)? {
        let db = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let result: String = db.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        ensure!(result == "ok", "{} failed SQLite's integrity check.", path.display());
        ensure!(db.prepare("PRAGMA foreign_key_check")?.query([])?.next()?.is_none(), "{} has a broken foreign key.", path.display());
    }
    Ok(())
}

fn invalidate_sessions(root: &Path, installation: &Installation) -> Result<usize> {
    let tables = installation.databases.iter().map(|database| (database.file, database.sign_out));
    let mut removed = 0;
    let mut clear = |path: &Path, names: &[&str]| -> Result<()> {
        let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        for name in names {
            if crate::table_exists(&connection, name)? {
                removed += connection.execute(&format!("DELETE FROM \"{name}\""), [])?;
            }
        }
        Ok(())
    };
    for (file, names) in tables {
        if root.join(file).is_file() {
            clear(&root.join(file), names)?;
        }
    }
    if let Ok(entries) = std::fs::read_dir(crate::layout::apps(&root)) {
        for entry in entries {
            let identity = entry?.path().join("data").join("identity.sqlite");
            if identity.is_file() {
                clear(&identity, &["app_sessions", "account_challenges", "entry_links"])?;
            }
        }
    }
    retire_restored_work(root)?;
    let mail = root.join(crate::mail::MAIL_DATABASE);
    if mail.is_file() {
        removed += crate::mail::end_pending_after_restore(&mail)?;
    }
    Ok(removed)
}

fn retire_restored_work(root: &Path) -> Result<()> {
    let path = root.join(crate::calls::CALLS_DATABASE);
    if path.try_exists()? {
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        if crate::table_exists(&db, "calls")? {
            db.execute_batch("UPDATE calls SET state='failed', error='restored-outcome-unknown', leased_until=NULL
                              WHERE state='pending';
                              UPDATE calls SET then_state='interrupted', then_message='Check the continuation before repeating it.', leased_until=NULL
                              WHERE then_state='processing';")?;
        }
    }
    // The first timer tick takes a new baseline. The run history stays available.
    let apps = crate::layout::apps(root);
    if apps.try_exists()? {
        for entry in std::fs::read_dir(apps)? {
            let path = entry?.path().join("data/usage.sqlite");
            if path.try_exists()? {
                let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
                if crate::table_exists(&db, "schedule_state")? {
                    db.execute("DELETE FROM schedule_state", [])?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    mod activation;
    mod integrity;
    use super::*;

    #[test]
    fn snapshot_refuses_a_missing_source_without_creating_either_file() {
        let root = scratch("missing-snapshot");
        std::fs::create_dir(&root).unwrap();
        assert!(vacuum_into(&root.join("source.sqlite"), &root.join("copy.sqlite")).is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn a_copy_restores_a_legacy_draft_without_creating_optional_stores() {
        let root = scratch("draft-inventory");
        let source = root.join("live");
        let copies = root.join("copies");
        let restored = root.join("restored");
        let id = "old_draft";
        crate::Settings::new(id, "Project", "project", &json!({"version":1})).write(&source).await.unwrap();
        let app = crate::layout::sources(&source, id);
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("index.html"), "<main>Draft</main>").unwrap();
        let private = crate::layout::private(&source, id);
        std::fs::create_dir_all(&private).unwrap();
        let metadata = json!({"name":"Original draft","slug":"original","manifest":{"version":1},"directories":["empty"],"updatedAt":77});
        std::fs::write(private.join("sources.json"), metadata.to_string()).unwrap();
        let original = crate::sources::read(&source, id).await.unwrap().unwrap();
        let state = state(&source);
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        let summary = restore(&Source::Directory(copies), None, &restored, None, &state.installation).await.unwrap();
        assert_eq!(summary.deployments, 0);
        assert_eq!(original["name"], "Original draft");
        assert_eq!(crate::sources::read(&restored, id).await.unwrap(), Some(original));
        for volume in [&source, &restored] {
            assert!(!crate::layout::data(volume, id).exists());
            assert!(!crate::layout::dist(volume, id).exists());
            assert!(!volume.join("google.sqlite").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_carries_the_custom_domain_with_the_record_and_the_maintenance_state() {
        let root = std::env::temp_dir().join(format!("konae-copy-domain-{}", uuid::Uuid::new_v4()));
        let (source, target) = (root.join("source"), root.join("target"));
        std::fs::create_dir_all(source.join(".konae")).unwrap();
        for name in ["deployment.json", crate::maintenance::MARKER_FILE, crate::domain::RECORD_FILE, crate::path::RECORD_FILE, crate::hostname::RECORD_FILE, "stray.tmp"] {
            std::fs::write(source.join(".konae").join(name), "{}").unwrap();
        }
        let target = target.join(".konae");
        copy_deployment(&source, &target.parent().unwrap()).unwrap();
        assert!(target.join("deployment.json").exists());
        assert!(target.join(crate::domain::RECORD_FILE).exists());
        assert!(target.join(crate::path::RECORD_FILE).exists());
        assert!(target.join(crate::hostname::RECORD_FILE).exists());
        assert!(target.join(crate::maintenance::MARKER_FILE).exists());
        assert!(!target.join("stray.tmp").exists());
        let _ = std::fs::remove_dir_all(root);
    }
    use crate::{
        Deployment, SignupPolicy, auth::AuthService, initialize_database, load_deployments, read_artifact,
    };
    use std::sync::Arc;

    fn installed_entries(root: &Path) -> usize {
        std::fs::read_dir(root).unwrap().filter(|entry| entry.as_ref().unwrap().file_name() != crate::file_store::KERNEL_LOCK_FILE).count()
    }

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("konae-{label}-{}", uuid::Uuid::new_v4()))
    }

    #[cfg(unix)]
    #[test]
    fn directory_copies_link_unchanged_files_and_copy_changed_files() {
        use std::os::unix::fs::MetadataExt;
        let root = scratch("hard-link");
        let (directory, first, second) = (root.join("copies"), root.join("first"), root.join("second"));
        let path = "apps/app/data/usage.sqlite";
        std::fs::create_dir_all(first.join("apps/app/data")).unwrap();
        std::fs::create_dir_all(second.join("apps/app/data")).unwrap();
        std::fs::write(first.join(path), b"same bytes").unwrap();
        std::fs::write(second.join(path), b"same bytes").unwrap();
        let (size, sha256) = checksum_file(&first.join(path)).unwrap();
        let manifest = |copy_id: &str| Manifest {
            version: 1, copy_id: copy_id.into(), created_at: 0,
            files: vec![ManifestFile { path: path.into(), size, sha256: sha256.clone(), home_copy: None }],
            encryption: None, mac: None,
        };
        std::fs::write(first.join(MANIFEST), serde_json::to_vec(&manifest("2026-01-01T00-00-00.000Z")).unwrap()).unwrap();
        let first_id = "2026-01-01T00-00-00.000Z";
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::rename(&first, directory.join(first_id)).unwrap();
        send_to_directory(&second, &directory, "2026-01-01T01-00-00.000Z", &manifest("2026-01-01T01-00-00.000Z"), &HashMap::new(), &Installation::for_test()).unwrap();
        let original = std::fs::metadata(directory.join(first_id).join(path)).unwrap();
        let linked = std::fs::metadata(directory.join("2026-01-01T01-00-00.000Z").join(path)).unwrap();
        assert_eq!((original.dev(), original.ino()), (linked.dev(), linked.ino()));
        assert!(linked.nlink() >= 2);
        std::fs::write(second.join(path), b"changed bytes").unwrap();
        let (size, sha256) = checksum_file(&second.join(path)).unwrap();
        let changed = ManifestFile { path: path.into(), size, sha256, home_copy: None };
        let mut changed_manifest = manifest("2026-01-01T02-00-00.000Z");
        changed_manifest.files = vec![changed];
        send_to_directory(&second, &directory, "2026-01-01T02-00-00.000Z", &changed_manifest, &HashMap::new(), &Installation::for_test()).unwrap();
        let changed_inode = std::fs::metadata(directory.join("2026-01-01T02-00-00.000Z").join(path)).unwrap();
        assert_ne!(linked.ino(), changed_inode.ino());
        let _ = std::fs::remove_dir_all(root);
    }

    fn state(root: &Path) -> AppState {
        AppState::for_test(&root)
    }

    struct CountingStore {
        inner: crate::file_store::LocalFolder,
        gets: Arc<std::sync::atomic::AtomicUsize>,
        get_bytes: Arc<std::sync::atomic::AtomicU64>,
    }

    impl crate::file_store::FileStore for CountingStore {
        fn kind(&self) -> &'static str { self.inner.kind() }
        fn put<'a>(&'a self, deployment: &'a str, file: &'a str, source: &'a Path) -> crate::file_store::BoxFuture<'a, Result<()>> {
            self.inner.put(deployment, file, source)
        }
        fn get<'a>(&'a self, deployment: &'a str, file: &'a str, range: Option<std::ops::Range<u64>>) -> crate::file_store::BoxFuture<'a, Result<Option<crate::file_store::Blob>>> {
            let gets = self.gets.clone();
            let get_bytes = self.get_bytes.clone();
            let future = self.inner.get(deployment, file, range);
            Box::pin(async move {
                let blob = future.await?;
                if let Some(blob) = blob {
                    gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    get_bytes.fetch_add(blob.length, std::sync::atomic::Ordering::SeqCst);
                    Ok(Some(blob))
                } else {
                    Ok(None)
                }
            })
        }
        fn delete<'a>(&'a self, deployment: &'a str, file: &'a str) -> crate::file_store::BoxFuture<'a, Result<()>> {
            self.inner.delete(deployment, file)
        }
        fn objects<'a>(&'a self, deployment: &'a str) -> crate::file_store::BoxFuture<'a, Result<Vec<crate::file_store::StoredObject>>> {
            self.inner.objects(deployment)
        }
        fn deployments(&self) -> crate::file_store::BoxFuture<'_, Result<Vec<String>>> { self.inner.deployments() }
    }

    #[tokio::test]
    async fn staging_reads_each_indexed_blob_and_accepts_an_import_under_the_same_id() {
        let (source, copies, restored) = (scratch("copy-state-source"), scratch("copy-state-target"), scratch("copy-state-restored"));
        let (deployment, _) = seed(&source);
        let files = crate::layout::files(&source, &deployment.id);
        let index: Value = serde_json::from_slice(&std::fs::read(files.join("index.json")).unwrap()).unwrap();
        let id = index[0]["id"].as_str().unwrap().to_owned();
        let gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let get_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut state = state(&source);
        state.files.store = Arc::new(CountingStore { inner: crate::file_store::LocalFolder::new(&source), gets: gets.clone(), get_bytes: get_bytes.clone() });
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 1);
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 2, "the second copy verifies the source bytes again");
        tokio::time::sleep(Duration::from_millis(3)).await;
        let import_dir = scratch("copy-state-import");
        std::fs::create_dir_all(&import_dir).unwrap();
        std::fs::write(import_dir.join("index.json"), json!([{"id": id, "name": "attachment", "size": 10, "visibility": "private"}]).to_string()).unwrap();
        std::fs::write(import_dir.join(format!("{id}.blob")), b"imported!!").unwrap();
        let archive = import_dir.join("files.tar");
        crate::package::folder_files_tar(&import_dir, &archive).unwrap();
        let accounts = Connection::open_in_memory().unwrap();
        accounts.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY);").unwrap();
        let blobs = crate::file_store::BlockingFiles::new(state.files.store.clone(), &deployment.id);
        tokio::task::spawn_blocking(move || crate::package::unpack_into(&archive, &accounts, &files, &blobs, &Default::default())).await.unwrap().unwrap();
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 3, "the import reused an ID with different bytes and the copy read it fresh");
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!(std::fs::read(crate::layout::files(&restored, &deployment.id).join(format!("{id}.blob"))).unwrap(), b"imported!!");
        std::fs::remove_file(source.join(".copy-state.json")).unwrap();
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 4, "missing state forces a full blob read");
        for path in [source, copies, restored, import_dir] { let _ = std::fs::remove_dir_all(path); }
    }

    fn seed(root: &Path) -> (Deployment, String) {
        let app_id = uuid::Uuid::new_v4().to_string();
        let deployment = Deployment {
            id: app_id.clone(),
            project_id: app_id,
            project_name: "Copy test".into(),
            slug: "app".into(),
            url: "http://copy-test.localhost".into(),
            created_at: now_ms(),
            released_at: 0,
            source_digest: None,
            manifest: json!({"visibility": "public"}),
            hostname: "copy-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: "v1".into(),
            signup_policy: SignupPolicy::default(),
            sign_in_methods: crate::default_sign_in_methods(),
            contact_email: None,
            modules: crate::modules::auth_enabled_defaults(),
            released_schema: String::new(),
        };
        let folder = crate::layout::folder(&root, &deployment.id);
        std::fs::create_dir_all(folder.join("dist")).unwrap();
        std::fs::write(folder.join("dist/index.html"), "<main>copied app</main>").unwrap();
        crate::write_record_for_test(root, &deployment);
        std::fs::create_dir_all(folder.join("data")).unwrap();
        initialize_database(&folder.join("data/database.sqlite")).unwrap();
        Connection::open(folder.join("data/database.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('kept');")
            .unwrap();
        let session = AuthService::open(folder.join("data/identity.sqlite"))
            .unwrap()
            .signup("writer@copy.test", "correct horse battery")
            .unwrap();
        std::fs::create_dir_all(folder.join("data/files")).unwrap();
        let file_id = uuid::Uuid::new_v4().to_string();
        std::fs::write(folder.join("data/files").join(format!("{file_id}.blob")), b"attachment").unwrap();
        std::fs::write(folder.join("data/files/index.json"), json!([{"id": file_id, "size": 10}]).to_string()).unwrap();
        std::fs::write(folder.join("data/files/.index.json.tmp-x"), "temporary").unwrap();
        std::fs::create_dir_all(folder.join(".konae/backups")).unwrap();
        let backup_id = uuid::Uuid::new_v4().to_string();
        let backup_dir = crate::layout::backups(root, &deployment.id);
        let snapshot = backup_dir.join(format!("{backup_id}.sqlite"));
        vacuum_into(&folder.join("data/database.sqlite"), &snapshot).unwrap();
        let record = json!({"id": backup_id, "key": format!("{backup_id}.sqlite"), "sizeBytes": snapshot.metadata().unwrap().len(), "label": "Manual", "createdAt": 1});
        std::fs::write(backup_dir.join(format!("{backup_id}.json")), record.to_string()).unwrap();
        std::fs::write(folder.join(".konae/backups/portable-data-x.sqlite"), "scratch").unwrap();
        crate::http::secrets_call(&crate::http::secrets_store(root, &deployment.id).unwrap(), &deployment.id, None, "secret_set", &json!({"name": "hook", "value": "a-copied-route-secret"})).unwrap();
        AuthService::open(root.join("accounts.sqlite"))
            .unwrap()
            .signup("operator@copy.test", "correct horse battery")
            .unwrap();
        (deployment, session.token)
    }

    fn count(path: &Path, sql: &str) -> i64 {
        Connection::open(path).unwrap().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn copy_ids_are_sortable_utc_timestamps() {
        let at = 1_793_973_600_123;
        assert_eq!(copy_id(at), "2026-11-06T14-00-00.123Z");
        assert_eq!(parse_copy_id(&copy_id(at)), Some(at));
        assert_eq!(parse_copy_id("2026-02-30T00-00-00.000Z"), None);
        assert_eq!(parse_copy_id(".partial-2026-11-06T14-00-00.123Z"), None);
        assert!(copy_id(at) < copy_id(at + 1));
    }

    #[test]
    fn retention_keeps_24_recent_copies_and_one_a_day_for_30_days() {
        let now = 1_793_973_600_000;
        let hour = 3_600_000;
        let ids = (0..40 * 24).map(|n| copy_id(now - n * hour)).collect::<Vec<_>>();
        let mut kept = ids.clone();
        let expired = expired_copies(&ids, now);
        kept.retain(|id| !expired.contains(id));
        kept.sort();
        kept.reverse();
        assert_eq!(&kept[..24], &ids[..24], "the 24 newest stay");
        let older = &kept[24..];
        let days = older.iter().map(|id| parse_copy_id(id).unwrap() / DAY_MS).collect::<HashSet<_>>();
        assert_eq!(days.len(), older.len(), "one copy a day beyond the newest 24");
        assert!(older.iter().all(|id| now - parse_copy_id(id).unwrap() <= 30 * DAY_MS));
        assert!(older.len() >= 29, "about 30 daily copies, kept {}", older.len());
        assert!(expired_copies(&["not-a-copy".into()], now).is_empty());
    }

    #[test]
    fn copy_settings_refuse_unsafe_targets() {
        let data = Path::new("/srv/konae/data");
        assert!(CopyConfig::parse(None, None, None, data, false).unwrap().is_none());
        let config = CopyConfig::parse(Some("/mnt/second"), Some("1"), None, data, true).unwrap().unwrap();
        assert_eq!(config.targets, vec![Target::Directory("/mnt/second".into()), Target::S3 { prefix: "copies/".into() }]);
        assert_eq!(config.interval, Duration::from_secs(3600));
        let refuse = |dir, copy_s3, interval, s3| CopyConfig::parse(dir, copy_s3, interval, data, s3).unwrap_err().to_string();
        assert!(refuse(Some("/srv/konae/data/copies"), None, None, false).contains("outside"));
        assert_eq!(refuse(None, Some("1"), None, false), "KONAE_COPY_S3=1 needs KONAE_S3_BUCKET.");
        assert_eq!(refuse(None, Some("copies/"), None, true), "KONAE_COPY_S3 must be 1 when set.");
        assert!(refuse(Some("/mnt/second"), None, Some("0"), false).contains("above zero"));
    }

    #[tokio::test]
    async fn a_copy_of_an_s3_installation_carries_the_file_bytes() {
        let (source, copies, restored) = (scratch("copy-s3-source"), scratch("copy-s3-target"), scratch("copy-s3-restored"));
        let (deployment, _) = seed(&source);
        let mut state = state(&source);
        let s3: Arc<dyn crate::file_store::FileStore> = Arc::new(crate::file_store::tests::test_s3_store().await);
        state.files.store = s3.clone();
        let files = crate::layout::files(&source, &deployment.id);
        let seeded: Value = serde_json::from_slice(&std::fs::read(files.join("index.json")).unwrap()).unwrap();
        let kept = seeded[0]["id"].as_str().unwrap().to_owned();
        let vanished = uuid::Uuid::new_v4().to_string();
        s3.put(&deployment.id, &kept, &files.join(format!("{kept}.blob"))).await.unwrap();
        std::fs::remove_file(files.join(format!("{kept}.blob"))).unwrap();
        std::fs::write(files.join("index.json"), json!([{"id": kept, "size": 10}, {"id": vanished, "size": 12}]).to_string()).unwrap();

        let error = run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap_err();
        assert!(error.to_string().contains(&vanished), "{error}");
        std::fs::write(files.join("index.json"), seeded.to_string()).unwrap();
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        let folder = crate::layout::files(&restored, &deployment.id);
        assert_eq!(std::fs::read(folder.join(format!("{kept}.blob"))).unwrap(), b"attachment");
        let index: Value = serde_json::from_slice(&std::fs::read(folder.join("index.json")).unwrap()).unwrap();
        assert_eq!(index, seeded);
        assert!(!restored.join(crate::file_store::MARKER_FILE).exists(), "a restore starts on its folder");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_copy_restores_the_same_app_into_an_empty_data_directory() {
        let (source, copies, restored) = (scratch("copy-source"), scratch("copy-target"), scratch("copy-restored"));
        let (deployment, old_token) = seed(&source);
        let state = state(&source);
        let id = run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(!source.join(".copy-staging").exists(), "staging is removed after a copy");
        let health = state.copies.health();
        assert_eq!(health["targets"].as_array().unwrap().len(), 0, "no configured targets in this state");

        let copied = crate::layout::folder(&copies.join(&id), &deployment.id);
        assert!(!copied.join("files/.index.json.tmp-x").exists(), "temporary files stay behind");
        assert!(!copied.join(".konae/backups/portable-data-x.sqlite").exists(), "export scratch stays behind");

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!((summary.copy_id.as_str(), summary.deployments), (id.as_str(), 1));
        let loaded = load_deployments(&restored, "http://localhost").await.unwrap();
        let served = loaded.get("copy-test.localhost").expect("the restored Kernel knows the app");
        assert_eq!(read_artifact(&self::state(&restored), served, "index.html").await.unwrap(), b"<main>copied app</main>");
        let folder = crate::layout::folder(&restored, &deployment.id);
        assert_eq!(count(&folder.join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        let identity = AuthService::open(folder.join("data/identity.sqlite")).unwrap();
        assert!(matches!(identity.authenticate(Some(&old_token)), Ok(None)), "the old session is gone");
        identity.login("writer@copy.test", "correct horse battery").expect("the account and password survive");
        AuthService::open(restored.join("accounts.sqlite"))
            .unwrap()
            .login("operator@copy.test", "correct horse battery")
            .expect("the installation account survives");
        assert_eq!(count(&restored.join("accounts.sqlite"), "SELECT COUNT(*) FROM app_sessions"), 1, "only the new login");
        let index: Value = serde_json::from_slice(&std::fs::read(folder.join("data/files/index.json")).unwrap()).unwrap();
        let file_id = index[0]["id"].as_str().unwrap();
        assert_eq!(std::fs::read(folder.join("data/files").join(format!("{file_id}.blob"))).unwrap(), b"attachment");
        assert_eq!(std::fs::read_dir(folder.join(".konae/backups")).unwrap().count(), 2, "the backup record and its database travel");
        assert!(crate::http::holds_route_secret(&crate::http::secrets_store(&restored, &deployment.id).unwrap(), "a-copied-route-secret").unwrap(), "the app's secrets travel in a copy");
        assert!(std::fs::read_dir(&restored).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".restore-")));

        let message = restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("not empty"), "{message}");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_restore_applies_the_revocation_record_or_holds_without_it() {
        let (source, copies, restored, held) = (scratch("record-source"), scratch("record-target"), scratch("record-restored"), scratch("record-held"));
        let (deployment, _) = seed(&source);
        let state = state(&source);
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(copies.join(recovery::LEDGER_DIR).join(recovery::LEDGER_START).is_file(), "the first copy starts the record");
        let identity = crate::layout::identity(&source, &deployment.id);
        let user: String = Connection::open(&identity).unwrap().query_row("SELECT id FROM app_users", [], |r| r.get(0)).unwrap();
        let entry = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        recovery::write(&[Target::Directory(copies.clone())], None, "data", &entry, None).await.unwrap();

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert!(recovery::hold(&restored).is_none());
        let folder = crate::layout::folder(&restored, &deployment.id);
        let restored_auth = AuthService::open(folder.join("data/identity.sqlite")).unwrap();
        assert!(restored_auth.login("writer@copy.test", "correct horse battery").is_err(), "the suspension survives the restore");

        std::fs::remove_file(copies.join(recovery::LEDGER_DIR).join(recovery::LEDGER_START)).unwrap();
        let summary = restore(&Source::Directory(copies.clone()), None, &held, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("started.json")));
        assert!(recovery::hold(&held).is_some(), "an unreadable record keeps the installation in maintenance");
        assert!(crate::maintenance::pause(&held, &deployment.id).is_some());
        for path in [source, copies, restored, held] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_copy_carries_the_maintenance_state() {
        let (source, copies, restored) = (scratch("marker-source"), scratch("marker-target"), scratch("marker-restored"));
        let (deployment, _) = seed(&source);
        std::fs::create_dir_all(crate::layout::private(&source, &deployment.id)).unwrap();
        std::fs::write(crate::layout::private(&source, &deployment.id).join(crate::maintenance::MARKER_FILE), json!({"since": 1}).to_string()).unwrap();
        run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        let message = crate::maintenance::pause(&restored, &deployment.id).expect("the restored app stays in maintenance");
        assert!(message.contains("in maintenance"));
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }


    fn seal_copy(plain: &Path, sealed: &Path, id: &str, key: &CopyKey) {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(plain.join(id).join(MANIFEST)).unwrap()).unwrap();
        for file in &manifest.files {
            let target = sealed.join(id).join(&file.path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, key.seal(id, &file.path, std::fs::read(plain.join(id).join(&file.path)).unwrap()).unwrap()).unwrap();
        }
        std::fs::write(sealed.join(id).join(MANIFEST), serde_json::to_vec(&sealed_manifest(&manifest, key)).unwrap()).unwrap();
        copy_tree(&plain.join(recovery::LEDGER_DIR), &sealed.join(recovery::LEDGER_DIR)).unwrap();
    }

    #[tokio::test]
    async fn an_encrypted_copy_restores_only_with_its_secret() {
        let (source, plain, sealed) = (scratch("sealed-source"), scratch("sealed-plain"), scratch("sealed-copy"));
        let (deployment, _) = seed(&source);
        let id = run_copy(&state(&source), &[Target::Directory(plain.clone())]).await.unwrap();
        let key = CopyKey::derive("a copy secret that is long enough for tests");
        seal_copy(&plain, &sealed, &id, &key);
        let database = format!("apps/{}/data/database.sqlite", deployment.id);
        let bytes = std::fs::read(sealed.join(&id).join(&database)).unwrap();
        assert!(!bytes.windows(4).any(|w| w == b"kept"), "rows are not readable at rest");

        let refused_root = scratch("sealed-refused");
        let without = restore(&Source::Directory(sealed.clone()), None, &refused_root, None, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(without.contains("is encrypted") && without.contains("KONAE_COPY_SECRET"), "{without}");
        let wrong = CopyKey::derive("another secret that is long enough for tests");
        let refused = restore(&Source::Directory(sealed.clone()), None, &refused_root, Some(&wrong), &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(refused.contains("does not open"), "{refused}");

        let identity = format!("apps/{}/data/identity.sqlite", deployment.id);
        let moved = scratch("sealed-moved-copy");
        copy_tree(&sealed, &moved).unwrap();
        std::fs::copy(moved.join(&id).join(&identity), moved.join(&id).join(&database)).unwrap();
        let damaged = restore(&Source::Directory(moved.clone()), Some(&id), &refused_root, Some(&key), &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(damaged.contains("does not decrypt"), "{damaged}");

        let restored = scratch("sealed-restored");
        restore(&Source::Directory(sealed.clone()), Some(&id), &restored, Some(&key), &crate::installation::Installation::for_test()).await.unwrap();
        let folder = crate::layout::folder(&restored, &deployment.id);
        assert_eq!(count(&folder.join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        AuthService::open(folder.join("data/identity.sqlite")).unwrap().login("writer@copy.test", "correct horse battery").unwrap();
        assert_eq!(installed_entries(&refused_root), 0, "a refused restore installs nothing");
        for path in [source, plain, sealed, restored, moved, refused_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    async fn fake_s3_client() -> S3Client {
        fake_s3_client_with_metrics().await.0
    }

    async fn fake_s3_client_with_metrics() -> (S3Client, crate::file_store::tests::FakeS3Metrics) {
        let credentials = aws_credential_types::Credentials::new("konae", "konae-secret", None, None, "copies-test");
        let (endpoint, metrics) = crate::file_store::tests::fake_s3_with_metrics().await;
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(credentials)
            .endpoint_url(endpoint)
            .force_path_style(true)
            .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
            .build();
        (S3Client::from_conf(config), metrics)
    }

    type S3MetricTotals = HashMap<&'static str, (usize, u64, u64)>;

    fn s3_metrics_delta(
        before: &[crate::file_store::tests::FakeS3Request],
        after: &[crate::file_store::tests::FakeS3Request],
    ) -> (S3MetricTotals, Vec<crate::file_store::tests::FakeS3Request>) {
        let new = after[before.len()..].to_vec();
        let mut totals = HashMap::new();
        for request in &new {
            let entry = totals.entry(request.method).or_insert((0, 0, 0));
            entry.0 += 1;
            entry.1 += request.request_bytes;
            entry.2 += request.response_bytes;
        }
        (totals, new)
    }

    fn s3_metrics_text(totals: &S3MetricTotals) -> String {
        ["GET", "PUT", "LIST", "DELETE"]
            .into_iter()
            .map(|method| {
                let (count, request_bytes, response_bytes) = totals.get(method).copied().unwrap_or_default();
                format!("{method}={count} request_bytes={request_bytes} response_bytes={response_bytes}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[tokio::test]
    async fn s3_copies_upload_only_changed_objects_and_restore_v2_references() {
        let (source, restored) = (scratch("incremental-s3-source"), scratch("incremental-s3-restored"));
        let (deployment, _) = seed(&source);
        let blob_dir = crate::layout::files(&source, &deployment.id);
        let index: Value = serde_json::from_slice(&std::fs::read(blob_dir.join("index.json")).unwrap()).unwrap();
        let blob_id = index[0]["id"].as_str().unwrap();
        let mut state = state(&source);
        let (client, metrics) = fake_s3_client_with_metrics().await;
        state.s3 = Some(client.clone());
        Arc::get_mut(&mut state.copies).unwrap().key = Some(Arc::new(CopyKey::derive(TEST_SECRET)));
        let target = s3_target();
        let store_gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let store_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
        state.files.store = Arc::new(CountingStore { inner: crate::file_store::LocalFolder::new(&source), gets: store_gets.clone(), get_bytes: store_bytes.clone() });
        let before_first = metrics.snapshot();
        let first = run_copy(&state, std::slice::from_ref(&target)).await.unwrap();
        let (first_s3, _) = s3_metrics_delta(&before_first, &metrics.snapshot());
        let first_downloaded = store_bytes.load(std::sync::atomic::Ordering::SeqCst);
        let first_store_gets = store_gets.load(std::sync::atomic::Ordering::SeqCst);
        let first_sizes = s3_object_sizes(&client, "data", &format!("copies/{first}/")).await.unwrap();
        let first_uploaded: u64 = first_sizes.iter().filter(|(key, _)| !key.ends_with(MANIFEST)).map(|(_, size)| size).sum();
        let first_manifest_bytes = read_s3_bytes(&client, "data", &format!("copies/{first}/{MANIFEST}")).await.unwrap();
        let mut legacy: Manifest = serde_json::from_slice(&first_manifest_bytes).unwrap();
        legacy.version = 1;
        for file in &mut legacy.files { file.home_copy = None; }
        let legacy = sealed_manifest(&legacy, state.copies.key().unwrap());
        client.put_object().bucket("data").key(format!("copies/{first}/{MANIFEST}"))
            .body(ByteStream::from(serde_json::to_vec(&legacy).unwrap())).send().await.unwrap();
        let copy_state_path = source.join(".copy-state.json");
        let mut copy_state: CopyState = serde_json::from_slice(&std::fs::read(&copy_state_path).unwrap()).unwrap();
        copy_state.s3_manifests.insert(manifest_cache_key(&s3_source(&client), &first), legacy);
        std::fs::write(copy_state_path, serde_json::to_vec(&copy_state).unwrap()).unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        let before_second = metrics.snapshot();
        let second_store_gets_before = store_gets.load(std::sync::atomic::Ordering::SeqCst);
        let second = run_copy(&state, std::slice::from_ref(&target)).await.unwrap();
        let (second_s3, second_requests) = s3_metrics_delta(&before_second, &metrics.snapshot());
        let second_store_gets = store_gets.load(std::sync::atomic::Ordering::SeqCst) - second_store_gets_before;
        let second_downloaded = store_bytes.load(std::sync::atomic::Ordering::SeqCst) - first_downloaded;
        let second_keys = list_s3_keys(&client, "data", &format!("copies/{second}/")).await.unwrap();
        let second_sizes = s3_object_sizes(&client, "data", &format!("copies/{second}/")).await.unwrap();
        let second_uploaded: u64 = second_sizes.iter().filter(|(key, _)| !key.ends_with(MANIFEST)).map(|(_, size)| size).sum();
        assert_eq!(second_keys, vec![format!("copies/{second}/{MANIFEST}")]);
        assert_eq!(second_store_gets, 1);
        assert_eq!(second_downloaded, 10);
        assert!(second_requests.iter().any(|request| request.method == "GET" && request.key.ends_with(".blob")));
        tokio::time::sleep(Duration::from_millis(2)).await;
        std::fs::write(blob_dir.join(format!("{blob_id}.blob")), b"new attachment bytes").unwrap();
        let updated_index = json!([{"id": blob_id, "size": 20, "sha256": hex::encode(Sha256::digest(b"new attachment bytes"))}]);
        std::fs::write(blob_dir.join("index.json"), updated_index.to_string()).unwrap();
        tokio::time::sleep(Duration::from_millis(3)).await;
        let before_third = metrics.snapshot();
        let third_store_gets_before = store_gets.load(std::sync::atomic::Ordering::SeqCst);
        let downloaded_before_third = store_bytes.load(std::sync::atomic::Ordering::SeqCst);
        let third = run_copy(&state, std::slice::from_ref(&target)).await.unwrap();
        let (third_s3, third_requests) = s3_metrics_delta(&before_third, &metrics.snapshot());
        let third_store_gets = store_gets.load(std::sync::atomic::Ordering::SeqCst) - third_store_gets_before;
        let changed_downloaded = store_bytes.load(std::sync::atomic::Ordering::SeqCst) - downloaded_before_third;
        let third_keys = list_s3_keys(&client, "data", &format!("copies/{third}/")).await.unwrap();
        let third_sizes = s3_object_sizes(&client, "data", &format!("copies/{third}/")).await.unwrap();
        let third_uploaded: u64 = third_sizes.iter().filter(|(key, _)| !key.ends_with(MANIFEST)).map(|(_, size)| size).sum();
        assert_eq!(third_keys.len(), 3, "the changed blob, index and manifest were written");
        let third_prefix = format!("data/copies/{third}/");
        let third_file_puts: Vec<_> = third_requests.iter().filter(|request| request.method == "PUT" && request.key.starts_with(&third_prefix) && !request.key.ends_with(&format!("/{MANIFEST}"))).map(|request| request.key.as_str()).collect();
        assert_eq!(third_file_puts.len(), 2);
        assert!(third_file_puts.iter().any(|path| path.ends_with(&format!("/{blob_id}.blob"))));
        println!("Copy {first}: file-store blob GETs {first_store_gets}, downloaded {first_downloaded} bytes; S3 {}; uploaded {first_uploaded} file bytes", s3_metrics_text(&first_s3));
        println!("Copy {second}: file-store blob GETs {second_store_gets}, downloaded {second_downloaded} bytes; S3 {}; uploaded {second_uploaded} file bytes", s3_metrics_text(&second_s3));
        println!("Copy {third}: file-store blob GETs {third_store_gets}, downloaded {changed_downloaded} bytes; S3 {}; uploaded {third_uploaded} file bytes", s3_metrics_text(&third_s3));
        println!("S3 copy objects after three copies: {}", list_s3_keys(&client, "data", "copies/").await.unwrap().len());
        let bytes = read_s3_bytes(&client, "data", &format!("copies/{third}/{MANIFEST}")).await.unwrap();
        let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(manifest.version, 2);
        assert!(manifest.files.iter().any(|file| file.path.ends_with(&format!("{blob_id}.blob")) && file.home_copy.as_deref() == Some(third.as_str())));
        assert!(manifest.files.iter().filter(|file| !file.path.ends_with(&format!("{blob_id}.blob")) && !file.path.ends_with("/files/index.json")).all(|file| file.home_copy.as_deref() == Some(first.as_str())));
        client.delete_object().bucket("data").key(format!("copies/{first}/{MANIFEST}")).send().await.unwrap();
        restore(&s3_source(&client), Some(&third), &restored, Some(state.copies.key().unwrap()), &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!(std::fs::read(crate::layout::files(&restored, &deployment.id).join(format!("{blob_id}.blob"))).unwrap(), b"new attachment bytes");
        for file in &manifest.files {
            if file.path.ends_with("/identity.sqlite") || file.path == "accounts.sqlite" { continue; }
            let source_file = source.join(&file.path);
            let restored_file = restored.join(&file.path);
            if file.path.ends_with("/database.sqlite") {
                let source_note: String = Connection::open(source_file).unwrap().query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
                let restored_note: String = Connection::open(restored_file).unwrap().query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
                assert_eq!(source_note, restored_note, "{}", file.path);
                continue;
            }
            if file.path.ends_with("/secrets.sqlite") {
                let read = |path: &Path| {
                    let connection = Connection::open(path).unwrap();
                    let mut query = connection.prepare("SELECT name, value, use FROM secrets ORDER BY name").unwrap();
                    query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))).unwrap()
                        .collect::<std::result::Result<Vec<_>, _>>().unwrap()
                };
                assert_eq!(read(&source_file), read(&restored_file), "{}", file.path);
                continue;
            }
            if file.path.ends_with("/index.json") {
                let source_index: Value = serde_json::from_slice(&std::fs::read(source_file).unwrap()).unwrap();
                let restored_index: Value = serde_json::from_slice(&std::fs::read(restored_file).unwrap()).unwrap();
                assert_eq!(source_index, restored_index, "{}", file.path);
                continue;
            }
            assert_eq!(checksum_file(&source_file).unwrap(), (file.size, file.sha256.clone()), "source {}", file.path);
            assert_eq!(checksum_file(&restored_file).unwrap(), (file.size, file.sha256.clone()), "{}", file.path);
        }
        let before_fourth = metrics.snapshot();
        let fourth = run_copy(&state, std::slice::from_ref(&target)).await.unwrap();
        let (_, fourth_requests) = s3_metrics_delta(&before_fourth, &metrics.snapshot());
        assert_eq!(list_s3_keys(&client, "data", &format!("copies/{fourth}/")).await.unwrap(), vec![format!("copies/{fourth}/{MANIFEST}")]);
        let fourth_prefix = format!("data/copies/{fourth}/");
        let fourth_puts: Vec<_> = fourth_requests.iter().filter(|request| request.method == "PUT" && request.key.starts_with(&fourth_prefix)).collect();
        assert_eq!(fourth_puts.len(), 1);
        assert!(fourth_puts[0].key.ends_with(&format!("/{MANIFEST}")));
        assert!(fourth_requests.iter().all(|request| request.method != "GET" || request.key != format!("data/copies/{first}/{MANIFEST}")));
        assert_eq!(store_gets.load(std::sync::atomic::Ordering::SeqCst), 4);
        let mut tampered = manifest.clone();
        tampered.files[0].home_copy = Some("2026-11-17T00-00-00.000Z".into());
        client.put_object().bucket("data").key(format!("copies/{third}/{MANIFEST}"))
            .body(ByteStream::from(serde_json::to_vec(&tampered).unwrap())).send().await.unwrap();
        let refused = scratch("tampered-reference");
        assert!(restore(&s3_source(&client), Some(&third), &refused, Some(state.copies.key().unwrap()), &crate::installation::Installation::for_test()).await.is_err());
        assert_eq!(installed_entries(&refused), 0);
        client.put_object().bucket("data").key(format!("copies/{third}/{MANIFEST}"))
            .body(ByteStream::from(bytes)).send().await.unwrap();
        let missing_file = manifest.files.iter().find(|file| file.home_copy.as_deref() == Some(first.as_str()) && !file.path.ends_with(".sqlite")).unwrap();
        client.delete_object().bucket("data").key(format!("copies/{}/{}", first, missing_file.path)).send().await.unwrap();
        let refused_missing = scratch("missing-reference");
        assert!(restore(&s3_source(&client), Some(&third), &refused_missing, Some(state.copies.key().unwrap()), &crate::installation::Installation::for_test()).await.is_err());
        assert_eq!(installed_entries(&refused_missing), 0);
        for path in [source, restored, refused, refused_missing] { let _ = std::fs::remove_dir_all(path); }
    }

    #[tokio::test]
    async fn s3_pruning_keeps_referenced_home_objects_and_removes_expired_objects() {
        let client = fake_s3_client().await;
        let now = now_ms();
        let home = copy_id(now - 40 * DAY_MS);
        let expired = copy_id(now - 41 * DAY_MS);
        let retained = copy_id(now);
        let path = "apps/app/data/files/one.blob";
        let content = b"retained bytes";
        let file = ManifestFile {
            path: path.into(), size: content.len() as u64, sha256: hex::encode(Sha256::digest(content)), home_copy: Some(home.clone()),
        };
        for (key, bytes) in [
            (format!("copies/{home}/{path}"), content.to_vec()),
            (format!("copies/{home}/unused.bin"), b"unused".to_vec()),
            (format!("copies/{expired}/old.bin"), b"old".to_vec()),
        ] {
            client.put_object().bucket("data").key(key).body(ByteStream::from(bytes)).send().await.unwrap();
        }
        let old = Manifest {
            version: 1, copy_id: expired.clone(), created_at: now - 41 * DAY_MS,
            files: vec![ManifestFile { path: "old.bin".into(), size: 3, sha256: hex::encode(Sha256::digest(b"old")), home_copy: None }],
            encryption: None, mac: None,
        };
        let keep = Manifest {
            version: 2, copy_id: retained.clone(), created_at: now,
            files: vec![file], encryption: None, mac: None,
        };
        put_manifest(&client, &expired, &old).await;
        put_manifest(&client, &retained, &keep).await;
        for hour in 0..25 {
            let id = copy_id(now - 40 * DAY_MS + hour * 3_600_000);
            if id != expired && id != retained {
                put_manifest(&client, &id, &Manifest { version: 1, copy_id: id.clone(), created_at: now, files: Vec::new(), encryption: None, mac: None }).await;
            }
        }
        assert!(prune_s3(&client, "data", "copies/", &retained, now).await.unwrap() >= 1);
        let keys = list_s3_keys(&client, "data", "copies/").await.unwrap();
        assert!(keys.contains(&format!("copies/{home}/{path}")));
        assert!(!keys.contains(&format!("copies/{home}/unused.bin")));
        assert!(!keys.contains(&format!("copies/{expired}/{MANIFEST}")));
        assert!(!keys.contains(&format!("copies/{expired}/old.bin")));
        assert!(keys.contains(&format!("copies/{retained}/{MANIFEST}")));
    }

    async fn send_and_prune_s3(client: &S3Client, key: &CopyKey, staging: &Path, file: &ManifestFile, id: String, at: u64) -> Result<()> {
        let manifest = Manifest { version: 2, copy_id: id.clone(), created_at: at, files: vec![file.clone()], encryption: None, mac: None };
        let mut cache = ManifestCache::new();
        send_to_s3(S3CopyTarget { client, bucket: "data", prefix: "copies/", key: Some(key) }, staging, &manifest, &id, &mut cache, &HashMap::new(), &Installation::for_test()).await?;
        prune_s3_cached(client, "data", "copies/", &id, at, &mut cache).await?;
        Ok(())
    }

    #[tokio::test]
    async fn s3_long_retention_restores_after_home_manifest_expiry() {
        let (client, metrics) = fake_s3_client_with_metrics().await;
        let key = CopyKey::derive(TEST_SECRET);
        let start = now_ms() - 100 * DAY_MS;
        let deployment = uuid::Uuid::new_v4();
        let path = format!("apps/{deployment}/app/index.html");
        let staging = scratch("long-retention-staging");
        std::fs::create_dir_all(staging.join(Path::new(&path).parent().unwrap())).unwrap();
        let bytes = b"<main>same snapshot</main>";
        std::fs::write(staging.join(&path), bytes).unwrap();
        let template = ManifestFile { path: path.clone(), size: bytes.len() as u64, sha256: hex::encode(Sha256::digest(bytes)), home_copy: None };
        let home = copy_id(start);
        send_and_prune_s3(&client, &key, &staging, &template, home.clone(), start).await.unwrap();
        let home_bytes = read_s3_bytes(&client, "data", &format!("copies/{home}/{MANIFEST}")).await.unwrap();
        let mut legacy: Manifest = serde_json::from_slice(&home_bytes).unwrap();
        legacy.version = 1;
        for file in &mut legacy.files { file.home_copy = None; }
        let legacy = sealed_manifest(&legacy, &key);
        client.put_object().bucket("data").key(format!("copies/{home}/{MANIFEST}"))
            .body(ByteStream::from(serde_json::to_vec(&legacy).unwrap())).send().await.unwrap();
        client.put_object().bucket("data").key(format!("copies/{home}/orphan.bin"))
            .body(ByteStream::from_static(b"orphan")).send().await.unwrap();
        let mut last = String::new();
        for hour in 1..26 {
            let at = start + hour * 3_600_000;
            last = copy_id(at);
            send_and_prune_s3(&client, &key, &staging, &template, last.clone(), at).await.unwrap();
        }
        for day in 1..=35 {
            let at = start + 26 * 3_600_000 + day * DAY_MS;
            last = copy_id(at);
            send_and_prune_s3(&client, &key, &staging, &template, last.clone(), at).await.unwrap();
        }
        let keys = list_s3_keys(&client, "data", "copies/").await.unwrap();
        assert!(!keys.contains(&format!("copies/{home}/{MANIFEST}")));
        assert!(keys.contains(&format!("copies/{home}/{path}")));
        assert!(!keys.contains(&format!("copies/{home}/orphan.bin")));
        let source = s3_source(&client);
        let retained = source.complete_copies().await.unwrap();
        let oldest = retained.first().unwrap().clone();
        for id in [&oldest, &last] {
            let destination = scratch("long-retention-restore");
            restore(&source, Some(id), &destination, Some(&key), &crate::installation::Installation::for_test()).await.unwrap();
            assert_eq!(std::fs::read(destination.join(&path)).unwrap(), bytes);
            let _ = std::fs::remove_dir_all(destination);
        }
        let at = start + 26 * 3_600_000 + 36 * DAY_MS;
        let unchanged = copy_id(at);
        let stable_manifest = Manifest { version: 2, copy_id: unchanged.clone(), created_at: at, files: vec![template], encryption: None, mac: None };
        let before_stable = metrics.snapshot();
        let mut cache = ManifestCache::new();
        send_to_s3(S3CopyTarget { client: &client, bucket: "data", prefix: "copies/", key: Some(&key) }, &staging, &stable_manifest, &unchanged, &mut cache, &HashMap::new(), &Installation::for_test()).await.unwrap();
        prune_s3_cached(&client, "data", "copies/", &unchanged, at, &mut cache).await.unwrap();
        let (stable_s3, stable_requests) = s3_metrics_delta(&before_stable, &metrics.snapshot());
        assert_eq!(list_s3_keys(&client, "data", &format!("copies/{unchanged}/")).await.unwrap(), vec![format!("copies/{unchanged}/{MANIFEST}")]);
        assert_eq!(stable_s3.get("PUT").map(|counts| counts.0), Some(1));
        assert!(stable_requests.iter().all(|request| request.key != format!("data/copies/{home}/{MANIFEST}")));
        assert!(stable_requests.iter().any(|request| request.method == "GET" && request.key == format!("data/copies/{home}/{path}")));
        assert!(last > oldest);
        let _ = std::fs::remove_dir_all(staging);
    }

    const TEST_SECRET: &str = "a copy secret that is long enough for tests";

    fn s3_source(client: &S3Client) -> Source {
        Source::S3 { client: client.clone(), bucket: "data".into(), prefix: "copies/".into() }
    }

    fn s3_target() -> Target {
        Target::S3 { prefix: "copies/".into() }
    }

    async fn upload_plain(client: &S3Client, copy: &Path, id: &str) {
        let mut paths = list_files(copy).unwrap().into_iter().map(|file| file.path).collect::<Vec<_>>();
        paths.push(MANIFEST.into());
        for path in paths {
            let body = ByteStream::from(std::fs::read(copy.join(&path)).unwrap());
            client.put_object().bucket("data").key(format!("copies/{id}/{path}")).body(body).send().await.unwrap();
        }
    }

    async fn send_genuine(client: &S3Client, copy: &Path, id: &str, key: Option<&CopyKey>) {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(copy.join(MANIFEST)).unwrap()).unwrap();
        let mut cache = ManifestCache::new();
        send_to_s3(S3CopyTarget { client, bucket: "data", prefix: "copies/", key: key.map(CopyKey::files) }, copy, &manifest, id, &mut cache, &HashMap::new(), &Installation::for_test()).await.unwrap();
        recovery::start(&s3_target(), Some(client), "data").await.unwrap();
    }

    async fn plain_copy(label: &str) -> (PathBuf, PathBuf, String, Deployment) {
        let (source, copies) = (scratch(&format!("{label}-source")), scratch(&format!("{label}-copies")));
        let (deployment, _) = seed(&source);
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        (source, copies, id, deployment)
    }

    #[tokio::test]
    async fn a_planted_plain_copy_is_refused_when_a_secret_is_set() {
        let (source, copies, id, _) = plain_copy("planted").await;
        let client = fake_s3_client().await;
        upload_plain(&client, &copies.join(&id), &id).await;
        recovery::start(&s3_target(), Some(&client), "data").await.unwrap();
        let key = CopyKey::derive(TEST_SECRET);
        let restored = scratch("planted-restored");
        let refused = restore(&s3_source(&client), None, &restored, Some(&key), &crate::installation::Installation::for_test()).await.err().expect("a plain copy is refused while a secret is set").to_string();
        assert!(refused.contains("is not encrypted") && refused.contains("KONAE_COPY_SECRET") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed_entries(&restored), 0, "a refused restore installs nothing");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    const LATER_ID: &str = "2099-01-01T00-00-00.000Z";

    async fn put_manifest(client: &S3Client, id: &str, manifest: &Manifest) {
        client.put_object().bucket("data").key(format!("copies/{id}/{MANIFEST}")).body(ByteStream::from(serde_json::to_vec_pretty(manifest).unwrap())).send().await.unwrap();
    }

    async fn restored_notes(source: &Source, copy: Option<&str>, label: &str, deployment: &Deployment, key: &CopyKey) -> (String, Result<String>) {
        let root = scratch(label);
        let outcome = restore(source, copy, &root, Some(key), &crate::installation::Installation::for_test()).await;
        let outcome = outcome.map(|summary| {
            assert_eq!(count(&crate::layout::folder(&root, &deployment.id).join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
            summary.copy_id
        });
        let installed = installed_entries(&root);
        let _ = std::fs::remove_dir_all(&root);
        (installed.to_string(), outcome)
    }

    #[tokio::test]
    async fn a_planted_plain_copy_does_not_hide_the_genuine_copy_from_the_default_choice() {
        let (source, copies, id, deployment) = plain_copy("planted-later").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        upload_plain(&client, &copies.join(&id), LATER_ID).await;
        let (_, outcome) = restored_notes(&s3_source(&client), None, "planted-later-restored", &deployment, &key).await;
        assert_eq!(outcome.expect("the genuine copy is installed"), id);
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_planted_sealed_looking_copy_with_a_wrong_or_missing_mac_is_skipped() {
        let (source, copies, id, deployment) = plain_copy("planted-mac").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let genuine: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap()).unwrap();
        put_manifest(&client, "2099-01-01T00-00-00.000Z", &genuine).await;
        let mut unsigned = genuine.clone();
        unsigned.mac = None;
        put_manifest(&client, "2099-01-02T00-00-00.000Z", &unsigned).await;
        let mut forged = genuine.clone();
        forged.mac = Some("not hex".into());
        put_manifest(&client, "2099-01-03T00-00-00.000Z", &forged).await;
        let (_, outcome) = restored_notes(&s3_source(&client), None, "planted-mac-restored", &deployment, &key).await;
        assert_eq!(outcome.expect("the genuine copy is installed"), id);
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn with_only_planted_copies_restore_refuses_and_installs_nothing() {
        let (source, copies, id, deployment) = plain_copy("planted-only").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        upload_plain(&client, &copies.join(&id), &id).await;
        upload_plain(&client, &copies.join(&id), LATER_ID).await;
        recovery::start(&s3_target(), Some(&client), "data").await.unwrap();
        let (installed, outcome) = restored_notes(&s3_source(&client), None, "planted-only-restored", &deployment, &key).await;
        let refused = outcome.err().expect("no copy can be authenticated").to_string();
        assert!(refused.contains("2 skipped") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed, "0");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn naming_a_planted_copy_is_refused_without_telling_the_operator_to_unset_the_secret() {
        let (source, copies, id, deployment) = plain_copy("planted-named").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        upload_plain(&client, &copies.join(&id), LATER_ID).await;
        let (installed, outcome) = restored_notes(&s3_source(&client), Some(LATER_ID), "planted-named-restored", &deployment, &key).await;
        let refused = outcome.err().expect("a planted copy is refused when named").to_string();
        assert!(refused.contains("is not encrypted") && refused.contains("KONAE_COPY_SECRET") && refused.contains("--copy") && refused.contains("Nothing was installed"), "{refused}");
        assert!(!refused.to_lowercase().contains("unset"), "{refused}");
        assert_eq!(installed, "0");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    async fn second_copy(source: &Path, copies: &Path) -> String {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        run_copy(&state(source), &[Target::Directory(copies.to_owned())]).await.unwrap()
    }

    #[tokio::test]
    async fn the_default_choice_needs_a_mac() {
        let (source, copies, id, deployment) = plain_copy("override-default").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let mut planted: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap()).unwrap();
        planted.mac = None;
        put_manifest(&client, LATER_ID, &planted).await;
        let root = scratch("override-default-restored");
        let summary = restore(&s3_source(&client), None, &root, Some(&key), &crate::installation::Installation::for_test()).await.expect("the verified copy is chosen");
        assert_eq!(summary.copy_id, id);
        assert_eq!(count(&crate::layout::folder(&root, &deployment.id).join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        for path in [source, copies, root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_genuine_folder_whose_manifest_was_replaced_by_an_unsigned_subset_is_skipped() {
        let (source, copies, older, deployment) = plain_copy("override-subset").await;
        let newer = second_copy(&source, &copies).await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&older), &older, Some(&key)).await;
        send_genuine(&client, &copies.join(&newer), &newer, Some(&key)).await;
        let mut subset: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{newer}/{MANIFEST}")).await.unwrap()).unwrap();
        subset.mac = None;
        let artifact = format!("apps/{}/dist/index.html", deployment.id);
        subset.files.retain(|file| file.path != artifact);
        put_manifest(&client, &newer, &subset).await;
        let root = scratch("override-subset-restored");
        let summary = restore(&s3_source(&client), None, &root, Some(&key), &crate::installation::Installation::for_test()).await.expect("the older verified copy is installed");
        assert_eq!(summary.copy_id, older);
        assert!(crate::layout::folder(&root, &deployment.id).join("dist/index.html").is_file(), "the whole copy is installed");
        for path in [source, copies, root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn each_copy_the_default_choice_passes_over_is_reported_with_its_reason() {
        let (source, copies, older, _) = plain_copy("skip-lines").await;
        let newer = second_copy(&source, &copies).await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&older), &older, Some(&key)).await;
        send_genuine(&client, &copies.join(&newer), &newer, Some(&key)).await;
        let mut subset: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{newer}/{MANIFEST}")).await.unwrap()).unwrap();
        subset.mac = None;
        put_manifest(&client, &newer, &subset).await;
        upload_plain(&client, &copies.join(&older), LATER_ID).await;
        let mut lines = Vec::new();
        let chosen = newest_authentic_copy(&s3_source(&client), &key, &mut |line| lines.push(line)).await.unwrap();
        assert_eq!(chosen, older);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(&format!("Skipped copy {LATER_ID}: ")) && lines[0].contains("is not encrypted"), "{lines:?}");
        assert!(lines[1].starts_with(&format!("Skipped copy {newer}: ")) && lines[1].contains("no manifest MAC"), "{lines:?}");
        assert!(lines.iter().all(|line| !line.contains("restore it anyway")), "{lines:?}");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    async fn s3_that_refuses_the_newest_manifest(newest: &str, older: &str) -> (S3Client, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::{Router, extract::Path as UrlPath, http::StatusCode, response::IntoResponse, routing::get};
        let listing = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>data</Name><Prefix>copies/</Prefix><KeyCount>2</KeyCount><IsTruncated>false</IsTruncated>\
             <Contents><Key>copies/{older}/{MANIFEST}</Key><Size>1</Size></Contents><Contents><Key>copies/{newest}/{MANIFEST}</Key><Size>1</Size></Contents></ListBucketResult>"
        );
        let list = move || {
            let body = listing.clone();
            async move { ([("content-type", "application/xml")], body) }
        };
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let older = older.to_owned();
        let app = Router::new()
            .route("/{bucket}", get(list.clone()))
            .route("/{bucket}/", get(list))
            .route("/{bucket}/{*key}", get(move |UrlPath((_, key)): UrlPath<(String, String)>| async move {
                if key.contains(&older) {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                (StatusCode::FORBIDDEN, "<Error><Code>AccessDenied</Code><Message>denied</Message></Error>").into_response()
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let credentials = aws_credential_types::Credentials::new("konae", "konae-secret", None, None, "copies-test");
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(credentials)
            .endpoint_url(format!("http://{address}"))
            .force_path_style(true)
            .build();
        (S3Client::from_conf(config), asked)
    }

    #[tokio::test]
    async fn a_read_error_on_the_newest_copy_stops_the_restore_instead_of_rolling_back() {
        let (client, older_asked) = s3_that_refuses_the_newest_manifest(LATER_ID, "2020-01-01T00-00-00.000Z").await;
        let root = scratch("read-error-restored");
        let key = CopyKey::derive(TEST_SECRET);
        let refused = restore(&s3_source(&client), None, &root, Some(&key), &crate::installation::Installation::for_test()).await.err().expect("a read error is not a verdict").to_string();
        assert!(refused.contains(LATER_ID) && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(older_asked.load(std::sync::atomic::Ordering::SeqCst), 0, "the restore did not roll back to the older copy");
        assert_eq!(installed_entries(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_manifest_over_four_mebibytes_is_refused() {
        let (source, copies, id, deployment) = plain_copy("big-manifest").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let mut manifest: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap()).unwrap();
        manifest.files.push(ManifestFile { path: "x".repeat(5 * 1024 * 1024), size: 0, sha256: String::new(), home_copy: None });
        put_manifest(&client, LATER_ID, &manifest).await;
        let (installed, outcome) = restored_notes(&s3_source(&client), Some(LATER_ID), "big-manifest-restored", &deployment, &key).await;
        let refused = outcome.err().expect("an oversized manifest is refused").to_string();
        assert!(refused.contains("4 MiB") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed, "0");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn the_default_choice_gives_up_after_fifty_skipped_copies() {
        let (source, copies, id, deployment) = plain_copy("fifty").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(copies.join(&id).join(MANIFEST)).unwrap()).unwrap();
        for second in 0..51 {
            put_manifest(&client, &format!("2099-01-01T00-00-{second:02}.000Z"), &manifest).await;
        }
        let (installed, outcome) = restored_notes(&s3_source(&client), None, "fifty-restored", &deployment, &key).await;
        let refused = outcome.err().expect("the genuine copy is out of reach").to_string();
        assert!(refused.contains("50 skipped") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed, "0");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[test]
    fn a_restore_signs_out_sessions_in_tables_spelled_in_capitals() {
        let root = scratch("signout-case");
        let identity = crate::layout::apps(&root).join("one").join("data").join("identity.sqlite");
        std::fs::create_dir_all(identity.parent().unwrap()).unwrap();
        let db = Connection::open(&identity).unwrap();
        db.execute_batch(
            "CREATE TABLE app_users(id TEXT PRIMARY KEY);
             CREATE TABLE APP_SESSIONS(token_hash TEXT); INSERT INTO app_sessions VALUES('session');
             CREATE TABLE Account_Challenges(token_hash TEXT); INSERT INTO account_challenges VALUES('link');",
        )
        .unwrap();
        drop(db);
        let removed = invalidate_sessions(&root, &crate::installation::Installation::for_test()).unwrap();
        let db = Connection::open(&identity).unwrap();
        let left: i64 = db.query_row("SELECT (SELECT COUNT(*) FROM app_sessions) + (SELECT COUNT(*) FROM account_challenges)", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "sessions or links survived the restore");
        assert_eq!(removed, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_sealed_copy_with_an_entry_removed_from_its_manifest_is_refused() {
        let (source, copies, id, deployment) = plain_copy("trimmed").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let sealed_manifest_bytes = read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap();
        let mut manifest: Manifest = serde_json::from_slice(&sealed_manifest_bytes).unwrap();
        let artifact = format!("apps/{}/dist/index.html", deployment.id);
        let before = manifest.files.len();
        manifest.files.retain(|file| file.path != artifact);
        assert_eq!(manifest.files.len() + 1, before, "the artifact was listed");
        client.put_object().bucket("data").key(format!("copies/{id}/{MANIFEST}")).body(ByteStream::from(serde_json::to_vec_pretty(&manifest).unwrap())).send().await.unwrap();
        let restored = scratch("trimmed-restored");
        let refused = restore(&s3_source(&client), None, &restored, Some(&key), &crate::installation::Installation::for_test()).await.err().expect("an altered manifest is refused").to_string();
        assert!(refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed_entries(&restored), 0);
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_forged_revocation_entry_does_not_suspend_an_account_when_a_secret_is_set() {
        let (source, copies, id, deployment) = plain_copy("forged").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let identity = crate::layout::identity(&source, &deployment.id);
        let user: String = Connection::open(&identity).unwrap().query_row("SELECT id FROM app_users", [], |r| r.get(0)).unwrap();
        let forged = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        let name = format!("copies/{}/0000000000001-forged.json", recovery::LEDGER_DIR);
        client.put_object().bucket("data").key(name).body(ByteStream::from(serde_json::to_vec(&forged).unwrap())).send().await.unwrap();
        let restored = scratch("forged-restored");
        let summary = restore(&s3_source(&client), None, &restored, Some(&key), &crate::installation::Installation::for_test()).await.unwrap();
        assert!(summary.reconciled.is_err(), "an entry without a valid MAC makes the record unreadable: {:?}", summary.reconciled);
        assert!(recovery::hold(&restored).is_some(), "the installation holds");
        let state: String = Connection::open(crate::layout::identity(&restored, &deployment.id)).unwrap().query_row("SELECT state FROM app_users", [], |r| r.get(0)).unwrap();
        assert_ne!(state, "suspended", "the forged entry changed nothing");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn copies_made_with_a_secret_and_without_one_restore_as_before() {
        let (source, copies, id, deployment) = plain_copy("pairs").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        let stores = crate::installation::Installation::for_test();
        let verify = |root: &Path| assert_eq!(count(&crate::layout::folder(root, &deployment.id).join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);

        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let sealed_root = scratch("pairs-sealed");
        restore(&s3_source(&client), None, &sealed_root, Some(&key), &stores).await.unwrap();
        verify(&sealed_root);

        let other = fake_s3_client().await;
        send_genuine(&other, &copies.join(&id), &id, None).await;
        let open_root = scratch("pairs-open");
        restore(&s3_source(&other), None, &open_root, None, &stores).await.unwrap();
        verify(&open_root);

        let directory_root = scratch("pairs-directory");
        restore(&Source::Directory(copies.clone()), None, &directory_root, Some(&key), &stores).await.unwrap();
        verify(&directory_root);
        for path in [source, copies, sealed_root, open_root, directory_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn the_writers_mac_is_the_one_the_reader_recomputes_after_a_round_trip_through_json() {
        let (source, copies, id, _) = plain_copy("roundtrip").await;
        let key = CopyKey::derive(TEST_SECRET);
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(copies.join(&id).join(MANIFEST)).unwrap()).unwrap();
        let sealed = sealed_manifest(&manifest, &key);
        let mac = sealed.mac.clone().expect("a sealed manifest carries a MAC");
        for bytes in [serde_json::to_vec(&sealed).unwrap(), serde_json::to_vec_pretty(&sealed).unwrap()] {
            let read: Manifest = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(read.mac.as_deref(), Some(mac.as_str()));
            assert!(key.verify_manifest(&id, &read, &mac));
            assert!(!key.verify_manifest("2020-01-01T00-00-00.000Z", &read, &mac), "the MAC is bound to the copy ID");
        }
        let mut shorter = sealed.clone();
        shorter.files.pop();
        assert!(!key.verify_manifest(&id, &shorter, &mac), "a dropped entry changes the MAC");
        assert!(!CopyKey::derive("another secret that is long enough for tests").verify_manifest(&id, &sealed, &mac));
        assert!(!key.verify_manifest(&id, &sealed, "not hex"));
        assert_ne!(mac, key.fingerprint());
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_sealed_copy_without_a_mac_does_not_restore() {
        let (source, copies, id, deployment) = plain_copy("unsigned").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let bytes = read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap();
        let mut manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
        manifest.mac = None;
        client.put_object().bucket("data").key(format!("copies/{id}/{MANIFEST}")).body(ByteStream::from(serde_json::to_vec_pretty(&manifest).unwrap())).send().await.unwrap();
        let stores = crate::installation::Installation::for_test();
        let refused_root = scratch("unsigned-refused");
        let refused = restore(&s3_source(&client), None, &refused_root, Some(&key), &stores).await.err().expect("no MAC, no restore").to_string();
        assert!(refused.contains("no manifest MAC") && refused.contains("was altered") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(installed_entries(&refused_root), 0);
        assert!(!crate::layout::folder(&refused_root, &deployment.id).exists());
        for path in [source, copies, refused_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_signed_revocation_entry_applies_and_an_unsigned_one_holds_the_installation() {
        let (source, copies, id, deployment) = plain_copy("signed").await;
        let key = CopyKey::derive(TEST_SECRET);
        let stores = crate::installation::Installation::for_test();
        let identity = crate::layout::identity(&source, &deployment.id);
        let user: String = Connection::open(&identity).unwrap().query_row("SELECT id FROM app_users", [], |r| r.get(0)).unwrap();
        let suspended = |root: &Path| -> String {
            Connection::open(crate::layout::identity(root, &deployment.id)).unwrap().query_row("SELECT state FROM app_users", [], |r| r.get(0)).unwrap()
        };

        let client = fake_s3_client().await;
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let entry = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        recovery::write(&[s3_target()], Some(&client), "data", &entry, Some(&key)).await.unwrap();
        let applied = scratch("signed-applied");
        let summary = restore(&s3_source(&client), None, &applied, Some(&key), &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert_eq!(suspended(&applied), "suspended");

        let other = fake_s3_client().await;
        send_genuine(&other, &copies.join(&id), &id, Some(&key)).await;
        let wrong = CopyKey::derive("another secret that is long enough for tests");
        recovery::write(&[s3_target()], Some(&other), "data", &entry, Some(&wrong)).await.unwrap();
        let wrong_root = scratch("signed-wrong");
        let summary = restore(&s3_source(&other), None, &wrong_root, Some(&key), &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("does not verify")), "{:?}", summary.reconciled);
        assert_ne!(suspended(&wrong_root), "suspended");

        let unsigned = fake_s3_client().await;
        send_genuine(&unsigned, &copies.join(&id), &id, Some(&key)).await;
        recovery::write(&[s3_target()], Some(&unsigned), "data", &entry, None).await.unwrap();
        let held = scratch("signed-held");
        let summary = restore(&s3_source(&unsigned), None, &held, Some(&key), &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("carries no MAC")), "{:?}", summary.reconciled);
        assert_ne!(suspended(&held), "suspended");

        let moved = fake_s3_client().await;
        send_genuine(&moved, &copies.join(&id), &id, Some(&key)).await;
        recovery::write(&[s3_target()], Some(&moved), "data", &entry, Some(&key)).await.unwrap();
        let original = format!("copies/{}/{}", recovery::LEDGER_DIR, entry.file_name_for_test());
        let bytes = read_s3_bytes(&moved, "data", &original).await.unwrap();
        moved.delete_object().bucket("data").key(&original).send().await.unwrap();
        moved.put_object().bucket("data").key(format!("copies/{}/0000000000001-moved.json", recovery::LEDGER_DIR)).body(ByteStream::from(bytes)).send().await.unwrap();
        let moved_root = scratch("signed-moved");
        let summary = restore(&s3_source(&moved), None, &moved_root, Some(&key), &stores).await.unwrap();
        assert!(summary.reconciled.is_err());
        for path in [source, copies, applied, wrong_root, held, moved_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_restore_keeps_api_keys_live_and_an_import_revokes_them() {
        let (source, copies, restored) = (scratch("keys-source"), scratch("keys-target"), scratch("keys-restored"));
        let (deployment, _) = seed(&source);
        let folder = crate::layout::folder(&source, &deployment.id);
        Connection::open(folder.join("data/identity.sqlite"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS api_keys(
                   id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                   actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER);
                 INSERT INTO api_keys VALUES ('key-live', 'hash-live', 'Orders', '[\"orders.receive\"]', '[]', 1, NULL);
                 INSERT INTO api_keys VALUES ('key-revoked', 'hash-revoked', 'Old', '[]', '[]', 1, 2);",
            )
            .unwrap();
        crate::usage::count(&source, &deployment.id, crate::usage::Operation::Login);
        crate::usage::flush();
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(crate::layout::usage(&copies.join(&id), &deployment.id).is_file());
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();

        let restored_folder = crate::layout::folder(&restored, &deployment.id);
        let identity = restored_folder.join("data/identity.sqlite");
        assert_eq!(count(&identity, "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL"), 1, "the live key stays live");
        assert_eq!(count(&identity, "SELECT COUNT(*) FROM api_keys WHERE id='key-revoked' AND revoked_at=2"), 1);
        assert_eq!(count(&restored_folder.join("data").join(crate::usage::USAGE_DATABASE), "SELECT SUM(count) FROM usage_daily WHERE kind='login'"), 1);

        let small = restored.join("small-import.sqlite");
        std::fs::copy(&identity, &small).unwrap();
        crate::http::revoke_imported_keys(&Connection::open(&small).unwrap()).unwrap();
        assert_eq!(count(&small, "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL"), 0, "project_export import revokes every key");
        let package = restored.join("package-import.sqlite");
        std::fs::copy(&identity, &package).unwrap();
        crate::package::revoke_access(&package).unwrap();
        assert_eq!(count(&package, "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL"), 0, "a package import holds no live key");
        assert_eq!(count(&package, "SELECT COUNT(*) FROM api_keys WHERE id='key-live' AND revoked_reason='import'"), 1, "the live key arrives revoked at import");
        assert_eq!(count(&package, "SELECT COUNT(*) FROM api_keys WHERE id='key-revoked' AND revoked_at=2 AND revoked_reason IS NULL"), 1);
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_copy_carries_the_mail_queue_and_a_restore_sends_none_of_its_pending_jobs() {
        let (source, copies, restored) = (scratch("mail-source"), scratch("mail-target"), scratch("mail-restored"));
        seed(&source);
        let mail = Connection::open(source.join(crate::mail::MAIL_DATABASE)).unwrap();
        crate::mail::install(&mail).unwrap();
        mail.execute_batch(
            "INSERT INTO jobs (id, purpose, recipient, scope, app_name, token_hash, link_nonce, link_sealed, link_expires_at,
               state, next_attempt_at, created_at, updated_at)
             VALUES ('pending-job', 'recovery', 'p@copy.test', 'installation', 'Konae', 'h', x'00', x'00', 9999999999, 'pending', 0, 0, 0),
                    ('sent-job', 'recovery', 'p@copy.test', 'installation', 'Konae', 'h', NULL, NULL, 9999999999, 'delivered', 0, 0, 0);",
        )
        .unwrap();
        drop(mail);
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(copies.join(&id).join(crate::mail::MAIL_DATABASE).is_file());
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        let restored_mail = restored.join(crate::mail::MAIL_DATABASE);
        let states: Vec<(String, String, bool)> = Connection::open(&restored_mail)
            .unwrap()
            .prepare("SELECT id, state, link_sealed IS NULL FROM jobs ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(states, vec![("pending-job".into(), "failed".into(), true), ("sent-job".into(), "delivered".into(), true)]);
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn restore_refuses_a_damaged_or_unfinished_copy_and_installs_nothing() {
        let (source, copies) = (scratch("damaged-source"), scratch("damaged-copies"));
        let (deployment, _) = seed(&source);
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();

        let unfinished = copies.join(copy_id(parse_copy_id(&id).unwrap() + 1));
        copy_tree(&copies.join(&id), &unfinished).unwrap();
        std::fs::remove_file(unfinished.join(MANIFEST)).unwrap();
        let destination = scratch("damaged-restore");
        let latest = restore(&Source::Directory(copies.clone()), None, &destination, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!(latest.copy_id, id, "a copy without a manifest is never picked");
        let named = unfinished.file_name().unwrap().to_str().unwrap();
        let empty = scratch("damaged-empty");
        let message = restore(&Source::Directory(copies.clone()), Some(named), &empty, None, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("incomplete"), "{message}");

        let artifact = crate::layout::dist(&copies.join(&id), &deployment.id).join("index.html");
        std::fs::write(&artifact, "<main>tampered</main>").unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("does not match its checksum"), "{message}");
        assert_eq!(installed_entries(&empty), 0, "a refused restore leaves the directory empty");

        let manifest_path = copies.join(&id).join(MANIFEST);
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["path"] = json!("../outside.sqlite");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("outside a data directory"), "{message}");
        for path in [source, copies, destination, empty] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_copy_waits_for_a_write_in_progress_and_includes_it() {
        let (source, copies) = (scratch("lock-source"), scratch("lock-copies"));
        let (deployment, _) = seed(&source);
        let state = state(&source);
        let held = database_lock(&state, &deployment.id).await;
        let copying = tokio::spawn({
            let (state, copies) = (state.clone(), copies.clone());
            async move { run_copy(&state, &[Target::Directory(copies)]).await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!copying.is_finished(), "the copy waits for the Deployment's lock");
        let database = crate::layout::database(&source, &deployment.id);
        Connection::open(&database).unwrap().execute("INSERT INTO notes VALUES ('during')", []).unwrap();
        drop(held);
        let id = copying.await.unwrap().unwrap();
        let copied = crate::layout::database(&copies.join(id), &deployment.id);
        assert_eq!(count(&copied, "SELECT COUNT(*) FROM notes"), 2);
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_failed_copy_shows_in_health_and_the_app_keeps_serving() {
        let source = scratch("failing-source");
        let (deployment, _) = seed(&source);
        let blocker = scratch("failing-target");
        std::fs::write(&blocker, "a file where the copy directory should be").unwrap();
        let config = CopyConfig { targets: vec![Target::Directory(blocker.clone())], interval: Duration::from_secs(3600) };
        let mut state = state(&source);
        state.copies = Arc::new(CopyStatus::new(Some(&config)));
        assert!(run_copy(&state, &config.targets).await.is_err());
        let health = state.copies.health();
        assert_eq!(health["intervalSeconds"], 3600);
        assert_eq!(health["targets"][0]["kind"], "directory");
        assert!(health["targets"][0]["lastSuccessAt"].is_null());
        assert!(health["alert"].as_str().unwrap().contains("directory target failed"), "{health}");
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<main>copied app</main>");

        std::fs::remove_file(&blocker).unwrap();
        run_copy(&state, &config.targets).await.unwrap();
        let health = state.copies.health();
        assert!(health["alert"].is_null(), "a later success clears the alert: {health}");
        assert!(health["targets"][0]["lastCopyId"].is_string());
        for path in [source, blocker] {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
