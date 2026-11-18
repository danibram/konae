
use crate::installation::Installation;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    hkdf, hmac,
    rand::{SecureRandom, SystemRandom},
};

use crate::{
    AppState, database_lock, list_s3_keys, now_ms, read_s3_bytes, recovery,
    safe_relative_path,
};

pub(crate) const DEPLOYMENT_DATABASES: [&str; 4] = ["database.sqlite", "identity.sqlite", "usage.sqlite", crate::layout::SECRETS_DATABASE];
const MANIFEST: &str = "manifest.json";
/// The prefix of copies in the installation's one bucket.
pub const S3_PREFIX: &str = "copies/";
const ENCRYPTION: &str = "AES-256-GCM";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SKIPPED_COPIES: usize = 50;
const KEEP_RECENT: usize = 24;
const KEEP_DAILY_DAYS: u64 = 30;
const DAY_MS: u64 = 86_400_000;
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
    /// Reads the secret as written, untrimmed.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var("KONAE_COPY_SECRET").ok().filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(secret) => {
                ensure!(secret.len() >= 32, "KONAE_COPY_SECRET must be at least 32 characters.");
                Ok(Some(Self::derive(&secret)))
            }
        }
    }

    /// The `keyCheck` a sealed manifest carries. It tells two secrets apart
    /// without revealing either.
    pub fn fingerprint(&self) -> &str {
        &self.check
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

    fn sign_manifest(&self, copy: &str, manifest: &Manifest) -> String {
        hex::encode(hmac::sign(&self.manifest_mac, &manifest_message(copy, manifest)).as_ref())
    }

    fn verify_manifest(&self, copy: &str, manifest: &Manifest, mac: &str) -> bool {
        hex::decode(mac).is_ok_and(|mac| hmac::verify(&self.manifest_mac, &manifest_message(copy, manifest), &mac).is_ok())
    }

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

fn manifest_message(copy: &str, manifest: &Manifest) -> Vec<u8> {
    let mut message = copy.as_bytes().to_vec();
    message.push(0);
    let plain = Manifest { mac: None, ..manifest.clone() };
    message.extend(serde_json::to_vec(&plain).expect("a manifest serialises"));
    message
}

fn entry_message(name: &str, entry: &recovery::Entry) -> Result<Vec<u8>> {
    let mut message = name.as_bytes().to_vec();
    message.push(0);
    message.extend(serde_json::to_vec(entry)?);
    Ok(message)
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

#[derive(Default)]
pub struct CopyStatus {
    targets: Vec<&'static str>,
    interval_seconds: Option<u64>,
    reports: std::sync::Mutex<HashMap<&'static str, TargetReport>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    home_copy: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CopyState {
    copy_id: String,
    files: Vec<ManifestFile>,
    blobs: HashMap<String, BlobStamp>,
    #[serde(default)]
    s3_manifest: Option<Manifest>,
    #[serde(default)]
    s3_manifests: HashMap<String, Manifest>,
}

type ManifestCache = HashMap<String, std::result::Result<Manifest, String>>;

struct S3CopyTarget<'a> {
    client: &'a S3Client,
    bucket: &'a str,
    prefix: &'a str,
    key: Option<&'a CopyKey>,
}

#[derive(Clone, Serialize, Deserialize)]
struct BlobStamp {
    size: u64,
    modified_ms: Option<u64>,
}

struct StagedCopy {
    manifest: Manifest,
    state: CopyState,
}

/// Takes one copy and sends it to every target, then prunes old copies.
/// Returns the copy ID when every target succeeded.
pub async fn run_copy(state: &AppState, targets: &[Target]) -> Result<String> {
    let started = now_ms();
    let id = copy_id(started);
    let staging_root = state.data_root.join(".copy-staging");
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    let staging = staging_root.join(&id);
    let mut cache = ManifestCache::new();
    let mut staged = stage_copy(state, targets, &staging, &id, started, &mut cache).await;
    let mut failures = Vec::new();
    for target in targets {
        let kind = target.kind();
        state.copies.update(kind, |report| report.last_attempt_at = Some(started));
        let outcome = match &staged {
            Err(error_value) => Err(anyhow::anyhow!("the snapshot failed: {error_value:#}")),
            Ok(staged) => match recovery::start(target, state.s3.as_ref(), &state.data_bucket).await {
                Ok(()) => send(state, target, &staging, &staged.manifest, &id, &mut cache).await,
                Err(error_value) => Err(error_value.context("the revocation record could not be started")),
            },
        };
        match outcome {
            Ok((pruned, s3_manifest)) => {
                println!("Copy {id} sent to the {kind} target; pruned {pruned} old copies.");
                if let (Ok(staged), Some(manifest)) = (&mut staged, s3_manifest) {
                    staged.state.s3_manifest = Some(manifest);
                }
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
    if failures.is_empty() {
        if let Ok(staged) = staged {
            let mut saved = staged.state;
            saved.copy_id = id.clone();
            saved.s3_manifests = cache.iter().filter_map(|(key, value)| value.as_ref().ok().map(|manifest| (key.clone(), manifest.clone()))).collect();
            tokio::fs::write(state.data_root.join(".copy-state.json"), serde_json::to_vec_pretty(&saved)?).await?;
        }
        Ok(id)
    } else { bail!("{}", failures.join("; ")) }
}

async fn send(state: &AppState, target: &Target, staging: &Path, manifest: &Manifest, id: &str, cache: &mut ManifestCache) -> Result<(usize, Option<Manifest>)> {
    match target {
        Target::Directory(directory) => {
            let (staging, directory, id, manifest) = (staging.to_owned(), directory.clone(), id.to_owned(), manifest.clone());
            tokio::task::spawn_blocking(move || {
                send_to_directory(&staging, &directory, &id, &manifest)?;
                Ok((prune_directory(&directory, now_ms())?, None))
            })
            .await?
        }
        Target::S3 { prefix } => {
            let s3 = state.s3.as_ref().context("S3 is not configured.")?;
            let stored = send_to_s3(S3CopyTarget { client: s3, bucket: &state.data_bucket, prefix, key: state.copies.key.as_deref() }, staging, manifest, id, cache).await?;
            let pruned = prune_s3_cached(s3, &state.data_bucket, prefix, id, now_ms(), cache).await?;
            Ok((pruned, Some(stored)))
        }
    }
}

/// Builds the copy in `staging`. Each Deployment is snapshotted under its
/// data and identity locks, the same locks an action, an upload, a signup and
/// a publish take, so its databases, files and artifact agree with each other.
async fn stage_copy(state: &AppState, targets: &[Target], staging: &Path, id: &str, created_at: u64, cache: &mut ManifestCache) -> Result<StagedCopy> {
    let prior = tokio::fs::read(state.data_root.join(".copy-state.json")).await.ok()
        .and_then(|bytes| serde_json::from_slice::<CopyState>(&bytes).ok());
    if let Some(prior) = prior.as_ref() {
        for (key, manifest) in &prior.s3_manifests {
            cache.insert(key.clone(), Ok(manifest.clone()));
        }
        if let (Some(manifest), Some(Target::S3 { prefix })) = (prior.s3_manifest.clone(), targets.iter().find(|target| matches!(target, Target::S3 { .. }))) {
            let source = Source::S3 { client: state.s3.as_ref().context("S3 is not configured.")?.clone(), bucket: state.data_bucket.clone(), prefix: normalize_prefix(prefix) };
            cache.insert(manifest_cache_key(&source, &prior.copy_id), Ok(manifest));
        }
    }
    let carry = match prior.as_ref() { Some(prior) => targets_hold_state(state, targets, prior, cache).await?, None => false };
    let mut carried_files = Vec::new();
    let mut blob_stamps = HashMap::new();
    tokio::fs::create_dir_all(crate::layout::apps(staging)).await?;
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
        let (source, target) = (entry.path(), crate::layout::folder(staging, &deployment));
        tokio::task::spawn_blocking(move || copy_deployment(&source, &target))
            .await?
            .with_context(|| format!("Deployment {deployment}"))?;
        drop((_project, _data, _identity));
        copy_file_bytes(state, &deployment, &crate::layout::files(staging, &deployment), prior.as_ref().filter(|_| carry), &mut carried_files, &mut blob_stamps)
            .await
            .with_context(|| format!("The files of Deployment {deployment}"))?;
    }
    let (root, id) = (staging.to_owned(), id.to_owned());
    tokio::task::spawn_blocking(move || {
        let mut files = list_files(&root)?;
        let present: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
        files.extend(carried_files.into_iter().filter(|file| !present.contains(&file.path)));
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = Manifest { version: 1, copy_id: id.clone(), created_at, files: files.clone(), encryption: None, mac: None };
        std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
        Ok(StagedCopy { manifest, state: CopyState { copy_id: id, files, blobs: blob_stamps, s3_manifest: None, s3_manifests: HashMap::new() } })
    })
    .await?
}

async fn copy_file_bytes(state: &AppState, deployment: &str, files: &Path, prior: Option<&CopyState>, carried: &mut Vec<ManifestFile>, stamps: &mut HashMap<String, BlobStamp>) -> Result<()> {
    let index_path = files.join("index.json");
    let index: Vec<serde_json::Value> = match tokio::fs::read(&index_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error_value) => return Err(error_value.into()),
    };
    let objects: HashMap<String, crate::file_store::StoredObject> = state.files.store.objects(deployment).await?.into_iter().map(|object| (object.id.clone(), object)).collect();
    let mut kept = Vec::with_capacity(index.len());
    for file in index {
        let id = file["id"].as_str().filter(|id| crate::valid_file_id(id)).context("A stored file has an invalid id.")?.to_owned();
        let path = format!("apps/{deployment}/data/files/{id}.blob");
        let Some(object) = objects.get(&id) else { continue };
        let stamp = BlobStamp { size: object.size, modified_ms: object.modified_ms };
        let old_stamp = prior.and_then(|prior| prior.blobs.get(&path));
        let old_file = prior.and_then(|prior| prior.files.iter().find(|file| file.path == path));
        if old_stamp.is_some_and(|old| old.size == stamp.size && old.modified_ms == stamp.modified_ms)
            && let Some(old_file) = old_file
        {
            carried.push(old_file.clone());
            stamps.insert(path, stamp);
            kept.push(file);
            continue;
        }
        if let Some(blob) = state.files.store.get(deployment, &id, None).await? {
            crate::file_store::save_blob(blob, &files.join(format!("{id}.blob"))).await?;
            stamps.insert(path, stamp);
            kept.push(file);
        }
    }
    tokio::fs::write(&index_path, serde_json::to_vec_pretty(&kept)?).await?;
    Ok(())
}

async fn targets_hold_state(state: &AppState, targets: &[Target], prior: &CopyState, cache: &mut ManifestCache) -> Result<bool> {
    if targets.is_empty() { return Ok(false); }
    for target in targets {
        let manifest = match target {
            Target::Directory(directory) => {
                let bytes = match tokio::fs::read(directory.join(&prior.copy_id).join(MANIFEST)).await {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(error.into()),
                };
                serde_json::from_slice::<Manifest>(&bytes)?
            }
            Target::S3 { prefix } => {
                let Some(s3) = state.s3.as_ref() else { return Ok(false) };
                let source = Source::S3 { client: s3.clone(), bucket: state.data_bucket.clone(), prefix: normalize_prefix(prefix) };
                match read_manifest_cached(&source, &prior.copy_id, cache).await? { Ok(manifest) => manifest, Err(_) => return Ok(false) }
            }
        };
        if manifest.copy_id != prior.copy_id || manifest.files.len() != prior.files.len() || manifest.files.iter().any(|file| !prior.files.iter().any(|old| old.path == file.path && old.size == file.size && old.sha256 == file.sha256)) { return Ok(false); }
        if let Target::Directory(directory) = target {
            for path in prior.blobs.keys() {
                let Some(file) = prior.files.iter().find(|file| file.path == *path) else { return Ok(false) };
                let stored = directory.join(&prior.copy_id).join(path);
                if !stored.is_file() || checksum_file(&stored)? != (file.size, file.sha256.clone()) { return Ok(false); }
            }
        }
        if let Target::S3 { prefix } = target {
            let s3 = state.s3.as_ref().context("S3 is not configured.")?;
            match state.copies.key() {
                Some(key) if manifest.encryption.as_ref().is_some_and(|encryption| encryption.key_check == key.fingerprint()) && check_sealed(&manifest.copy_id, &manifest, Some(key)).is_ok() => {}
                None if manifest.encryption.is_none() => {}
                _ => return Ok(false),
            }
            let sizes = s3_object_sizes(s3, &state.data_bucket, prefix).await?;
            if !sizes.contains_key(&format!("{}{}/{MANIFEST}", normalize_prefix(prefix), prior.copy_id)) { return Ok(false); }
            for file in &manifest.files {
                let home = file.home_copy.as_deref().unwrap_or(&prior.copy_id);
                if parse_copy_id(home).is_none() { return Ok(false); }
                let size = file.size + if manifest.encryption.is_some() { (NONCE_LEN + 16) as u64 } else { 0 };
                if sizes.get(&format!("{}{home}/{}", normalize_prefix(prefix), file.path)) != Some(&size) { return Ok(false); }
            }
        }
    }
    Ok(true)
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
            ".konae" if kind.is_dir() => copy_private(&from, &to)?,
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
                if from.join("index.json").is_file() {
                    std::fs::copy(from.join("index.json"), to.join("index.json"))?;
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
            _ => {}
        }
    }
    Ok(())
}

fn is_backup_file(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else { return false };
    (extension == "sqlite" || extension == "json")
        && stem.len() == 36
        && stem.chars().all(|character| character.is_ascii_hexdigit() || character == '-')
}

pub fn vacuum_into(source: &Path, target: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
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

fn send_to_directory(staging: &Path, directory: &Path, id: &str, manifest: &Manifest) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Could not create {}", directory.display()))?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".partial-") {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    let partial = directory.join(format!(".partial-{id}"));
    let previous = std::fs::read_dir(directory)?.filter_map(|entry| entry.ok()).filter_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        (parse_copy_id(&name).is_some() && entry.path().join(MANIFEST).is_file()).then_some((name, entry.path()))
    }).max_by(|left, right| left.0.cmp(&right.0));
    let previous_files = previous.as_ref().and_then(|(_, path)| {
        serde_json::from_slice::<Manifest>(&std::fs::read(path.join(MANIFEST)).ok()?).ok()
    }).map(|manifest| manifest.files.into_iter().map(|file| (file.path.clone(), file)).collect::<HashMap<_, _>>()).unwrap_or_default();
    let previous_root = previous.map(|(_, path)| path);
    copy_tree_incremental(staging, staging, &partial, previous_root.as_deref(), &previous_files)?;
    if let Some(previous_root) = previous_root {
        for file in &manifest.files {
            let target = partial.join(&file.path);
            if target.exists() { continue; }
            let source = previous_root.join(&file.path);
            if !source.is_file() || checksum_file(&source)? != (file.size, file.sha256.clone()) {
                anyhow::bail!("Previous directory copy is missing unchanged file {}.", file.path);
            }
            if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
            if std::fs::hard_link(&source, &target).is_err() { std::fs::copy(source, target)?; }
        }
    }
    std::fs::rename(&partial, directory.join(id))?;
    Ok(())
}

