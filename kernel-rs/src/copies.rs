//! One folder per Deployment, copied off the volume.
//!
//! The data directory holds `owners.sqlite`, `mcp.sqlite` and one folder per
//! Deployment under `deployments/<id>/`. A copy is a consistent snapshot of
//! those, written to a directory target, an S3 prefix, or both, with a
//! `manifest.json` that lists every file with its size and SHA-256. The
//! `restore` subcommand installs one copy into an empty data directory.

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

use crate::{
    AppState, database_lock, list_s3_keys, now_ms, read_s3_bytes, s3_client_from_env,
    safe_relative_path,
};

/// SQLite files in a Deployment folder. Each is copied with `VACUUM INTO`,
/// never byte for byte, so a WAL database copies consistently.
/// `usage.sqlite` is ADR-0010's store; it is copied when it exists.
const DEPLOYMENT_DATABASES: [&str; 3] = ["database.sqlite", "identity.sqlite", "usage.sqlite"];
const ROOT_DATABASES: [&str; 2] = ["owners.sqlite", "mcp.sqlite"];
const MANIFEST: &str = "manifest.json";
const KEEP_RECENT: usize = 24;
const KEEP_DAILY_DAYS: u64 = 30;
const DAY_MS: u64 = 86_400_000;
const DEFAULT_INTERVAL_SECONDS: u64 = 3600;

// ---------------------------------------------------------------------------
// Layout migration

/// Moves `artifacts/<id>` and `backups/<id>` from the layout before
/// Deployment folders into `deployments/<id>/artifact` and
/// `deployments/<id>/backups`. Each move is one rename, so a Kernel stopped
/// halfway resumes on the next start. Returns one line per move. Refuses when
/// a destination already holds files, because one of the two copies would be
/// lost.
pub fn migrate_layout(root: &Path) -> Result<Vec<String>> {
    let mut moved = Vec::new();
    for (old, new) in [("artifacts", "artifact"), ("backups", "backups")] {
        let old_root = root.join(old);
        let entries = match std::fs::read_dir(&old_root) {
            Ok(entries) => entries,
            Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error_value) => {
                return Err(error_value).with_context(|| format!("Layout migration could not read {}.", old_root.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            let source = entry.path();
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                eprintln!("Layout migration left {} in place: the name is not UTF-8.", source.display());
                continue;
            };
            if !entry.file_type()?.is_dir() {
                eprintln!("Layout migration left {} in place: it is not a Deployment folder.", source.display());
                continue;
            }
            let target = root.join("deployments").join(&id).join(new);
            if target.exists() {
                let empty = std::fs::read_dir(&target).map(|mut entries| entries.next().is_none()).unwrap_or(false);
                if !empty {
                    bail!(
                        "Layout migration stopped: {} and {} both exist. Keep the one that holds the Deployment's files, delete the other, and start the Kernel again. Already moved: {}.",
                        source.display(),
                        target.display(),
                        if moved.is_empty() { "nothing".to_owned() } else { moved.join("; ") }
                    );
                }
                std::fs::remove_dir(&target)?;
            }
            std::fs::create_dir_all(target.parent().expect("target has a parent"))?;
            std::fs::rename(&source, &target).with_context(|| {
                format!(
                    "Layout migration stopped: moving {} to {} failed. Already moved: {}.",
                    source.display(),
                    target.display(),
                    if moved.is_empty() { "nothing".to_owned() } else { moved.join("; ") }
                )
            })?;
            moved.push(format!("{} -> {}", source.display(), target.display()));
        }
        // Succeeds only when nothing was left behind.
        let _ = std::fs::remove_dir(&old_root);
    }
    Ok(moved)
}

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
        Self::parse(
            read("LITTLEBOX_COPY_DIR").as_deref(),
            read("LITTLEBOX_COPY_S3_PREFIX").as_deref(),
            read("LITTLEBOX_COPY_INTERVAL_SECONDS").as_deref(),
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
                "LITTLEBOX_COPY_DIR must be outside LITTLEBOX_DATA_DIR, on another disk."
            );
            targets.push(Target::Directory(directory));
        }
        if let Some(prefix) = s3_prefix {
            ensure!(s3_configured, "LITTLEBOX_COPY_S3_PREFIX needs LITTLEBOX_S3_ENDPOINT.");
            let prefix = normalize_prefix(prefix);
            ensure!(!prefix.is_empty(), "LITTLEBOX_COPY_S3_PREFIX must name a prefix, such as copies/.");
            // Publishing deletes stale artifact keys under deployments/<id>/.
            ensure!(
                !prefix.starts_with("deployments/"),
                "LITTLEBOX_COPY_S3_PREFIX must not start with deployments/, where artifacts live."
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
                .context("LITTLEBOX_COPY_INTERVAL_SECONDS must be a whole number of seconds above zero.")?,
            None => DEFAULT_INTERVAL_SECONDS,
        };
        Ok(Some(Self { targets, interval: Duration::from_secs(seconds) }))
    }
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

