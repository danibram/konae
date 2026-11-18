//! Write directory v1 and S3 v2 copies. Read directory and S3 copies in either format.
pub mod blocks;
mod crypto;
mod reader;
mod writer;
mod snapshot;
pub use snapshot::SqliteSnapshot;
#[cfg(feature = "s3")]
mod s3;

pub use crypto::CopyKey;
pub use reader::{CopyReader, Source, UnencryptedS3Copy, check_sealed, newest_authentic_copy, read_manifest, verified_bytes};
#[cfg(feature = "s3")]
pub use reader::manifest_cache_key;
pub use writer::{checksum_file, expired_copies, list_files, prune_directory, send_to_directory, vacuum_into};
#[cfg(feature = "s3")]
pub use s3::{S3CopyTarget, list_s3_keys, prune_s3_cached, read_s3_bytes, send_to_s3};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

/// The application selects which paths can enter a copy or a restored volume.
pub trait PathScope: Sync {
    fn allows(&self, path: &str) -> bool;

    fn sqlite_image(&self, _path: &str) -> bool { false }
}

impl<F: Fn(&str) -> bool + Sync> PathScope for F {
    fn allows(&self, path: &str) -> bool {
        self(path)
    }
}

pub const MANIFEST: &str = "manifest.json";
const ENCRYPTION: &str = "AES-256-GCM";
pub const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SKIPPED_COPIES: usize = 50;
const KEEP_RECENT: usize = 24;
const KEEP_DAILY_DAYS: u64 = 30;
pub const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Encryption {
    pub algorithm: String,
    pub key_check: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub copy_id: String,
    pub created_at: u64,
    pub files: Vec<ManifestFile>,
    /// Present when the files are sealed; sizes and checksums are of the
    /// plain files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<Encryption>,
    /// HMAC-SHA256 covers the copy ID and the manifest with this field omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_copy: Option<String>,
}

pub type ManifestCache = HashMap<String, std::result::Result<Manifest, String>>;

pub fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}/") }
}

pub fn copy_id(at_ms: u64) -> String {
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

pub fn parse_copy_id(id: &str) -> Option<u64> {
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

pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn check_manifest_paths(scope: &dyn PathScope, copy: &str, manifest: &Manifest) -> Result<()> {
    for file in &manifest.files {
        let allowed = scope.allows(&file.path);
        ensure!(
            safe_relative_path(&file.path) && allowed,
            "The copy's manifest names a path outside a data directory: {}",
            file.path
        );
    }
    for file in &manifest.files {
        let home = file.home_copy.as_deref().unwrap_or(copy);
        ensure!(
            parse_copy_id(home).is_some(),
            "Copy {copy} has an invalid object reference. Nothing was installed."
        );
    }
    Ok(())
}

pub fn encode_manifest(scope: &dyn PathScope, id: &str, manifest: &Manifest) -> Result<Vec<u8>> {
    ensure!(parse_copy_id(id).is_some() && manifest.copy_id == id, "The manifest must name the copy being published.");
    check_manifest_paths(scope, id, manifest)?;
    let bytes = serde_json::to_vec_pretty(manifest)?;
    ensure!(bytes.len() as u64 <= MAX_MANIFEST_BYTES,
        "The final manifest of copy {} is {} bytes. Restore accepts at most {MAX_MANIFEST_BYTES} bytes.",
        manifest.copy_id, bytes.len());
    Ok(bytes)
}

pub fn sealed_manifest(manifest: &Manifest, key: &CopyKey) -> Manifest {
    let mut sealed = Manifest {
        encryption: Some(Encryption { algorithm: ENCRYPTION.into(), key_check: key.fingerprint().to_owned() }),
        mac: None,
        ..manifest.clone()
    };
    sealed.mac = Some(key.sign_manifest(&manifest.copy_id, &sealed));
    sealed
}

impl Manifest {
    /// Inventory the regular files in a caller-owned staging directory.
    pub fn capture(staging: &Path, created_at: u64) -> Result<Self> {
        Ok(Self {
            version: 1,
            copy_id: copy_id(created_at),
            created_at,
            files: list_files(staging)?,
            encryption: None,
            mac: None,
        })
    }
}