fn copy_tree_incremental(root: &Path, source: &Path, target: &Path, previous: Option<&Path>, previous_files: &HashMap<String, ManifestFile>) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree_incremental(root, &from, &to, previous, previous_files)?;
            continue;
        }
        if entry.file_name() == MANIFEST {
            std::fs::copy(&from, &to)?;
            continue;
        }
        let rel = from.strip_prefix(root)?;
        let candidate = previous.map(|root| root.join(rel));
        let same = if let (Some(candidate), Some(expected)) = (candidate.as_ref(), previous_files.get(&rel.to_string_lossy().replace('\\', "/"))) {
            candidate.is_file() && checksum_file(&from)? == (expected.size, expected.sha256.clone()) && checksum_file(candidate)? == (expected.size, expected.sha256.clone())
        } else {
            false
        };
        if same {
            if std::fs::hard_link(candidate.expect("same candidate"), &to).is_err() {
                std::fs::copy(&from, &to)?;
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
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

fn sealed_manifest(manifest: &Manifest, key: &CopyKey) -> Manifest {
    let mut sealed = Manifest {
        encryption: Some(Encryption { algorithm: ENCRYPTION.into(), key_check: key.check.clone() }),
        mac: None,
        ..manifest.clone()
    };
    sealed.mac = Some(key.sign_manifest(&manifest.copy_id, &sealed));
    sealed
}

async fn send_to_s3(
    target: S3CopyTarget<'_>,
    staging: &Path,
    manifest: &Manifest,
    id: &str,
    cache: &mut ManifestCache,
) -> Result<Manifest> {
    let S3CopyTarget { client: s3, bucket, prefix, key } = target;
    let source = Source::S3 { client: s3.clone(), bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) };
    let previous_id = source.complete_copies().await?.into_iter().filter(|copy| copy.as_str() < id).max();
    let previous = match previous_id {
        Some(copy) => read_manifest_cached(&source, &copy, cache).await?.ok(),
        None => None,
    };
    let previous = previous.filter(|manifest| match key {
        Some(key) => manifest.encryption.as_ref().is_some_and(|encryption| {
            encryption.algorithm == ENCRYPTION && encryption.key_check == key.fingerprint()
        }) && check_sealed(&manifest.copy_id, manifest, Some(key)).is_ok(),
        None => manifest.encryption.is_none(),
    });
    let previous_files: HashMap<&str, &ManifestFile> = previous.as_ref().map(|manifest| {
        manifest.files.iter().map(|file| (file.path.as_str(), file)).collect()
    }).unwrap_or_default();
    let existing_sizes = s3_object_sizes(s3, bucket, prefix).await?;
    let mut next = manifest.clone();
    next.version = 2;
    let mut uploads = tokio::task::JoinSet::new();
    for file in &mut next.files {
        let unchanged = previous_files.get(file.path.as_str()).filter(|old| {
            let home = old.home_copy.as_deref().unwrap_or_else(|| previous.as_ref().map(|manifest| manifest.copy_id.as_str()).unwrap_or(id));
            let sealed_size = old.size + if key.is_some() { (NONCE_LEN + 16) as u64 } else { 0 };
            old.size == file.size && old.sha256 == file.sha256
                && parse_copy_id(home).is_some()
                && existing_sizes.get(&format!("{}{home}/{}", normalize_prefix(prefix), old.path)) == Some(&sealed_size)
        });
        if let Some(old) = unchanged {
            file.home_copy = Some(old.home_copy.clone().unwrap_or_else(|| previous.as_ref().unwrap().copy_id.clone()));
            continue;
        }
        file.home_copy = Some(id.to_owned());
        if uploads.len() >= 8 && let Some(result) = uploads.join_next().await {
            result??;
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
    let stored = match key {
        Some(key) => sealed_manifest(&next, key),
        None => next.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&stored)?;
    let present = s3_object_sizes(s3, bucket, prefix).await?;
    for file in &next.files {
        let home = file.home_copy.as_deref().unwrap_or(id);
        let object_key = format!("{prefix}{home}/{}", file.path);
        let expected = file.size + if key.is_some() { (NONCE_LEN + 16) as u64 } else { 0 };
        ensure!(safe_relative_path(&file.path) && parse_copy_id(home).is_some(), "Copy {id} has an invalid object reference.");
        ensure!(present.get(&object_key) == Some(&expected), "Copy {id} refers to missing or wrongly sized object {object_key}.");
    }
    s3.put_object()
        .bucket(bucket)
        .key(format!("{prefix}{id}/{MANIFEST}"))
        .body(ByteStream::from(manifest_bytes))
        .send()
        .await?;
    cache.insert(manifest_cache_key(&source, id), Ok(stored.clone()));
    Ok(stored)
}

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

async fn prune_s3_cached(s3: &S3Client, bucket: &str, prefix: &str, current: &str, now: u64, cache: &mut ManifestCache) -> Result<usize> {
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
    let expired = expired_copies(&complete, now);
    let mut keep = complete.iter().filter(|id| !expired.contains(id)).cloned().collect::<HashSet<_>>();
    keep.insert(current.to_owned());
    let mut marked = HashSet::new();
    for id in &keep {
        let source = Source::S3 { client: s3.clone(), bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) };
        let manifest = read_manifest_cached(&source, id, cache).await?
            .map_err(|reason| anyhow::anyhow!("Cannot prune copy {id} because its manifest is invalid: {reason}"))?;
        ensure!(manifest.copy_id == *id, "Cannot prune copy {id} because its manifest names another copy.");
        for file in manifest.files {
            let home = file.home_copy.as_deref().unwrap_or(id);
            ensure!(safe_relative_path(&file.path) && parse_copy_id(home).is_some(), "Cannot prune copy {id} because it has an invalid object reference.");
            marked.insert(format!("{prefix}{home}/{}", file.path));
        }
    }
    for id in &expired {
        let key = format!("{prefix}{id}/{MANIFEST}");
        s3.delete_object().bucket(bucket).key(key).send().await?;
    }
    for (id, keys) in &groups {
        for key in keys {
            if key.ends_with(&manifest_suffix) {
                if !keep.contains(id) && !expired.contains(id) {
                    s3.delete_object().bucket(bucket).key(key).send().await?;
                }
            } else if !marked.contains(key) {
                s3.delete_object().bucket(bucket).key(key).send().await?;
            }
        }
    }
    let cache_prefix = format!("{bucket}/{}", normalize_prefix(prefix));
    cache.retain(|key, _| {
        let Some(id) = key.strip_prefix(&cache_prefix) else { return true };
        parse_copy_id(id).is_none() || keep.contains(id)
    });
    Ok(expired.len())
}

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
    let summary = restore(&source, copy.as_deref(), &destination, CopyKey::from_env()?.as_ref(), installation).await?;
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
pub async fn read_record(from: &str, key: Option<&CopyKey>) -> Result<Option<Vec<Vec<u8>>>> {
    let source = source_for(from).await?;
    let source = match source {
        Source::Directory(path) if path.join(MANIFEST).is_file() => Source::Directory(path.parent().unwrap_or(Path::new(".")).to_owned()),
        other => other,
    };
    source.record(key).await
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

enum Source {
    Directory(PathBuf),
    S3 { client: S3Client, bucket: String, prefix: String },
}

impl Source {
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

    async fn record(&self, key: Option<&CopyKey>) -> Result<Option<Vec<Vec<u8>>>> {
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
}

struct RestoreSummary {
    copy_id: String,
    deployments: usize,
    files: usize,
    sessions_removed: usize,
    reconciled: std::result::Result<usize, String>,
}

async fn read_manifest(source: &Source, copy: &str) -> Result<std::result::Result<Manifest, String>> {
    let Some(bytes) = source.read_manifest_bytes(copy).await? else {
        return Ok(Err(format!("The manifest of copy {copy} is larger than 4 MiB.")));
    };
    let manifest: Manifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(Err(format!("The manifest of copy {copy} is invalid."))),
    };
    if manifest.version != 1 && manifest.version != 2 {
        return Ok(Err(format!("The manifest version of copy {copy} is not supported.")));
    }
    Ok(Ok(manifest))
}

fn manifest_cache_key(source: &Source, copy: &str) -> String {
    match source {
        Source::Directory(path) => format!("{}:{copy}", path.display()),
        Source::S3 { bucket, prefix, .. } => format!("{bucket}/{prefix}{copy}"),
    }
}

async fn read_manifest_cached(source: &Source, copy: &str, cache: &mut ManifestCache) -> Result<std::result::Result<Manifest, String>> {
    let cache_key = manifest_cache_key(source, copy);
    if let Some(manifest) = cache.get(&cache_key) {
        return Ok(manifest.clone());
    }
    let manifest = read_manifest(source, copy).await?;
    cache.insert(cache_key, manifest.clone());
    Ok(manifest)
}

fn not_encrypted(copy: &str) -> String {
    format!("Copy {copy} is not encrypted, but KONAE_COPY_SECRET is set, and a copy this installation sends to S3 with the secret is always encrypted.")
}

fn check_sealed(copy: &str, manifest: &Manifest, key: Option<&CopyKey>) -> Result<()> {
    let Some(encryption) = &manifest.encryption else {
        return Ok(());
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
        Some(mac) => ensure!(
            key.verify_manifest(copy, manifest, mac),
            "Copy {copy} has a manifest that does not match its MAC: the file list was altered after the Kernel wrote it. Nothing was installed."
        ),
        None => bail!("Copy {copy} has no manifest MAC. The Kernel signs every manifest it encrypts, so this one was altered. Nothing was installed."),
    }
    Ok(())
}

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
            check_sealed(copy, &manifest, Some(key)).map_err(|error| format!("{error:#}"))
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

async fn restore(
    source: &Source,
    copy: Option<&str>,
    destination: &Path,
    key: Option<&CopyKey>,
    installation: &Installation,
) -> Result<RestoreSummary> {
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
    ensure!(
        !copy.is_empty() && !copy.contains('/') && copy != "." && copy != "..",
        "The copy ID is invalid."
    );
    let manifest = read_manifest(source, &copy)
        .await
        .with_context(|| format!("Copy {copy} has no manifest.json, so it is incomplete or missing."))?
        .map_err(|reason| anyhow::anyhow!("{reason} Nothing was installed."))?;
    ensure!(
        !(matches!(source, Source::S3 { .. }) && key.is_some() && manifest.encryption.is_none()),
        "{} Name a copy this installation sent with --copy, for example from the copy IDs under the S3 prefix: {} restore --from <source> --copy <copy ID>. Nothing was installed.",
        not_encrypted(&copy),
        crate::program()
    );
    check_sealed(&copy, &manifest, key)?;
    for file in &manifest.files {
        let allowed = installation.holds(&file.path) || file.path.starts_with("apps/");
        ensure!(
            safe_relative_path(&file.path) && allowed,
            "The copy's manifest names a path outside a data directory: {}",
            file.path
        );
    }
    for file in &manifest.files {
        let home = file.home_copy.as_deref().unwrap_or(&copy);
        ensure!(
            parse_copy_id(home).is_some(),
            "Copy {copy} has an invalid object reference. Nothing was installed."
        );
    }
    let staging = destination.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    let staged = async {
        for file in &manifest.files {
            let home = file.home_copy.as_deref().unwrap_or(&copy);
            let mut bytes = source.read(home, &file.path).await.with_context(|| format!("Could not read {} from copy {home}", file.path))?;
            if let (Some(_), Some(key)) = (&manifest.encryption, key) {
                bytes = key
                    .open(home, &file.path, &bytes)
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
        let record = source.record(key).await;
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
        let result: String = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        ensure!(result == "ok", "{} failed SQLite's integrity check.", path.display());
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
                clear(&identity, &["app_sessions", "account_challenges"])?;
            }
        }
    }
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
        send_to_directory(&second, &directory, "2026-01-01T01-00-00.000Z", &manifest("2026-01-01T01-00-00.000Z")).unwrap();
        let original = std::fs::metadata(directory.join(first_id).join(path)).unwrap();
        let linked = std::fs::metadata(directory.join("2026-01-01T01-00-00.000Z").join(path)).unwrap();
        assert_eq!((original.dev(), original.ino()), (linked.dev(), linked.ino()));
        assert!(linked.nlink() >= 2);
        std::fs::write(second.join(path), b"changed bytes").unwrap();
        let (size, sha256) = checksum_file(&second.join(path)).unwrap();
        let changed = ManifestFile { path: path.into(), size, sha256, home_copy: None };
        let mut changed_manifest = manifest("2026-01-01T02-00-00.000Z");
        changed_manifest.files = vec![changed];
        send_to_directory(&second, &directory, "2026-01-01T02-00-00.000Z", &changed_manifest).unwrap();
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
    async fn staging_skips_current_blobs_and_missing_state_forces_a_full_read() {
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
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 1, "the second copy carries the previous hash without a blob read");
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
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 2, "the import reused an ID with different bytes and the copy read it fresh");
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
        assert_eq!(std::fs::read(crate::layout::files(&restored, &deployment.id).join(format!("{id}.blob"))).unwrap(), b"imported!!");
        std::fs::remove_file(source.join(".copy-state.json")).unwrap();
        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 3, "missing state forces a full blob read");
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
        std::fs::write(files.join("index.json"), json!([{"id": kept}, {"id": vanished}]).to_string()).unwrap();

        run_copy(&state, &[Target::Directory(copies.clone())]).await.unwrap();
        restore(&Source::Directory(copies.clone()), None, &restored, None, &crate::installation::Installation::for_test()).await.unwrap();
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
        assert_eq!(std::fs::read_dir(folder.join(".konae/backups")).unwrap().count(), 1, "the saved backup travels");
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
        assert_eq!(std::fs::read_dir(&refused_root).unwrap().count(), 0, "a refused restore installs nothing");
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
        state.copies = Arc::new(CopyStatus {
            key: Some(Arc::new(CopyKey::derive(TEST_SECRET))),
            ..CopyStatus::default()
        });
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
        copy_state.s3_manifest = Some(legacy);
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
        assert_eq!(second_store_gets, 0);
        assert_eq!(second_downloaded, 0);
        assert!(second_requests.iter().all(|request| request.method != "GET" || request.key.ends_with(&format!("/{MANIFEST}"))));
        assert_eq!(second_s3.get("GET").map_or(0, |counts| counts.0), 0, "the stable copy reads no S3 objects or manifests");
        tokio::time::sleep(Duration::from_millis(2)).await;
        std::fs::write(blob_dir.join(format!("{blob_id}.blob")), b"new attachment bytes").unwrap();
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
        assert_eq!(third_keys.len(), 2, "the changed blob and manifest were written");
        let third_prefix = format!("data/copies/{third}/");
        let third_file_puts: Vec<_> = third_requests.iter().filter(|request| request.method == "PUT" && request.key.starts_with(&third_prefix) && !request.key.ends_with(&format!("/{MANIFEST}"))).map(|request| request.key.as_str()).collect();
        assert_eq!(third_file_puts.len(), 1);
        assert!(third_file_puts[0].ends_with(&format!("/{blob_id}.blob")));
        println!("Copy {first}: file-store blob GETs {first_store_gets}, downloaded {first_downloaded} bytes; S3 {}; uploaded {first_uploaded} file bytes", s3_metrics_text(&first_s3));
        println!("Copy {second}: file-store blob GETs {second_store_gets}, downloaded {second_downloaded} bytes; S3 {}; uploaded {second_uploaded} file bytes", s3_metrics_text(&second_s3));
        println!("Copy {third}: file-store blob GETs {third_store_gets}, downloaded {changed_downloaded} bytes; S3 {}; uploaded {third_uploaded} file bytes", s3_metrics_text(&third_s3));
        println!("S3 copy objects after three copies: {}", list_s3_keys(&client, "data", "copies/").await.unwrap().len());
        let bytes = read_s3_bytes(&client, "data", &format!("copies/{third}/{MANIFEST}")).await.unwrap();
        let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(manifest.version, 2);
        assert!(manifest.files.iter().any(|file| file.path.ends_with(&format!("{blob_id}.blob")) && file.home_copy.as_deref() == Some(third.as_str())));
        assert!(manifest.files.iter().filter(|file| !file.path.ends_with(&format!("{blob_id}.blob"))).all(|file| file.home_copy.as_deref() == Some(first.as_str())));
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
        assert_eq!(store_gets.load(std::sync::atomic::Ordering::SeqCst), 2);
        let mut tampered = manifest.clone();
        tampered.files[0].home_copy = Some("2026-11-17T00-00-00.000Z".into());
        client.put_object().bucket("data").key(format!("copies/{third}/{MANIFEST}"))
            .body(ByteStream::from(serde_json::to_vec(&tampered).unwrap())).send().await.unwrap();
        let refused = scratch("tampered-reference");
        assert!(restore(&s3_source(&client), Some(&third), &refused, Some(state.copies.key().unwrap()), &crate::installation::Installation::for_test()).await.is_err());
        assert_eq!(std::fs::read_dir(&refused).unwrap().count(), 0);
        client.put_object().bucket("data").key(format!("copies/{third}/{MANIFEST}"))
            .body(ByteStream::from(bytes)).send().await.unwrap();
        let missing_file = manifest.files.iter().find(|file| file.home_copy.as_deref() == Some(first.as_str()) && !file.path.ends_with(".sqlite")).unwrap();
        client.delete_object().bucket("data").key(format!("copies/{}/{}", first, missing_file.path)).send().await.unwrap();
        let refused_missing = scratch("missing-reference");
        assert!(restore(&s3_source(&client), Some(&third), &refused_missing, Some(state.copies.key().unwrap()), &crate::installation::Installation::for_test()).await.is_err());
        assert_eq!(std::fs::read_dir(&refused_missing).unwrap().count(), 0);
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
        send_to_s3(S3CopyTarget { client, bucket: "data", prefix: "copies/", key: Some(key) }, staging, &manifest, &id, &mut cache).await?;
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
        send_to_s3(S3CopyTarget { client: &client, bucket: "data", prefix: "copies/", key: Some(&key) }, &staging, &stable_manifest, &unchanged, &mut cache).await.unwrap();
        prune_s3_cached(&client, "data", "copies/", &unchanged, at, &mut cache).await.unwrap();
        let (stable_s3, stable_requests) = s3_metrics_delta(&before_stable, &metrics.snapshot());
        assert_eq!(list_s3_keys(&client, "data", &format!("copies/{unchanged}/")).await.unwrap(), vec![format!("copies/{unchanged}/{MANIFEST}")]);
        assert_eq!(stable_s3.get("PUT").map(|counts| counts.0), Some(1));
        assert!(stable_requests.iter().all(|request| request.method != "GET" || request.key.ends_with(&format!("/{MANIFEST}"))));
        assert!(stable_requests.iter().all(|request| request.key != format!("data/copies/{home}/{MANIFEST}")));
        assert!(stable_requests.iter().all(|request| request.method != "GET" || request.key != format!("data/copies/{home}/{path}")));
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
        send_to_s3(S3CopyTarget { client, bucket: "data", prefix: "copies/", key }, copy, &manifest, id, &mut cache).await.unwrap();
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
        assert_eq!(std::fs::read_dir(&restored).map(|entries| entries.count()).unwrap_or(0), 0, "a refused restore installs nothing");
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
        assert_ne!(mac, key.check);
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
        assert_eq!(std::fs::read_dir(&refused_root).map(|entries| entries.count()).unwrap_or(0), 0);
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
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "a refused restore leaves the directory empty");

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
