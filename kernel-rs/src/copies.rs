//! One folder per Deployment, copied off the volume.
//!
//! The data directory holds `owners.sqlite`, `mcp.sqlite`, `mail.sqlite`, `calls.sqlite` and one folder per
//! Deployment under `deployments/<id>/`. A copy is a consistent snapshot of
//! those, written to a directory target, an S3 prefix, or both, with a
//! `manifest.json` that lists every file with its size and SHA-256. The
//! `restore` subcommand installs one copy into an empty data directory.
//!
//! With `KONAE_COPY_SECRET` set, every file sent to S3 is sealed with
//! AES-256-GCM, as `mail.rs` seals queued links, and `restore` needs the
//! same secret to open it. Every target also keeps the revocation record of
//! `recovery.rs`, which `restore` applies before the Kernel serves.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    hkdf,
    rand::{SecureRandom, SystemRandom},
};

use crate::{
    AppState, database_lock, list_s3_keys, now_ms, read_s3_bytes, recovery, s3_client_from_env,
    safe_relative_path,
};

/// SQLite files in an app's `data/`. Each is copied with `VACUUM INTO`,
/// never byte for byte, so a WAL database copies consistently.
/// `usage.sqlite` is ADR-0010's store; it is copied when it exists.
/// `secrets.sqlite` stays on this installation and in its copies, never in an export.
const DEPLOYMENT_DATABASES: [&str; 4] = ["database.sqlite", "identity.sqlite", "usage.sqlite", crate::layout::SECRETS_DATABASE];
/// `mail.sqlite` is the email queue (`mail.rs`); its links stay sealed in a copy.
/// `calls.sqlite` is the call actions' queue and outbox (`calls.rs`).
const ROOT_DATABASES: [&str; 4] = ["owners.sqlite", "mcp.sqlite", crate::mail::MAIL_DATABASE, crate::calls::CALLS_DATABASE];
const MANIFEST: &str = "manifest.json";
/// The only cipher a copy names in its manifest.
const ENCRYPTION: &str = "AES-256-GCM";
const KEEP_RECENT: usize = 24;
const KEEP_DAILY_DAYS: u64 = 30;
const DAY_MS: u64 = 86_400_000;
const DEFAULT_INTERVAL_SECONDS: u64 = 3600;

// ---------------------------------------------------------------------------
// Configuration and status

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
    pub fn from_env(data_root: &Path, s3_configured: bool) -> Result<Option<Self>> {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
        // Refuses a short secret at startup rather than at the first copy.
        CopyKey::from_env()?;
        Self::parse(
            read("KONAE_COPY_DIR").as_deref(),
            read("KONAE_COPY_S3_PREFIX").as_deref(),
            read("KONAE_COPY_INTERVAL_SECONDS").as_deref(),
            data_root,
            s3_configured,
        )
    }

    fn parse(
        directory: Option<&str>,
        s3_prefix: Option<&str>,
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
        if let Some(prefix) = s3_prefix {
            ensure!(s3_configured, "KONAE_COPY_S3_PREFIX needs KONAE_S3_ENDPOINT.");
            let prefix = normalize_prefix(prefix);
            ensure!(!prefix.is_empty(), "KONAE_COPY_S3_PREFIX must name a prefix, such as copies/.");
            // Publishing deletes stale artifact keys under deployments/<id>/.
            ensure!(
                !prefix.starts_with("deployments/"),
                "KONAE_COPY_S3_PREFIX must not start with deployments/, where artifacts live."
            );
            targets.push(Target::S3 { prefix });
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

// ---------------------------------------------------------------------------
// Encryption at rest

/// The key that seals copy files on S3. HKDF-SHA256 of `KONAE_COPY_SECRET`
/// with a fixed salt and label, as `mail.rs` derives its link key, so the
/// secret itself never encrypts anything.
pub(crate) struct CopyKey {
    key: LessSafeKey,
    check: String,
}

impl CopyKey {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        match std::env::var("KONAE_COPY_SECRET").ok().filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(secret) => {
                ensure!(secret.len() >= 32, "KONAE_COPY_SECRET must be at least 32 characters.");
                Ok(Some(Self::derive(&secret)))
            }
        }
    }

    pub(crate) fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v1").extract(secret.as_bytes());
        let okm = prk.expand(&[b"copy file"], &AES_256_GCM).expect("HKDF output fits AES-256");
        let key = LessSafeKey::new(UnboundKey::from(okm));
        // A second expansion, so the manifest can tell a wrong secret from a
        // damaged file without revealing anything about the key.
        let mut check = [0u8; 32];
        prk.expand(&[b"key check"], HkdfLength(32)).expect("HKDF output fits 32 bytes").fill(&mut check).expect("32 bytes");
        Self { key, check: hex::encode(check) }
    }

    /// Seals one file. The copy ID and the file's path are the associated
    /// data, so a sealed file moved to another copy or path does not open.
    /// The output is the nonce followed by the ciphertext and its tag.
    pub(crate) fn seal(&self, copy: &str, path: &str, mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("Could not generate a nonce."))?;
        self.key
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(format!("{copy}/{path}").as_bytes()), &mut bytes)
            .map_err(|_| anyhow::anyhow!("Could not encrypt {path}."))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&bytes);
        Ok(sealed)
    }

    pub(crate) fn open(&self, copy: &str, path: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            return None;
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut buffer = body.to_vec();
        let plain = self.key.open_in_place(nonce, Aad::from(format!("{copy}/{path}").as_bytes()), &mut buffer).ok()?;
        Some(plain.to_vec())
    }
}