/// What `/_littlebox/health` shows about copies. A failed copy never stops
/// the Kernel from serving; it shows here and in the log.
#[derive(Default)]
pub struct CopyStatus {
    targets: Vec<&'static str>,
    interval_seconds: Option<u64>,
    reports: std::sync::Mutex<HashMap<&'static str, TargetReport>>,
}

impl CopyStatus {
    pub fn new(config: Option<&CopyConfig>) -> Self {
        Self {
            targets: config.map(|config| config.targets.iter().map(Target::kind).collect()).unwrap_or_default(),
            interval_seconds: config.map(|config| config.interval.as_secs()),
            reports: Default::default(),
        }
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    copy_id: String,
    created_at: u64,
    files: Vec<ManifestFile>,
}

#[derive(Serialize, Deserialize)]
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
            Ok(manifest) => send(state, target, &staging, manifest, &id).await,
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
            send_to_s3(s3, &state.data_bucket, prefix, staging, manifest, id).await?;
            prune_s3(s3, &state.data_bucket, prefix, id, now_ms()).await
        }
    }
}

/// Builds the copy in `staging`. Each Deployment is snapshotted under its
/// data and identity locks, the same locks an action, an upload, a signup and
/// a publish take, so its databases, files and artifact agree with each other.
async fn stage_copy(state: &AppState, staging: &Path, id: &str, created_at: u64) -> Result<Manifest> {
    tokio::fs::create_dir_all(staging.join("deployments")).await?;
    for name in ROOT_DATABASES {
        let source = state.data_root.join(name);
        if source.exists() {
            let target = staging.join(name);
            tokio::task::spawn_blocking(move || vacuum_into(&source, &target)).await??;
        }
    }
    let mut entries = tokio::fs::read_dir(state.data_root.join("deployments")).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let Some(deployment) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let _data = database_lock(state, &deployment).await;
        let _identity = database_lock(state, &format!("identity:{deployment}")).await;
        let (source, target) = (entry.path(), staging.join("deployments").join(&deployment));
        tokio::task::spawn_blocking(move || copy_deployment(&source, &target))
            .await?
            .with_context(|| format!("Deployment {deployment}"))?;
    }
    let (root, id) = (staging.to_owned(), id.to_owned());
    tokio::task::spawn_blocking(move || {
        let manifest = Manifest { version: 1, copy_id: id, created_at, files: list_files(&root)? };
        std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
        Ok(manifest)
    })
    .await?
}

