//! One folder per Deployment, copied off the volume.
//!
//! The data directory holds the databases its `Installation` lists and one
//! app folder per Deployment under `apps/<id>/`. A copy is a consistent snapshot of
//! those, written to a directory target, an S3 prefix, or both, with a
//! `manifest.json` that lists every file with its size and SHA-256. The
//! `restore` subcommand installs one copy into an empty data directory.
//!
//! With `KONAE_COPY_SECRET` set, every file sent to S3 is sealed with
//! AES-256-GCM, as `mail.rs` seals queued links, and `restore` needs the
//! same secret to open it. The manifest of a sealed copy carries an
//! HMAC-SHA256 `mac` over its copy ID and file list, and `restore` from S3
//! refuses a copy that is not sealed, whose MAC fails, or that has none.
//! `KONAE_COPY_ACCEPT_UNSIGNED=1` lets one restore accept a sealed copy and
//! revocation entries from before MACs existed. Every target also keeps the
//! revocation record of `recovery.rs`, which `restore` applies before the
//! Kernel serves; entries sent to S3 carry a MAC too.

use crate::installation::Installation;
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
    hkdf, hmac,
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
/// The databases at the data root come from the `Installation` the Kernel
/// was started with; `mail.sqlite` keeps its links sealed in a copy.
const MANIFEST: &str = "manifest.json";
/// The only cipher a copy names in its manifest.
const ENCRYPTION: &str = "AES-256-GCM";
/// The largest manifest.json restore reads.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// Copies the default choice passes over before it gives up.
const MAX_SKIPPED_COPIES: usize = 50;
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
            // Publishing deletes stale artifact keys under the bucket's deployments/<id>/.
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
pub struct CopyKey {
    key: LessSafeKey,
    check: String,
    /// Signs each manifest of a sealed copy. Its own HKDF label, so it is
    /// neither the sealing key nor the `check` value.
    manifest_mac: hmac::Key,
    /// Signs each revocation entry sent to S3, under another label.
    entry_mac: hmac::Key,
}