struct HkdfLength(usize);

impl hkdf::KeyType for HkdfLength {
    fn len(&self) -> usize {
        self.0
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Encryption {
    algorithm: String,
    key_check: String,
}

fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}/") }
}

#[derive(Clone, Default)]
struct TargetReport {
    last_attempt_at: Option<u64>,
    last_success_at: Option<u64>,
    last_copy_id: Option<String>,
    last_error: Option<String>,
}

/// What `/_konae/health` shows about copies. A failed copy never stops
/// the Kernel from serving; it shows here and in the log.
#[derive(Default)]
pub struct CopyStatus {
    targets: Vec<&'static str>,
    interval_seconds: Option<u64>,
    reports: std::sync::Mutex<HashMap<&'static str, TargetReport>>,
    /// Where `recovery.rs` writes the revocation record.
    ledger: Vec<Target>,
    key: Option<std::sync::Arc<CopyKey>>,
}

impl CopyStatus {
    pub fn new(config: Option<&CopyConfig>) -> Self {
        Self {
            targets: config.map(|config| config.targets.iter().map(Target::kind).collect()).unwrap_or_default(),
            interval_seconds: config.map(|config| config.interval.as_secs()),
            reports: Default::default(),
            ledger: config.map(|config| config.targets.clone()).unwrap_or_default(),
            key: config.and_then(|_| CopyKey::from_env().ok().flatten()).map(std::sync::Arc::new),
        }
    }

    pub(crate) fn ledger_targets(&self) -> &[Target] {
        &self.ledger
    }

    fn update(&self, kind: &'static str, change: impl FnOnce(&mut TargetReport)) {
        let mut reports = self.reports.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        change(reports.entry(kind).or_default());
    }

    pub fn health(&self) -> Value {
        let reports = self.reports.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut alerts = Vec::new();
        let targets = self
            .targets
            .iter()
            .map(|kind| {
                let report = reports.get(kind).cloned().unwrap_or_default();
                if let Some(error_value) = &report.last_error {
                    alerts.push(format!("The last copy to the {kind} target failed: {error_value}"));
                }
                json!({
                    "kind": kind,
                    "encrypted": *kind == "s3" && self.key.is_some(),
                    "lastAttemptAt": report.last_attempt_at,
                    "lastSuccessAt": report.last_success_at,
                    "lastCopyId": report.last_copy_id,
                    "lastError": report.last_error,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "targets": targets,
            "intervalSeconds": self.interval_seconds,
            "alert": if alerts.is_empty() { Value::Null } else { Value::String(alerts.join(" ")) },
        })
    }
}

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
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    copy_id: String,
    created_at: u64,
    files: Vec<ManifestFile>,
    /// Present when the files are sealed; sizes and checksums are of the
    /// plain files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encryption: Option<Encryption>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}

/// Takes one copy and sends it to every target, then prunes old copies.
/// Returns the copy ID when every target succeeded.
pub async fn run_copy(state: &AppState, targets: &[Target]) -> Result<String> {
    let started = now_ms();
    let id = copy_id(started);
    let staging_root = state.data_root.join(".copy-staging");
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    let staging = staging_root.join(&id);
    let staged = stage_copy(state, &staging, &id, started).await;
    let mut failures = Vec::new();
    for target in targets {
        let kind = target.kind();
        state.copies.update(kind, |report| report.last_attempt_at = Some(started));
        let outcome = match &staged {
            Err(error_value) => Err(anyhow::anyhow!("the snapshot failed: {error_value:#}")),
            Ok(manifest) => match recovery::start(target, state.s3.as_ref(), &state.data_bucket).await {
                Ok(()) => send(state, target, &staging, manifest, &id).await,
                Err(error_value) => Err(error_value.context("the revocation record could not be started")),
            },
        };
        match outcome {
            Ok(pruned) => {
                println!("Copy {id} sent to the {kind} target; pruned {pruned} old copies.");
                state.copies.update(kind, |report| {
                    report.last_success_at = Some(now_ms());
                    report.last_copy_id = Some(id.clone());
                    report.last_error = None;
                });
            }
            Err(error_value) => {
                let message = format!("{error_value:#}");
                eprintln!("Copy {id} to the {kind} target failed: {message}");
                failures.push(format!("{kind}: {message}"));
                state.copies.update(kind, |report| report.last_error = Some(message));
            }
        }
    }
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    if failures.is_empty() { Ok(id) } else { bail!("{}", failures.join("; ")) }
}

async fn send(state: &AppState, target: &Target, staging: &Path, manifest: &Manifest, id: &str) -> Result<usize> {
    match target {
        Target::Directory(directory) => {
            let (staging, directory, id) = (staging.to_owned(), directory.clone(), id.to_owned());
            tokio::task::spawn_blocking(move || {
                send_to_directory(&staging, &directory, &id)?;
                prune_directory(&directory, now_ms())
            })
            .await?
        }
        Target::S3 { prefix } => {
            let s3 = state.s3.as_ref().context("S3 is not configured.")?;
            send_to_s3(s3, &state.data_bucket, prefix, staging, manifest, id, state.copies.key.as_deref()).await?;
            prune_s3(s3, &state.data_bucket, prefix, id, now_ms()).await
        }
    }
}

/// Builds the copy in `staging`. Each Deployment is snapshotted under its
/// data and identity locks, the same locks an action, an upload, a signup and
/// a publish take, so its databases, files and artifact agree with each other.
async fn stage_copy(state: &AppState, staging: &Path, id: &str, created_at: u64) -> Result<Manifest> {
    tokio::fs::create_dir_all(crate::layout::apps(&staging)).await?;
    for name in ROOT_DATABASES {
        let source = state.data_root.join(name);
        if source.exists() {
            let target = staging.join(name);
            tokio::task::spawn_blocking(move || vacuum_into(&source, &target)).await??;
        }
    }
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
        let (source, target) = (entry.path(), crate::layout::folder(&staging, &deployment));
        tokio::task::spawn_blocking(move || copy_deployment(&source, &target))
            .await?
            .with_context(|| format!("Deployment {deployment}"))?;
        drop((_project, _data, _identity));
        copy_file_bytes(state, &deployment, &crate::layout::files(&staging, &deployment))
            .await
            .with_context(|| format!("The files of Deployment {deployment}"))?;
    }
    let (root, id) = (staging.to_owned(), id.to_owned());
    tokio::task::spawn_blocking(move || {
        let manifest = Manifest { version: 1, copy_id: id, created_at, files: list_files(&root)?, encryption: None };
        std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
        Ok(manifest)
    })
    .await?
}

/// Reads every file the copied index names from the installation's store into
/// the copy, as `<id>.blob`, so a copy restores without the store (ADR-0025).
/// The bytes under a file ID never change, so reading them after the locks is
/// safe; a file deleted since the index was taken is left out of the copy.
async fn copy_file_bytes(state: &AppState, deployment: &str, files: &Path) -> Result<()> {
    let index_path = files.join("index.json");
    let index: Vec<serde_json::Value> = match tokio::fs::read(&index_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error_value) => return Err(error_value.into()),
    };
    let mut kept = Vec::with_capacity(index.len());
    for file in index {
        let id = file["id"].as_str().filter(|id| crate::valid_file_id(id)).context("A stored file has an invalid id.")?.to_owned();
        if let Some(blob) = state.files.store.get(deployment, &id, None).await? {
            crate::file_store::save_blob(blob, &files.join(format!("{id}.blob"))).await?;
            kept.push(file);
        }
    }
    tokio::fs::write(&index_path, serde_json::to_vec_pretty(&kept)?).await?;
    Ok(())
}

