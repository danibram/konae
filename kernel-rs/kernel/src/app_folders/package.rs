use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
};

use anyhow::{Context, bail, ensure};
use rusqlite::params;

use crate::transfers::{MAX_ADMIN_PRINCIPAL_TRANSFERS, new_transfer, transfer, transfer_path};
use crate::*;

pub const PACKAGE_FORMAT: &str = "konae-package";
pub const PACKAGE_VERSION: u64 = 1;
/// `usage` travels when the Deployment has a Usage store, ADR-0010.
pub fn component_names() -> Vec<String> { crate::modules::export_components() }
fn registered_component(name: &'static str) -> Result<&'static str> {
    ensure!(component_names().iter().any(|component| component == name), "Module data inventory does not register package component {name}.");
    Ok(name)
}
const FOLDER_DOWNLOAD: &str = "folder";
const MIB: u64 = 1024 * 1024;
/// The largest component a transfer may carry, a database at its quota.
pub const MAX_COMPONENT_BYTES: u64 = MAX_APP_DATABASE_BYTES;
const MAX_FILE_INDEX_BYTES: u64 = 32 * MIB;
const SPACE_MARGIN: u64 = 64 * MIB;
const ACCESS_TABLES: [&str; 3] = ["app_sessions", "account_challenges", "entry_links"];
const EXCLUDED: [&str; 5] = [
    "app sessions",
    "verification, recovery and invitation links",
    "owner accounts, owner sessions and MCP credentials",
    "Data panel backups",
    "operator email and storage keys",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub package_id: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub deployment_id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub modules: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub data_versions: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub signup_policy: Option<SignupPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_methods: Option<Vec<SignInMethod>>,
    #[serde(default)]
    pub published_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<crate::ReleasedSettings>,
    #[serde(default)]
    pub artifact_version: Option<String>,
    #[serde(default)]
    pub sources_revision: Option<u64>,
    #[serde(default)]
    pub contents: Vec<Content>,
    #[serde(default)]
    pub excluded: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<crate::path::Carried>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Content {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialPackage {
    pub manifest: Manifest,
    /// Component name to the completed upload transfer that holds it.
    pub components: HashMap<String, String>,
}

/// The size an upload for this component may declare.
pub fn component_limit(name: &str, files: &file_store::FileLimits) -> Result<u64> {
    ensure!(component_names().iter().any(|component| component == name), "Unknown package component {name}.");
    match name {
        "database" => Ok(MAX_APP_DATABASE_BYTES),
        "identity" => Ok(auth::MAX_IDENTITY_DATABASE_BYTES),
        "files" => Ok(MAX_COMPONENT_BYTES
            .max(files.quota_bytes + MAX_FILE_INDEX_BYTES + (files::MAX_FILES as u64 + 2) * 1024)),
        "usage" => Ok(usage::MAX_USAGE_DATABASE_BYTES),
        other => bail!("Unknown package component {other}."),
    }
}

/// Checks what the manifest itself says, before any component is read.
pub fn check_manifest(manifest: &Manifest, files: &file_store::FileLimits) -> Result<()> {
    ensure!(
        manifest.format == PACKAGE_FORMAT,
        "This is not a Konae package manifest."
    );
    ensure!(
        manifest.version == PACKAGE_VERSION,
        "Package version {} is not supported. This Kernel reads version {PACKAGE_VERSION}.",
        manifest.version
    );
    crate::modules::check_data_versions(&manifest.data_versions)?;
    ensure!(
        ["public", "private"].contains(&manifest.visibility.as_str()),
        "The package manifest has no valid visibility."
    );
    let mut seen = HashSet::new();
    for content in &manifest.contents {
        let limit = component_limit(&content.name, files)?;
        ensure!(
            seen.insert(content.name.as_str()),
            "The manifest lists the {} component twice.",
            content.name
        );
        ensure!(
            content.size > 0 && content.size <= limit,
            "The {} component is {} bytes; its limit is {limit} bytes.",
            content.name,
            content.size
        );
        ensure!(
            content.sha256.len() == 64
                && content
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "The {} component has no valid SHA-256 in the manifest.",
            content.name
        );
    }
    let actions_on = manifest
        .modules
        .get("actions")
        .is_none_or(|state| state["on"] != false);
    ensure!(
        seen.contains("database") || !actions_on,
        "The package manifest lists no database. This Kernel requires a database component for apps with actions on."
    );
    if let Some(policy) = &manifest.signup_policy {
        SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?;
    }
    if let Some(methods) = &manifest.sign_in_methods {
        sign_in_methods_from_input(methods).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub fn ensure_space(free: u64, needed: u64, operation: &str) -> Result<()> {
    ensure!(
        free >= needed,
        "Not enough free space on the Kernel volume to {operation}: {} MiB needed, {} MiB free.",
        needed.div_ceil(MIB),
        free / MIB
    );
    Ok(())
}

/// Refuses a component upload the volume cannot hold, before its bytes arrive.
pub fn ensure_upload_space(state: &AppState, size: u64) -> Result<()> {
    ensure_space(
        free_bytes(&state.data_root)?,
        size + SPACE_MARGIN,
        "receive this component",
    )
}

#[cfg(unix)]
pub fn free_bytes(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };
    ensure!(
        status == 0,
        "Could not read the free space on the Kernel volume."
    );
    #[allow(clippy::unnecessary_cast)]
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn free_bytes(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

/// The revisions an export's manifest names: the release the app serves
/// and the saved sources, when the caller keeps a count of saves.
pub struct Revisions {
    pub published: u64,
    pub sources: Option<u64>,
}

/// Captures one Deployment and returns its manifest and one download
/// transfer per component. The capture holds the project, data and identity
/// locks, so no write, signup, policy change or release lands halfway.
/// `revisions` runs under those locks.
pub async fn export(
    state: &AppState,
    deployment: Deployment,
    principal: &str,
    revisions: impl FnOnce(&Deployment) -> Result<Revisions>,
) -> Result<Value> {
    let id = deployment.id.clone();
    let _transfer_hold = crate::notify::TransferHold::begin(&state.data_root, &id)?;
    let held: usize = crate::transfers::store(state)?.query_row(
        "SELECT COUNT(*) FROM transfers WHERE principal=?1 AND expires>?2",
        params![principal, now_ms()],
        |r| r.get(0),
    )?;
    let component_names = component_names();
    ensure!(
        held + component_names.len() < MAX_ADMIN_PRINCIPAL_TRANSFERS,
        "An export needs {} free transfers. Cancel completed transfers first.",
        component_names.len() + 1
    );
    let folder = crate::layout::data(&state.data_root, &deployment.id);
    let scratch = state
        .data_root
        .join("transfers")
        .join(format!(".package-{}", Uuid::new_v4()));
    let captured = {
        let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
        let _release = crate::named_lock(state, &format!("publish:{id}"), false).await;
        let deployment = state
            .deployments
            .read()
            .await
            .values()
            .find(|d| d.id == id)
            .cloned()
            .unwrap_or(deployment);
        let Revisions {
            published: published_revision,
            sources: sources_revision,
        } = revisions(&deployment)?;
        let _data = database_lock(state, &deployment.id).await;
        crate::notify::prepare_transfer(state, &id)?;
        let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
        if crate::modules::is_on(&deployment.modules, "auth") {
            open_auth(state, &deployment)?;
        }
        let elsewhere: u64 = match state.files.store.kind() {
            "local" => 0,
            _ => file_store::indexed_files(&state.data_root, &deployment.id)
                .await?
                .unwrap_or_default()
                .iter()
                .map(|(_, size)| size)
                .sum(),
        };
        let needed = folder_size(&folder)? + elsewhere + SPACE_MARGIN;
        ensure_space(free_bytes(&state.data_root)?, needed, "capture this app")?;
        let (source, target) = (folder.clone(), scratch.clone());
        let blobs = file_store::BlockingFiles::new(state.files.store.clone(), &deployment.id);
        let app_folder = crate::layout::folder(&state.data_root, &deployment.id);
        let export_settings =
            crate::sources::settings_for_export(&state.data_root, &deployment.id).await?;
        let published_snapshot = export_settings.released.clone();
        let export_settings = serde_json::to_vec_pretty(&export_settings)?;
        let app_sources = crate::layout::sources(&state.data_root, &deployment.id);
        let saved_actions = tokio::fs::read_to_string(app_sources.join("actions.json"))
            .await
            .ok();
        let mut sources_ahead = saved_actions.is_some_and(|saved| saved != deployment.actions);
        if let Some(released) = deployment.source_digest.as_deref() {
            let saved = crate::sources::digest_folder(&app_sources).await?;
            sources_ahead |= saved != released;
        }
        let modules = deployment.modules.clone();
        let components = tokio::task::spawn_blocking(move || {
            let components = capture(&source, &target, &blobs, &modules)?;
            write_folder_tar(&app_folder, &target.join("folder.tar"), &export_settings)?;
            Ok::<_, anyhow::Error>(components)
        })
        .await?;
        components.map(|components| {
            (
                components,
                deployment,
                published_revision,
                sources_revision,
                sources_ahead,
                published_snapshot,
            )
        })
    };
    let (
        components,
        deployment,
        published_revision,
        sources_revision,
        sources_ahead,
        published_snapshot,
    ) = match captured {
        Ok(value) => value,
        Err(error_value) => {
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(error_value.context("The export capture failed"));
        }
    };
    let manifest = Manifest {
        format: PACKAGE_FORMAT.into(),
        version: PACKAGE_VERSION,
        package_id: Uuid::new_v4().to_string(),
        created_at: now_ms(),
        deployment_id: deployment.id.clone(),
        project_id: deployment.project_id.clone(),
        visibility: if is_private(&deployment) {
            "private"
        } else {
            "public"
        }
        .into(),
        modules: deployment.modules.clone(),
        data_versions: crate::modules::data_versions_for_components(components.iter().map(|(name, _, _, _)| name.as_str())),
        signup_policy: Some(deployment.signup_policy.clone()),
        sign_in_methods: Some(deployment.sign_in_methods.clone()),
        released: published_snapshot,
        published_revision,
        artifact_version: Some(deployment.artifact_version.clone()),
        sources_revision,
        contents: components
            .iter()
            .map(|(name, _, size, sha256)| Content {
                name: name.clone(),
                size: *size,
                sha256: sha256.clone(),
            })
            .collect(),
        excluded: EXCLUDED.iter().map(|item| (*item).to_owned()).collect(),
        domain: crate::domain::for_package(state, &deployment.id),
        path: crate::path::for_package(state, &deployment.id),
    };
    let mut downloads = serde_json::Map::new();
    let mut failure = None;
    let folder_download = {
        let folder_tar = scratch.join("folder.tar");
        let (size, sha256) = copies::checksum_file(&folder_tar)?;
        (FOLDER_DOWNLOAD.to_owned(), folder_tar, size, sha256)
    };
    for (name, path, size, sha256) in components.iter().chain(std::iter::once(&folder_download)) {
        let body = json!({"direction":"download","size":size,"sha256":sha256,"complete":true,"projectId":deployment.project_id,
            "exportData":true,"deploymentId":deployment.id,"component":name});
        match new_transfer(state, principal, body).await {
            Ok(created) => {
                let id = created["transferId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                downloads.insert(name.clone(), created);
                if let Err(error_value) = tokio::fs::rename(path, transfer_path(state, &id)?).await
                {
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
            crate::transfers::store(state)?.execute(
                "DELETE FROM transfers WHERE id=?1 AND principal=?2",
                params![id, principal],
            )?;
            let _ = tokio::fs::remove_file(transfer_path(state, id)?).await;
        }
        return Err(error_value);
    }
    let folder = downloads.remove(FOLDER_DOWNLOAD);
    Ok(
        json!({"manifest": manifest, "downloads": downloads, "folder": folder, "sourcesAhead": sources_ahead}),
    )
}

pub fn folder_size(folder: &Path) -> Result<u64> {
    let mut total = 0;
    for name in ["database.sqlite", "identity.sqlite", usage::USAGE_DATABASE] {
        total += std::fs::metadata(folder.join(name))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    if let Ok(entries) = std::fs::read_dir(folder.join("files")) {
        for entry in entries {
            total += entry?.metadata()?.len() + 1024;
        }
    }
    Ok(total)
}

fn capture(
    folder: &Path,
    scratch: &Path,
    blobs: &file_store::BlockingFiles,
    modules: &std::collections::BTreeMap<String, Value>,
) -> Result<Vec<(String, PathBuf, u64, String)>> {
    let components = snapshot(folder, scratch, blobs, modules)?;
    if let Some((_, identity)) = components.iter().find(|(name, _)| *name == "identity") {
        revoke_access(identity)?;
    }
    components
        .into_iter()
        .map(|(name, path)| {
            let (size, sha256) = copies::checksum_file(&path)?;
            Ok((name.to_owned(), path, size, sha256))
        })
        .collect()
}

/// Writes a consistent copy of the app's `data/` folder into `target`:
/// `database.sqlite`, `identity.sqlite`, `files.tar` and, when the app has
/// one, `usage.sqlite`. The caller holds the data and identity locks. An
/// export then revokes the copy's access (`capture`); an import's backup
/// keeps it as it is (`import.rs`).
pub fn snapshot(
    folder: &Path,
    target: &Path,
    blobs: &file_store::BlockingFiles,
    modules: &std::collections::BTreeMap<String, Value>,
) -> Result<Vec<(&'static str, PathBuf)>> {
    // Usage queued before the copy is written first, so the copy holds it.
    usage::flush();
    std::fs::create_dir_all(target)?;
    let mut components = Vec::new();
    if crate::modules::is_on(modules, "actions") || folder.join("database.sqlite").is_file() {
        let database = target.join("database.sqlite");
        copies::vacuum_into(&folder.join("database.sqlite"), &database).context("database")?;
        crate::notify::pause_exported_intents(&database).context("pending notices")?;
        components.push((registered_component("database")?, database));
    }
    if folder.join("identity.sqlite").is_file() {
        let identity = target.join("identity.sqlite");
        copies::vacuum_into(&folder.join("identity.sqlite"), &identity).context("accounts")?;
        components.push((registered_component("identity")?, identity));
    }
    let references = crate::modules::blob_references_in_data_dir(folder)?;
    if folder.join("files").is_dir() || !references.is_empty() {
        let files = target.join("files.tar");
        write_files_tar_with_references(&folder.join("files"), &files, &references, |id| blobs.open(id)).context("files")?;
        if folder.join("files/index.json").is_file() || !references.is_empty() {
            components.push((registered_component("files")?, files));
        }
    }
    if folder.join(usage::USAGE_DATABASE).is_file() {
        let usage = target.join(usage::USAGE_DATABASE);
        copies::vacuum_into(&folder.join(usage::USAGE_DATABASE), &usage).context("usage")?;
        components.push((registered_component("usage")?, usage));
    }
    Ok(components)
}

/// Deletes sessions, open email links and invitation tokens from an identity file and marks
/// every live API key revoked at import. `secure_delete` overwrites the
/// freed pages, so the session and link token hashes do not stay in the
/// file. A key's hash stays with its revoked row; it opens nothing.
pub fn revoke_access(path: &Path) -> Result<usize> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON;")?;
    let mut removed = 0;
    for table in ACCESS_TABLES {
        let exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE lower(type)='table' AND lower(name)=?1",
                [table],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            removed += connection.execute(&format!("DELETE FROM \"{table}\""), [])?;
        }
    }
    removed += crate::http::revoke_imported_keys(&connection)?;
    crate::invitations::forget_tokens(&connection)?;
    Ok(removed)
}

#[cfg(test)]
fn write_files_tar<R: Read>(
    directory: &Path,
    target: &Path,
    open: impl Fn(&str) -> Result<Option<(u64, R)>>,
) -> Result<()> {
    let references = crate::modules::blob_references_in_files_dir(directory)?;
    write_files_tar_with_references(directory, target, &references, open)
}

fn write_files_tar_with_references<R: Read>(
    directory: &Path,
    target: &Path,
    references: &[crate::modules::BlobReference],
    open: impl Fn(&str) -> Result<Option<(u64, R)>>,
) -> Result<()> {
    let index_bytes = match std::fs::read(directory.join("index.json")) {
        Ok(bytes) => bytes,
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => {
            if directory.is_dir() {
                for entry in std::fs::read_dir(directory)? {
                    let entry = entry?;
                    ensure!(!entry.file_name().to_string_lossy().ends_with(".blob"), "The Files directory has blobs but no readable index. Export stopped.");
                }
            }
            b"[]".to_vec()
        },
        Err(error_value) => return Err(error_value.into()),
    };
    let index: Vec<Value> = serde_json::from_slice(&index_bytes)?;
    let mut live_files = Vec::with_capacity(references.len());
    for file in &index {
        let id = file["id"].as_str().context("A stored file has an invalid id.")?;
        if let Some(reference) = references.iter().find(|reference| reference.id == id) {
            live_files.push((reference.id.clone(), reference.size));
        } else {
            anyhow::bail!("The Files module does not register blob {id} in its inventory.");
        }
    }
    let listed = live_files.iter().map(|(id, _)| id.as_str()).collect::<HashSet<_>>();
    let mut additional = references.iter().filter(|reference| !listed.contains(reference.id.as_str())).collect::<Vec<_>>();
    additional.sort_by(|left, right| left.id.cmp(&right.id));
    live_files.extend(additional.into_iter().map(|reference| (reference.id.clone(), reference.size)));
    let mut output = std::io::BufWriter::new(std::fs::File::create(target)?);
    let index_bytes = serde_json::to_vec(&index)?;
    output.write_all(&tar_header("index.json", index_bytes.len() as u64))?;
    output.write_all(&index_bytes)?;
    output.write_all(&vec![0; tar_padding(index_bytes.len() as u64)])?;
    for (id, expected_size) in live_files {
        let name = format!("{id}.blob");
        let (size, mut blob) =
            open(&id)?.with_context(|| format!("Stored file {id} has no data."))?;
        output.write_all(&tar_header(&name, size))?;
        let copied = std::io::copy(&mut blob, &mut output)?;
        ensure!(
            copied == size && size == expected_size,
            "Stored file {id} changed during the capture."
        );
        output.write_all(&vec![0; tar_padding(size)])?;
    }
    output.write_all(&[0; 1024])?;
    output
        .into_inner()
        .map_err(|error_value| error_value.into_error())?
        .sync_all()?;
    Ok(())
}

/// Writes the files component of an app folder's `data/files/`, where each
/// file's bytes sit beside the index as `<id>.blob`.
pub fn folder_files_tar(directory: &Path, target: &Path) -> Result<()> {
    let references = crate::modules::blob_references_in_files_dir(directory)?;
    folder_files_tar_with_references(directory, target, &references)
}

pub(crate) fn folder_files_tar_with_references(
    directory: &Path,
    target: &Path,
    references: &[crate::modules::BlobReference],
) -> Result<()> {
    write_files_tar_with_references(directory, target, references, |id| {
        let path = directory.join(format!("{id}.blob"));
        match std::fs::File::open(&path) {
            Ok(file) => Ok(Some((file.metadata()?.len(), file))),
            Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error_value) => Err(error_value.into()),
        }
    })
}

fn write_folder_tar(folder: &Path, target: &Path, settings: &[u8]) -> Result<()> {
    let mut output = std::io::BufWriter::new(std::fs::File::create(target)?);
    let mut entries = vec![crate::layout::SETTINGS_FILE.to_owned()];
    crate::functions::read_artifact(&folder.join("server"))?;
    for top in ["app", "dist", "server"] {
        let mut pending = vec![folder.join(top)];
        while let Some(directory) = pending.pop() {
            let Ok(listing) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in listing {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    pending.push(entry.path());
                } else {
                    let relative = entry
                        .path()
                        .strip_prefix(folder)?
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    entries.push(relative);
                }
            }
        }
    }
    entries.sort();
    for relative in entries {
        let (mut input, size): (Box<dyn Read>, u64) = if relative == crate::layout::SETTINGS_FILE {
            (
                Box::new(std::io::Cursor::new(settings.to_vec())),
                settings.len() as u64,
            )
        } else {
            let file = std::fs::File::open(folder.join(&relative))
                .with_context(|| format!("{relative} is not readable."))?;
            let size = file.metadata()?.len();
            (Box::new(file), size)
        };
        output.write_all(&path_header(&relative, size)?)?;
        let copied = std::io::copy(&mut input, &mut output)?;
        ensure!(copied == size, "{relative} changed during the export.");
        output.write_all(&vec![0; tar_padding(size)])?;
    }
    output.write_all(&[0; 1024])?;
    output
        .into_inner()
        .map_err(|error_value| error_value.into_error())?
        .sync_all()?;
    Ok(())
}

fn path_header(path: &str, size: u64) -> Result<[u8; 512]> {
    if path.len() <= 100 {
        return Ok(tar_header(path, size));
    }
    let split = path
        .char_indices()
        .filter(|(index, c)| *c == '/' && *index <= 155 && path.len() - index - 1 <= 100)
        .map(|(index, _)| index)
        .next()
        .with_context(|| format!("{path} is too long for the export archive."))?;
    let mut header = tar_header(&path[split + 1..], size);
    header[345..345 + split].copy_from_slice(&path.as_bytes()[..split]);
    header[148..156].fill(b' ');
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    Ok(header)
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

fn read_tar_header(input: &mut impl Read) -> Result<Option<(String, u64)>> {
    let mut header = [0u8; 512];
    input
        .read_exact(&mut header)
        .context("The files component ends early.")?;
    if header.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let stored = parse_octal(&header[148..156])?;
    let mut copy = header;
    copy[148..156].fill(b' ');
    let checksum: u64 = copy.iter().map(|byte| u64::from(*byte)).sum();
    ensure!(
        stored == checksum,
        "The files component has a damaged header."
    );
    ensure!(
        header[156] == b'0' || header[156] == 0,
        "The files component holds an entry that is not a file."
    );
    ensure!(
        header[345..500].iter().all(|byte| *byte == 0),
        "The files component holds an unexpected path."
    );
    let end = header[..100]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(100);
    let name = std::str::from_utf8(&header[..end])
        .context("The files component holds an unexpected path.")?
        .to_owned();
    Ok(Some((name, parse_octal(&header[124..136])?)))
}

pub(crate) fn files_index_from_archive(archive: &Path) -> Result<Vec<u8>> {
    let mut input = std::io::BufReader::new(std::fs::File::open(archive)?);
    let (name, size) = read_tar_header(&mut input)?.context("The files component is empty.")?;
    ensure!(name == "index.json", "The files component must start with index.json.");
    ensure!(size <= MAX_FILE_INDEX_BYTES, "The file index exceeds {} MiB.", MAX_FILE_INDEX_BYTES / MIB);
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut input).take(size).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == size, "The files component ends early.");
    serde_json::from_slice::<Vec<Value>>(&bytes).context("The file index is not valid JSON.")?;
    Ok(bytes)
}

pub(crate) fn references_for_archive(archive: &Path, database: Option<&Path>, scratch_root: &Path) -> Result<Vec<crate::modules::BlobReference>> {
    let scratch = scratch_root.join(format!(".package-inventory-{}", Uuid::new_v4()));
    let result = (|| {
        std::fs::create_dir_all(scratch.join("files"))?;
        if let Some(database) = database.filter(|path| path.is_file()) {
            std::fs::copy(database, scratch.join("database.sqlite"))?;
        }
        std::fs::write(scratch.join("files/index.json"), files_index_from_archive(archive)?)?;
        crate::modules::blob_references_in_data_dir(&scratch)
    })();
    let _ = std::fs::remove_dir_all(scratch);
    result
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let text = std::str::from_utf8(field)?.trim_matches(|c: char| c == '\0' || c == ' ');
    u64::from_str_radix(text, 8).context("The files component has a damaged header.")
}

pub(crate) fn unpack_files(
    archive: &Path,
    accounts: Option<&Connection>,
    destination: Option<(&Path, &file_store::BlockingFiles)>,
    limits: &file_store::FileLimits,
) -> Result<usize> {
    unpack_files_with_references(archive, accounts, destination, limits, &[])
}

pub(crate) fn unpack_files_with_references(
    archive: &Path,
    accounts: Option<&Connection>,
    destination: Option<(&Path, &file_store::BlockingFiles)>,
    limits: &file_store::FileLimits,
    references: &[crate::modules::BlobReference],
) -> Result<usize> {
    let mut input = std::io::BufReader::new(std::fs::File::open(archive)?);
    let (name, size) = read_tar_header(&mut input)?.context("The files component is empty.")?;
    ensure!(
        name == "index.json",
        "The files component must start with index.json."
    );
    ensure!(
        size <= MAX_FILE_INDEX_BYTES,
        "The file index exceeds {} MiB.",
        MAX_FILE_INDEX_BYTES / MIB
    );
    let mut index_bytes = Vec::new();
    (&mut input).take(size).read_to_end(&mut index_bytes)?;
    ensure!(
        index_bytes.len() as u64 == size,
        "The files component ends early."
    );
    skip(&mut input, tar_padding(size) as u64)?;
    let index: Vec<Value> =
        serde_json::from_slice(&index_bytes).context("The file index is not valid JSON.")?;
    let mut expected: HashMap<String, u64> = HashMap::new();
    let mut order = Vec::with_capacity(index.len());
    let mut total = 0u64;
    let mut normalized = Vec::with_capacity(index.len());
    for file in &index {
        let id = file["id"]
            .as_str()
            .filter(|id| files::valid_file_id(id))
            .context("The file index has an invalid file id.")?;
        let declared = file["size"]
            .as_u64()
            .context("The file index has a file without a size.")?;
        ensure!(
            declared <= limits.max_file_bytes,
            "File {id} exceeds this installation's {} limit per file.",
            limits.max_file()
        );
        total += declared;
        ensure!(
            total <= limits.quota_bytes,
            "The files exceed this installation's {} file quota.",
            limits.quota()
        );
        ensure!(
            expected.insert(id.to_owned(), declared).is_none(),
            "The file index lists file {id} twice."
        );
        order.push(id.to_owned());
        let mut restored = json!({
            "id": id,
            "name": file["name"].as_str().unwrap_or("file").replace(['/', '\\'], "-").chars().take(180).collect::<String>(),
            "type": file["type"].as_str().unwrap_or("application/octet-stream"),
            "size": declared,
            "visibility": files::file_visibility(file),
            "createdAt": file["createdAt"].as_u64().unwrap_or_else(now_ms),
        });
        if let Some(owner) = files::file_owner(file) {
            ensure!(
                accepted_owner_id(owner),
                "File {id} names an invalid owner."
            );
            let known = match accounts {
                Some(db) => db
                    .query_row("SELECT 1 FROM app_users WHERE id=?1", [owner], |_| Ok(()))
                    .optional()?
                    .is_some(),
                None => false,
            };
            ensure!(
                known,
                "File {id} belongs to an account the export does not contain."
            );
            restored["ownerId"] = json!(owner);
        }
        normalized.push(restored);
    }
    let mut additional = HashMap::<String, u64>::new();
    for reference in references {
        ensure!(files::valid_file_id(&reference.id), "Blob reference {} has an invalid ID.", reference.id);
        if !expected.contains_key(&reference.id) {
            ensure!(additional.insert(reference.id.clone(), reference.size).is_none(), "The blob inventory lists {} twice.", reference.id);
        }
    }
    let mut additional_order = additional.keys().cloned().collect::<Vec<_>>();
    additional_order.sort();
    if let Some((directory, _)) = destination {
        std::fs::create_dir_all(directory)?;
    }
    let mut seen = 0;
    while let Some((name, size)) = read_tar_header(&mut input)? {
        let id = name
            .strip_suffix(".blob")
            .context("The files component holds an unexpected entry.")?;
        let declared = if seen < order.len() {
            let declared = expected.remove(id).with_context(|| format!("The files component holds data for {id}, which the index does not list."))?;
            ensure!(order.get(seen).is_some_and(|next| next == id), "The files component is out of index order.");
            declared
        } else {
            let expected = additional.remove(id).with_context(|| format!("The files component holds data for {id}, which the index or any module inventory does not list."))?;
            ensure!(additional_order.get(seen - order.len()).is_some_and(|next| next == id), "The files component is out of module inventory order.");
            expected
        };
        ensure!(
            size == declared,
            "The data of file {id} does not match its indexed size."
        );
        let mut body = (&mut input).take(size);
        let verify_digest = |digest: &str| -> Result<()> {
            if let Some(expected) = index.get(seen).and_then(|file| file.get("sha256")) {
                ensure!(
                    expected.as_str() == Some(digest),
                    "File {id} does not match its indexed SHA-256."
                );
            }
            Ok(())
        };
        let copied = match destination {
            Some((directory, blobs)) => {
                let temp = directory.join(format!(".{id}.import-{}", Uuid::new_v4()));
                let stored = (|| {
                    let copied = std::io::copy(&mut body, &mut std::fs::File::create(&temp)?)?;
                    ensure!(copied == size, "The files component ends early.");
                    let (_, digest) = copies::checksum_file(&temp)?;
                    verify_digest(&digest)?;
                    if seen < normalized.len() { normalized[seen]["sha256"] = json!(digest); }
                    blobs.put(id, &temp)?;
                    Ok(copied)
                })();
                let _ = std::fs::remove_file(&temp);
                stored?
            }
            None if index[seen].get("sha256").is_some() => {
                let mut hasher = sha2::Sha256::default();
                let length = std::io::copy(&mut body, &mut hasher)?;
                verify_digest(&hex::encode(sha2::Digest::finalize(hasher)))?;
                length
            }
            None => std::io::copy(&mut body, &mut std::io::sink())?,
        };
        ensure!(copied == size, "The files component ends early.");
        skip(&mut input, tar_padding(size) as u64)?;
        seen += 1;
    }
    ensure!(
        expected.is_empty() && additional.is_empty(),
        "The files component is missing {} indexed or module-owned blobs.",
        expected.len() + additional.len()
    );
    if let Some((directory, _)) = destination {
        let bytes = serde_json::to_vec_pretty(&normalized)?;
        let temp = directory.join(format!(".index.json.tmp-{}", Uuid::new_v4()));
        std::fs::write(&temp, bytes)?;
        std::fs::rename(temp, directory.join("index.json"))?;
    }
    Ok(seen)
}

/// Unpacks a files component into `directory`, which receives `index.json`,
/// and the store `blobs`, which receives each file. `accounts` is the
/// identity file whose accounts the files may name as owners.
pub fn unpack_into(
    archive: &Path,
    accounts: &Connection,
    directory: &Path,
    blobs: &file_store::BlockingFiles,
    limits: &file_store::FileLimits,
) -> Result<usize> {
    let data = directory.parent().context("The files directory has no app data folder.")?;
    let database = data.join("database.sqlite");
    let references = references_for_archive(archive, Some(&database), data)?;
    unpack_files_with_references(archive, Some(accounts), Some((directory, blobs)), limits, &references)
}

fn skip(input: &mut impl Read, bytes: u64) -> Result<()> {
    let skipped = std::io::copy(&mut input.take(bytes), &mut std::io::sink())?;
    ensure!(skipped == bytes, "The files component ends early.");
    Ok(())
}

fn component_uploads(
    state: &AppState,
    principal: Option<&str>,
    project_id: &str,
    package: &InitialPackage,
) -> Result<Vec<(Content, PathBuf)>> {
    check_manifest(&package.manifest, &state.files.limits)?;
    for name in package.components.keys() {
        ensure!(
            package
                .manifest
                .contents
                .iter()
                .any(|content| &content.name == name),
            "The payload names a {name} component the manifest does not list."
        );
    }
    let mut uploads = Vec::new();
    for content in &package.manifest.contents {
        let name = &content.name;
        let id = package
            .components
            .get(name)
            .with_context(|| format!("The {name} component has not been uploaded."))?;
        let upload = match principal {
            Some(principal) => transfer(state, principal, &json!({"transferId": id}))?,
            None => {
                let body: Option<String> = crate::transfers::store(state)?
                    .query_row(
                        "SELECT body FROM transfers WHERE id=?1 AND expires>?2",
                        params![id, now_ms()],
                        |r| r.get(0),
                    )
                    .optional()?;
                serde_json::from_str(
                    &body.with_context(|| format!("The {name} upload has expired."))?,
                )?
            }
        };
        ensure!(
            upload["direction"] == "upload" && upload["complete"] == true,
            "The {name} upload is not complete."
        );
        ensure!(
            upload["component"] == name.as_str(),
            "Transfer {id} was not started for the {name} component."
        );
        ensure!(
            upload["projectId"] == project_id,
            "The {name} upload belongs to another project."
        );
        ensure!(
            upload["size"] == content.size && upload["sha256"] == content.sha256.as_str(),
            "The {name} component does not match the size and SHA-256 in the manifest."
        );
        uploads.push((content.clone(), transfer_path(state, id)?));
    }
    Ok(uploads)
}

/// The file of each component the package names, checked against the
/// manifest and the project. The caller has checked who uploaded them.
pub fn component_paths(
    state: &AppState,
    project_id: &str,
    package: &InitialPackage,
) -> Result<Vec<(String, PathBuf)>> {
    Ok(component_uploads(state, None, project_id, package)?
        .into_iter()
        .map(|(content, path)| (content.name, path))
        .collect())
}

/// Runs at `release_plan`, before any Deployment exists: the manifest, each
/// upload's checksum against the manifest, SQLite integrity, the files and
/// the accounts they name, and the free space the import will use.
pub async fn verify(
    state: &AppState,
    principal: &str,
    project_id: &str,
    package: &InitialPackage,
) -> Result<()> {
    let uploads = component_uploads(state, Some(principal), project_id, package)?;
    let database = uploads
        .iter()
        .find(|(c, _)| c.name == "database")
        .map(|(c, _)| c.size)
        .unwrap_or(0);
    let total: u64 = uploads.iter().map(|(c, _)| c.size).sum();
    ensure_space(
        free_bytes(&state.data_root)?,
        total + database + SPACE_MARGIN,
        "import this app",
    )?;
    let file_limits = state.files.limits;
    let actions_on = package
        .manifest
        .modules
        .get("actions")
        .is_none_or(|state| state["on"] != false);
    tokio::task::spawn_blocking(move || {
        for (content, path) in &uploads {
            let (size, sha256) = copies::checksum_file(path)?;
            ensure!(
                size == content.size && sha256 == content.sha256,
                "The {} component does not match the SHA-256 in the manifest.",
                content.name
            );
        }
        let components = uploads
            .into_iter()
            .map(|(content, path)| (content.name, path))
            .collect::<Vec<_>>();
        check_components_for_modules(&components, &file_limits, actions_on)
    })
    .await?
}

/// The checks every component gets before an import puts it in place:
/// SQLite integrity, no stored trigger or view, an accounts file with its
/// users, and files that fit the limits and belong to accounts it holds.
pub fn check_components(
    components: &[(String, PathBuf)],
    file_limits: &file_store::FileLimits,
) -> Result<()> {
    check_components_for_modules(components, file_limits, true)
}

pub fn check_components_for_modules(
    components: &[(String, PathBuf)],
    file_limits: &file_store::FileLimits,
    actions_on: bool,
) -> Result<()> {
    let path = |name: &str| {
        components
            .iter()
            .find(|(component, _)| component == name)
            .map(|(_, p)| p.clone())
    };
    let database = path("database");
    ensure!(
        database.is_some() || !actions_on,
        "The package has no database. This Kernel requires a database component for apps with actions on."
    );
    if let Some(database) = &database {
        no_trigger_or_view(database, "database", "database")?;
        integrity(database, "database")?;
    }
    for (name, label, what) in [
        ("identity", "accounts", "accounts component"),
        ("usage", "usage", "usage component"),
    ] {
        if let Some(component) = path(name) {
            no_trigger_or_view(&component, label, what)?;
        }
    }
    let accounts = match path("identity") {
        Some(identity) => {
            integrity(&identity, "accounts")?;
            let db =
                Connection::open_with_flags(&identity, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let has_users: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE lower(type)='table' AND lower(name)='app_users')",
                [],
                |r| r.get(0),
            )?;
            ensure!(has_users, "The accounts component has no app_users table.");
            match db.prepare("SELECT password_hash FROM app_users") {
                Ok(mut hashes) => {
                    let rows =
                        hashes.query_map([], |row| row.get::<_, rusqlite::types::Value>(0))?;
                    for hash in rows {
                        let known = matches!(hash?, rusqlite::types::Value::Text(hash) if auth::is_stored_password_hash(&hash));
                        ensure!(
                            known,
                            "The accounts component holds a password hash this Kernel did not write."
                        );
                    }
                }
                Err(
                    rusqlite::Error::SqlInputError { msg, .. }
                    | rusqlite::Error::SqliteFailure(_, Some(msg)),
                ) if msg.starts_with("no such column") => {}
                Err(error) => return Err(error.into()),
            }
            Some(db)
        }
        None => None,
    };
    if let Some(files) = path("files") {
        unpack_files(&files, accounts.as_ref(), None, file_limits)?;
    }
    if let Some(usage) = path("usage") {
        integrity(&usage, "usage")?;
    }
    Ok(())
}

fn no_trigger_or_view(path: &Path, label: &str, what: &str) -> Result<()> {
    let stored = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(
            |db| match crate::sql_rules::stored_trigger_or_view(&db, what)? {
                Some(refusal) => Ok(Some(refusal)),
                None => crate::sql_rules::stored_virtual_table(&db, what),
            },
        )
        .with_context(|| format!("The {label} component is not a SQLite database."))?;
    match stored {
        Some(refusal) => anyhow::bail!(refusal),
        None => Ok(()),
    }
}

fn integrity(path: &Path, label: &str) -> Result<()> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("The {label} component is not a SQLite database."))?;
    let result: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .with_context(|| format!("The {label} component is not a SQLite database."))?;
    ensure!(
        result == "ok",
        "The {label} component failed SQLite's integrity check."
    );
    Ok(())
}

/// Copies the uploaded database to `target`, where the release check tries
/// the schema against it.
pub fn copy_database(state: &AppState, package: &InitialPackage, target: &Path) -> Result<()> {
    let id = package
        .components
        .get("database")
        .context("The package has no database.")?;
    std::fs::copy(transfer_path(state, id)?, target)?;
    Ok(())
}

/// Runs inside the release, after the app folder exists and before
/// its record does. Copies each component in, checking its SHA-256 as it
/// goes, revokes sessions, links and keys, and unpacks the files. The
/// uploads stay intact, so a failed release can be retried.
/// Copies the components into the `data/` of the app `id`.
pub async fn install(state: &AppState, package: &InitialPackage, id: &str) -> Result<()> {
    let uploads = component_uploads(state, None, id, package)?;
    let directory = crate::layout::data(&state.data_root, id);
    let file_limits = state.files.limits;
    let deployment_id = id;
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
            ensure!(
                size == content.size && sha256 == content.sha256,
                "The files component does not match the SHA-256 in the manifest."
            );
            let accounts = crate::app_db::read_optional(&identity)?;
            let references = references_for_archive(source, Some(&directory.join("database.sqlite")), &directory)?;
            unpack_files_with_references(
                source,
                accounts.as_ref(),
                Some((&directory.join("files"), &blobs)),
                &file_limits,
                &references,
            )?;
        }
        if let Some((content, source)) = find("usage") {
            copy_verified(source, &directory.join(usage::USAGE_DATABASE), content)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    crate::domain::arrive(state, id, package.manifest.domain.as_deref()).await
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn functions_export_import_preserves_published_bundles_accounts_rows_and_rules() {
        functions_round_trip(None).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires KONAE_TEST_FUNCTION_WORKER pointing to the built external JavaScript worker"]
    async fn functions_export_import_executes_the_published_bundle_with_imported_identity() {
        functions_round_trip(Some(
            std::env::var_os("KONAE_TEST_FUNCTION_WORKER")
                .expect("KONAE_TEST_FUNCTION_WORKER")
                .into(),
        ))
        .await;
    }

    async fn functions_round_trip(worker: Option<PathBuf>) {
        let root =
            std::env::temp_dir().join(format!("konae-functions-transfer-{}", Uuid::new_v4()));
        let (a_root, b_root) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a_root).unwrap();
        std::fs::create_dir_all(&b_root).unwrap();
        let (mut a, mut b) = (AppState::for_test(&a_root), AppState::for_test(&b_root));
        if let Some(worker) = &worker {
            let policy = std::env::var_os("KONAE_TEST_FUNCTION_CGROUP")
                .map(|root| konae_functions_executor::MemoryPolicy::LinuxCgroup {
                    delegated_root: root.into(),
                })
                .unwrap_or(konae_functions_executor::MemoryPolicy::DevelopmentPolling);
            a.functions = Arc::new(crate::functions::runtime::Runtime::configured(
                worker.clone(),
                policy.clone(),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_millis(100),
            ));
            b.functions = Arc::new(crate::functions::runtime::Runtime::configured(
                worker.clone(),
                policy,
                std::time::Duration::from_secs(2),
                std::time::Duration::from_millis(100),
            ));
        }
        let id = "portable_functions";
        let bundle = if worker.is_some() {
            r#"
            export async function handle(request, context) {
                if (request.method === 'POST') await context.actions.call('notes.touch', {});
                return Response.json(await context.actions.call('notes.list', {}));
            }
        "#
        } else {
            "throw new Error('no evaluation'); export function handle() {}"
        };
        let mut payload = crate::functions::tests::release(id, bundle);
        payload["initialModules"] =
            json!({"functions":{"on":true},"auth":{"on":true},"actions":{"on":true}});
        payload["initialSignupPolicy"] = json!({"mode":"open","allowlist":[]});
        payload["project"]["files"]["schema.sql"] = json!(
            "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, body TEXT NOT NULL, touches INTEGER NOT NULL DEFAULT 0);"
        );
        let actions = json!({"version":1,"actions":{
            "notes.list":{"kind":"query","access":"authenticated","statement":"SELECT body,touches FROM notes WHERE owner_id=?1 LIMIT 10","bindings":[{"source":"actor","field":"id"}]},
            "notes.touch":{"kind":"mutation","access":"authenticated","statement":"UPDATE notes SET touches=touches+1 WHERE owner_id=?1","bindings":[{"source":"actor","field":"id"}]}
        }});
        payload["project"]["files"]["actions.json"] = json!(actions.to_string());
        payload["serverArtifact"]["manifest"]["functions"]["orders"]["access"] =
            json!("authenticated");
        payload["serverArtifact"]["manifest"]["functions"]["orders"]["actions"] =
            json!(["notes.list", "notes.touch"]);
        payload["project"]["files"]["functions.json"] =
            json!(payload["serverArtifact"]["manifest"].to_string());
        let cleanup = "export function handle() { return new Response(null, {status: 204}); }";
        let cron_source = json!({"version":1,"crons":{"cleanup":{"cron":"0 8 * * 1-5","function":"cleanup"}}});
        payload["project"]["files"]["crons.json"] = json!(cron_source.to_string());
        payload["project"]["files"]["functions/cleanup.ts"] = json!(cleanup);
        payload["serverArtifact"]["bundles"]["cleanup"] = json!(cleanup);
        let scheduled = json!({"entry":"functions/cleanup.ts","access":"scheduled","mount":"/","methods":["POST"],"actions":[]});
        payload["serverArtifact"]["manifest"]["functions"]["cleanup"] = scheduled.clone();
        payload["serverArtifact"]["manifest"]["crons"] = cron_source["crons"].clone();
        let mut function_source: Value = serde_json::from_str(payload["project"]["files"]["functions.json"].as_str().unwrap()).unwrap();
        function_source["functions"]["cleanup"] = scheduled;
        payload["project"]["files"]["functions.json"] = json!(function_source.to_string());
        let deployed = crate::functions::tests::publish(&a, &payload).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let serving = tokio::spawn(crate::serve(listener, a.clone(), axum::Router::new()));
        let client = reqwest::Client::new();
        let account = json!({"email":"portable@example.test","password":"portable password"});
        let response = client
            .post(format!("http://{address}/_konae/auth/signup"))
            .header("host", &deployed.hostname)
            .json(&account)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let cookie = response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let response: Value = response.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{response}");
        let user_id = response["result"]["user"]["userId"].as_str().unwrap();
        Connection::open(crate::layout::database(&a_root, id)).unwrap()
            .execute("INSERT INTO notes(owner_id,body) VALUES (?1,'portable row'), ('other-owner','private decoy')", [user_id]).unwrap();
        Connection::open(crate::layout::database(&a_root, id)).unwrap().execute_batch(
            "CREATE TABLE __konae_notify_intents (id TEXT PRIMARY KEY, payload_json TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('pending','handed_off')), created_at INTEGER NOT NULL) WITHOUT ROWID;
             INSERT INTO __konae_notify_intents VALUES ('intent-portable', '{\"app\":\"portable_functions\",\"release\":\"r1\",\"action\":\"orders.notify\"}', 'pending', 17);",
        ).unwrap();
        let mut original_rows = None;
        if worker.is_some() {
            let response = client
                .post(format!("http://{address}/api/orders"))
                .header("host", &deployed.hostname)
                .header("cookie", &cookie)
                .header("idempotency-key", "portable-receipt")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let rows: Value = response.json().await.unwrap();
            assert_eq!(rows, json!([{"body":"portable row","touches":1}]));
            original_rows = Some(rows);
        }
        std::fs::write(
            crate::layout::sources(&a_root, id).join("functions.json"),
            "invalid newer draft",
        )
        .unwrap();
        std::fs::remove_file(crate::layout::sources(&a_root, id).join("functions/orders.ts"))
            .unwrap();
        let secret = crate::http::secrets_store(&a_root, id).unwrap();
        secret.execute("INSERT INTO secrets(name,value,updated_at,use) VALUES ('private','never-export-this',0,'call')", []).unwrap();
        drop(secret);
        for path in [
            "server/orders.mjs",
            "server/manifest.json",
            "functions/orders.ts",
            ".konae/cache/snapshot",
        ] {
            let response = client
                .get(format!("http://{address}/{path}"))
                .header("host", &deployed.hostname)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
            assert!(!response.text().await.unwrap().contains("no evaluation"));
        }
        let public_crons = client
            .get(format!("http://{address}/crons.json"))
            .header("host", &deployed.hostname)
            .send()
            .await
            .unwrap();
        assert!(!public_crons.text().await.unwrap().contains("0 8 * * 1-5"));
        let exported = export(&a, deployed.clone(), "owner:test", |_| {
            Ok(Revisions {
                published: 1,
                sources: Some(2),
            })
        })
        .await
        .unwrap();
        assert_eq!(exported["sourcesAhead"], true);
        let tar_id = exported["folder"]["transferId"].as_str().unwrap();
        let mut archive = std::fs::File::open(transfer_path(&a, tar_id).unwrap()).unwrap();
        let unpacked = root.join("exported");
        std::fs::create_dir_all(&unpacked).unwrap();
        let mut names = Vec::new();
        while let Some((name, size)) = read_tar_header(&mut archive).unwrap() {
            assert!(
                name == "konae.json"
                    || name.starts_with("app/")
                    || name.starts_with("dist/")
                    || name.starts_with("server/"),
                "{name}"
            );
            let mut bytes = vec![0; size as usize];
            archive.read_exact(&mut bytes).unwrap();
            let path = unpacked.join(&name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
            archive.read_exact(&mut vec![0; tar_padding(size)]).unwrap();
            names.push(name);
        }
        assert!(names.contains(&"server/orders.mjs".to_owned()));
        assert!(names.contains(&"app/crons.json".to_owned()));
        assert!(
            !names
                .iter()
                .any(|name| name.contains("secrets") || name.starts_with(".konae/"))
        );
        let incoming_artifact = crate::functions::read_artifact(&unpacked.join("server"))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::functions::validate_artifact(
                Some(&incoming_artifact.manifest),
                Some(&incoming_artifact)
            )
            .unwrap(),
            deployed.server
        );
        let mut components = HashMap::new();
        for (name, download) in exported["downloads"].as_object().unwrap() {
            let source = transfer_path(&a, download["transferId"].as_str().unwrap()).unwrap();
            let bytes = std::fs::read(source).unwrap();
            if name == "identity" {
                assert!(
                    !bytes
                        .windows(b"never-export-this".len())
                        .any(|part| part == b"never-export-this")
                );
                let identity = root.join("exported-identity.sqlite");
                std::fs::write(&identity, &bytes).unwrap();
                let db = Connection::open(identity).unwrap();
                assert_eq!(
                    db.query_row("SELECT COUNT(*) FROM app_sessions", [], |row| row
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
            let begun = crate::transfers::transfer_call(&b, "owner:test", "transfer_begin", &json!({"size":bytes.len(),"sha256":digest(&bytes),"component":name,"projectId":id}), false).await.unwrap();
            let transfer_id = begun["transferId"].as_str().unwrap();
            crate::transfers::receive_body(&b, "owner:test", transfer_id, Body::from(bytes))
                .await
                .unwrap();
            components.insert(name.clone(), transfer_id.to_owned());
        }
        let package = InitialPackage {
            manifest: serde_json::from_value(exported["manifest"].clone()).unwrap(),
            components,
        };
        verify(&b, "owner:test", id, &package).await.unwrap();
        let saved = crate::sources::read(&a_root, id).await.unwrap().unwrap();
        payload["project"]["files"] = saved["files"].clone();
        payload["initialReleased"] = exported["manifest"]["released"].clone();
        payload["initialPackage"] =
            json!({"manifest":package.manifest,"components":package.components});
        payload["serverArtifact"] = serde_json::to_value(incoming_artifact).unwrap();
        let imported = crate::functions::tests::publish(&b, &payload).await;
        assert_eq!(imported.server, deployed.server);
        assert_eq!(crate::functions::cron::declarations(&imported).unwrap()["cleanup"].cron.text(), "0 8 * * 1-5");
        assert_eq!(imported.actions, deployed.actions);
        assert_eq!(imported.signup_policy, deployed.signup_policy);
        assert_eq!(imported.modules, deployed.modules);
        assert!(!crate::layout::secrets(&b_root, id).exists());
        let db = Connection::open(crate::layout::database(&b_root, id)).unwrap();
        assert_eq!(db.query_row("SELECT payload_json FROM __konae_notify_intents WHERE id='intent-portable' AND state='pending'", [], |row| row.get::<_, String>(0)).unwrap(),
            r#"{"app":"portable_functions","release":"r1","action":"orders.notify"}"#);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT body FROM notes WHERE owner_id=?1",
                [user_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "portable row"
        );
        drop(db);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let destination = tokio::spawn(crate::serve(listener, b.clone(), axum::Router::new()));
        let action_call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"actions_invoke","arguments":{"name":"notes.list","input":{}}}});
        let old_session: Value = client
            .post(format!("http://{address}/_konae/mcp"))
            .header("host", &imported.hostname)
            .header("cookie", &cookie)
            .json(&action_call)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!old_session.to_string().contains("portable row"));
        let response = client
            .post(format!("http://{address}/_konae/auth/login"))
            .header("host", &imported.hostname)
            .json(&account)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let new_cookie = response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let login: Value = response.json().await.unwrap();
        assert_eq!(login["result"]["user"]["userId"], user_id);
        let rows: Value = client
            .post(format!("http://{address}/_konae/mcp"))
            .header("host", &imported.hostname)
            .header("cookie", &new_cookie)
            .json(&action_call)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let permission_proof = json!({
            "release": imported.artifact_version,
            "appActor": user_id,
            "rowRule": "notes.owner_id = actor.id",
            "result": rows,
        });
        let result = permission_proof["result"].to_string();
        assert!(!permission_proof["release"].as_str().unwrap_or_default().is_empty());
        assert!(result.contains("portable row"), "{permission_proof}");
        assert!(!result.contains("private decoy"), "{permission_proof}");
        if let Some(original_rows) = original_rows {
            let old = client
                .get(format!("http://{address}/api/orders"))
                .header("host", &imported.hostname)
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(old.status(), reqwest::StatusCode::UNAUTHORIZED);
            let replay = client
                .post(format!("http://{address}/api/orders"))
                .header("host", &imported.hostname)
                .header("cookie", &new_cookie)
                .header("idempotency-key", "portable-receipt")
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), reqwest::StatusCode::CONFLICT);
            assert_eq!(
                replay.json::<Value>().await.unwrap()["code"],
                crate::functions::invocation::IDEMPOTENCY_REPLAY_CODE
            );
            let response = client
                .get(format!("http://{address}/api/orders"))
                .header("host", &imported.hostname)
                .header("cookie", &new_cookie)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let rows: Value = response.json().await.unwrap();
            assert_eq!(status, reqwest::StatusCode::OK, "{rows}");
            assert_eq!(rows, original_rows);
        }
        assert_eq!(
            std::fs::read_to_string(crate::layout::sources(&b_root, id).join("functions.json"))
                .unwrap(),
            "invalid newer draft"
        );
        assert!(
            !crate::layout::sources(&b_root, id)
                .join("functions/orders.ts")
                .exists()
        );
        let mut restart = AppState::for_test(&b_root);
        restart.execution_mode = crate::execution::ExecutionMode::Live;
        let reopened = crate::folder::open(&restart, id, None).await.unwrap();
        assert_eq!(reopened.server, deployed.server);
        serving.abort();
        destination.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn snapshots_carry_an_existing_database_when_actions_are_off() {
        let root = std::env::temp_dir().join(format!("konae-package-off-{}", Uuid::new_v4()));
        let folder = root.join("app/data");
        let target = root.join("snapshot");
        std::fs::create_dir_all(&folder).unwrap();
        let database = Connection::open(folder.join("database.sqlite")).unwrap();
        database
            .execute_batch("CREATE TABLE notes(value TEXT); INSERT INTO notes VALUES('kept');")
            .unwrap();
        drop(database);
        let store: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let blobs = file_store::BlockingFiles::new(store, "app");
        let mut modules = crate::modules::defaults();
        modules.get_mut("actions").unwrap()["on"] = json!(false);
        let parts = snapshot(&folder, &target, &blobs, &modules).unwrap();
        assert!(parts.iter().any(|(name, _)| *name == "database"));
        let copied = Connection::open(target.join("database.sqlite")).unwrap();
        assert_eq!(
            copied
                .query_row("SELECT value FROM notes", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "kept"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn export_pauses_only_unhanded_notice_work_and_import_resumes_that_work() {
        let root = std::env::temp_dir().join(format!("konae-package-notify-handoff-{}", Uuid::new_v4()));
        let folder = crate::layout::data(&root, "app");
        let target = root.join("snapshot");
        std::fs::create_dir_all(&folder).unwrap();
        let source = Connection::open(folder.join("database.sqlite")).unwrap();
        source.execute_batch(
            "CREATE TABLE __konae_notify_intents (id TEXT PRIMARY KEY, payload_json TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('pending','handed_off')), created_at INTEGER NOT NULL);
             INSERT INTO __konae_notify_intents VALUES ('waiting', '{\"results\":{}}', 'pending', 1);
             INSERT INTO __konae_notify_intents VALUES ('accepted', '{\"results\":{\"owner@example.test\":\"uncertain\"}}', 'handed_off', 2);",
        ).unwrap();
        drop(source);
        let store: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let blobs = file_store::BlockingFiles::new(store, "app");
        let parts = snapshot(&folder, &target, &blobs, &crate::modules::defaults()).unwrap();
        assert!(parts.iter().any(|(name, _)| *name == "database"));
        let copied_path = target.join("database.sqlite");
        let copied = Connection::open(&copied_path).unwrap();
        let states: Vec<(String, String, Option<String>)> = copied.prepare(
            "SELECT id, state, transfer_state FROM __konae_notify_intents ORDER BY id",
        ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
            .collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(states, vec![
            ("accepted".into(), "handed_off".into(), None),
            ("waiting".into(), "pending".into(), Some("paused".into())),
        ]);
        let preserved: String = copied.query_row("SELECT payload_json FROM __konae_notify_intents WHERE id='accepted'", [], |row| row.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&preserved).unwrap()["results"]["owner@example.test"], "uncertain");
        drop(copied);
        crate::notify::resume_imported_intents(&copied_path).unwrap();
        let imported = Connection::open(&copied_path).unwrap();
        let resumed: Option<String> = imported.query_row("SELECT transfer_state FROM __konae_notify_intents WHERE id='waiting'", [], |row| row.get(0)).unwrap();
        let terminal: String = imported.query_row("SELECT state FROM __konae_notify_intents WHERE id='accepted'", [], |row| row.get(0)).unwrap();
        assert_eq!(resumed, None);
        assert_eq!(terminal, "handed_off");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn snapshots_keep_existing_accounts_and_files_when_their_modules_are_off() {
        let root = std::env::temp_dir().join(format!("konae-package-disabled-state-{}", Uuid::new_v4()));
        let folder = crate::layout::data(&root, "app");
        let target = root.join("snapshot");
        std::fs::create_dir_all(&folder).unwrap();
        let identity = Connection::open(folder.join("identity.sqlite")).unwrap();
        identity.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('user-1');").unwrap();
        drop(identity);
        let files = crate::layout::files(&root, "app");
        std::fs::create_dir_all(&files).unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        std::fs::write(files.join("index.json"), json!([{"id":id,"name":"kept","size":4,"visibility":"private"}]).to_string()).unwrap();
        std::fs::write(files.join(format!("{id}.blob")), b"kept").unwrap();
        let store: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let blobs = file_store::BlockingFiles::new(store, "app");
        let mut modules = crate::modules::defaults();
        modules.get_mut("auth").unwrap()["on"] = json!(false);
        modules.get_mut("files").unwrap()["on"] = json!(false);
        let parts = tokio::task::spawn_blocking(move || snapshot(&folder, &target, &blobs, &modules)).await.unwrap().unwrap();
        assert!(parts.iter().any(|(name, _)| *name == "identity"));
        assert!(parts.iter().any(|(name, _)| *name == "files"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_export_stops_when_blobs_exist_without_their_index() {
        let root = std::env::temp_dir().join(format!("konae-package-missing-index-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("00000000-0000-4000-8000-000000000001.blob"), b"keep").unwrap();
        let error = folder_files_tar(&root, &root.join("files.tar")).unwrap_err().to_string();
        assert!(error.contains("blobs but no readable index. Export stopped."), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn private_inventory_blob_round_trips_outside_the_files_index() {
        let root = std::env::temp_dir().join(format!("konae-package-private-inventory-{}", Uuid::new_v4()));
        let data = root.join("app-data");
        let files = data.join("files");
        std::fs::create_dir_all(&files).unwrap();
        let id = "00000000-0000-4000-8000-000000000002".to_owned();
        let bytes = b"private module bytes".to_vec();
        let source = root.join("private.blob");
        std::fs::write(&source, &bytes).unwrap();
        let inventories = std::collections::BTreeMap::from([
            ("files".to_owned(), "file-index".to_owned()),
            ("private-test".to_owned(), "private-test".to_owned()),
        ]);
        let references = crate::modules::blob_references_with(&data, "app", &inventories, |provider, _, _| match provider {
            "file-index" => Ok(Vec::new()),
            "private-test" => Ok(vec![(id.clone(), bytes.len() as u64)]),
            _ => unreachable!(),
        }).unwrap();
        assert!(!crate::modules::is_on(&crate::modules::defaults(), "files"));
        let archive = root.join("files.tar");
        let target_root = root.join("imported");
        let target_store: std::sync::Arc<dyn file_store::FileStore> = std::sync::Arc::new(file_store::LocalFolder::new(&target_root));
        let target_store_for_task = target_store.clone();
        let target_root_for_task = target_root.clone();
        let references_for_task = references.clone();
        let source_for_task = source.clone();
        let archive_for_task = archive.clone();
        let id_for_task = id.clone();
        tokio::task::spawn_blocking(move || {
            write_files_tar_with_references(&files, &archive_for_task, &references_for_task, |requested| {
                anyhow::ensure!(requested == id_for_task, "Unexpected blob request.");
                let file = std::fs::File::open(&source_for_task)?;
                let size = file.metadata()?.len();
                Ok(Some((size, file)))
            }).unwrap();
            let destination = target_root_for_task.join("apps").join("app").join("data").join("files");
            let store = file_store::BlockingFiles::new(target_store_for_task, "app");
            unpack_files_with_references(&archive_for_task, None, Some((&destination, &store)), &Default::default(), &references_for_task).unwrap();
        }).await.unwrap();
        assert_eq!(target_store.list("app").await.unwrap(), vec![id.clone()]);
        assert_eq!(std::fs::read(target_root.join("apps/app/data/files/index.json")).unwrap(), b"[]");
        assert_eq!(std::fs::read(target_root.join(format!("apps/app/data/files/{id}.blob"))).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(root);
    }

    fn manifest() -> Manifest {
        Manifest {
            format: PACKAGE_FORMAT.into(),
            version: PACKAGE_VERSION,
            package_id: "p".into(),
            created_at: 1,
            deployment_id: "d".into(),
            project_id: "app".into(),
            visibility: "private".into(),
            modules: std::collections::BTreeMap::new(),
            data_versions: crate::modules::data_versions_for_components(["database"]),
            signup_policy: None,
            sign_in_methods: None,
            published_revision: 1,
            released: None,
            artifact_version: Some("v1".into()),
            sources_revision: None,
            contents: vec![Content {
                name: "database".into(),
                size: 4096,
                sha256: "a".repeat(64),
            }],
            excluded: vec![],
            domain: None,
            path: None,
        }
    }

    #[test]
    fn manifests_name_their_version_components_and_limits() {
        assert!(check_manifest(&manifest(), &Default::default()).is_ok());
        let mut unknown_data = manifest();
        unknown_data.data_versions.insert("private-test".into(), 1);
        assert!(check_manifest(&unknown_data, &Default::default()).unwrap_err().to_string().contains("unknown module data private-test"));
        let mut incompatible_data = manifest();
        incompatible_data.data_versions.insert("files".into(), 99);
        assert!(check_manifest(&incompatible_data, &Default::default()).unwrap_err().to_string().contains("Module files data version 99 is not supported"));
        let mut legacy_json = serde_json::to_value(manifest()).unwrap();
        legacy_json.as_object_mut().unwrap().remove("dataVersions");
        let legacy: Manifest = serde_json::from_value(legacy_json).unwrap();
        assert!(check_manifest(&legacy, &Default::default()).is_ok());
        let mut next = manifest();
        next.version = 2;
        assert!(
            check_manifest(&next, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("Package version 2 is not supported")
        );
        let mut foreign = manifest();
        foreign.format = "zip".into();
        assert!(check_manifest(&foreign, &Default::default()).is_err());
        let mut unknown = manifest();
        unknown.contents.push(Content {
            name: "secrets".into(),
            size: 1,
            sha256: "a".repeat(64),
        });
        assert!(
            check_manifest(&unknown, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("Unknown package component secrets")
        );
        let mut twice = manifest();
        twice.contents.push(twice.contents[0].clone());
        assert!(
            check_manifest(&twice, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("twice")
        );
        let mut large = manifest();
        large.contents[0].size = MAX_APP_DATABASE_BYTES + 1;
        assert!(
            check_manifest(&large, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("its limit is")
        );
        let mut identity = manifest();
        identity.contents.push(Content {
            name: "identity".into(),
            size: auth::MAX_IDENTITY_DATABASE_BYTES + 1,
            sha256: "b".repeat(64),
        });
        assert!(check_manifest(&identity, &Default::default()).is_err());
        let mut empty = manifest();
        empty.contents.clear();
        assert!(
            check_manifest(&empty, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("no database")
        );
        let mut static_app = manifest();
        static_app.contents.clear();
        static_app
            .modules
            .insert("actions".into(), json!({"on":false}));
        assert!(check_manifest(&static_app, &Default::default()).is_ok());
        assert!(check_components_for_modules(&[], &Default::default(), false).is_ok());
        let mut checksum = manifest();
        checksum.contents[0].sha256 = "A".repeat(64);
        assert!(check_manifest(&checksum, &Default::default()).is_err());
    }

    #[test]
    fn sign_in_methods_travel_in_the_manifest_and_older_manifests_mean_password() {
        let mut manifest = manifest();
        let older: Manifest =
            serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
        assert!(older.sign_in_methods.is_none());
        assert!(
            serde_json::to_value(&manifest)
                .unwrap()
                .get("signInMethods")
                .is_none()
        );
        manifest.sign_in_methods = Some(vec![SignInMethod::Password, SignInMethod::Google]);
        let written = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            written["signInMethods"],
            serde_json::json!(["password", "google"])
        );
        assert!(check_manifest(&manifest, &Default::default()).is_ok());
        manifest.sign_in_methods = Some(vec![SignInMethod::Google]);
        assert_eq!(
            check_manifest(&manifest, &Default::default())
                .unwrap_err()
                .to_string(),
            PASSWORD_REQUIRED
        );
    }

    #[test]
    fn space_checks_refuse_with_both_numbers() {
        assert!(ensure_space(300 * MIB, 200 * MIB, "import this app").is_ok());
        let refused = ensure_space(100 * MIB, 200 * MIB, "import this app")
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            "Not enough free space on the Kernel volume to import this app: 200 MiB needed, 100 MiB free."
        );
        assert!(free_bytes(Path::new(".")).unwrap() > 0);
    }

    #[test]
    fn identity_copies_lose_sessions_and_links_and_carry_keys_revoked() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("identity.sqlite");
        let db = Connection::open(&path).unwrap();
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
        assert_eq!(
            revoke_access(&path).unwrap(),
            3,
            "a session, a link and the live key"
        );
        let bytes = std::fs::read(&path).unwrap();
        for secret in ["secret-session-hash", "secret-link-hash"] {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{secret} survived"
            );
        }
        let db = Connection::open(&path).unwrap();
        let users: i64 = db
            .query_row("SELECT COUNT(*) FROM app_users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users, 1);
        let keys: Vec<(String, Option<i64>, Option<String>)> = db
            .prepare("SELECT id, revoked_at, revoked_reason FROM api_keys ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            matches!(&keys[0], (id, Some(at), Some(reason)) if id == "key-live" && *at > 2 && reason == "import")
        );
        assert_eq!(
            keys[1],
            ("key-old".to_owned(), Some(2), None),
            "an owner's revocation keeps its time and has no reason"
        );
        assert_eq!(
            revoke_access(&path).unwrap(),
            0,
            "a second pass changes nothing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn revoking_access_finds_tables_and_columns_whatever_their_case() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("identity.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE app_users(id TEXT PRIMARY KEY, state TEXT); INSERT INTO app_users VALUES('u1', 'active');
             CREATE TABLE APP_SESSIONS(token_hash TEXT); INSERT INTO APP_SESSIONS VALUES('secret-session-hash');
             CREATE TABLE Account_Challenges(token_hash TEXT); INSERT INTO account_challenges VALUES('secret-link-hash');
             CREATE TABLE ENTRY_LINKS(token_hash TEXT); INSERT INTO entry_links VALUES('secret-entry-hash');
             CREATE TABLE API_KEYS(id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
               actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER, REVOKED_REASON TEXT);
             INSERT INTO api_keys VALUES('key-live', 'live-key-hash', 'Mirror', '[]', '[\"public\"]', 1, NULL, NULL);",
        )
        .unwrap();
        drop(db);
        assert_eq!(
            revoke_access(&path).unwrap(),
            4,
            "a session, two links and the live key"
        );
        let db = Connection::open(&path).unwrap();
        for table in ["app_sessions", "account_challenges", "entry_links"] {
            let rows: i64 = db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{table} still holds rows");
        }
        let live: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "the key is still live");
        std::fs::remove_dir_all(root).unwrap();
    }

    async fn round_trip(
        source: Arc<dyn file_store::FileStore>,
        target: Arc<dyn file_store::FileStore>,
        root: &Path,
    ) {
        let (a, b) = (
            "00000000-0000-4000-8000-00000000000a",
            "00000000-0000-4000-8000-00000000000b",
        );
        let deployment = Uuid::new_v4().to_string();
        let index_dir = crate::layout::files(&root, &deployment);
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
        let target_dir = crate::layout::files(&root, &arrival);
        let (from, to) = (
            file_store::BlockingFiles::new(source.clone(), &deployment),
            file_store::BlockingFiles::new(target.clone(), &arrival),
        );
        let (archive_path, index_path, target_path) =
            (archive.clone(), index_dir.clone(), target_dir.clone());
        tokio::task::spawn_blocking(move || {
            write_files_tar(&index_path, &archive_path, |id| from.open(id)).unwrap();
            let accounts = Connection::open_in_memory().unwrap();
            accounts.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('u1');").unwrap();
            assert_eq!(unpack_files(&archive_path, Some(&accounts), Some((&target_path, &to)), &Default::default()).unwrap(), 2);
        })
        .await
        .unwrap();
        for id in [a, b] {
            let sent = file_store::read_all(source.as_ref(), &deployment, id)
                .await
                .unwrap()
                .unwrap();
            let arrived = file_store::read_all(target.as_ref(), &arrival, id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(crate::digest(&arrived), crate::digest(&sent), "{id}");
        }
        let restored: Vec<Value> =
            serde_json::from_slice(&std::fs::read(target_dir.join("index.json")).unwrap()).unwrap();
        assert_eq!(restored[0]["ownerId"], "u1");
        assert_eq!(restored[1]["visibility"], "public");
        assert!(std::fs::read_dir(&target_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_travel_between_an_s3_and_a_local_installation_with_the_same_bytes() {
        let root = std::env::temp_dir().join(format!("konae-package-stores-{}", Uuid::new_v4()));
        let local: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let s3: Arc<dyn file_store::FileStore> = Arc::new(file_store::tests::test_s3_store().await);
        round_trip(s3.clone(), local.clone(), &root).await;
        round_trip(local.clone(), s3.clone(), &root).await;
        round_trip(local.clone(), local, &root).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_refuse_foreign_owners_extra_data_and_more_than_the_quota() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        let store: Arc<dyn file_store::FileStore> = Arc::new(file_store::LocalFolder::new(&root));
        let deployment = "source";
        let source = crate::layout::files(&root, &deployment);
        std::fs::create_dir_all(&source).unwrap();
        let (a, b) = (
            "00000000-0000-4000-8000-00000000000a",
            "00000000-0000-4000-8000-00000000000b",
        );
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
            write_files_tar(&source, &archive, |id| blobs.open(id)).unwrap();
            let strangers = Connection::open_in_memory().unwrap();
            strangers.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY);").unwrap();
            assert!(unpack_files(&archive, Some(&strangers), None, &Default::default()).unwrap_err().to_string().contains("does not contain"));
            assert!(unpack_files(&archive, None, None, &Default::default()).is_err());
            let accounts = Connection::open_in_memory().unwrap();
            accounts.execute_batch("CREATE TABLE app_users(id TEXT PRIMARY KEY); INSERT INTO app_users VALUES('u1');").unwrap();

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

            let mut damaged_index = index.clone();
            damaged_index[1]["sha256"] = json!("0".repeat(64));
            std::fs::write(source.join("index.json"), damaged_index.to_string()).unwrap();
            let damaged = root.join("wrong-digest.tar");
            write_files_tar(&source, &damaged, |id| blobs.open(id)).unwrap();
            assert!(unpack_files(&damaged, Some(&accounts), None, &Default::default())
                .unwrap_err().to_string().contains("indexed SHA-256"));

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

    fn file_with(root: &Path, name: &str, statements: &str, edit_type: Option<&str>) -> PathBuf {
        let path = root.join(name);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(statements).unwrap();
        if let Some(kind) = edit_type {
            db.execute_batch("PRAGMA writable_schema=ON;").unwrap();
            db.execute(
                "UPDATE sqlite_master SET type = ?1 WHERE type = 'trigger'",
                [kind],
            )
            .unwrap();
            db.execute_batch("PRAGMA writable_schema=OFF;").unwrap();
        }
        path
    }

    fn components(
        root: &Path,
        database: PathBuf,
        identity: Option<PathBuf>,
        usage: Option<PathBuf>,
    ) -> Vec<(String, PathBuf)> {
        let _ = root;
        let mut list = vec![("database".to_owned(), database)];
        list.extend(identity.map(|path| ("identity".to_owned(), path)));
        list.extend(usage.map(|path| ("usage".to_owned(), path)));
        list
    }

    fn accounts_with_hash(root: &Path, name: &str, hash: &str) -> PathBuf {
        let path = root.join(name);
        AuthService::open(&path).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO app_users (id, email, password_hash, created_at) VALUES ('u1', 'a@example.test', ?1, 0)", [hash])
            .unwrap();
        path
    }

    #[test]
    fn an_accounts_file_with_a_password_hash_this_kernel_did_not_write_is_refused() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = file_with(&root, "database.sqlite", "CREATE TABLE t(v)", None);
        let (salt, hash) = ("00".repeat(16), "00".repeat(32));
        let refused = "The accounts component holds a password hash this Kernel did not write.";
        for hash in [
            format!("pbkdf2-sha256$4294967295${salt}${hash}"),
            format!("pbkdf2-sha256$1${salt}${hash}"),
            "x".to_owned(),
        ] {
            let identity = accounts_with_hash(&root, "bad.sqlite", &hash);
            let error = check_components(
                &components(&root, database.clone(), Some(identity), None),
                &Default::default(),
            )
            .unwrap_err();
            assert_eq!(error.to_string(), refused, "{hash}");
            std::fs::remove_file(root.join("bad.sqlite")).unwrap();
        }
        let identity = root.join("identity.sqlite");
        let auth = AuthService::open(&identity).unwrap();
        auth.signup("ana@example.test", "ana's password").unwrap();
        drop(auth);
        let db = Connection::open(&identity).unwrap();
        db.execute("INSERT INTO app_users (id, email, password_hash, created_at, state) VALUES ('p1', 'p@example.test', ?1, 0, 'pending')", [auth::NO_PASSWORD]).unwrap();
        drop(db);
        check_components(
            &components(&root, database, Some(identity), None),
            &Default::default(),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_component_is_refused_when_it_holds_a_trigger_or_a_view_of_any_spelling() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let plain = file_with(&root, "database.sqlite", "CREATE TABLE t(v)", None);
        let trigger =
            "CREATE TABLE t(v); CREATE TRIGGER leak AFTER INSERT ON t BEGIN SELECT 1; END;";
        let upper = file_with(&root, "upper.sqlite", trigger, Some("TRIGGER"));
        let error = check_components(&components(&root, upper, None, None), &Default::default())
            .unwrap_err();
        assert!(error.to_string().contains("CREATE TRIGGER leak"), "{error}");

        let identity = root.join("identity.sqlite");
        AuthService::open(&identity).unwrap();
        Connection::open(&identity)
            .unwrap()
            .execute_batch("CREATE TRIGGER leak AFTER INSERT ON app_sessions BEGIN SELECT 1; END;")
            .unwrap();
        let error = check_components(
            &components(&root, plain.clone(), Some(identity), None),
            &Default::default(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("accounts component holds CREATE TRIGGER leak"),
            "{error}"
        );

        let usage = file_with(
            &root,
            "usage.sqlite",
            "CREATE TABLE u(v); CREATE VIEW seen AS SELECT v FROM u;",
            None,
        );
        let error = check_components(
            &components(&root, plain, None, Some(usage)),
            &Default::default(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("usage component holds CREATE VIEW seen"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_password_hash_column_is_checked_whatever_its_case() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = file_with(&root, "database.sqlite", "CREATE TABLE t(v)", None);
        let (salt, hash) = ("00".repeat(16), "00".repeat(32));
        for (table, column) in [
            ("app_users", "PASSWORD_HASH"),
            ("APP_USERS", "PASSWORD_HASH"),
            ("APP_USERS", "password_hash"),
        ] {
            let identity = root.join("upper.sqlite");
            let db = Connection::open(&identity).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (id TEXT PRIMARY KEY, email TEXT NOT NULL, {column} TEXT NOT NULL, created_at INTEGER NOT NULL)")).unwrap();
            db.execute("INSERT INTO app_users (id, email, password_hash, created_at) VALUES ('u1', 'a@example.test', ?1, 0)", [format!("pbkdf2-sha256$4294967295${salt}${hash}")]).unwrap();
            drop(db);
            let error = check_components(
                &components(&root, database.clone(), Some(identity.clone()), None),
                &Default::default(),
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "The accounts component holds a password hash this Kernel did not write.",
                "{table} {column}"
            );
            std::fs::remove_file(identity).unwrap();
        }
        let _ = std::fs::remove_dir_all(root);
    }

    fn damaged_fts(root: &Path, name: &str, base: &str) -> PathBuf {
        let path = file_with(
            root,
            name,
            &format!(
                "{base}; CREATE VIRTUAL TABLE notes USING fts5(body); INSERT INTO notes VALUES('one two three');"
            ),
            None,
        );
        let db = Connection::open(&path).unwrap();
        db.execute("UPDATE notes_data SET block = x'0001' WHERE id > 10", [])
            .unwrap();
        let problem: String = db
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_ne!(problem, "ok", "the fixture must fail the integrity check");
        path
    }

    #[test]
    fn a_generated_password_hash_column_is_scanned_too() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = file_with(&root, "database.sqlite", "CREATE TABLE t(v)", None);
        let (salt, hash) = ("00".repeat(16), "00".repeat(32));
        let identity = file_with(
            &root,
            "generated.sqlite",
            &format!(
                "CREATE TABLE app_users(id TEXT PRIMARY KEY, email TEXT NOT NULL, created_at INTEGER NOT NULL,
                   password_hash TEXT GENERATED ALWAYS AS ('pbkdf2-sha256$4294967295${salt}${hash}') VIRTUAL);
                 INSERT INTO app_users (id, email, created_at) VALUES ('u1', 'a@example.test', 0);"
            ),
            None,
        );
        let error = check_components(
            &components(&root, database.clone(), Some(identity), None),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The accounts component holds a password hash this Kernel did not write."
        );
        let bare = file_with(
            &root,
            "bare.sqlite",
            "CREATE TABLE app_users(id TEXT PRIMARY KEY, email TEXT)",
            None,
        );
        check_components(
            &components(&root, database, Some(bare), None),
            &Default::default(),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_virtual_table_is_refused_before_sqlite_runs_its_code_on_the_file() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let plain = file_with(&root, "plain.sqlite", "CREATE TABLE t(v)", None);
        let identity = damaged_fts(
            &root,
            "identity.sqlite",
            "CREATE TABLE app_users(id TEXT PRIMARY KEY, password_hash TEXT)",
        );
        let usage = damaged_fts(&root, "usage.sqlite", "CREATE TABLE u(v)");
        let database = damaged_fts(&root, "database.sqlite", "CREATE TABLE d(v)");
        let sentence = |what: &str| {
            format!(
                "The {what} holds CREATE VIRTUAL TABLE notes, and Konae runs no virtual tables. Drop it before importing."
            )
        };
        let error = check_components(
            &components(&root, database, None, None),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), sentence("database"));
        let error = check_components(
            &components(&root, plain.clone(), Some(identity), None),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), sentence("accounts component"));
        let error = check_components(
            &components(&root, plain, None, Some(usage)),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), sentence("usage component"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_component_is_refused_when_it_holds_a_virtual_table_and_a_kernel_file_still_imports() {
        let root = std::env::temp_dir().join(format!("konae-package-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let plain = file_with(&root, "database.sqlite", "CREATE TABLE t(v)", None);
        let identity = root.join("identity.sqlite");
        AuthService::open(&identity).unwrap();
        let usage = file_with(&root, "usage.sqlite", "CREATE TABLE u(v)", None);
        check_components(
            &components(
                &root,
                plain.clone(),
                Some(identity.clone()),
                Some(usage.clone()),
            ),
            &Default::default(),
        )
        .unwrap();

        let fts = file_with(
            &root,
            "fts.sqlite",
            "CREATE TABLE t(v); CREATE VIRTUAL TABLE notes USING fts5(body)",
            None,
        );
        let error =
            check_components(&components(&root, fts, None, None), &Default::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The database holds CREATE VIRTUAL TABLE notes, and Konae runs no virtual tables. Drop it before importing."
        );

        Connection::open(&identity)
            .unwrap()
            .execute_batch("CREATE VIRTUAL TABLE places USING rtree(id, x0, x1)")
            .unwrap();
        let error = check_components(
            &components(&root, plain.clone(), Some(identity), None),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The accounts component holds CREATE VIRTUAL TABLE places, and Konae runs no virtual tables. Drop it before importing."
        );

        Connection::open(&usage)
            .unwrap()
            .execute_batch("CREATE VIRTUAL TABLE seen USING fts5(body)")
            .unwrap();
        let error = check_components(
            &components(&root, plain, None, Some(usage)),
            &Default::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The usage component holds CREATE VIRTUAL TABLE seen, and Konae runs no virtual tables. Drop it before importing."
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