impl CopyKey {
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var("KONAE_COPY_SECRET").ok().filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(secret) => {
                ensure!(secret.len() >= 32, "KONAE_COPY_SECRET must be at least 32 characters.");
                Ok(Some(Self::derive(&secret)))
            }
        }
    }

    pub fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v1").extract(secret.as_bytes());
        let okm = prk.expand(&[b"copy file"], &AES_256_GCM).expect("HKDF output fits AES-256");
        let key = LessSafeKey::new(UnboundKey::from(okm));
        // A second expansion, so the manifest can tell a wrong secret from a
        // damaged file without revealing anything about the key.
        let mut check = [0u8; 32];
        prk.expand(&[b"key check"], HkdfLength(32)).expect("HKDF output fits 32 bytes").fill(&mut check).expect("32 bytes");
        let mac_key = |label: &'static [u8]| hmac::Key::from(prk.expand(&[label], hmac::HMAC_SHA256).expect("HKDF output fits HMAC-SHA256"));
        Self { key, check: hex::encode(check), manifest_mac: mac_key(b"manifest mac"), entry_mac: mac_key(b"revocation entry mac") }
    }

    /// The MAC of a manifest for the copy folder `copy`. It covers the
    /// canonical form of the manifest without its `mac` field.
    fn sign_manifest(&self, copy: &str, manifest: &Manifest) -> String {
        hex::encode(hmac::sign(&self.manifest_mac, &manifest_message(copy, manifest)).as_ref())
    }

    fn verify_manifest(&self, copy: &str, manifest: &Manifest, mac: &str) -> bool {
        hex::decode(mac).is_ok_and(|mac| hmac::verify(&self.manifest_mac, &manifest_message(copy, manifest), &mac).is_ok())
    }

    /// The MAC of a revocation entry stored under the file name `name`.
    pub(crate) fn sign_entry(&self, name: &str, entry: &recovery::Entry) -> Result<String> {
        Ok(hex::encode(hmac::sign(&self.entry_mac, &entry_message(name, entry)?).as_ref()))
    }

    pub(crate) fn verify_entry(&self, name: &str, entry: &recovery::Entry, mac: &str) -> Result<bool> {
        Ok(hex::decode(mac).is_ok_and(|mac| hmac::verify(&self.entry_mac, &entry_message(name, entry).unwrap_or_default(), &mac).is_ok()))
    }

    /// Seals one file. The copy ID and the file's path are the associated
    /// data, so a sealed file moved to another copy or path does not open.
    /// The output is the nonce followed by the ciphertext and its tag.
    pub fn seal(&self, copy: &str, path: &str, mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("Could not generate a nonce."))?;
        self.key
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(format!("{copy}/{path}").as_bytes()), &mut bytes)
            .map_err(|_| anyhow::anyhow!("Could not encrypt {path}."))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&bytes);
        Ok(sealed)
    }

    pub fn open(&self, copy: &str, path: &str, sealed: &[u8]) -> Option<Vec<u8>> {
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

/// The bytes a manifest's MAC covers: the copy ID, a zero byte, and the
/// manifest serialised by its struct, so the field order is fixed, with
/// `mac` left out. The writer and `restore` both build it from the struct.
fn manifest_message(copy: &str, manifest: &Manifest) -> Vec<u8> {
    let mut message = copy.as_bytes().to_vec();
    message.push(0);
    let plain = Manifest { mac: None, ..manifest.clone() };
    message.extend(serde_json::to_vec(&plain).expect("a manifest serialises"));
    message
}

/// The bytes an entry's MAC covers: its file name, a zero byte and the entry
/// serialised by its struct.
fn entry_message(name: &str, entry: &recovery::Entry) -> Result<Vec<u8>> {
    let mut message = name.as_bytes().to_vec();
    message.push(0);
    message.extend(serde_json::to_vec(entry)?);
    Ok(message)
}

/// Whether this restore accepts what an older Kernel wrote without a MAC.
/// Read once by the caller, so the code under test never reads the environment.
pub fn accept_unsigned_from_env() -> bool {
    std::env::var("KONAE_COPY_ACCEPT_UNSIGNED").is_ok_and(|value| value == "1")
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

    pub fn ledger_targets(&self) -> &[Target] {
        &self.ledger
    }

    pub fn key(&self) -> Option<&CopyKey> {
        self.key.as_deref()
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
    /// Present on a sealed copy: HMAC-SHA256 of the copy ID and the
    /// manifest without this field, so no entry can be added or dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mac: Option<String>,
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
    for name in state.installation.databases.iter().map(|database| database.file) {
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
        let manifest = Manifest { version: 1, copy_id: id, created_at, files: list_files(&root)?, encryption: None, mac: None };
        std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
        Ok(manifest)
    })
    .await?
}

/// Reads every file the copied index names from the installation's store into
/// the copy, as `<id>.blob`, so a copy restores without the store (ADR-0009).
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
            "deployment.json" | crate::layout::SOURCES_METADATA | crate::maintenance::MARKER_FILE | crate::domain::RECORD_FILE | crate::path::RECORD_FILE | crate::hostname::RECORD_FILE if kind.is_file() => {
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

pub fn vacuum_into(source: &Path, target: &Path) -> Result<()> {
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

/// The manifest a sealed copy carries, signed for its copy ID.
fn sealed_manifest(manifest: &Manifest, key: &CopyKey) -> Manifest {
    let mut sealed = Manifest {
        encryption: Some(Encryption { algorithm: ENCRYPTION.into(), key_check: key.check.clone() }),
        mac: None,
        ..manifest.clone()
    };
    sealed.mac = Some(key.sign_manifest(&manifest.copy_id, &sealed));
    sealed
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

pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

fn restore_usage() -> String {
    format!("Usage: {} restore --from <copy directory | s3://bucket/prefix> [--copy <copy ID>]", crate::program())
}

/// Handles the `restore` command. Returns false for any other command.
pub async fn operator_command(installation: &Installation) -> Result<bool> {
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
            _ => bail!("{}", restore_usage()),
        }
    }
    let from = from.with_context(restore_usage)?;
    let destination = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let source = source_for(&from).await?;
    let summary = restore(&source, copy.as_deref(), &destination, CopyKey::from_env()?.as_ref(), accept_unsigned_from_env(), installation).await?;
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

/// Reads the revocation record next to the copies at `from`, for
/// The `reconcile` command. `None` when the source keeps no record.
pub async fn read_record(from: &str, key: Option<&CopyKey>, accept_unsigned: bool) -> Result<Option<Vec<Vec<u8>>>> {
    let source = source_for(from).await?;
    // A directory that is itself one copy keeps the record one level up.
    let source = match source {
        Source::Directory(path) if path.join(MANIFEST).is_file() => Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned()),
        other => other,
    };
    source.record(key, accept_unsigned).await
}

async fn source_for(from: &str) -> Result<Source> {
    Ok(match from.strip_prefix("s3://") {
        Some(location) => {
            let (bucket, prefix) = location.split_once('/').unwrap_or((location, ""));
            ensure!(!bucket.is_empty(), "{}", restore_usage());
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

    /// A copy's manifest bytes. `Ok(None)` when the manifest is over
    /// `MAX_MANIFEST_BYTES`, checked on the object's length before the body is
    /// read and again while reading.
    async fn read_manifest_bytes(&self, copy: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Source::Directory(root) => {
                let path = root.join(copy).join(MANIFEST);
                if tokio::fs::metadata(&path).await?.len() > MAX_MANIFEST_BYTES {
                    return Ok(None);
                }
                Ok(Some(tokio::fs::read(path).await?))
            }
            Source::S3 { client, bucket, prefix } => {
                let object = client.get_object().bucket(bucket).key(format!("{prefix}{copy}/{MANIFEST}")).send().await?;
                if object.content_length().is_some_and(|length| length > MAX_MANIFEST_BYTES as i64) {
                    return Ok(None);
                }
                let mut body = object.body;
                let mut bytes = Vec::new();
                while let Some(chunk) = body.try_next().await? {
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
                        return Ok(None);
                    }
                }
                Ok(Some(bytes))
            }
        }
    }

    /// The revocation record's entries. `None` when `started.json` is
    /// missing: the source never kept a record, so it cannot prove none.
    /// With a key, each entry on S3 must carry a valid MAC for its file name,
    /// or the whole record is unreadable. An entry with no MAC passes only
    /// under `accept_unsigned`. A directory is never sealed and is not checked.
    async fn record(&self, key: Option<&CopyKey>, accept_unsigned: bool) -> Result<Option<Vec<Vec<u8>>>> {
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
                let mut unsigned = 0;
                for object in keys.iter().filter(|object| object.ends_with(".json") && object[base.len()..] != *start) {
                    let bytes = read_s3_bytes(client, bucket, object).await?;
                    if let Some(key) = key {
                        unsigned += recovery::check_signed(key, &object[base.len()..], &bytes, accept_unsigned)?;
                    }
                    files.push(bytes);
                }
                if unsigned > 0 {
                    println!("KONAE_COPY_ACCEPT_UNSIGNED is set: accepted {unsigned} revocation entries that carry no MAC.");
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

/// What a manifest is worth once read. The outer error is a failure to read
/// it (transport, throttling, permission), which says nothing about the copy.
/// The inner error is a verdict on the bytes: too large, unparsable, or a
/// version this Kernel does not read.
async fn read_manifest(source: &Source, copy: &str) -> Result<std::result::Result<Manifest, String>> {
    let Some(bytes) = source.read_manifest_bytes(copy).await? else {
        return Ok(Err(format!("The manifest of copy {copy} is larger than 4 MiB.")));
    };
    let manifest: Manifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(Err(format!("The manifest of copy {copy} is invalid."))),
    };
    if manifest.version != 1 {
        return Ok(Err(format!("The manifest version of copy {copy} is not supported.")));
    }
    Ok(Ok(manifest))
}

fn not_encrypted(copy: &str) -> String {
    format!("Copy {copy} is not encrypted, but KONAE_COPY_SECRET is set, and a copy this installation sends to S3 with the secret is always encrypted.")
}

/// Checks an encrypted copy's manifest against the secret. Returns true when
/// the manifest carries no MAC and `accept_unsigned` let it pass. The default
/// choice passes `automatic` and never accepts a missing MAC.
fn check_sealed(copy: &str, manifest: &Manifest, key: Option<&CopyKey>, accept_unsigned: bool, automatic: bool) -> Result<bool> {
    let Some(encryption) = &manifest.encryption else {
        return Ok(false);
    };
    ensure!(encryption.algorithm == ENCRYPTION, "Copy {copy} uses {:?}, which this Kernel cannot open.", encryption.algorithm);
    let key = key.with_context(|| {
        format!("Copy {copy} is encrypted. Set KONAE_COPY_SECRET to the secret the copies were made with. Nothing was installed.")
    })?;
    ensure!(
        key.check == encryption.key_check,
        "KONAE_COPY_SECRET does not open copy {copy}: it is not the secret the copy was encrypted with. Nothing was installed."
    );
    match &manifest.mac {
        Some(mac) => {
            ensure!(
                key.verify_manifest(copy, manifest, mac),
                "Copy {copy} has a manifest that does not match its MAC: the file list was altered after the Kernel wrote it. Nothing was installed."
            );
            Ok(false)
        }
        None if accept_unsigned && !automatic => Ok(true),
        None if automatic => bail!(
            "Copy {copy} has no manifest MAC, so it is not chosen automatically. A copy an older Kernel made before signing can be named with --copy together with KONAE_COPY_ACCEPT_UNSIGNED=1, which accepts a sealed copy whose file list is not authenticated. Nothing was installed."
        ),
        None => bail!(
            "Copy {copy} has no manifest MAC: an older Kernel made it, or it was altered. Set KONAE_COPY_ACCEPT_UNSIGNED=1 to restore it anyway, which accepts a sealed copy whose file list is not authenticated. Nothing was installed."
        ),
    }
}

/// The newest copy on S3 whose manifest is encrypted and carries a MAC that
/// verifies under the secret. `KONAE_COPY_ACCEPT_UNSIGNED` does not apply here:
/// a manifest without a MAC can be planted or cut down by anyone who can write
/// to the bucket. Each copy passed over is reported with its reason, so a
/// planted copy is seen and does not hide the genuine ones. Only a verdict on
/// the copy itself passes it over; a failure to read it stops the restore.
async fn newest_authentic_copy(source: &Source, key: &CopyKey, report: &mut dyn FnMut(String)) -> Result<String> {
    let copies = source.complete_copies().await?;
    ensure!(!copies.is_empty(), "No complete copy found. A complete copy has a manifest.json.");
    let mut skipped = 0;
    let mut newest_reason = String::new();
    for copy in copies.iter().rev() {
        ensure!(
            skipped < MAX_SKIPPED_COPIES,
            "Stopped after {MAX_SKIPPED_COPIES} skipped copies without finding one that could be authenticated. The newest: {newest_reason} Nothing was installed."
        );
        let manifest = read_manifest(source, copy)
            .await
            .map_err(|error| anyhow::anyhow!("Could not read the manifest of copy {copy}: {error:#}. Nothing was installed."))?;
        let verdict = manifest.and_then(|manifest| {
            if manifest.encryption.is_none() {
                return Err(not_encrypted(copy));
            }
            check_sealed(copy, &manifest, Some(key), false, true).map(|_| ()).map_err(|error| format!("{error:#}"))
        });
        match verdict {
            Ok(()) => return Ok(copy.clone()),
            Err(reason) => {
                let reason = reason.trim_end_matches(" Nothing was installed.").to_owned();
                report(format!("Skipped copy {copy}: {reason}"));
                if skipped == 0 {
                    newest_reason = reason;
                }
                skipped += 1;
            }
        }
    }
    bail!(
        "No copy in the source could be authenticated, {skipped} skipped. The newest: {newest_reason} While KONAE_COPY_SECRET is set, only a copy encrypted and signed with it is chosen automatically, and a copy that is not encrypted never is. Nothing was installed."
    )
}

/// `accept_unsigned` is `KONAE_COPY_ACCEPT_UNSIGNED`, read once by the caller.
async fn restore(
    source: &Source,
    copy: Option<&str>,
    destination: &Path,
    key: Option<&CopyKey>,
    accept_unsigned: bool,
    installation: &Installation,
) -> Result<RestoreSummary> {
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
        (source, None) if matches!(source, Source::S3 { .. }) && key.is_some() => {
            let key = key.expect("checked by the guard");
            (source, newest_authentic_copy(source, key, &mut |line| println!("{line}")).await?)
        }
        (source, None) => {
            let latest = source.complete_copies().await?.pop();
            (source, latest.context("No complete copy found. A complete copy has a manifest.json.")?)
        }
    };
    ensure!(
        !copy.is_empty() && !copy.contains('/') && copy != "." && copy != "..",
        "The copy ID is invalid."
    );
    let manifest = read_manifest(source, &copy)
        .await
        .with_context(|| format!("Copy {copy} has no manifest.json, so it is incomplete or missing."))?
        .map_err(|reason| anyhow::anyhow!("{reason} Nothing was installed."))?;
    // Copies on S3 are sealed whenever a secret is set. A plain manifest there
    // is not one this installation made, so the secret is not set aside for it.
    ensure!(
        !(matches!(source, Source::S3 { .. }) && key.is_some() && manifest.encryption.is_none()),
        "{} Name a copy this installation sent with --copy, for example from the copy IDs under the S3 prefix: {} restore --from <source> --copy <copy ID>. Nothing was installed.",
        not_encrypted(&copy),
        crate::program()
    );
    if check_sealed(&copy, &manifest, key, accept_unsigned, false)? {
        println!("KONAE_COPY_ACCEPT_UNSIGNED is set: restoring copy {copy} without checking its manifest, which carries no MAC.");
    }
    for file in &manifest.files {
        let allowed = installation.holds(&file.path) || file.path.starts_with("apps/");
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
        let stores = installation.clone();
        let removed = tokio::task::spawn_blocking(move || -> Result<usize> {
            check_databases(&root, &stores)?;
            invalidate_sessions(&root, &stores)
        })
        .await??;
        // Applied before anything is in place, or the installation holds.
        let record = source.record(key, accept_unsigned).await;
        let root = staging.clone();
        let stores = installation.clone();
        let reconciled = tokio::task::spawn_blocking(move || recovery::apply_or_hold(&root, &stores, record)).await??;
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
        let result: String = Connection::open(&path)?.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        ensure!(result == "ok", "{} failed SQLite's integrity check.", path.display());
    }
    Ok(())
}

/// Signs out everyone: app sessions and open links, unfinished transfers,
/// and whatever each database of the installation lists to sign out. A
/// session revoked after the copy was taken must not come back, and the
/// transfer files never leave the old volume.
fn invalidate_sessions(root: &Path, installation: &Installation) -> Result<usize> {
    let tables = installation.databases.iter().map(|database| (database.file, database.sign_out));
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

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("konae-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn state(root: &Path) -> AppState {
        AppState::for_test(&root)
    }

    /// A data directory with one published app: artifact, rows, an account,
    /// a live session and an uploaded file, plus an installation account with a session.
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
        restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();
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

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();
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
        assert_eq!(std::fs::read_dir(folder.join(".konae/backups")).unwrap().count(), 1, "the saved backup travels");
        assert!(crate::http::holds_route_secret(&crate::http::secrets_store(&restored, &deployment.id).unwrap(), "a-copied-route-secret").unwrap(), "the app's secrets travel in a copy");
        assert!(std::fs::read_dir(&restored).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".restore-")));

        let message = restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
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
        recovery::write(&[Target::Directory(copies.clone())], None, "data", &entry, None).await.unwrap();

        let summary = restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert!(recovery::hold(&restored).is_none());
        let folder = crate::layout::folder(&restored, &deployment.id);
        let restored_auth = AuthService::open(folder.join("data/identity.sqlite")).unwrap();
        assert!(restored_auth.login("writer@copy.test", "correct horse battery").is_err(), "the suspension survives the restore");

        std::fs::remove_file(copies.join(recovery::LEDGER_DIR).join(recovery::LEDGER_START)).unwrap();
        let summary = restore(&Source::Directory(copies.clone()), None, &held, None, false, &crate::installation::Installation::for_test()).await.unwrap();
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
        restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();
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
        let without = restore(&Source::Directory(sealed.clone()), None, &refused_root, None, false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(without.contains("is encrypted") && without.contains("KONAE_COPY_SECRET"), "{without}");
        let wrong = CopyKey::derive("another secret that is long enough for tests");
        let refused = restore(&Source::Directory(sealed.clone()), None, &refused_root, Some(&wrong), false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(refused.contains("does not open"), "{refused}");

        // A sealed file moved to another path does not open.
        let identity = format!("apps/{}/data/identity.sqlite", deployment.id);
        let moved = scratch("sealed-moved-copy");
        copy_tree(&sealed, &moved).unwrap();
        std::fs::copy(moved.join(&id).join(&identity), moved.join(&id).join(&database)).unwrap();
        let damaged = restore(&Source::Directory(moved.clone()), Some(&id), &refused_root, Some(&key), false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(damaged.contains("does not decrypt"), "{damaged}");

        let restored = scratch("sealed-restored");
        restore(&Source::Directory(sealed.clone()), Some(&id), &restored, Some(&key), false, &crate::installation::Installation::for_test()).await.unwrap();
        let folder = crate::layout::folder(&restored, &deployment.id);
        assert_eq!(count(&folder.join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        AuthService::open(folder.join("data/identity.sqlite")).unwrap().login("writer@copy.test", "correct horse battery").unwrap();
        assert_eq!(std::fs::read_dir(&refused_root).unwrap().count(), 0, "a refused restore installs nothing");
        for path in [source, plain, sealed, restored, moved, refused_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    async fn fake_s3_client() -> S3Client {
        let credentials = aws_credential_types::Credentials::new("konae", "konae-secret", None, None, "copies-test");
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(credentials)
            .endpoint_url(crate::file_store::tests::fake_s3().await)
            .force_path_style(true)
            .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
            .build();
        S3Client::from_conf(config)
    }

    const TEST_SECRET: &str = "a copy secret that is long enough for tests";

    fn s3_source(client: &S3Client) -> Source {
        Source::S3 { client: client.clone(), bucket: "data".into(), prefix: "copies/".into() }
    }

    fn s3_target() -> Target {
        Target::S3 { prefix: "copies/".into() }
    }

    /// Puts a plain copy folder in the bucket as it is, the way someone who
    /// can write to the bucket but does not hold the secret would.
    async fn upload_plain(client: &S3Client, copy: &Path, id: &str) {
        let mut paths = list_files(copy).unwrap().into_iter().map(|file| file.path).collect::<Vec<_>>();
        paths.push(MANIFEST.into());
        for path in paths {
            let body = ByteStream::from(std::fs::read(copy.join(&path)).unwrap());
            client.put_object().bucket("data").key(format!("copies/{id}/{path}")).body(body).send().await.unwrap();
        }
    }

    /// What a Kernel sends to S3: the writer's own function, sealed when a key is given.
    async fn send_genuine(client: &S3Client, copy: &Path, id: &str, key: Option<&CopyKey>) {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(copy.join(MANIFEST)).unwrap()).unwrap();
        send_to_s3(client, "data", "copies/", copy, &manifest, id, key).await.unwrap();
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
        // Plain files, a manifest with no encryption and checksums that match.
        upload_plain(&client, &copies.join(&id), &id).await;
        recovery::start(&s3_target(), Some(&client), "data").await.unwrap();
        let key = CopyKey::derive(TEST_SECRET);
        let restored = scratch("planted-restored");
        let refused = restore(&s3_source(&client), None, &restored, Some(&key), false, &crate::installation::Installation::for_test()).await.err().expect("a plain copy is refused while a secret is set").to_string();
        assert!(refused.contains("is not encrypted") && refused.contains("KONAE_COPY_SECRET") && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(std::fs::read_dir(&restored).map(|entries| entries.count()).unwrap_or(0), 0, "a refused restore installs nothing");
        for path in [source, copies, restored] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// A copy ID that sorts after every real one, as a planted copy would choose.
    const LATER_ID: &str = "2099-01-01T00-00-00.000Z";

    async fn put_manifest(client: &S3Client, id: &str, manifest: &Manifest) {
        client.put_object().bucket("data").key(format!("copies/{id}/{MANIFEST}")).body(ByteStream::from(serde_json::to_vec_pretty(manifest).unwrap())).send().await.unwrap();
    }

    async fn restored_notes(source: &Source, copy: Option<&str>, label: &str, deployment: &Deployment, key: &CopyKey) -> (String, Result<String>) {
        let root = scratch(label);
        let outcome = restore(source, copy, &root, Some(key), false, &crate::installation::Installation::for_test()).await;
        let outcome = outcome.map(|summary| {
            assert_eq!(count(&crate::layout::folder(&root, &deployment.id).join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
            summary.copy_id
        });
        let installed = std::fs::read_dir(&root).map(|entries| entries.count()).unwrap_or(0);
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
        // The genuine manifest under another copy ID: sealed, right key check, MAC bound to the other ID.
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
    async fn the_default_choice_needs_a_mac_even_when_the_override_is_on() {
        let (source, copies, id, deployment) = plain_copy("override-default").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        // Planted: the genuine manifest, sealed-looking with a copied key check, and no MAC.
        let mut planted: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap()).unwrap();
        planted.mac = None;
        put_manifest(&client, LATER_ID, &planted).await;
        let root = scratch("override-default-restored");
        let summary = restore(&s3_source(&client), None, &root, Some(&key), true, &crate::installation::Installation::for_test()).await.expect("the verified copy is chosen");
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
        let summary = restore(&s3_source(&client), None, &root, Some(&key), true, &crate::installation::Installation::for_test()).await.expect("the older verified copy is installed");
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
        assert!(lines[1].starts_with(&format!("Skipped copy {newer}: ")) && lines[1].contains("not chosen automatically") && lines[1].contains("--copy"), "{lines:?}");
        assert!(lines.iter().all(|line| !line.contains("restore it anyway")), "{lines:?}");
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// An S3 endpoint that lists two copies and refuses to read the newest
    /// manifest. The counter holds how many objects of the older copy were asked for.
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
        let refused = restore(&s3_source(&client), None, &root, Some(&key), false, &crate::installation::Installation::for_test()).await.err().expect("a read error is not a verdict").to_string();
        assert!(refused.contains(LATER_ID) && refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(older_asked.load(std::sync::atomic::Ordering::SeqCst), 0, "the restore did not roll back to the older copy");
        assert_eq!(std::fs::read_dir(&root).map(|entries| entries.count()).unwrap_or(0), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_manifest_over_four_mebibytes_is_refused() {
        let (source, copies, id, deployment) = plain_copy("big-manifest").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let mut manifest: Manifest = serde_json::from_slice(&read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap()).unwrap();
        manifest.files.push(ManifestFile { path: "x".repeat(5 * 1024 * 1024), size: 0, sha256: String::new() });
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
        let refused = restore(&s3_source(&client), None, &restored, Some(&key), false, &crate::installation::Installation::for_test()).await.err().expect("an altered manifest is refused").to_string();
        assert!(refused.contains("Nothing was installed"), "{refused}");
        assert_eq!(std::fs::read_dir(&restored).map(|entries| entries.count()).unwrap_or(0), 0);
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
        // Written by someone who can write to the bucket and holds no secret.
        let forged = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        let name = format!("copies/{}/0000000000001-forged.json", recovery::LEDGER_DIR);
        client.put_object().bucket("data").key(name).body(ByteStream::from(serde_json::to_vec(&forged).unwrap())).send().await.unwrap();
        let restored = scratch("forged-restored");
        let summary = restore(&s3_source(&client), None, &restored, Some(&key), false, &crate::installation::Installation::for_test()).await.unwrap();
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
        restore(&s3_source(&client), None, &sealed_root, Some(&key), false, &stores).await.unwrap();
        verify(&sealed_root);

        let other = fake_s3_client().await;
        send_genuine(&other, &copies.join(&id), &id, None).await;
        let open_root = scratch("pairs-open");
        restore(&s3_source(&other), None, &open_root, None, false, &stores).await.unwrap();
        verify(&open_root);

        // The directory target is never sealed, so a secret does not touch it.
        let directory_root = scratch("pairs-directory");
        restore(&Source::Directory(copies.clone()), None, &directory_root, Some(&key), false, &stores).await.unwrap();
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
        // The file-sealing key and the check value do not sign anything.
        assert_ne!(mac, key.check);
        for path in [source, copies] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_sealed_copy_without_a_mac_restores_only_when_the_override_is_passed() {
        let (source, copies, id, deployment) = plain_copy("unsigned").await;
        let client = fake_s3_client().await;
        let key = CopyKey::derive(TEST_SECRET);
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        // What an older Kernel wrote, or what someone stripping the field leaves.
        let bytes = read_s3_bytes(&client, "data", &format!("copies/{id}/{MANIFEST}")).await.unwrap();
        let mut manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
        manifest.mac = None;
        client.put_object().bucket("data").key(format!("copies/{id}/{MANIFEST}")).body(ByteStream::from(serde_json::to_vec_pretty(&manifest).unwrap())).send().await.unwrap();
        let stores = crate::installation::Installation::for_test();
        let refused_root = scratch("unsigned-refused");
        let refused = restore(&s3_source(&client), Some(&id), &refused_root, Some(&key), false, &stores).await.err().expect("no MAC, no restore").to_string();
        assert!(refused.contains("no manifest MAC") && refused.contains("KONAE_COPY_ACCEPT_UNSIGNED=1") && refused.contains("older Kernel"), "{refused}");
        assert_eq!(std::fs::read_dir(&refused_root).map(|entries| entries.count()).unwrap_or(0), 0);
        let restored = scratch("unsigned-restored");
        restore(&s3_source(&client), Some(&id), &restored, Some(&key), true, &stores).await.unwrap();
        assert_eq!(count(&crate::layout::folder(&restored, &deployment.id).join("data/database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        // The override never lets a plain manifest through under a secret.
        let plain_root = scratch("unsigned-plain");
        let plain = fake_s3_client().await;
        upload_plain(&plain, &copies.join(&id), &id).await;
        recovery::start(&s3_target(), Some(&plain), "data").await.unwrap();
        assert!(restore(&s3_source(&plain), None, &plain_root, Some(&key), true, &stores).await.is_err());
        for path in [source, copies, refused_root, restored, plain_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn a_signed_revocation_entry_applies_and_an_unsigned_one_only_under_the_override() {
        let (source, copies, id, deployment) = plain_copy("signed").await;
        let key = CopyKey::derive(TEST_SECRET);
        let stores = crate::installation::Installation::for_test();
        let identity = crate::layout::identity(&source, &deployment.id);
        let user: String = Connection::open(&identity).unwrap().query_row("SELECT id FROM app_users", [], |r| r.get(0)).unwrap();
        let suspended = |root: &Path| -> String {
            Connection::open(crate::layout::identity(root, &deployment.id)).unwrap().query_row("SELECT state FROM app_users", [], |r| r.get(0)).unwrap()
        };

        // Written by a Kernel with the secret.
        let client = fake_s3_client().await;
        send_genuine(&client, &copies.join(&id), &id, Some(&key)).await;
        let entry = recovery::Entry::new("app-user", Some(&deployment.id), &user, "suspended");
        recovery::write(&[s3_target()], Some(&client), "data", &entry, Some(&key)).await.unwrap();
        let applied = scratch("signed-applied");
        let summary = restore(&s3_source(&client), None, &applied, Some(&key), false, &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert_eq!(suspended(&applied), "suspended");

        // Signed with another secret: refused even under the override.
        let other = fake_s3_client().await;
        send_genuine(&other, &copies.join(&id), &id, Some(&key)).await;
        let wrong = CopyKey::derive("another secret that is long enough for tests");
        recovery::write(&[s3_target()], Some(&other), "data", &entry, Some(&wrong)).await.unwrap();
        let wrong_root = scratch("signed-wrong");
        let summary = restore(&s3_source(&other), None, &wrong_root, Some(&key), true, &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("does not verify")), "{:?}", summary.reconciled);
        assert_ne!(suspended(&wrong_root), "suspended");

        // Written before entries were signed.
        let legacy = fake_s3_client().await;
        send_genuine(&legacy, &copies.join(&id), &id, Some(&key)).await;
        recovery::write(&[s3_target()], Some(&legacy), "data", &entry, None).await.unwrap();
        let held = scratch("signed-held");
        let summary = restore(&s3_source(&legacy), None, &held, Some(&key), false, &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_err_and(|reason| reason.contains("KONAE_COPY_ACCEPT_UNSIGNED=1")), "{:?}", summary.reconciled);
        assert_ne!(suspended(&held), "suspended");
        let accepted = scratch("signed-accepted");
        let summary = restore(&s3_source(&legacy), None, &accepted, Some(&key), true, &stores).await.unwrap();
        assert!(summary.reconciled.as_ref().is_ok_and(|changed| *changed >= 1), "{:?}", summary.reconciled);
        assert_eq!(suspended(&accepted), "suspended");

        // A signed entry moved to another file name does not verify.
        let moved = fake_s3_client().await;
        send_genuine(&moved, &copies.join(&id), &id, Some(&key)).await;
        recovery::write(&[s3_target()], Some(&moved), "data", &entry, Some(&key)).await.unwrap();
        let original = format!("copies/{}/{}", recovery::LEDGER_DIR, entry.file_name_for_test());
        let bytes = read_s3_bytes(&moved, "data", &original).await.unwrap();
        moved.delete_object().bucket("data").key(&original).send().await.unwrap();
        moved.put_object().bucket("data").key(format!("copies/{}/0000000000001-moved.json", recovery::LEDGER_DIR)).body(ByteStream::from(bytes)).send().await.unwrap();
        let moved_root = scratch("signed-moved");
        let summary = restore(&s3_source(&moved), None, &moved_root, Some(&key), false, &stores).await.unwrap();
        assert!(summary.reconciled.is_err());
        for path in [source, copies, applied, wrong_root, held, accepted, moved_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// A restore brings back the same installation for the same owner, so
    /// API keys stay valid (ADR-0008); an import
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
        restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();

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
             VALUES ('pending-job', 'recovery', 'p@copy.test', 'installation', 'Konae', 'h', x'00', x'00', 9999999999, 'pending', 0, 0, 0),
                    ('sent-job', 'recovery', 'p@copy.test', 'installation', 'Konae', 'h', NULL, NULL, 9999999999, 'delivered', 0, 0, 0);",
        )
        .unwrap();
        drop(mail);
        let id = run_copy(&state(&source), &[Target::Directory(copies.clone())]).await.unwrap();
        assert!(copies.join(&id).join(crate::mail::MAIL_DATABASE).is_file());
        restore(&Source::Directory(copies.clone()), None, &restored, None, false, &crate::installation::Installation::for_test()).await.unwrap();
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
        let latest = restore(&Source::Directory(copies.clone()), None, &destination, None, false, &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!(latest.copy_id, id, "a copy without a manifest is never picked");
        let named = unfinished.file_name().unwrap().to_str().unwrap();
        let empty = scratch("damaged-empty");
        let message = restore(&Source::Directory(copies.clone()), Some(named), &empty, None, false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("incomplete"), "{message}");

        let artifact = crate::layout::dist(&copies.join(&id), &deployment.id).join("index.html");
        std::fs::write(&artifact, "<main>tampered</main>").unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None, false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
        assert!(message.contains("does not match its checksum"), "{message}");
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "a refused restore leaves the directory empty");

        let manifest_path = copies.join(&id).join(MANIFEST);
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["path"] = json!("../outside.sqlite");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty, None, false, &crate::installation::Installation::for_test()).await.err().unwrap().to_string();
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