/// Copies one app folder: settings, sources, built files and the
/// installation's record as they are, the databases with `VACUUM INTO`, and
/// the file index. The maintenance state travels, so a restore never revives
/// an app that was in maintenance, and so do the custom domain and the path.
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
            ".konae" if kind.is_dir() => copy_private(&from, &to)?,
            // Anything unknown stays behind.
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
            // Only the index is taken under the locks; `copy_file_bytes`
            // reads the bytes from the store once they are released.
            "files" if kind.is_dir() => {
                std::fs::create_dir_all(&to)?;
                if from.join("index.json").is_file() {
                    std::fs::copy(from.join("index.json"), to.join("index.json"))?;
                }
            }
            // WAL and journal files, temporary files and anything unknown stay behind.
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
            "deployment.json" | crate::layout::SOURCES_METADATA | crate::maintenance::MARKER_FILE | crate::domain::RECORD_FILE | crate::path::RECORD_FILE if kind.is_file() => {
                std::fs::copy(&from, &to)?;
            }
            "backups" if kind.is_dir() => {
                std::fs::create_dir_all(&to)?;
                for backup in std::fs::read_dir(&from)? {
                    let backup = backup?;
                    let name = backup.file_name();
                    if backup.file_type()?.is_file() && is_backup_file(&name.to_string_lossy()) {
                        std::fs::copy(backup.path(), to.join(name))?;
                    }
                }
            }
            // Staged and retired release folders and anything unknown stay behind.
            _ => {}
        }
    }
    Ok(())
}

/// A saved backup is `<uuid>.sqlite` with its `<uuid>.json`. Export scratch
/// files share the folder and are skipped.
fn is_backup_file(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else { return false };
    (extension == "sqlite" || extension == "json")
        && stem.len() == 36
        && stem.chars().all(|character| character.is_ascii_hexdigit() || character == '-')
}

pub(crate) fn vacuum_into(source: &Path, target: &Path) -> Result<()> {
    let connection = Connection::open(source)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
    Ok(())
}

/// Copies a tree of regular files. Hidden names are temporary files from
/// atomic writes; symlinks are never followed.
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        // Temporary files start with a dot. `.konae/` is the installation's part of an app.
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

fn list_files(root: &Path) -> Result<Vec<ManifestFile>> {
    fn walk(root: &Path, directory: &Path, files: &mut Vec<ManifestFile>) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, files)?;
                continue;
            }
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if relative == MANIFEST {
                continue;
            }
            let (size, sha256) = checksum_file(&path)?;
            files.push(ManifestFile { path: relative, size, sha256 });
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub(crate) fn checksum_file(path: &Path) -> Result<(u64, String)> {
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

/// Writes the copy under a hidden name, then renames it, so a directory named
/// after a copy ID always holds a complete copy.
fn send_to_directory(staging: &Path, directory: &Path, id: &str) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Could not create {}", directory.display()))?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".partial-") {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    let partial = directory.join(format!(".partial-{id}"));
    copy_tree(staging, &partial)?;
    std::fs::rename(&partial, directory.join(id))?;
    Ok(())
}

