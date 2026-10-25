//! The chunked package of one Deployment.
//!
//! `package_export` captures the Deployment's database, accounts and files
//! under its locks, the same way a scheduled copy does, and offers each
//! component as a download transfer. A versioned manifest lists every
//! component with its size and SHA-256 and names the revisions that produced
//! the app.
//!
//! An import uploads the same components as transfers started with their
//! component name, then names them in the release payload as
//! `initialPackage`. `release_plan` verifies the manifest, the checksums, the
//! references between files and accounts and the free space before any
//! Deployment exists; `release_apply` copies the components into the new
//! Deployment's folder. Only a project without a Deployment accepts a package.
//!
//! Every step reads and writes components in 64 KiB blocks or lets SQLite
//! page through them, so no component sits in memory on the Kernel.

use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
};

use anyhow::{Context, bail, ensure};
use rusqlite::params;

use super::mcp::{
    MAX_OWNER_PRINCIPAL_TRANSFERS, Owner, new_transfer, owned_deployment, owned_project, store,
    transfer, transfer_path,
};
use super::*;

pub(super) const PACKAGE_FORMAT: &str = "littlebox-package";
pub(super) const PACKAGE_VERSION: u64 = 1;
/// A migration package: version 1 plus a `migration` stamp. A Kernel that
/// reads only version 1 refuses it instead of serving it at once.
pub(super) const MIGRATION_PACKAGE_VERSION: u64 = 2;
/// `usage` travels when the Deployment has a Usage store, ADR-0010.
pub(super) const COMPONENT_NAMES: [&str; 4] = ["database", "identity", "files", "usage"];
const MIB: u64 = 1024 * 1024;
/// The largest component a transfer may carry, a database at its quota.
pub(super) const MAX_COMPONENT_BYTES: u64 = MAX_APP_DATABASE_BYTES;
/// `index.json` inside the files component.
const MAX_FILE_INDEX_BYTES: u64 = 32 * MIB;
/// Free space kept on the volume after a capture or an import.
const SPACE_MARGIN: u64 = 64 * MIB;
/// Rows that let someone in without a password. They never leave the origin
/// and an import deletes any it finds. API keys travel instead, revoked at
/// import, so the destination lists them (`http::revoke_imported_keys`).
const ACCESS_TABLES: [&str; 2] = ["app_sessions", "account_challenges"];
const EXCLUDED: [&str; 5] = [
    "app sessions",
    "verification, recovery and invitation links",
    "owner accounts, owner sessions and MCP credentials",
    "Data panel backups",
    "operator email and storage keys",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Manifest {
    #[serde(default)]
    pub(super) format: String,
    #[serde(default)]
    pub(super) version: u64,
    #[serde(default)]
    pub(super) package_id: String,
    #[serde(default)]
    pub(super) created_at: u64,
    #[serde(default)]
    pub(super) deployment_id: String,
    #[serde(default)]
    pub(super) project_id: String,
    #[serde(default)]
    pub(super) visibility: String,
    #[serde(default)]
    pub(super) signup_policy: Option<SignupPolicy>,
    /// Releases applied to the project when the package was captured.
    #[serde(default)]
    pub(super) published_revision: u64,
    #[serde(default)]
    pub(super) artifact_version: Option<String>,
    #[serde(default)]
    pub(super) source_authority: Option<String>,
    /// Saves of the Managed sources. It counts saves, not releases, so a
    /// package never claims the saved sources are the published ones.
    #[serde(default)]
    pub(super) sources_revision: Option<u64>,
    #[serde(default)]
    pub(super) contents: Vec<Content>,
    #[serde(default)]
    pub(super) excluded: Vec<String>,
    /// Present only in a migration package; `migration.rs` reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) migration: Option<Value>,
    /// The custom domain's hostname, without its token. The destination
    /// issues its own token (`domain::arrive`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) domain: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Content {
    pub(super) name: String,
    pub(super) size: u64,
    pub(super) sha256: String,
}

/// The `initialPackage` field of a release payload.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitialPackage {
    pub(super) manifest: Manifest,
    /// Component name to the completed upload transfer that holds it.
    pub(super) components: HashMap<String, String>,
}

/// The size an upload for this component may declare.
pub(super) fn component_limit(name: &str, files: &file_store::FileLimits) -> Result<u64> {
    match name {
        "database" => Ok(MAX_APP_DATABASE_BYTES),
        "identity" => Ok(auth::MAX_IDENTITY_DATABASE_BYTES),
        // The files check caps the file data at this installation's quota.
        // Each file adds a header and padding of at most 1 KiB to the
        // archive, and the index adds its own size, so the archive gets that
        // much room, and never less than the largest component.
        "files" => Ok(MAX_COMPONENT_BYTES.max(
            files.quota_bytes + MAX_FILE_INDEX_BYTES + (files::MAX_FILES as u64 + 2) * 1024,
        )),
        "usage" => Ok(usage::MAX_USAGE_DATABASE_BYTES),
        other => bail!("Unknown package component {other}."),
    }
}