fn copy_deployment(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let (from, to) = (entry.path(), target.join(&*name));
        match &*name {
            "deployment.json" if kind.is_file() => {
                std::fs::copy(&from, &to)?;
            }
            database if kind.is_file() && DEPLOYMENT_DATABASES.contains(&database) => vacuum_into(&from, &to)?,
            "artifact" | "files" if kind.is_dir() => copy_tree(&from, &to)?,
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
            // WAL and journal files, temporary files and anything unknown stay behind.
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

fn vacuum_into(source: &Path, target: &Path) -> Result<()> {
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
        if name.to_string_lossy().starts_with('.') {
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

fn checksum_file(path: &Path) -> Result<(u64, String)> {
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

/// Uploads every file, then the manifest. A prefix without a manifest is an
/// unfinished upload and restore refuses it.
async fn send_to_s3(s3: &S3Client, bucket: &str, prefix: &str, staging: &Path, manifest: &Manifest, id: &str) -> Result<()> {
    let mut uploads = tokio::task::JoinSet::new();
    for file in &manifest.files {
        if uploads.len() >= 8 {
            if let Some(result) = uploads.join_next().await {
                result??;
            }
        }
        let bytes = tokio::fs::read(staging.join(&file.path)).await?;
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
    s3.put_object()
        .bucket(bucket)
        .key(format!("{prefix}{id}/{MANIFEST}"))
        .body(ByteStream::from(tokio::fs::read(staging.join(MANIFEST)).await?))
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

fn civil_from_days(days: i64) -> (i64, u32, u32) {
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
    "Usage: littlebox-kernel restore --from <copy directory | s3://bucket/prefix> [--copy <copy ID>]";

/// Handles `littlebox-kernel restore`. Returns false for any other command.
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
    let destination = PathBuf::from(std::env::var("LITTLEBOX_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let source = match from.strip_prefix("s3://") {
        Some(location) => {
            let (bucket, prefix) = location.split_once('/').unwrap_or((location, ""));
            ensure!(!bucket.is_empty(), "{RESTORE_USAGE}");
            let client = s3_client_from_env()
                .await
                .context("Set LITTLEBOX_S3_ENDPOINT and its credentials to restore from S3.")?;
            Source::S3 { client, bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) }
        }
        None => Source::Directory(PathBuf::from(from)),
    };
    let summary = restore(&source, copy.as_deref(), &destination).await?;
    println!(
        "Restored copy {} into {}: {} Deployments, {} files, checksums verified. Signed out {} sessions and credentials; owners, agents and app users sign in again.",
        summary.copy_id,
        destination.display(),
        summary.deployments,
        summary.files,
        summary.sessions_removed
    );
    Ok(true)
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
}

struct RestoreSummary {
    copy_id: String,
    deployments: usize,
    files: usize,
    sessions_removed: usize,
}

async fn restore(source: &Source, copy: Option<&str>, destination: &Path) -> Result<RestoreSummary> {
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
    for file in &manifest.files {
        let allowed = ROOT_DATABASES.contains(&file.path.as_str()) || file.path.starts_with("deployments/");
        ensure!(
            safe_relative_path(&file.path) && allowed,
            "The copy's manifest names a path outside a data directory: {}",
            file.path
        );
    }
    let staging = destination.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    let staged = async {
        for file in &manifest.files {
            let bytes = source.read(&copy, &file.path).await.with_context(|| format!("Could not read {}", file.path))?;
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
        tokio::task::spawn_blocking(move || -> Result<usize> {
            check_databases(&root)?;
            invalidate_sessions(&root)
        })
        .await?
    }
    .await;
    let sessions_removed = match staged {
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
    Ok(RestoreSummary { copy_id: copy, deployments, files: manifest.files.len(), sessions_removed })
}

/// Restore writes only into an empty data directory, so it can never merge a
/// copy with live state. A filesystem's `lost+found` does not count.
fn ensure_empty(destination: &Path) -> Result<()> {
    match std::fs::read_dir(destination) {
        Ok(entries) => {
            for entry in entries {
                if entry?.file_name() != "lost+found" {
                    bail!(
                        "Restore refuses {}: it is not empty. Point LITTLEBOX_DATA_DIR at an empty directory.",
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
    if let Ok(entries) = std::fs::read_dir(root.join("deployments")) {
        for entry in entries {
            let folder = entry?.path();
            for name in DEPLOYMENT_DATABASES {
                if folder.join(name).is_file() {
                    paths.push(folder.join(name));
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
        ("owners.sqlite", &["app_sessions", "creator_sessions"]),
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
    if let Ok(entries) = std::fs::read_dir(root.join("deployments")) {
        for entry in entries {
            let identity = entry?.path().join("identity.sqlite");
            if identity.is_file() {
                clear(&identity, &["app_sessions"])?;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Deployment, LockMap, SignupPolicy, auth::AuthService, initialize_database, limits::Guards,
        load_deployments, read_artifact,
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::RwLock;

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("littlebox-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn state(root: &Path) -> AppState {
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(CopyStatus::default()),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        }
    }

    /// A data directory with one published app: artifact, rows, an account,
    /// a live session and an uploaded file, plus an owner with a session.
    fn seed(root: &Path) -> (Deployment, String) {
        let deployment = Deployment {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: "app_copy".into(),
            project_name: "Copy test".into(),
            url: "http://copy-test.localhost".into(),
            created_at: now_ms(),
            manifest: json!({"visibility": "public"}),
            hostname: "copy-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: Some("v1".into()),
            signup_policy: SignupPolicy::default(),
        };
        let folder = root.join("deployments").join(&deployment.id);
        std::fs::create_dir_all(folder.join("artifact/v1")).unwrap();
        std::fs::write(folder.join("artifact/v1/index.html"), "<main>copied app</main>").unwrap();
        std::fs::write(folder.join("deployment.json"), serde_json::to_vec(&deployment).unwrap()).unwrap();
        initialize_database(&folder.join("database.sqlite")).unwrap();
        Connection::open(folder.join("database.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('kept');")
            .unwrap();
        let session = AuthService::open(folder.join("identity.sqlite"))
            .unwrap()
            .signup("writer@copy.test", "correct horse battery")
            .unwrap();
        std::fs::create_dir_all(folder.join("files")).unwrap();
        let file_id = uuid::Uuid::new_v4().to_string();
        std::fs::write(folder.join("files").join(format!("{file_id}.blob")), b"attachment").unwrap();
        std::fs::write(folder.join("files/index.json"), json!([{"id": file_id}]).to_string()).unwrap();
        std::fs::write(folder.join("files/.index.json.tmp-x"), "temporary").unwrap();
        std::fs::create_dir_all(folder.join("backups")).unwrap();
        let backup_id = uuid::Uuid::new_v4();
        std::fs::write(folder.join("backups").join(format!("{backup_id}.json")), "{}").unwrap();
        std::fs::write(folder.join("backups/portable-data-x.sqlite"), "scratch").unwrap();
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
    fn the_old_layout_moves_into_deployment_folders_once() {
        let root = scratch("migrate");
        std::fs::create_dir_all(root.join("artifacts/app-1/v1")).unwrap();
        std::fs::write(root.join("artifacts/app-1/v1/index.html"), "one").unwrap();
        std::fs::create_dir_all(root.join("backups/app-1")).unwrap();
        std::fs::write(root.join("backups/app-1/b.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("deployments/app-1/backups")).unwrap();
        std::fs::write(root.join("artifacts/stray.txt"), "not a folder").unwrap();

        let moved = migrate_layout(&root).unwrap();
        assert_eq!(moved.len(), 2, "{moved:?}");
        assert_eq!(std::fs::read(root.join("deployments/app-1/artifact/v1/index.html")).unwrap(), b"one");
        assert!(root.join("deployments/app-1/backups/b.json").is_file());
        assert!(!root.join("backups").exists(), "an emptied old root is removed");
        assert!(root.join("artifacts/stray.txt").is_file(), "a stray file stays where it was");
        assert!(migrate_layout(&root).unwrap().is_empty(), "a second start moves nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_migration_refuses_to_start_when_both_layouts_hold_files() {
        let root = scratch("migrate-conflict");
        std::fs::create_dir_all(root.join("artifacts/app-1/v1")).unwrap();
        std::fs::create_dir_all(root.join("deployments/app-1/artifact/v2")).unwrap();
        let message = migrate_layout(&root).unwrap_err().to_string();
        assert!(message.contains("both exist"), "{message}");
        assert!(root.join("artifacts/app-1/v1").is_dir(), "nothing moved");
        assert!(root.join("deployments/app-1/artifact/v2").is_dir());
        let _ = std::fs::remove_dir_all(root);
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
        let data = Path::new("/srv/littlebox/data");
        assert!(CopyConfig::parse(None, None, None, data, false).unwrap().is_none());
        let config = CopyConfig::parse(Some("/mnt/second"), Some("/copies"), None, data, true).unwrap().unwrap();
        assert_eq!(config.targets, vec![Target::Directory("/mnt/second".into()), Target::S3 { prefix: "copies/".into() }]);
        assert_eq!(config.interval, Duration::from_secs(3600));
        let refuse = |dir, prefix, interval, s3| CopyConfig::parse(dir, prefix, interval, data, s3).unwrap_err().to_string();
        assert!(refuse(Some("/srv/littlebox/data/copies"), None, None, false).contains("outside"));
        assert!(refuse(None, Some("deployments/x"), None, true).contains("must not start with deployments/"));
        assert!(refuse(None, Some("copies"), None, false).contains("LITTLEBOX_S3_ENDPOINT"));
        assert!(refuse(Some("/mnt/second"), None, Some("0"), false).contains("above zero"));
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

        let copied = copies.join(&id).join("deployments").join(&deployment.id);
        assert!(!copied.join("files/.index.json.tmp-x").exists(), "temporary files stay behind");
        assert!(!copied.join("backups/portable-data-x.sqlite").exists(), "export scratch stays behind");

        let summary = restore(&Source::Directory(copies.clone()), None, &restored).await.unwrap();
        assert_eq!((summary.copy_id.as_str(), summary.deployments), (id.as_str(), 1));
        let loaded = load_deployments(&restored).await.unwrap();
        let served = loaded.get("copy-test.localhost").expect("the restored Kernel knows the app");
        assert_eq!(read_artifact(&self::state(&restored), served, "index.html").await.unwrap(), b"<main>copied app</main>");
        let folder = restored.join("deployments").join(&deployment.id);
        assert_eq!(count(&folder.join("database.sqlite"), "SELECT COUNT(*) FROM notes"), 1);
        let identity = AuthService::open(folder.join("identity.sqlite")).unwrap();
        assert!(matches!(identity.authenticate(Some(&old_token)), Ok(None)), "the old session is gone");
        identity.login("writer@copy.test", "correct horse battery").expect("the account and password survive");
        AuthService::open(restored.join("owners.sqlite"))
            .unwrap()
            .login("owner@copy.test", "correct horse battery")
            .expect("the owner account survives");
        assert_eq!(count(&restored.join("owners.sqlite"), "SELECT COUNT(*) FROM app_sessions"), 1, "only the new login");
        let index: Value = serde_json::from_slice(&std::fs::read(folder.join("files/index.json")).unwrap()).unwrap();
        let file_id = index[0]["id"].as_str().unwrap();
        assert_eq!(std::fs::read(folder.join("files").join(format!("{file_id}.blob"))).unwrap(), b"attachment");
        assert_eq!(std::fs::read_dir(folder.join("backups")).unwrap().count(), 1, "the saved backup travels");
        assert!(std::fs::read_dir(&restored).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".restore-")));

        let message = restore(&Source::Directory(copies.clone()), None, &restored).await.err().unwrap().to_string();
        assert!(message.contains("not empty"), "{message}");
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
        let latest = restore(&Source::Directory(copies.clone()), None, &destination).await.unwrap();
        assert_eq!(latest.copy_id, id, "a copy without a manifest is never picked");
        let named = unfinished.file_name().unwrap().to_str().unwrap();
        let empty = scratch("damaged-empty");
        let message = restore(&Source::Directory(copies.clone()), Some(named), &empty).await.err().unwrap().to_string();
        assert!(message.contains("incomplete"), "{message}");

        let artifact = copies.join(&id).join("deployments").join(&deployment.id).join("artifact/v1/index.html");
        std::fs::write(&artifact, "<main>tampered</main>").unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty).await.err().unwrap().to_string();
        assert!(message.contains("does not match its checksum"), "{message}");
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "a refused restore leaves the directory empty");

        let manifest_path = copies.join(&id).join(MANIFEST);
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][0]["path"] = json!("../outside.sqlite");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        let message = restore(&Source::Directory(copies.join(&id)), None, &empty).await.err().unwrap().to_string();
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
        let database = source.join("deployments").join(&deployment.id).join("database.sqlite");
        Connection::open(&database).unwrap().execute("INSERT INTO notes VALUES ('during')", []).unwrap();
        drop(held);
        let id = copying.await.unwrap().unwrap();
        let copied = copies.join(id).join("deployments").join(&deployment.id).join("database.sqlite");
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