fn prune_directory(directory: &Path, now: u64) -> Result<usize> {
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

/// The manifest a sealed copy carries.
fn sealed_manifest(manifest: &Manifest, key: &CopyKey) -> Manifest {
    Manifest { encryption: Some(Encryption { algorithm: ENCRYPTION.into(), key_check: key.check.clone() }), ..manifest.clone() }
}

/// Uploads every file, then the manifest. A prefix without a manifest is an
/// unfinished upload and restore refuses it. With a key each file is sealed.
async fn send_to_s3(
    s3: &S3Client,
    bucket: &str,
    prefix: &str,
    staging: &Path,
    manifest: &Manifest,
    id: &str,
    key: Option<&CopyKey>,
) -> Result<()> {
    let mut uploads = tokio::task::JoinSet::new();
    for file in &manifest.files {
        if uploads.len() >= 8 {
            if let Some(result) = uploads.join_next().await {
                result??;
            }
        }
        let mut bytes = tokio::fs::read(staging.join(&file.path)).await?;
        if let Some(key) = key {
            bytes = key.seal(id, &file.path, bytes)?;
        }
        let request = s3
            .put_object()
            .bucket(bucket)
            .key(format!("{prefix}{id}/{}", file.path))
            .body(ByteStream::from(bytes));
        uploads.spawn(async move { request.send().await.map(|_| ()).map_err(anyhow::Error::from) });
    }
    while let Some(result) = uploads.join_next().await {
        result??;
    }
    let manifest_bytes = match key {
        Some(key) => serde_json::to_vec_pretty(&sealed_manifest(manifest, key))?,
        None => tokio::fs::read(staging.join(MANIFEST)).await?,
    };
    s3.put_object()
        .bucket(bucket)
        .key(format!("{prefix}{id}/{MANIFEST}"))
        .body(ByteStream::from(manifest_bytes))
        .send()
        .await?;
    Ok(())
}

async fn prune_s3(s3: &S3Client, bucket: &str, prefix: &str, current: &str, now: u64) -> Result<usize> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for key in list_s3_keys(s3, bucket, prefix).await? {
        let Some((id, _)) = key[prefix.len()..].split_once('/') else { continue };
        if parse_copy_id(id).is_some() {
            groups.entry(id.to_owned()).or_default().push(key);
        }
    }
    let manifest_suffix = format!("/{MANIFEST}");
    let complete = groups
        .iter()
        .filter(|(_, keys)| keys.iter().any(|key| key.ends_with(&manifest_suffix)))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    // Unfinished uploads from earlier runs; only one copy runs at a time.
    let mut doomed = groups
        .keys()
        .filter(|id| id.as_str() != current && !complete.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    let expired = expired_copies(&complete, now);
    doomed.extend(expired.iter().cloned());
    for id in &doomed {
        for key in &groups[id] {
            s3.delete_object().bucket(bucket).key(key).send().await?;
        }
    }
    Ok(expired.len())
}

/// Keeps the 24 newest copies and the newest copy of each day for 30 days.
/// Names that are not copy IDs are never returned.
fn expired_copies(ids: &[String], now: u64) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// Copy IDs are UTC timestamps that sort by name: 2026-11-06T14-00-00.123Z

fn copy_id(at_ms: u64) -> String {
    let days = (at_ms / DAY_MS) as i64;
    let (year, month, day) = civil_from_days(days);
    let in_day = at_ms % DAY_MS;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}.{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    )
}

fn parse_copy_id(id: &str) -> Option<u64> {
    if id.len() != 24 || !id.is_ascii() {
        return None;
    }
    let number = |range: std::ops::Range<usize>| id[range].parse::<u64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second, millis) = (number(11..13)?, number(14..16)?, number(17..19)?, number(20..23)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = u64::try_from(days_from_civil(year as i64, month as u32, day as u32)).ok()?;
    let at = days * DAY_MS + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis;
    (copy_id(at) == id).then_some(at)
}

pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe as i64 + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let mp = u64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy = (153 * mp + 2) / 5 + u64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

// ---------------------------------------------------------------------------
// Restore

const RESTORE_USAGE: &str =
    "Usage: konae-kernel restore --from <copy directory | s3://bucket/prefix> [--copy <copy ID>]";

/// Handles `konae-kernel restore`. Returns false for any other command.
pub async fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("restore") {
        return Ok(false);
    }
    let mut from = None;
    let mut copy = None;
    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--from" => from = rest.next().cloned(),
            "--copy" => copy = rest.next().cloned(),
            _ => bail!("{RESTORE_USAGE}"),
        }
    }
    let from = from.context(RESTORE_USAGE)?;
    let destination = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let source = source_for(&from).await?;
    let summary = restore(&source, copy.as_deref(), &destination, CopyKey::from_env()?.as_ref()).await?;
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
            "The installation starts in maintenance: {reason} Run konae-kernel reconcile --from <the same source> once the record is reachable, or konae-kernel reconcile --without-record to reopen without it."
        ),
    }
    Ok(true)
}

/// Reads the revocation record next to the copies at `from`, for
/// `konae-kernel reconcile`. `None` when the source keeps no record.
pub(crate) async fn read_record(from: &str) -> Result<Option<Vec<Vec<u8>>>> {
    let source = source_for(from).await?;
    // A directory that is itself one copy keeps the record one level up.
    let source = match source {
        Source::Directory(path) if path.join(MANIFEST).is_file() => Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned()),
        other => other,
    };
    source.record().await
}

async fn source_for(from: &str) -> Result<Source> {
    Ok(match from.strip_prefix("s3://") {
        Some(location) => {
            let (bucket, prefix) = location.split_once('/').unwrap_or((location, ""));
            ensure!(!bucket.is_empty(), "{RESTORE_USAGE}");
            let client = s3_client_from_env()
                .await
                .context("Set KONAE_S3_ENDPOINT and its credentials to restore from S3.")?;
            Source::S3 { client, bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) }
        }
        None => Source::Directory(PathBuf::from(from)),
    })
}

