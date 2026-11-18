use super::{CopyConfig, CopyKey, Source, Target, normalize_prefix};
use crate::now_ms;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex}};

const GRACE_SECONDS: u64 = 300;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TargetReport {
    pub(super) last_attempt_at: Option<u64>,
    pub(super) last_cut_at: Option<u64>,
    pub(super) last_success_at: Option<u64>,
    pub(super) last_copy_id: Option<String>,
    pub(super) last_error: Option<String>,
    pub(super) running: bool,
    #[serde(skip)]
    write_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreReceipt {
    copy_id: String,
    verified_at: u64,
}

pub(crate) fn status_directory(root: &Path) -> PathBuf {
    std::env::var_os("KONAE_COPY_HEALTH_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".copy-health"))
}

fn target_file(directory: &Path, target: &Target, endpoint: &str, bucket: &str) -> PathBuf {
    let identity = match target {
        Target::Directory(path) => {
            let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
            json!(["directory", path])
        }
        Target::S3 { prefix } => json!(["s3", endpoint.trim_end_matches('/'), bucket, normalize_prefix(prefix)]),
    };
    let hash = hex::encode(Sha256::digest(identity.to_string().as_bytes()));
    directory.join(format!("{}-{hash}", target.kind()))
}

fn read_report<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[derive(Default)]
pub struct CopyStatus {
    interval_seconds: Option<u64>,
    reports: Mutex<HashMap<&'static str, TargetReport>>,
    ledger: Vec<Target>,
    pub(super) key: Option<Arc<CopyKey>>,
    paths: HashMap<&'static str, PathBuf>,
}

impl CopyStatus {
    pub fn new(config: Option<&CopyConfig>) -> Self {
        Self {
            interval_seconds: config.map(|config| config.interval.as_secs()),
            ledger: config.map(|config| config.targets.clone()).unwrap_or_default(),
            key: config.and_then(|_| CopyKey::from_env().ok().flatten()).map(Arc::new),
            ..Default::default()
        }
    }

    pub(crate) fn open(config: Option<&CopyConfig>, directory: &Path, endpoint: &str, bucket: &str) -> Self {
        let mut status = Self::new(config);
        for target in &status.ledger {
            let path = target_file(directory, target, endpoint, bucket);
            let mut report = match read_report::<TargetReport>(&path.with_extension("json")) {
                Ok(report) => report.unwrap_or_default(),
                Err(error) => TargetReport { last_error: Some(format!("Could not read the saved copy status ({error}).")), ..Default::default() },
            };
            if report.running {
                report.running = false;
                report.last_error = Some("The Kernel stopped before recording a result for this attempt.".into());
            }
            status.reports.get_mut().unwrap().insert(target.kind(), report);
            status.paths.insert(target.kind(), path);
        }
        status
    }

    pub fn ledger_targets(&self) -> &[Target] {
        &self.ledger
    }

    pub fn key(&self) -> Option<&CopyKey> {
        self.key.as_deref()
    }

    pub(super) async fn update(&self, kind: &'static str, change: impl FnOnce(&mut TargetReport)) {
        let report = {
            let mut reports = self.reports.lock().unwrap_or_else(|err| err.into_inner());
            let report = reports.entry(kind).or_default();
            change(report);
            report.clone()
        };
        if let Some(path) = self.paths.get(kind) {
            let result = crate::write_atomic(&path.with_extension("json"), &serde_json::to_vec(&report).expect("serializable status")).await;
            let error = result.err().map(|error| format!("Could not save the {kind} copy status ({error})."));
            if let Some(error) = &error { eprintln!("{error}"); }
            self.reports.lock().unwrap_or_else(|err| err.into_inner()).entry(kind).or_default().write_error = error;
        }
    }

    pub fn health(&self) -> Value {
        self.health_at(now_ms())
    }

    pub(super) fn health_at(&self, now: u64) -> Value {
        let reports = self.reports.lock().unwrap_or_else(|err| err.into_inner());
        let mut alerts = Vec::new();
        let max_age = self.interval_seconds.unwrap_or(0).saturating_add(GRACE_SECONDS).saturating_mul(1000);
        let targets = self.ledger.iter().map(|target| {
            let kind = target.kind();
            let report = reports.get(kind).cloned().unwrap_or_default();
            let age = report.last_cut_at.map(|cut| now.saturating_sub(cut));
            let stale = age.is_none_or(|age| age > max_age);
            let mut problems = Vec::new();
            if let Some(error) = &report.last_error {
                problems.push(format!("The last copy to the {kind} target failed: {error}"));
            }
            if let Some(error) = report.write_error { problems.push(error); }
            if stale {
                problems.push(match age {
                    None => format!("The {kind} target has no recorded complete copy."),
                    Some(age) => format!("The latest complete {kind} copy has a cut {age} ms old. The limit is {max_age} ms."),
                });
            }
            let receipt = self.paths.get(kind).and_then(|path| {
                match read_report::<RestoreReceipt>(&path.with_extension("restore.json")) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        problems.push(format!("Could not read the verified restore for {kind} ({error})."));
                        None
                    }
                }
            });
            alerts.extend(problems.iter().cloned());
            json!({
                "kind": kind,
                "encrypted": kind == "s3" && self.key.is_some(),
                "lastAttemptAt": report.last_attempt_at,
                "lastCutAt": report.last_cut_at,
                "lastSuccessAt": report.last_success_at,
                "lastCopyId": report.last_copy_id,
                "lastError": report.last_error,
                "inProgress": report.running,
                "cutAgeMs": age,
                "stale": stale,
                "alert": (!problems.is_empty()).then(|| problems.join(" ")),
                "lastVerifiedRestoreAt": receipt.as_ref().map(|receipt| receipt.verified_at),
                "lastVerifiedRestoreCopyId": receipt.map(|receipt| receipt.copy_id),
            })
        }).collect::<Vec<_>>();
        json!({
            "targets": targets, "intervalSeconds": self.interval_seconds,
            "maxCutAgeMs": self.interval_seconds.map(|_| max_age),
            "alert": (!alerts.is_empty()).then(|| alerts.join(" ")),
        })
    }
}

pub(super) async fn record_restore(source: &Source, directory: &Path, copy_id: &str) -> Result<()> {
    let path = match source {
        Source::Directory(root) => {
            let root = if root.join(super::MANIFEST).is_file() { root.parent().unwrap_or(Path::new(".")) } else { root };
            target_file(directory, &Target::Directory(root.to_owned()), "", "")
        }
        Source::S3 { bucket, prefix, .. } => target_file(directory, &Target::S3 { prefix: prefix.clone() }, &crate::file_store::env_var("KONAE_S3_ENDPOINT").unwrap_or_default(), bucket),
    }.with_extension("restore.json");
    let receipt = RestoreReceipt { copy_id: copy_id.to_owned(), verified_at: now_ms() };
    crate::write_atomic(&path, &serde_json::to_vec(&receipt)?).await
        .with_context(|| format!("Copy {copy_id} was restored. Could not save its verified restore receipt to {}", path.display()))
}