/// Checks what the manifest itself says, before any component is read.
pub(super) fn check_manifest(manifest: &Manifest, files: &file_store::FileLimits) -> Result<()> {
    ensure!(manifest.format == PACKAGE_FORMAT, "This is not a Littlebox package manifest.");
    ensure!(
        manifest.version == PACKAGE_VERSION || manifest.version == MIGRATION_PACKAGE_VERSION,
        "Package version {} is not supported. This Kernel reads versions {PACKAGE_VERSION} and {MIGRATION_PACKAGE_VERSION}.",
        manifest.version
    );
    match (&manifest.migration, manifest.version) {
        (None, PACKAGE_VERSION) => {}
        (Some(stamp), MIGRATION_PACKAGE_VERSION) => migration::check_stamp(stamp)?,
        (None, _) => bail!("A version {MIGRATION_PACKAGE_VERSION} package must name its migration."),
        (Some(_), _) => bail!("Only a version {MIGRATION_PACKAGE_VERSION} package may name a migration."),
    }
    ensure!(
        ["public", "private"].contains(&manifest.visibility.as_str()),
        "The package manifest has no valid visibility."
    );
    let mut seen = HashSet::new();
    for content in &manifest.contents {
        let limit = component_limit(&content.name, files)?;
        ensure!(seen.insert(content.name.as_str()), "The manifest lists the {} component twice.", content.name);
        ensure!(
            content.size > 0 && content.size <= limit,
            "The {} component is {} bytes; its limit is {limit} bytes.",
            content.name,
            content.size
        );
        ensure!(
            content.sha256.len() == 64 && content.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "The {} component has no valid SHA-256 in the manifest.",
            content.name
        );
    }
    ensure!(seen.contains("database"), "The package manifest lists no database.");
    if let Some(policy) = &manifest.signup_policy {
        SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub(super) fn ensure_space(free: u64, needed: u64, operation: &str) -> Result<()> {
    ensure!(
        free >= needed,
        "Not enough free space on the Kernel volume to {operation}: {} MiB needed, {} MiB free.",
        needed.div_ceil(MIB),
        free / MIB
    );
    Ok(())
}

/// Refuses a component upload the volume cannot hold, before its bytes arrive.
pub(super) fn ensure_upload_space(state: &AppState, size: u64) -> Result<()> {
    ensure_space(free_bytes(&state.data_root)?, size + SPACE_MARGIN, "receive this component")
}

#[cfg(unix)]
fn free_bytes(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: statvfs writes into the zeroed struct and reads a NUL-terminated path.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };
    ensure!(status == 0, "Could not read the free space on the Kernel volume.");
    #[allow(clippy::unnecessary_cast)]
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

// ---------------------------------------------------------------------------
// Export

/// Captures one Deployment and returns its manifest and one download
/// transfer per component. The capture holds the project, data and identity
/// locks, so no write, signup, policy change or release lands halfway.
pub(super) async fn export(state: &AppState, owner: &Owner, principal: &str, id: &str) -> Result<Value> {
    export_with(state, owner, principal, id, None).await
}

/// What a migration adds to a capture: the stamp that goes into the
/// manifest, and the folder that keeps the components, so the same package
/// can be offered again after its transfers expire.
pub(super) struct Stamp {
    pub(super) migration: Value,
    pub(super) keep: PathBuf,
}

/// `export`, with an optional migration stamp. A stamped capture is a
/// version 2 package and its components stay in `stamp.keep`.
pub(super) async fn export_with(state: &AppState, owner: &Owner, principal: &str, id: &str, stamp: Option<Stamp>) -> Result<Value> {
    let deployment = owned_deployment(state, owner, id, "export_data").await?;
    let held: usize = store(state)?.query_row(
        "SELECT COUNT(*) FROM transfers WHERE principal=?1 AND expires>?2",
        params![principal, now_ms()],
        |r| r.get(0),
    )?;
    ensure!(
        held + COMPONENT_NAMES.len() <= MAX_OWNER_PRINCIPAL_TRANSFERS,
        "A package export needs {} free transfers. Cancel completed transfers first.",
        COMPONENT_NAMES.len()
    );
    let folder = deployment_dir(state, &deployment.id);
    let scratch = state.data_root.join("transfers").join(format!(".package-{}", Uuid::new_v4()));
    let captured = {
        let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
        let deployment = state.deployments.read().await.values().find(|d| d.id == id).cloned().unwrap_or(deployment);
        let (project, published_revision) = owned_project(state, owner, &deployment.project_id, "export_data")?;
        let _data = database_lock(state, &deployment.id).await;
        let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
        open_auth(state, &deployment)?;
        // Bytes in a bucket are not in the folder, but the archive holds them.
        let elsewhere: u64 = match state.files.store.kind() {
            "local" => 0,
            _ => file_store::indexed_files(&state.data_root, &deployment.id).await?.unwrap_or_default().iter().map(|(_, size)| size).sum(),
        };
        let needed = folder_size(&folder)? + elsewhere + SPACE_MARGIN;
        ensure_space(free_bytes(&state.data_root)?, needed, "capture this app")?;
        let (source, target) = (folder.clone(), scratch.clone());
        let blobs = file_store::BlockingFiles::new(state.files.store.clone(), &deployment.id);
        let components = tokio::task::spawn_blocking(move || capture(&source, &target, &blobs)).await?;
        let sources_revision: Option<u64> = if project["sourceAuthority"] == "managed" {
            store(state)?
                .query_row("SELECT revision FROM managed_sources WHERE project=?1", [&deployment.project_id], |r| r.get(0))
                .optional()?
        } else {
            None
        };
        components.map(|components| (components, deployment, project, published_revision, sources_revision))
    };
    let (components, deployment, project, published_revision, sources_revision) = match captured {
        Ok(value) => value,
        Err(error_value) => {
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(error_value.context("The package capture failed"));
        }
    };
    let manifest = Manifest {
        format: PACKAGE_FORMAT.into(),
        version: if stamp.is_some() { MIGRATION_PACKAGE_VERSION } else { PACKAGE_VERSION },
        package_id: Uuid::new_v4().to_string(),
        created_at: now_ms(),
        deployment_id: deployment.id.clone(),
        project_id: deployment.project_id.clone(),
        visibility: if is_private(&deployment) { "private" } else { "public" }.into(),
        signup_policy: Some(deployment.signup_policy.clone()),
        published_revision,
        artifact_version: deployment.artifact_version.clone(),
        source_authority: project["sourceAuthority"].as_str().map(str::to_owned),
        sources_revision,
        contents: components.iter().map(|(name, _, size, sha256)| Content { name: name.clone(), size: *size, sha256: sha256.clone() }).collect(),
        excluded: EXCLUDED.iter().map(|item| (*item).to_owned()).collect(),
        migration: stamp.as_ref().map(|stamp| stamp.migration.clone()),
        domain: super::domain::for_package(state, &deployment.id),
    };
    if let Some(stamp) = &stamp {
        let kept = keep(&components, &stamp.keep).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        kept?;
        let downloads = offer(state, principal, &deployment.id, &deployment.project_id, &stamp.keep, &manifest.contents).await?;
        return Ok(json!({"manifest": manifest, "downloads": downloads}));
    }
    let mut downloads = serde_json::Map::new();
    let mut failure = None;
    for (name, path, size, sha256) in &components {
        let body = json!({"direction":"download","size":size,"sha256":sha256,"complete":true,"projectId":deployment.project_id,
            "exportData":true,"deploymentId":deployment.id,"component":name});
        match new_transfer(state, principal, body).await {
            Ok(created) => {
                let id = created["transferId"].as_str().unwrap_or_default().to_owned();
                downloads.insert(name.clone(), created);
                if let Err(error_value) = tokio::fs::rename(path, transfer_path(state, &id)?).await {
                    failure = Some(anyhow::Error::from(error_value));
                    break;
                }
            }
            Err(error_value) => {
                failure = Some(error_value);
                break;
            }
        }
    }
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    if let Some(error_value) = failure {
        for created in downloads.values() {
            let id = created["transferId"].as_str().unwrap_or_default();
            store(state)?.execute("DELETE FROM transfers WHERE id=?1 AND principal=?2", params![id, principal])?;
            let _ = tokio::fs::remove_file(transfer_path(state, id)?).await;
        }
        return Err(error_value);
    }
    Ok(json!({"manifest": manifest, "downloads": downloads}))
}

/// Moves captured components into `keep`, named after their component.
async fn keep(components: &[(String, PathBuf, u64, String)], keep: &Path) -> Result<()> {
    tokio::fs::create_dir_all(keep).await?;
    for (name, path, _, _) in components {
        tokio::fs::rename(path, keep.join(name)).await?;
    }
    Ok(())
}

/// Offers kept components as `export_data` downloads. Each transfer file is
/// a hard link to the kept file, so a second offer costs no space and the
/// bytes cannot differ from the manifest.
pub(super) async fn offer(
    state: &AppState,
    principal: &str,
    deployment_id: &str,
    project_id: &str,
    keep: &Path,
    contents: &[Content],
) -> Result<serde_json::Map<String, Value>> {
    let mut downloads = serde_json::Map::new();
    let created = async {
        for content in contents {
            let source = keep.join(&content.name);
            let (size, sha256) = copies::checksum_file(&source).with_context(|| format!("The kept {} component is missing.", content.name))?;
            ensure!(size == content.size && sha256 == content.sha256, "The kept {} component no longer matches its manifest.", content.name);
            let body = json!({"direction":"download","size":size,"sha256":sha256,"complete":true,"projectId":project_id,
                "exportData":true,"deploymentId":deployment_id,"component":content.name});
            let transfer = new_transfer(state, principal, body).await?;
            let target = transfer_path(state, transfer["transferId"].as_str().unwrap_or_default())?;
            downloads.insert(content.name.clone(), transfer);
            tokio::fs::remove_file(&target).await?;
            tokio::fs::hard_link(&source, &target).await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error_value) = created {
        for transfer in downloads.values() {
            let id = transfer["transferId"].as_str().unwrap_or_default();
            store(state)?.execute("DELETE FROM transfers WHERE id=?1 AND principal=?2", params![id, principal])?;
            let _ = tokio::fs::remove_file(transfer_path(state, id)?).await;
        }
        return Err(error_value);
    }
    Ok(downloads)
}

fn folder_size(folder: &Path) -> Result<u64> {
    let mut total = 0;
    for name in ["database.sqlite", "identity.sqlite", usage::USAGE_DATABASE] {
        total += std::fs::metadata(folder.join(name)).map(|m| m.len()).unwrap_or(0);
    }
    if let Ok(entries) = std::fs::read_dir(folder.join("files")) {
        for entry in entries {
            total += entry?.metadata()?.len() + 1024;
        }
    }
    Ok(total)
}

/// Writes the components into `scratch` and returns each with its size and
/// SHA-256. `usage` is left out when the Deployment has no Usage store yet.
fn capture(folder: &Path, scratch: &Path, blobs: &file_store::BlockingFiles) -> Result<Vec<(String, PathBuf, u64, String)>> {
    // Usage queued before the export is written first, so the package holds it.
    usage::flush();
    std::fs::create_dir_all(scratch)?;
    let database = scratch.join("database.sqlite");
    copies::vacuum_into(&folder.join("database.sqlite"), &database).context("database")?;
    let identity = scratch.join("identity.sqlite");
    copies::vacuum_into(&folder.join("identity.sqlite"), &identity).context("accounts")?;
    revoke_access(&identity)?;
    let files = scratch.join("files.tar");
    write_files_tar(&folder.join("files"), &files, blobs).context("files")?;
    let mut components = vec![("database", database), ("identity", identity), ("files", files)];
    if folder.join(usage::USAGE_DATABASE).is_file() {
        let usage = scratch.join(usage::USAGE_DATABASE);
        copies::vacuum_into(&folder.join(usage::USAGE_DATABASE), &usage).context("usage")?;
        components.push(("usage", usage));
    }
    components
        .into_iter()
        .map(|(name, path)| {
            let (size, sha256) = copies::checksum_file(&path)?;
            Ok((name.to_owned(), path, size, sha256))
        })
        .collect()
}

/// Deletes sessions and open email links from an identity file and marks
/// every live API key revoked at import. `secure_delete` overwrites the
/// freed pages, so the session and link token hashes do not stay in the
/// file. A key's hash stays with its revoked row; it opens nothing.
pub(super) fn revoke_access(path: &Path) -> Result<usize> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON;")?;
    let mut removed = 0;
    for table in ACCESS_TABLES {
        let exists = connection
            .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [table], |_| Ok(()))
            .optional()?
            .is_some();
        if exists {
            removed += connection.execute(&format!("DELETE FROM \"{table}\""), [])?;
        }
    }
    removed += super::http::revoke_imported_keys(&connection)?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// The files component: a ustar archive of `index.json` followed by one
// `<id>.blob` per indexed file, in index order.

fn write_files_tar(directory: &Path, target: &Path, blobs: &file_store::BlockingFiles) -> Result<()> {
    let index: Vec<Value> = match std::fs::read(directory.join("index.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error_value) => return Err(error_value.into()),
    };
    let mut output = std::io::BufWriter::new(std::fs::File::create(target)?);
    let index_bytes = serde_json::to_vec(&index)?;
    output.write_all(&tar_header("index.json", index_bytes.len() as u64))?;
    output.write_all(&index_bytes)?;
    output.write_all(&vec![0; tar_padding(index_bytes.len() as u64)])?;
    for file in &index {
        let id = file["id"].as_str().filter(|id| valid_file_id(id)).context("A stored file has an invalid id.")?;
        let name = format!("{id}.blob");
        let (size, mut blob) = blobs.open(id)?.with_context(|| format!("Stored file {id} has no data."))?;
        output.write_all(&tar_header(&name, size))?;
        let copied = std::io::copy(&mut blob, &mut output)?;
        ensure!(copied == size, "Stored file {id} changed during the capture.");
        output.write_all(&vec![0; tar_padding(size)])?;
    }
    output.write_all(&[0; 1024])?;
    output.into_inner().map_err(|error_value| error_value.into_error())?.sync_all()?;
    Ok(())
}

fn tar_padding(size: u64) -> usize {
    ((512 - size % 512) % 512) as usize
}

fn tar_header(name: &str, size: u64) -> [u8; 512] {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    let mut octal = |range: std::ops::Range<usize>, value: u64| {
        let width = range.len() - 1;
        header[range].copy_from_slice(format!("{value:0width$o}\0").as_bytes());
    };
    octal(100..108, 0o600);
    octal(108..116, 0);
    octal(116..124, 0);
    octal(124..136, size);
    octal(136..148, 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    header
}

/// Reads one header. Returns `None` at the end-of-archive block.
fn read_tar_header(input: &mut impl Read) -> Result<Option<(String, u64)>> {
    let mut header = [0u8; 512];
    input.read_exact(&mut header).context("The files component ends early.")?;
    if header.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let stored = parse_octal(&header[148..156])?;
    let mut copy = header;
    copy[148..156].fill(b' ');
    let checksum: u64 = copy.iter().map(|byte| u64::from(*byte)).sum();
    ensure!(stored == checksum, "The files component has a damaged header.");
    ensure!(header[156] == b'0' || header[156] == 0, "The files component holds an entry that is not a file.");
    ensure!(header[345..500].iter().all(|byte| *byte == 0), "The files component holds an unexpected path.");
    let end = header[..100].iter().position(|byte| *byte == 0).unwrap_or(100);
    let name = std::str::from_utf8(&header[..end]).context("The files component holds an unexpected path.")?.to_owned();
    Ok(Some((name, parse_octal(&header[124..136])?)))
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let text = std::str::from_utf8(field)?.trim_matches(|c: char| c == '\0' || c == ' ');
    u64::from_str_radix(text, 8).context("The files component has a damaged header.")
}

/// Reads the files component, checks it and, with a destination, writes the
/// blobs and a normalized `index.json` there. A file owner must be an
/// account in `accounts`.
fn unpack_files(
    archive: &Path,
    accounts: Option<&Connection>,
    destination: Option<(&Path, &file_store::BlockingFiles)>,
    limits: &file_store::FileLimits,
) -> Result<usize> {
    let mut input = std::io::BufReader::new(std::fs::File::open(archive)?);
    let (name, size) = read_tar_header(&mut input)?.context("The files component is empty.")?;
    ensure!(name == "index.json", "The files component must start with index.json.");
    ensure!(size <= MAX_FILE_INDEX_BYTES, "The file index exceeds {} MiB.", MAX_FILE_INDEX_BYTES / MIB);
    let mut index_bytes = Vec::new();
    (&mut input).take(size).read_to_end(&mut index_bytes)?;
    ensure!(index_bytes.len() as u64 == size, "The files component ends early.");
    skip(&mut input, tar_padding(size) as u64)?;
    let index: Vec<Value> = serde_json::from_slice(&index_bytes).context("The file index is not valid JSON.")?;
    let mut expected: HashMap<String, u64> = HashMap::new();
    let mut order = Vec::with_capacity(index.len());
    let mut total = 0u64;
    let mut normalized = Vec::with_capacity(index.len());
    for file in &index {
        let id = file["id"].as_str().filter(|id| valid_file_id(id)).context("The file index has an invalid file id.")?;
        let declared = file["size"].as_u64().context("The file index has a file without a size.")?;
        ensure!(declared <= limits.max_file_bytes, "File {id} exceeds this installation's {} limit per file.", limits.max_file());
        total += declared;
        ensure!(total <= limits.quota_bytes, "The files exceed this installation's {} file quota.", limits.quota());
        ensure!(expected.insert(id.to_owned(), declared).is_none(), "The file index lists file {id} twice.");
        order.push(id.to_owned());
        let mut restored = json!({
            "id": id,
            "name": file["name"].as_str().unwrap_or("file").replace(['/', '\\'], "-").chars().take(180).collect::<String>(),
            "type": file["type"].as_str().unwrap_or("application/octet-stream"),
            "size": declared,
            "visibility": file_visibility(file),
            "createdAt": file["createdAt"].as_u64().unwrap_or_else(now_ms),
        });
        if let Some(owner) = file_owner(file) {
            ensure!(accepted_owner_id(owner), "File {id} names an invalid owner.");
            let known = match accounts {
                Some(db) => db.query_row("SELECT 1 FROM app_users WHERE id=?1", [owner], |_| Ok(())).optional()?.is_some(),
                None => false,
            };
            ensure!(known, "File {id} belongs to an account the package does not contain.");
            restored["ownerId"] = json!(owner);
        }
        normalized.push(restored);
    }
    if let Some((directory, _)) = destination {
        std::fs::create_dir_all(directory)?;
    }
    let mut seen = 0;
    while let Some((name, size)) = read_tar_header(&mut input)? {
        let id = name.strip_suffix(".blob").context("The files component holds an unexpected entry.")?;
        let declared = expected.remove(id).with_context(|| format!("The files component holds data for {id}, which the index does not list."))?;
        ensure!(order.get(seen).is_some_and(|next| next == id), "The files component is out of index order.");
        ensure!(size == declared, "The data of file {id} does not match its indexed size.");
        let mut body = (&mut input).take(size);
        let copied = match destination {
            Some((directory, blobs)) => {
                // Each file lands beside the index first, then goes to the store.
                let temp = directory.join(format!(".{id}.import-{}", Uuid::new_v4()));
                let stored = (|| {
                    let copied = std::io::copy(&mut body, &mut std::fs::File::create(&temp)?)?;
                    ensure!(copied == size, "The files component ends early.");
                    blobs.put(id, &temp)?;
                    Ok(copied)
                })();
                let _ = std::fs::remove_file(&temp);
                stored?
            }
            None => std::io::copy(&mut body, &mut std::io::sink())?,
        };
        ensure!(copied == size, "The files component ends early.");
        skip(&mut input, tar_padding(size) as u64)?;
        seen += 1;
    }
    ensure!(expected.is_empty(), "The files component is missing the data of {} indexed files.", expected.len());
    if let Some((directory, _)) = destination {
        let bytes = serde_json::to_vec_pretty(&normalized)?;
        let temp = directory.join(format!(".index.json.tmp-{}", Uuid::new_v4()));
        std::fs::write(&temp, bytes)?;
        std::fs::rename(temp, directory.join("index.json"))?;
    }
    Ok(seen)
}

fn skip(input: &mut impl Read, bytes: u64) -> Result<()> {
    let skipped = std::io::copy(&mut input.take(bytes), &mut std::io::sink())?;
    ensure!(skipped == bytes, "The files component ends early.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Import

/// The uploads named by the payload, checked against the manifest. With a
/// principal the transfers must belong to that caller and project.
fn component_uploads(state: &AppState, principal: Option<&str>, project_id: &str, package: &InitialPackage) -> Result<Vec<(Content, PathBuf)>> {
    check_manifest(&package.manifest, &state.files.limits)?;
    for name in package.components.keys() {
        ensure!(
            package.manifest.contents.iter().any(|content| &content.name == name),
            "The payload names a {name} component the manifest does not list."
        );
    }
    let mut uploads = Vec::new();
    for content in &package.manifest.contents {
        let name = &content.name;
        let id = package.components.get(name).with_context(|| format!("The {name} component has not been uploaded."))?;
        let upload = match principal {
            Some(principal) => transfer(state, principal, &json!({"transferId": id}))?,
            None => {
                let body: Option<String> = store(state)?
                    .query_row("SELECT body FROM transfers WHERE id=?1 AND expires>?2", params![id, now_ms()], |r| r.get(0))
                    .optional()?;
                serde_json::from_str(&body.with_context(|| format!("The {name} upload has expired."))?)?
            }
        };
        ensure!(upload["direction"] == "upload" && upload["complete"] == true, "The {name} upload is not complete.");
        ensure!(upload["component"] == name.as_str(), "Transfer {id} was not started for the {name} component.");
        ensure!(upload["projectId"] == project_id, "The {name} upload belongs to another project.");
        ensure!(
            upload["size"] == content.size && upload["sha256"] == content.sha256.as_str(),
            "The {name} component does not match the size and SHA-256 in the manifest."
        );
        uploads.push((content.clone(), transfer_path(state, id)?));
    }
    Ok(uploads)
}

/// Runs at `release_plan`, before any Deployment exists: the manifest, each
/// upload's checksum against the manifest, SQLite integrity, the files and
/// the accounts they name, and the free space the import will use.
pub(super) async fn verify(state: &AppState, principal: &str, project_id: &str, package: &InitialPackage) -> Result<()> {
    let uploads = component_uploads(state, Some(principal), project_id, package)?;
    let database = uploads.iter().find(|(c, _)| c.name == "database").map(|(c, _)| c.size).unwrap_or(0);
    let total: u64 = uploads.iter().map(|(c, _)| c.size).sum();
    // The release check copies the database once more to try the schema.
    ensure_space(free_bytes(&state.data_root)?, total + database + SPACE_MARGIN, "import this app")?;
    let file_limits = state.files.limits;
    tokio::task::spawn_blocking(move || {
        let path = |name: &str| uploads.iter().find(|(c, _)| c.name == name).map(|(_, p)| p.clone());
        for (content, path) in &uploads {
            let (size, sha256) = copies::checksum_file(path)?;
            ensure!(
                size == content.size && sha256 == content.sha256,
                "The {} component does not match the SHA-256 in the manifest.",
                content.name
            );
        }
        let database = path("database").context("The package has no database.")?;
        integrity(&database, "database")?;
        let stored = Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|db| super::sql_rules::stored_trigger_or_view(&db))?;
        if let Some(refusal) = stored {
            anyhow::bail!(refusal);
        }
        let accounts = match path("identity") {
            Some(identity) => {
                integrity(&identity, "accounts")?;
                let db = Connection::open_with_flags(&identity, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let has_users: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_users')",
                    [],
                    |r| r.get(0),
                )?;
                ensure!(has_users, "The accounts component has no app_users table.");
                Some(db)
            }
            None => None,
        };
        if let Some(files) = path("files") {
            unpack_files(&files, accounts.as_ref(), None, &file_limits)?;
        }
        if let Some(usage) = path("usage") {
            integrity(&usage, "usage")?;
        }
        Ok(())
    })
    .await?
}

fn integrity(path: &Path, label: &str) -> Result<()> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("The {label} component is not a SQLite database."))?;
    let result: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .with_context(|| format!("The {label} component is not a SQLite database."))?;
    ensure!(result == "ok", "The {label} component failed SQLite's integrity check.");
    Ok(())
}

/// Copies the uploaded database to `target`, where the release check tries
/// the schema against it.
pub(super) fn copy_database(state: &AppState, package: &InitialPackage, target: &Path) -> Result<()> {
    let id = package.components.get("database").context("The package has no database.")?;
    std::fs::copy(transfer_path(state, id)?, target)?;
    Ok(())
}

/// Runs inside the release, after the Deployment's folder exists and before
/// its record does. Copies each component in, checking its SHA-256 as it
/// goes, revokes sessions, links and keys, and unpacks the files. The
/// uploads stay intact, so a failed release can be retried.
pub(super) async fn install(state: &AppState, package: &InitialPackage, project_id: &str, directory: &Path) -> Result<()> {
    let uploads = component_uploads(state, None, project_id, package)?;
    let folder = directory.to_owned();
    let directory = directory.to_owned();
    let file_limits = state.files.limits;
    let deployment_id = directory.file_name().and_then(|name| name.to_str()).context("The Deployment folder has no name.")?;
    let blobs = file_store::BlockingFiles::new(state.files.store.clone(), deployment_id);
    tokio::task::spawn_blocking(move || {
        let find = |name: &str| uploads.iter().find(|(c, _)| c.name == name);
        if let Some((content, source)) = find("database") {
            copy_verified(source, &directory.join("database.sqlite"), content)?;
        }
        let identity = directory.join("identity.sqlite");
        if let Some((content, source)) = find("identity") {
            copy_verified(source, &identity, content)?;
            AuthService::open(&identity).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            revoke_access(&identity)?;
        }
        if let Some((content, source)) = find("files") {
            let (size, sha256) = copies::checksum_file(source)?;
            ensure!(size == content.size && sha256 == content.sha256, "The files component does not match the SHA-256 in the manifest.");
            let accounts = identity.exists().then(|| Connection::open(&identity)).transpose()?;
            unpack_files(source, accounts.as_ref(), Some((&directory.join("files"), &blobs)), &file_limits)?;
        }
        if let Some((content, source)) = find("usage") {
            copy_verified(source, &directory.join(usage::USAGE_DATABASE), content)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    super::domain::arrive(state, &folder, package.manifest.domain.as_deref()).await
}

fn copy_verified(source: &Path, target: &Path, content: &Content) -> Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::File::create(target)?;
    let mut hasher = sha2::Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
        output.write_all(&buffer[..read])?;
        size += read as u64;
    }
    output.sync_all()?;
    ensure!(
        size == content.size && hex::encode(sha2::Digest::finalize(hasher)) == content.sha256,
        "The {} component does not match the SHA-256 in the manifest.",
        content.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            format: PACKAGE_FORMAT.into(),
            version: PACKAGE_VERSION,
            package_id: "p".into(),
            created_at: 1,
            deployment_id: "d".into(),
            project_id: "app".into(),
            visibility: "private".into(),
            signup_policy: None,
            published_revision: 1,
            artifact_version: None,
            source_authority: None,
            sources_revision: None,
            contents: vec![Content { name: "database".into(), size: 4096, sha256: "a".repeat(64) }],
            excluded: vec![],
            migration: None,
            domain: None,
        }
    }

    #[test]
    fn manifests_name_their_version_components_and_limits() {
        assert!(check_manifest(&manifest(), &Default::default()).is_ok());
        let mut next = manifest();
        next.version = 3;
        assert!(check_manifest(&next, &Default::default()).unwrap_err().to_string().contains("Package version 3 is not supported"));
        let mut unstamped = manifest();
        unstamped.version = MIGRATION_PACKAGE_VERSION;
        assert!(check_manifest(&unstamped, &Default::default()).unwrap_err().to_string().contains("must name its migration"));
        let mut stamped = manifest();
        stamped.migration = Some(json!({"operationId": "op", "confirmationHash": "c".repeat(64)}));
        assert!(check_manifest(&stamped, &Default::default()).unwrap_err().to_string().contains("Only a version 2 package"));
        stamped.version = MIGRATION_PACKAGE_VERSION;
        assert!(check_manifest(&stamped, &Default::default()).is_ok());
        stamped.migration = Some(json!({"operationId": "op", "confirmationHash": "not a hash"}));
        assert!(check_manifest(&stamped, &Default::default()).is_err());
        let mut foreign = manifest();
        foreign.format = "zip".into();
        assert!(check_manifest(&foreign, &Default::default()).is_err());
        let mut unknown = manifest();
        unknown.contents.push(Content { name: "secrets".into(), size: 1, sha256: "a".repeat(64) });
        assert!(check_manifest(&unknown, &Default::default()).unwrap_err().to_string().contains("Unknown package component secrets"));
        let mut twice = manifest();
        twice.contents.push(twice.contents[0].clone());
        assert!(check_manifest(&twice, &Default::default()).unwrap_err().to_string().contains("twice"));
        let mut large = manifest();
        large.contents[0].size = MAX_APP_DATABASE_BYTES + 1;
        assert!(check_manifest(&large, &Default::default()).unwrap_err().to_string().contains("its limit is"));
        let mut identity = manifest();
        identity.contents.push(Content { name: "identity".into(), size: auth::MAX_IDENTITY_DATABASE_BYTES + 1, sha256: "b".repeat(64) });
        assert!(check_manifest(&identity, &Default::default()).is_err());
        let mut empty = manifest();
        empty.contents.clear();
        assert!(check_manifest(&empty, &Default::default()).unwrap_err().to_string().contains("no database"));
        let mut checksum = manifest();
        checksum.contents[0].sha256 = "A".repeat(64);
        assert!(check_manifest(&checksum, &Default::default()).is_err());
    }

    #[test]
    fn space_checks_refuse_with_both_numbers() {
        assert!(ensure_space(300 * MIB, 200 * MIB, "import this app").is_ok());
        let refused = ensure_space(100 * MIB, 200 * MIB, "import this app").unwrap_err().to_string();
        assert_eq!(refused, "Not enough free space on the Kernel volume to import this app: 200 MiB needed, 100 MiB free.");
        assert!(free_bytes(Path::new(".")).unwrap() > 0);
    }

    #[test]
    fn identity_copies_lose_sessions_and_links_and_carry_keys_revoked() {
        let root = std::env::temp_dir().join(format!("littlebox-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("identity.sqlite");
        let db = Connection::open(&path).unwrap();
        // The keys table as a Kernel from before `revoked_reason` wrote it.
        db.execute_batch(
            "CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('u1');
             CREATE TABLE app_sessions(token_hash TEXT); INSERT INTO app_sessions VALUES('secret-session-hash');
             CREATE TABLE account_challenges(token_hash TEXT); INSERT INTO account_challenges VALUES('secret-link-hash');
             CREATE TABLE api_keys(id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
               actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER);
             INSERT INTO api_keys VALUES('key-live', 'live-key-hash', 'Mirror', '[]', '[\"public\"]', 1, NULL);
             INSERT INTO api_keys VALUES('key-old', 'old-key-hash', 'Old', '[]', '[\"public\"]', 1, 2);",
        )
        .unwrap();
        drop(db);
        assert_eq!(revoke_access(&path).unwrap(), 3, "a session, a link and the live key");
        let bytes = std::fs::read(&path).unwrap();
        for secret in ["secret-session-hash", "secret-link-hash"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()), "{secret} survived");
        }
        let db = Connection::open(&path).unwrap();
        let users: i64 = db.query_row("SELECT COUNT(*) FROM app_users", [], |r| r.get(0)).unwrap();
        assert_eq!(users, 1);
        let keys: Vec<(String, Option<i64>, Option<String>)> = db
            .prepare("SELECT id, revoked_at, revoked_reason FROM api_keys ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(matches!(&keys[0], (id, Some(at), Some(reason)) if id == "key-live" && *at > 2 && reason == "import"));
        assert_eq!(keys[1], ("key-old".to_owned(), Some(2), None), "an owner's revocation keeps its time and has no reason");
        assert_eq!(revoke_access(&path).unwrap(), 0, "a second pass changes nothing");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Writes a Deployment's files into an archive from one store and unpacks
    /// it into another, on a blocking thread as the capture and the import do.
    async fn round_trip(source: Arc<dyn file_store::FileStore>, target: Arc<dyn file_store::FileStore>, root: &Path) {
        let (a, b) = ("00000000-0000-4000-8000-00000000000a", "00000000-0000-4000-8000-00000000000b");
        let deployment = Uuid::new_v4().to_string();
        let index_dir = root.join("deployments").join(&deployment).join("files");
        std::fs::create_dir_all(&index_dir).unwrap();
        for (id, bytes) in [(a, b"first file".to_vec()), (b, vec![7u8; 100_000])] {
            let temp = root.join("upload");
            std::fs::write(&temp, bytes).unwrap();
            source.put(&deployment, id, &temp).await.unwrap();
        }
        let index = json!([
            {"id": a, "name": "a.txt", "type": "text/plain", "size": 10, "visibility": "private", "ownerId": "u1", "createdAt": 5},
            {"id": b, "name": "b.bin", "size": 100_000, "visibility": "public"},
        ]);
        std::fs::write(index_dir.join("index.json"), index.to_string()).unwrap();
        let archive = root.join("files.tar");
        let arrival = Uuid::new_v4().to_string();
        let target_dir = root.join("deployments").join(&arrival).join("files");
        let (from, to) = (file_store::BlockingFiles::new(source.clone(), &deployment), file_store::BlockingFiles::new(target.clone(), &arrival));
        let (archive_path, index_path, target_path) = (archive.clone(), index_dir.clone(), target_dir.clone());
        tokio::task::spawn_blocking(move || {
            write_files_tar(&index_path, &archive_path, &from).unwrap();
            let accounts = Connection::open_in_memory().unwrap();
            accounts.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('u1');").unwrap();
            assert_eq!(unpack_files(&archive_path, Some(&accounts), Some((&target_path, &to)), &Default::default()).unwrap(), 2);
        })
        .await
        .unwrap();
        for id in [a, b] {
            let sent = file_store::read_all(source.as_ref(), &deployment, id).await.unwrap().unwrap();
            let arrived = file_store::read_all(target.as_ref(), &arrival, id).await.unwrap().unwrap();
            assert_eq!(super::mcp::digest(&arrived), super::mcp::digest(&sent), "{id}");
        }
        let restored: Vec<Value> = serde_json::from_slice(&std::fs::read(target_dir.join("index.json")).unwrap()).unwrap();
        assert_eq!(restored[0]["ownerId"], "u1");
        assert_eq!(restored[1]["visibility"], "public");
        assert!(std::fs::read_dir(&target_dir).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with('.')));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_travel_between_an_s3_and_a_local_installation_with_the_same_bytes() {
        let root = std::env::temp_dir().join(format!("littlebox-package-stores-{}", Uuid::new_v4()));
        let local: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let s3: Arc<dyn file_store::FileStore> = Arc::new(file_store::tests::test_s3_store().await);
        round_trip(s3.clone(), local.clone(), &root).await;
        round_trip(local.clone(), s3.clone(), &root).await;
        round_trip(local.clone(), local, &root).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_refuse_foreign_owners_extra_data_and_more_than_the_quota() {
        let root = std::env::temp_dir().join(format!("littlebox-package-{}", Uuid::new_v4()));
        let store: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let deployment = "source";
        let source = root.join("deployments").join(deployment).join("files");
        std::fs::create_dir_all(&source).unwrap();
        let (a, b) = ("00000000-0000-4000-8000-00000000000a", "00000000-0000-4000-8000-00000000000b");
        std::fs::write(source.join(format!("{a}.blob")), b"first file").unwrap();
        std::fs::write(source.join(format!("{b}.blob")), vec![7u8; 1000]).unwrap();
        let index = json!([
            {"id": a, "name": "a.txt", "type": "text/plain", "size": 10, "visibility": "private", "ownerId": "u1", "createdAt": 5},
            {"id": b, "name": "b.bin", "size": 1000, "visibility": "public"},
        ]);
        std::fs::write(source.join("index.json"), index.to_string()).unwrap();
        let archive = root.join("files.tar");
        let blobs = file_store::BlockingFiles::new(store, deployment);
        let root_path = root.clone();
        tokio::task::spawn_blocking(move || {
            let root = root_path;
            write_files_tar(&source, &archive, &blobs).unwrap();
            let strangers = Connection::open_in_memory().unwrap();
            strangers.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY);").unwrap();
            assert!(unpack_files(&archive, Some(&strangers), None, &Default::default()).unwrap_err().to_string().contains("does not contain"));
            assert!(unpack_files(&archive, None, None, &Default::default()).is_err());
            let accounts = Connection::open_in_memory().unwrap();
            accounts.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('u1');").unwrap();

            // A destination whose limits are below the package's files.
            let small = file_store::FileLimits::parse(Some("1000"), Some("1000")).unwrap();
            assert_eq!(
                unpack_files(&archive, Some(&accounts), None, &small).unwrap_err().to_string(),
                "The files exceed this installation's 1000 bytes file quota."
            );
            let tiny = file_store::FileLimits::parse(Some("2048"), Some("999")).unwrap();
            assert_eq!(
                unpack_files(&archive, Some(&accounts), None, &tiny).unwrap_err().to_string(),
                format!("File {b} exceeds this installation's 999 bytes limit per file.")
            );

            // An archive with data the index does not list.
            let mut bytes = std::fs::read(&archive).unwrap();
            bytes.truncate(bytes.len() - 1024);
            bytes.extend_from_slice(&tar_header("00000000-0000-4000-8000-00000000000c.blob", 1));
            bytes.extend_from_slice(&[1; 512]);
            bytes.extend_from_slice(&[0; 1024]);
            let extra = root.join("extra.tar");
            std::fs::write(&extra, bytes).unwrap();
            assert!(unpack_files(&extra, Some(&accounts), None, &Default::default()).unwrap_err().to_string().contains("does not list"));
        })
        .await
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