enum Source {
    Directory(PathBuf),
    S3 { client: S3Client, bucket: String, prefix: String },
}

impl Source {
    /// Copy IDs that have a manifest, oldest first.
    async fn complete_copies(&self) -> Result<Vec<String>> {
        let mut ids = match self {
            Source::Directory(root) => {
                let mut ids = Vec::new();
                for entry in std::fs::read_dir(root).with_context(|| format!("Could not read {}", root.display()))? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if parse_copy_id(&name).is_some() && entry.path().join(MANIFEST).is_file() {
                        ids.push(name);
                    }
                }
                ids
            }
            Source::S3 { client, bucket, prefix } => list_s3_keys(client, bucket, prefix)
                .await?
                .iter()
                .filter_map(|key| key[prefix.len()..].strip_suffix(&format!("/{MANIFEST}")).map(str::to_owned))
                .filter(|id| parse_copy_id(id).is_some())
                .collect(),
        };
        ids.sort();
        Ok(ids)
    }

    async fn read(&self, copy: &str, path: &str) -> Result<Vec<u8>> {
        match self {
            Source::Directory(root) => Ok(tokio::fs::read(root.join(copy).join(path)).await?),
            Source::S3 { client, bucket, prefix } => read_s3_bytes(client, bucket, &format!("{prefix}{copy}/{path}")).await,
        }
    }

    /// The revocation record's entries. `None` when `started.json` is
    /// missing: the source never kept a record, so it cannot prove none.
    async fn record(&self) -> Result<Option<Vec<Vec<u8>>>> {
        let dir = recovery::LEDGER_DIR;
        let start = recovery::LEDGER_START;
        match self {
            Source::Directory(root) => {
                let folder = root.join(dir);
                if !folder.join(start).is_file() {
                    return Ok(None);
                }
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
                let mut files = Vec::new();
                for key in keys.iter().filter(|key| key.ends_with(".json") && key[base.len()..] != *start) {
                    files.push(read_s3_bytes(client, bucket, key).await?);
                }
                Ok(Some(files))
            }
        }
    }
}

struct RestoreSummary {
    copy_id: String,
    deployments: usize,
    files: usize,
    sessions_removed: usize,
    /// Rows the revocation record changed, or why the installation holds.
    reconciled: std::result::Result<usize, String>,
}

async fn restore(source: &Source, copy: Option<&str>, destination: &Path, key: Option<&CopyKey>) -> Result<RestoreSummary> {
    ensure_empty(destination)?;
    // A directory that is itself one copy.
    let direct;
    let (source, copy) = match (source, copy) {
        (Source::Directory(path), None) if path.join(MANIFEST).is_file() => {
            let name = path.file_name().and_then(|name| name.to_str()).context("The copy directory has no name.")?;
            direct = Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned());
            (&direct, name.to_owned())
        }
        (source, Some(copy)) => (source, copy.to_owned()),
        (source, None) => {
            let latest = source.complete_copies().await?.pop();
            (source, latest.context("No complete copy found. A complete copy has a manifest.json.")?)
        }
    };
    ensure!(
        !copy.is_empty() && !copy.contains('/') && copy != "." && copy != "..",
        "The copy ID is invalid."
    );
    let manifest_bytes = source
        .read(&copy, MANIFEST)
        .await
        .with_context(|| format!("Copy {copy} has no manifest.json, so it is incomplete or missing."))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).context("The copy's manifest.json is invalid.")?;
    ensure!(manifest.version == 1, "The copy's manifest version is not supported.");
    if let Some(encryption) = &manifest.encryption {
        ensure!(encryption.algorithm == ENCRYPTION, "Copy {copy} uses {}, which this Kernel cannot open.", encryption.algorithm);
        let key = key.with_context(|| {
            format!("Copy {copy} is encrypted. Set KONAE_COPY_SECRET to the secret the copies were made with. Nothing was installed.")
        })?;
        ensure!(
            key.check == encryption.key_check,
            "KONAE_COPY_SECRET does not open copy {copy}: it is not the secret the copy was encrypted with. Nothing was installed."
        );
    }
    for file in &manifest.files {
        let allowed = ROOT_DATABASES.contains(&file.path.as_str()) || file.path.starts_with("apps/");
        ensure!(
            safe_relative_path(&file.path) && allowed,
            "The copy's manifest names a path outside a data directory: {}",
            file.path
        );
    }
    let staging = destination.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    let staged = async {
        for file in &manifest.files {
            let mut bytes = source.read(&copy, &file.path).await.with_context(|| format!("Could not read {}", file.path))?;
            if let (Some(_), Some(key)) = (&manifest.encryption, key) {
                bytes = key
                    .open(&copy, &file.path, &bytes)
                    .with_context(|| format!("Copy {copy} is damaged: {} does not decrypt. Nothing was installed.", file.path))?;
            }
            ensure!(
                bytes.len() as u64 == file.size && hex::encode(Sha256::digest(&bytes)) == file.sha256,
                "Copy {copy} is damaged: {} does not match its checksum. Nothing was installed.",
                file.path
            );
            let target = staging.join(&file.path);
            tokio::fs::create_dir_all(target.parent().expect("file has a parent")).await?;
            tokio::fs::write(&target, &bytes).await?;
        }
        let root = staging.clone();
        let removed = tokio::task::spawn_blocking(move || -> Result<usize> {
            check_databases(&root)?;
            invalidate_sessions(&root)
        })
        .await??;
        // Applied before anything is in place, or the installation holds.
        let record = source.record().await;
        let root = staging.clone();
        let reconciled = tokio::task::spawn_blocking(move || recovery::apply_or_hold(&root, record)).await??;
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
    let mut entries = tokio::fs::read_dir(&staging).await?;
    while let Some(entry) = entries.next_entry().await? {
        tokio::fs::rename(entry.path(), destination.join(entry.file_name())).await?;
    }
    tokio::fs::remove_dir(&staging).await?;
    let deployments = manifest
        .files
        .iter()
        .filter(|file| file.path.ends_with("/deployment.json"))
        .count();
    Ok(RestoreSummary { copy_id: copy, deployments, files: manifest.files.len(), sessions_removed, reconciled })
}

/// Restore writes only into an empty data directory, so it can never merge a
/// copy with live state. A filesystem's `lost+found` does not count.
fn ensure_empty(destination: &Path) -> Result<()> {
    match std::fs::read_dir(destination) {
        Ok(entries) => {
            for entry in entries {
                if entry?.file_name() != "lost+found" {
                    bail!(
                        "Restore refuses {}: it is not empty. Point KONAE_DATA_DIR at an empty directory.",
                        destination.display()
                    );
                }
            }
            Ok(())
        }
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(destination)?;
            Ok(())
        }
        Err(error_value) => Err(error_value.into()),
    }
}

fn sqlite_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = ROOT_DATABASES.iter().map(|name| root.join(name)).filter(|path| path.is_file()).collect::<Vec<_>>();
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

fn check_databases(root: &Path) -> Result<()> {
    for path in sqlite_files(root)? {
        let result: String = Connection::open(&path)?.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        ensure!(result == "ok", "{} failed SQLite's integrity check.", path.display());
    }
    Ok(())
}

/// Signs out everyone: owner and app sessions, Clerk creator sessions, agent
/// credentials and OAuth tokens, and unfinished OAuth codes, transfers and
/// release plans. A session revoked after the copy was taken must not come
/// back, and the transfer files never leave the old volume.
fn invalidate_sessions(root: &Path) -> Result<usize> {
    let tables: [(&str, &[&str]); 2] = [
        ("owners.sqlite", &["app_sessions", "creator_sessions", "account_challenges"]),
        ("mcp.sqlite", &["credentials", "oauth_codes", "transfers", "plans"]),
    ];
    let mut removed = 0;
    let mut clear = |path: &Path, names: &[&str]| -> Result<()> {
        let connection = Connection::open(path)?;
        for name in names {
            let exists = connection
                .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(()))
                .optional()?
                .is_some();
            if exists {
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
                clear(&identity, &["app_sessions", "account_challenges"])?;
            }
        }
    }
    // Pending email jobs carry links to challenges a restore must not honour.
    let mail = root.join(crate::mail::MAIL_DATABASE);
    if mail.is_file() {
        removed += crate::mail::end_pending_after_restore(&mail)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_carries_the_custom_domain_with_the_record_and_the_maintenance_state() {
        let root = std::env::temp_dir().join(format!("konae-copy-domain-{}", uuid::Uuid::new_v4()));
        let (source, target) = (root.join("source"), root.join("target"));
        std::fs::create_dir_all(source.join(".konae")).unwrap();
        for name in ["deployment.json", crate::maintenance::MARKER_FILE, crate::domain::RECORD_FILE, crate::path::RECORD_FILE, "stray.tmp"] {
            std::fs::write(source.join(".konae").join(name), "{}").unwrap();
        }
        let target = target.join(".konae");
        copy_deployment(&source, &target.parent().unwrap()).unwrap();
        assert!(target.join("deployment.json").exists());
        assert!(target.join(crate::domain::RECORD_FILE).exists());
        assert!(target.join(crate::path::RECORD_FILE).exists());
        assert!(target.join(crate::maintenance::MARKER_FILE).exists());
        assert!(!target.join("stray.tmp").exists());
        let _ = std::fs::remove_dir_all(root);
    }
    use crate::{
        Deployment, LockMap, SignupPolicy, auth::AuthService, initialize_database, limits::Guards,
        load_deployments, read_artifact,
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::RwLock;

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("konae-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn state(root: &Path) -> AppState {
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            files: crate::file_store::Files::local(&root),
            data_root: root.into(),
            copies: Arc::new(CopyStatus::default()),
            accounts: crate::accounts::AccountSettings::local(),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            mcp_origin: None,
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        }
    }

    /// A data directory with one published app: artifact, rows, an account,
    /// a live session and an uploaded file, plus an owner with a session.
    fn seed(root: &Path) -> (Deployment, String) {
        let app_id = uuid::Uuid::new_v4().to_string();
        let deployment = Deployment {
            id: app_id.clone(),
            project_id: app_id,
            project_name: "Copy test".into(),
            slug: "app".into(),
            url: "http://copy-test.localhost".into(),
            created_at: now_ms(),
            manifest: json!({"visibility": "public"}),
            hostname: "copy-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: "v1".into(),
            signup_policy: SignupPolicy::default(),
            sign_in_methods: crate::default_sign_in_methods(),
            contact_email: None,
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
        std::fs::write(folder.join("data/files/index.json"), json!([{"id": file_id}]).to_string()).unwrap();
        std::fs::write(folder.join("data/files/.index.json.tmp-x"), "temporary").unwrap();
        std::fs::create_dir_all(folder.join(".konae/backups")).unwrap();
        let backup_id = uuid::Uuid::new_v4();
        std::fs::write(folder.join(".konae/backups").join(format!("{backup_id}.json")), "{}").unwrap();
        std::fs::write(folder.join(".konae/backups/portable-data-x.sqlite"), "scratch").unwrap();
        crate::http::secrets_call(&crate::http::secrets_store(root, &deployment.id).unwrap(), &deployment.id, None, "secret_set", &json!({"name": "hook", "value": "a-copied-route-secret"})).unwrap();
        AuthService::open(root.join("owners.sqlite"))
            .unwrap()
            .signup("owner@copy.test", "correct horse battery")
            .unwrap();
        (deployment, session.token)
    }

    fn count(path: &Path, sql: &str) -> i64 {
        Connection::open(path).unwrap().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn copy_ids_are_sortable_utc_timestamps() {
        // 2026-11-06T14:00:00.123Z
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
        // One copy an hour for 40 days.
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
        let config = CopyConfig::parse(Some("/mnt/second"), Some("/copies"), None, data, true).unwrap().unwrap();
        assert_eq!(config.targets, vec![Target::Directory("/mnt/second".into()), Target::S3 { prefix: "copies/".into() }]);
        assert_eq!(config.interval, Duration::from_secs(3600));
        let refuse = |dir, prefix, interval, s3| CopyConfig::parse(dir, prefix, interval, data, s3).unwrap_err().to_string();
        assert!(refuse(Some("/srv/konae/data/copies"), None, None, false).contains("outside"));
        assert!(refuse(None, Some("deployments/x"), None, true).contains("must not start with deployments/"));
        assert!(refuse(None, Some("copies"), None, false).contains("KONAE_S3_ENDPOINT"));
        assert!(refuse(Some("/mnt/second"), None, Some("0"), false).contains("above zero"));
    }

    /// With the bytes in a bucket, the copy carries them, so the restored data
    /// directory holds them in its folder and needs no bucket. A file whose
    /// bytes are gone by the time the copy reads them is left out.
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
        std::fs::write(files.join("index.json"), json!([{"id": kept}, {"id": vanished}]).to_string()).unwrap();

        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();
        let folder = crate::layout::files(&restored, &deployment.id);
        assert_eq!(std::fs::read(folder.join(format!("{kept}.blob"))).unwrap(), b"attachment");
        let index: Value = serde_json::from_slice(&std::fs::read(folder.join("index.json")).unwrap()).unwrap();
        assert_eq!(index, json!([{"id": kept}]));
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
        assert!(!copied.join("backups/portable-data-x.sqlite").exists(), "export scratch stays behind");

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();
        assert_eq!((summary.copy_id.as_str(), summary.deployments), (id.as_str(), 1));
        let loaded = load_deployments(&restored, "http://localhost").await.unwrap();
        let served = loaded.get("copy-test.localhost").expect("the restored Kernel knows the app");
        assert_eq!(read_artifact(&self::state(&restored), served, "index.html").await.unwrap(), b"<main>copied app</main>");
        let folder = crate::layout::folder(&restored, &deployment.id);
        assert_eq!(count(&folder.join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        let identity = AuthService::open(folder.join("data/identity.sqlite")).unwrap();
        assert!(matches!(identity.authenticate(Some(&old_token)), Ok(None)), "the old session is gone");
        identity.login("writer@copy.test", "correct horse battery").expect("the account and password survive");
        AuthService::open(restored.join("owners.sqlite"))
            .unwrap()
            .login("owner@copy.test", "correct horse battery")
            .expect("the owner account survives");
        assert_eq!(count(&restored.join("owners.sqlite"), "SELECT COUNT(*) FROM app_sessions"), 1, "only the new login");
        let index: Value = serde_json::from_slice(&std::fs::read(folder.join("data/files/index.json")).unwrap()).unwrap();
        let file_id = index[0]["id"].as_str().unwrap();
        assert_eq!(std::fs::read(folder.join("data/files").join(format!("{file_id}.blob"))).unwrap(), b"attachment");
        assert_eq!(std::fs::read_dir(folder.join(".konae/backups")).unwrap().count(), 1, "the saved backup travels");
        assert!(crate::http::holds_route_secret(&crate::http::secrets_store(&restored, &deployment.id).unwrap(), "a-copied-route-secret").unwrap(), "the app's secrets travel in a copy");
        assert!(std::fs::read_dir(&restored).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".restore-")));

        let message = restore(&Source::Directory(copies.clone()), None, &restored, None).await.err().unwrap().to_string();
        assert!(message.contains("not empty"), "{message}");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// A suspension recorded after the copy is applied by the restore, and a
    /// source without a record leaves the installation in maintenance.
    #[tokio::test]
    async fn a_restore_applies_the_revocation_record_or_holds_without_it() {
        let (source, copies, restored, held) = (scratch("record-source"), scratch("record-target"), scratch("record-restored"), scratch("record-held"));
        let (deployment, _) = seed(&source);
        let state = state(&source);
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(copies.join(recovery::LEDGER_DIR).join(recovery::LEDGER_START).is_file(), "the first copy starts the record");
        let identity = crate::layout::identity(&source, &deployment.id);
        let user: String = Connection::open(&identity).unwrap().query_row("SELECT id FROM app_users", [], |r| r.get(0)).unwrap();
        // Suspended after the copy was taken.
        let entry = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        recovery::write(&[Target::Directory(copies.clone())], None, "data", &entry).await.unwrap();

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert!(recovery::hold(&restored).is_none());
        let folder = crate::layout::folder(&restored, &deployment.id);
        let restored_auth = AuthService::open(folder.join("data/identity.sqlite")).unwrap();
        assert!(restored_auth.login("writer@copy.test", "correct horse battery").is_err(), "the suspension survives the restore");

        std::fs::remove_file(copies.join(recovery::LEDGER_DIR).join(recovery::LEDGER_START)).unwrap();
        let summary = restore(&Source::Directory(copies.clone()), None, &held, None).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("started.json")));
        assert!(recovery::hold(&held).is_some(), "an unreadable record keeps the installation in maintenance");
        assert!(crate::maintenance::pause(&held, &deployment.id).is_some());
        for path in [source, copies, restored, held] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// An app in maintenance restored from a copy stays in maintenance, so a
    /// restore cannot add a second writer.
    #[tokio::test]
    async fn a_copy_carries_the_maintenance_state() {
        let (source, copies, restored) = (scratch("marker-source"), scratch("marker-target"), scratch("marker-restored"));
        let (deployment, _) = seed(&source);
        std::fs::create_dir_all(crate::layout::private(&source, &deployment.id)).unwrap();
        std::fs::write(crate::layout::private(&source, &deployment.id).join(crate::maintenance::MARKER_FILE), json!({"since": 1}).to_string()).unwrap();
        run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();
        let message = crate::maintenance::pause(&restored, &deployment.id).expect("the restored app stays in maintenance");
        assert!(message.contains("in maintenance"));
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }


    /// The files an S3 copy holds when KONAE_COPY_SECRET is set: each
    /// sealed with the copy ID and path, and a manifest naming the cipher.
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
        let without = restore(&Source::Directory(sealed.clone()), None, &refused_root, None).await.err().unwrap().to_string();
        assert!(without.contains("is encrypted") && without.contains("KONAE_COPY_SECRET"), "{without}");
        let wrong = CopyKey::derive("another secret that is long enough for tests");
        let refused = restore(&Source::Directory(sealed.clone()), None, &refused_root, Some(&wrong)).await.err().unwrap().to_string();
        assert!(refused.contains("does not open"), "{refused}");

        // A sealed file moved to another path does not open.
        let identity = format!("apps/{}/data/identity.sqlite", deployment.id);
        let moved = scratch("sealed-moved-copy");
        copy_tree(&sealed, &moved).unwrap();
        std::fs::copy(moved.join(&id).join(&identity), moved.join(&id).join(&database)).unwrap();
        let damaged = restore(&Source::Directory(moved.clone()), Some(&id), &refused_root, Some(&key)).await.err().unwrap().to_string();
        assert!(damaged.contains("does not decrypt"), "{damaged}");

        let restored = scratch("sealed-restored");
        restore(&Source::Directory(sealed.clone()), Some(&id), &restored, Some(&key)).await.unwrap();
        let folder = crate::layout::folder(&restored, &deployment.id);
        assert_eq!(count(&folder.join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        AuthService::open(folder.join("data/identity.sqlite")).unwrap().login("writer@copy.test", "correct horse battery").unwrap();
        assert_eq!(std::fs::read_dir(&refused_root).unwrap().count(), 0, "a refused restore installs nothing");
        for path in [source, plain, sealed, restored, moved, refused_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// ADR-0011, note of 2026-11-06: a restore brings back the same
    /// installation for the same owner, so API keys stay valid; an import
    /// into another installation revokes them. The Usage store travels.
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
        restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();

        let restored_folder = crate::layout::folder(&restored, &deployment.id);
        let identity = restored_folder.join("data/identity.sqlite");
        assert_eq!(count(&identity, "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL"), 1, "the live key stays live");
        assert_eq!(count(&identity, "SELECT COUNT(*) FROM api_keys WHERE id='key-revoked' AND revoked_at=2"), 1);
        assert_eq!(count(&restored_folder.join("data").join(crate::usage::USAGE_DATABASE), "SELECT SUM(count) FROM usage_daily WHERE kind='login'"), 1);

        // The two import paths, on copies of the same accounts file.
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
             VALUES ('pending-job', 'recovery', 'p@copy.test', 'creator', 'Konae', 'h', x'00', x'00', 9999999999, 'pending', 0, 0, 0),
                    ('sent-job', 'recovery', 'p@copy.test', 'creator', 'Konae', 'h', NULL, NULL, 9999999999, 'delivered', 0, 0, 0);",
        )
        .unwrap();
        drop(mail);
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(copies.join(&id).join(crate::mail::MAIL_DATABASE).is_file());
        restore(&Source::Directory(copies.clone()), None, &restored, None).await.unwrap();
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
        let latest = restore(&Source::Directory(copies.clone()), None, &destination, None).await.unwrap();
        assert_eq!(latest.copy_id, id, "a copy without a manifest is never picked");
        let named = unfinished.file_name().unwrap().to_str().unwrap();
        let empty = scratch("damaged-empty");
        let message = restore(&Source::Directory(copies.clone()), Some(named), &empty, None).await.err().unwrap().to_string();
        assert!(message.contains("incomplete"), "{message}");

        let artifact = crate::layout::dist(&copies.join(&id), &deployment.id).join("index.html");
        std::fs::write(&artifact, "<main>tampered</main>").unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None).await.err().unwrap().to_string();
        assert!(message.contains("does not match its checksum"), "{message}");
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "a refused restore leaves the directory empty");

        let manifest_path = copies.join(&id).join(MANIFEST);
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["path"] = json!("../outside.sqlite");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None).await.err().unwrap().to_string();
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
