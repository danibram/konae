use std::{
    collections::HashMap,
    fmt,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thirtyftp::{
    Backend, Config, Entry, Events, FsError, FtpPath, LoginContext, LoginError, PassiveHost,
    SessionEnd, TransferEnd, TransferInfo, serve,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod credentials;

use crate::{AppState, Deployment};

const SETTINGS_FILE: &str = "ftps.json";
const DEFAULT_MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;
const STAGING_DIR: &str = "transfers/ftps";
const MAX_SESSIONS: usize = 32;
const MAX_RELATIVE_PATH_BYTES: usize = 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    bind: String,
    passive_ports: [u16; 2],
    #[serde(default)]
    passive_host: Option<String>,
    #[serde(default)]
    advertised_host: Option<String>,
    #[serde(default)]
    advertised_port: Option<u16>,
    certificate: PathBuf,
    private_key: PathBuf,
    #[serde(default = "default_max_upload_bytes")]
    max_upload_bytes: u64,
    #[serde(default)]
    credentials: Vec<Credential>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credential {
    username: String,
    password: String,
    app_id: String,
    source_id: String,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("bind", &self.bind)
            .field("passive_ports", &self.passive_ports)
            .field("passive_host", &self.passive_host)
            .field("advertised_host", &self.advertised_host)
            .field("advertised_port", &self.advertised_port)
            .field("certificate", &self.certificate)
            .field("private_key", &"[redacted]")
            .field("max_upload_bytes", &self.max_upload_bytes)
            .field("credential_count", &self.credentials.len())
            .finish()
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("app_id", &self.app_id)
            .field("source_id", &self.source_id)
            .finish()
    }
}

fn default_max_upload_bytes() -> u64 {
    DEFAULT_MAX_UPLOAD_BYTES
}

#[derive(Clone)]
struct BoundUser {
    legacy: Option<Arc<Credential>>,
    managed: Option<credentials::Credential>,
    deployment: Arc<Deployment>,
}

impl fmt::Debug for BoundUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundUser")
            .field(
                "username",
                &self
                    .legacy
                    .as_ref()
                    .map(|credential| credential.username.as_str())
                    .or_else(|| {
                        self.managed
                            .as_ref()
                            .map(|credential| credential.username.as_str())
                    }),
            )
            .field("app_id", &self.deployment.id)
            .field(
                "destination",
                &self
                    .managed
                    .as_ref()
                    .map(|credential| credential.destination.as_str()),
            )
            .finish()
    }
}

#[derive(Clone)]
struct BackendState {
    state: AppState,
    credentials: Arc<HashMap<String, Arc<Credential>>>,
    staging: PathBuf,
    max_upload_bytes: u64,
}

impl fmt::Debug for BackendState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendState")
            .field("staging", &self.staging)
            .field("max_upload_bytes", &self.max_upload_bytes)
            .field("credential_count", &self.credentials.len())
            .finish()
    }
}

struct StagedFile {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

struct UploadReservation {
    root: PathBuf,
    app_id: String,
    credential_id: String,
    upload_id: String,
    finished: bool,
}

impl UploadReservation {
    fn finish(&mut self, accepted: bool) {
        self.finished = credentials::finish(
            &self.root,
            &self.app_id,
            &self.credential_id,
            &self.upload_id,
            accepted,
        )
        .is_ok();
    }
}

impl Drop for UploadReservation {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let root = std::mem::take(&mut self.root);
        let app = std::mem::take(&mut self.app_id);
        let credential = std::mem::take(&mut self.credential_id);
        let upload = std::mem::take(&mut self.upload_id);
        tokio::task::spawn_blocking(move || {
            let _ = credentials::finish(&root, &app, &credential, &upload, false);
        });
    }
}

impl StagedFile {
    fn create(staging: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(staging)?;
        let path = staging.join(format!("{}.part", Uuid::new_v4()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(tokio::fs::File::from_std(file)),
        })
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

fn normalize_device_path(path: &FtpPath) -> std::result::Result<String, FsError> {
    if path.is_root() {
        return Err(FsError::NameNotAllowed);
    }
    path.to_portable(Path::new("/"))
        .map_err(|_| FsError::NameNotAllowed)?;
    let components = path.components();
    if components.is_empty() {
        return Err(FsError::NameNotAllowed);
    }
    let relative = components.join("/");
    if relative.len() > MAX_RELATIVE_PATH_BYTES {
        return Err(FsError::NameNotAllowed);
    }
    Ok(relative)
}

fn destination_path(
    folder: Option<&str>,
    relative_path: &str,
) -> std::result::Result<String, FsError> {
    let Some(folder) = folder else {
        return Ok(relative_path.to_owned());
    };
    let prefix = Path::new(folder);
    if prefix.is_absolute()
        || prefix
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(FsError::NameNotAllowed);
    }
    let combined = prefix.join(relative_path);
    combined
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or(FsError::NameNotAllowed)
}

fn file_error(error: std::io::Error) -> FsError {
    FsError::from(error)
}

async fn stage(
    input: &mut (dyn AsyncRead + Send + Unpin),
    staging: &Path,
    max_bytes: u64,
) -> std::result::Result<(StagedFile, u64), FsError> {
    let staging = staging.to_owned();
    // The open task owns cleanup even if the transfer stops before it receives the file.
    let mut staged = tokio::task::spawn_blocking(move || StagedFile::create(&staging))
        .await
        .map_err(|error| FsError::Local(format!("Could not open FTPS staging file: {error}")))?
        .map_err(file_error)?;
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).await.map_err(|error| {
            FsError::Local(format!("FTPS data stream ended before completion: {error}"))
        })?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > max_bytes {
            return Err(FsError::QuotaExceeded);
        }
        staged
            .file
            .as_mut()
            .ok_or_else(|| FsError::Local("FTPS staging file was closed".into()))?
            .write_all(&buffer[..count])
            .await
            .map_err(file_error)?;
    }
    let mut file = staged
        .file
        .take()
        .ok_or_else(|| FsError::Local("FTPS staging file was closed".into()))?;
    file.flush().await.map_err(file_error)?;
    file.sync_all().await.map_err(file_error)?;
    drop(file);
    Ok((staged, total))
}

impl Backend for BackendState {
    type User = BoundUser;

    async fn login(
        &self,
        context: &LoginContext,
        username: &str,
        password: &str,
    ) -> std::result::Result<Self::User, LoginError> {
        if !context.encrypted {
            return Err(LoginError::Incorrect);
        }
        if let Some(credential) = self.credentials.get(username) {
            let supplied = Sha256::digest(password.as_bytes());
            let expected = Sha256::digest(credential.password.as_bytes());
            if !bool::from(supplied.ct_eq(&expected)) {
                return Err(LoginError::Incorrect);
            }
            let deployment = self
                .state
                .deployments
                .read()
                .await
                .values()
                .find(|deployment| deployment.id == credential.app_id)
                .cloned()
                .ok_or(LoginError::Unavailable)?;
            ensure_declared_source(&deployment, &credential.source_id)
                .map_err(|_| LoginError::Unavailable)?;
            return Ok(BoundUser {
                legacy: Some(credential.clone()),
                managed: None,
                deployment: Arc::new(deployment),
            });
        }
        let app_key = username.split('.').nth(1).ok_or(LoginError::Incorrect)?;
        let deployment = self
            .state
            .deployments
            .read()
            .await
            .values()
            .find(|deployment| deployment.id == app_key)
            .cloned()
            .ok_or(LoginError::Unavailable)?;
        let managed =
            credentials::verify(&self.state.data_root, &deployment.id, username, password)
                .map_err(|_| LoginError::Unavailable)?
                .ok_or(LoginError::Incorrect)?;
        let target =
            crate::ftps_declaration::published_destination(&deployment, &managed.destination)
                .ok_or(LoginError::Unavailable)?;
        let target_kind = match target.kind {
            crate::ftps_declaration::DestinationKind::Member => "member",
            crate::ftps_declaration::DestinationKind::Device => "device",
        };
        if target_kind != managed.kind {
            return Err(LoginError::Unavailable);
        }
        if managed.kind == "member" {
            if !crate::modules::is_on(&deployment.modules, "files") {
                return Err(LoginError::Incorrect);
            }
            let user_id = managed.user_id.as_deref().ok_or(LoginError::Incorrect)?;
            let accounts = crate::accounts::app_store(&self.state, &deployment)
                .map_err(|_| LoginError::Unavailable)?;
            if !accounts
                .is_active(user_id)
                .map_err(|_| LoginError::Unavailable)?
            {
                return Err(LoginError::Incorrect);
            }
        }
        Ok(BoundUser {
            legacy: None,
            managed: Some(managed),
            deployment: Arc::new(deployment),
        })
    }

    async fn metadata(
        &self,
        _user: &Self::User,
        path: &FtpPath,
    ) -> std::result::Result<Entry, FsError> {
        if path.is_root() {
            Ok(Entry::directory("/", None))
        } else {
            Err(FsError::PermissionDenied)
        }
    }

    async fn list(
        &self,
        _user: &Self::User,
        _path: &FtpPath,
    ) -> std::result::Result<Vec<Entry>, FsError> {
        Err(FsError::PermissionDenied)
    }

    async fn read(
        &self,
        _user: &Self::User,
        _path: &FtpPath,
        _offset: u64,
    ) -> std::result::Result<Box<dyn AsyncRead + Send + Unpin>, FsError> {
        Err(FsError::PermissionDenied)
    }

    async fn write(
        &self,
        user: &Self::User,
        path: &FtpPath,
        data: &mut (dyn AsyncRead + Send + Unpin),
    ) -> std::result::Result<u64, FsError> {
        let relative_path = normalize_device_path(path)?;
        if let Some(credential) = user
            .managed
            .as_ref()
            .filter(|credential| credential.kind == "member")
        {
            let upload_id = credential
                .upload_id
                .as_deref()
                .ok_or(FsError::PermissionDenied)?;
            let user_id = credential
                .user_id
                .as_deref()
                .ok_or(FsError::PermissionDenied)?;
            if crate::files::ftps_upload_file_id(
                &self.state.data_root,
                &user.deployment.id,
                user_id,
                upload_id,
            )
            .map_err(|error| {
                FsError::Local(format!("Could not check FTPS upload receipt: {error:#}"))
            })?
            .is_some()
            {
                return Err(FsError::PermissionDenied);
            }
        }
        let (staged, bytes) = stage(data, &self.staging, self.max_upload_bytes).await?;
        if let Some(legacy) = &user.legacy {
            crate::file_intake::accept_staged(
                &self.state,
                &user.deployment,
                &legacy.source_id,
                &staged.path,
                &relative_path,
                None,
            )
            .await
            .map_err(|error| {
                FsError::Local(format!("Intake did not accept the upload: {error:#}"))
            })?;
        } else if let Some(credential) = &user.managed {
            let declaration = crate::ftps_declaration::published_destination(
                &user.deployment,
                &credential.destination,
            )
            .ok_or_else(|| FsError::PermissionDenied)?;
            let expected_kind = match declaration.kind {
                crate::ftps_declaration::DestinationKind::Member => "member",
                crate::ftps_declaration::DestinationKind::Device => "device",
            };
            if credential.kind != expected_kind {
                return Err(FsError::PermissionDenied);
            }
            let relative_path = destination_path(declaration.folder.as_deref(), &relative_path)?;
            let upload_id = credential
                .upload_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let root = self.state.data_root.clone();
            let app_id = user.deployment.id.clone();
            let owner_id = credential.user_id.clone();
            if !credentials::reserve(
                &root,
                &app_id,
                &credential.id,
                credential.generation,
                &upload_id,
                owner_id.as_deref(),
            )
            .map_err(|error| FsError::Local(format!("Could not reserve FTPS upload: {error:#}")))?
            {
                return Err(FsError::PermissionDenied);
            }
            let mut reservation = UploadReservation {
                root: root.clone(),
                app_id: app_id.clone(),
                credential_id: credential.id.clone(),
                upload_id: upload_id.clone(),
                finished: false,
            };
            let auth_root = root.clone();
            let auth_app = app_id.clone();
            let auth_credential = credential.clone();
            let auth_upload = upload_id.clone();
            let auth_state = self.state.clone();
            let auth_deployment = user.deployment.clone();
            let auth_destination = credential.destination.clone();
            let auth_kind = credential.kind.clone();
            let auth_folder = declaration.folder.clone();
            let authorize = move || -> Result<()> {
                ensure!(
                    credentials::authorized(&auth_root, &auth_app, &auth_credential, &auth_upload)?,
                    "FTPS credential was revoked or rotated."
                );
                ensure!(
                    crate::maintenance::pause(&auth_state.data_root, &auth_app).is_none(),
                    "FTPS uploads are paused while the app is in maintenance."
                );
                let current = auth_state
                    .deployments
                    .try_read()
                    .ok()
                    .and_then(|deployments| {
                        deployments
                            .values()
                            .find(|current| {
                                current.id == auth_app
                                    && current.project_id == auth_deployment.project_id
                            })
                            .cloned()
                    })
                    .context("FTPS upload Deployment is no longer active.")?;
                ensure!(
                    current.artifact_version == auth_deployment.artifact_version,
                    "FTPS upload Deployment changed during the transfer."
                );
                let current_destination =
                    crate::ftps_declaration::published_destination(&current, &auth_destination)
                        .context("FTPS destination is no longer published.")?;
                ensure!(
                    current_destination.folder == auth_folder,
                    "FTPS destination folder changed during the transfer."
                );
                let current_kind = match current_destination.kind {
                    crate::ftps_declaration::DestinationKind::Member => "member",
                    crate::ftps_declaration::DestinationKind::Device => "device",
                };
                ensure!(
                    current_kind == auth_kind,
                    "FTPS destination kind changed during the transfer."
                );
                if let Some(owner_id) = auth_credential.user_id.as_deref() {
                    ensure!(
                        crate::modules::is_on(&current.modules, "files"),
                        "The Files module is off."
                    );
                    ensure!(
                        crate::accounts::app_store(&auth_state, &current)?.is_active(owner_id)?,
                        "Only an active app account can upload FTPS files."
                    );
                }
                Ok(())
            };
            let accepted = match expected_kind {
                "member" => crate::files::accept_ftps_staged(
                    &self.state,
                    &user.deployment,
                    &upload_id,
                    owner_id.as_deref().unwrap_or_default(),
                    &relative_path,
                    &staged.path,
                    authorize,
                )
                .await
                .map(|_| ()),
                _ => crate::file_intake::accept_ftps_staged(
                    &self.state,
                    &user.deployment,
                    &credential.destination,
                    &staged.path,
                    &relative_path,
                    &upload_id,
                    authorize,
                )
                .await
                .map(|_| ()),
            };
            reservation.finish(accepted.is_ok());
            accepted.map_err(|error| {
                FsError::Local(format!("FTPS destination refused the upload: {error:#}"))
            })?;
        }
        Ok(bytes)
    }

    async fn make_dir(
        &self,
        _user: &Self::User,
        _path: &FtpPath,
    ) -> std::result::Result<(), FsError> {
        Err(FsError::PermissionDenied)
    }

    async fn change_dir(
        &self,
        _user: &Self::User,
        path: &FtpPath,
    ) -> std::result::Result<(), FsError> {
        if path.is_root() {
            Ok(())
        } else {
            normalize_device_path(path).map(|_| ())
        }
    }
}

#[derive(Debug)]
struct OperatorEvents;

impl Events for OperatorEvents {
    fn signed_in(&self, peer: std::net::SocketAddr, username: &str) {
        eprintln!("FTPS authenticated session from {peer} as {username}.");
    }

    fn login_failed(&self, peer: std::net::SocketAddr, username: &str) {
        eprintln!("FTPS authentication failed for {username} from {peer}.");
    }

    fn transfer_started(
        &self,
        peer: std::net::SocketAddr,
        username: &str,
        transfer: &TransferInfo,
    ) {
        eprintln!(
            "FTPS {:?} started for {username} at {} from {peer}.",
            transfer.direction, transfer.path
        );
    }

    fn transfer_ended(
        &self,
        peer: std::net::SocketAddr,
        username: &str,
        transfer: &TransferInfo,
        end: &TransferEnd,
    ) {
        eprintln!(
            "FTPS {:?} ended for {username} at {} from {peer}: {end:?}.",
            transfer.direction, transfer.path
        );
    }
}

fn tls_config(certificate: &Path, private_key: &Path) -> Result<rustls::ServerConfig> {
    let certificates = CertificateDer::pem_file_iter(certificate)
        .with_context(|| format!("Could not read FTPS certificate {}", certificate.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        !certificates.is_empty(),
        "FTPS certificate {} contains no certificates.",
        certificate.display()
    );
    let key = PrivateKeyDer::from_pem_file(private_key)
        .with_context(|| format!("Could not read FTPS private key {}", private_key.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .context("FTPS certificate and private key do not match")?;
    Ok(config)
}

fn session_config(settings: &Settings, tls: Arc<rustls::ServerConfig>) -> Result<Config> {
    let passive_host = match settings.passive_host.as_deref() {
        None => PassiveHost::FromConnection,
        Some(value) => match value.parse::<std::net::Ipv4Addr>() {
            Ok(address) => PassiveHost::Ip(address),
            Err(_) => PassiveHost::Dns(value.to_owned()),
        },
    };
    let mut config = Config::default();
    config.tls = Some(tls);
    config.require_tls = true;
    config.passive_ports = settings.passive_ports[0]..=settings.passive_ports[1];
    config.passive_host = passive_host;
    config.limits.max_transfer_time = Some(Duration::from_secs(3600));
    config.limits.tls_handshake_timeout = Duration::from_secs(15);
    config.limits.data_accept_timeout = Duration::from_secs(30);
    Ok(config)
}

pub struct Runtime {
    shutdown: CancellationToken,
}

impl Runtime {
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Loads installation-only FTPS settings and starts the optional listener.
pub async fn start(state: &AppState) -> Result<Option<Runtime>> {
    let settings_path = state.data_root.join(SETTINGS_FILE);
    let bytes = match tokio::fs::read(&settings_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "FTPS listener disabled. No {} file exists.",
                settings_path.display()
            );
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not read {}", settings_path.display()));
        }
    };
    ensure_settings_private(&settings_path)?;
    let settings: Settings = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{} is not valid FTPS settings JSON",
            settings_path.display()
        )
    })?;
    validate(&settings)?;
    let certificate = installation_path(&state.data_root, &settings.certificate)?;
    let private_key = installation_path(&state.data_root, &settings.private_key)?;
    ensure!(
        certificate.is_file(),
        "FTPS certificate {} does not exist.",
        certificate.display()
    );
    ensure!(
        private_key.is_file(),
        "FTPS private key {} does not exist.",
        private_key.display()
    );
    ensure_private_key(&private_key)?;
    let tls = Arc::new(tls_config(&certificate, &private_key)?);
    let config = Arc::new(session_config(&settings, tls)?);

    let deployments = state.deployments.read().await;
    let mut credentials = HashMap::new();
    for credential in &settings.credentials {
        let deployment = deployments
            .values()
            .find(|deployment| deployment.id == credential.app_id)
            .with_context(|| {
                format!(
                    "FTPS credential {} names app {}, which is not served by this installation",
                    credential.username, credential.app_id
                )
            })?;
        ensure_declared_source(deployment, &credential.source_id).with_context(|| {
            format!(
                "FTPS credential {} must bind to a declared intake source",
                credential.username
            )
        })?;
        credentials.insert(credential.username.clone(), Arc::new(credential.clone()));
    }
    for deployment in deployments.values() {
        credentials::clear_stopped_reservations(&state.data_root, &deployment.id)?;
    }
    drop(deployments);

    let staging = state.data_root.join(STAGING_DIR);
    tokio::fs::create_dir_all(&staging).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let canonical_root = tokio::fs::canonicalize(&state.data_root).await?;
    let canonical_staging = tokio::fs::canonicalize(&staging).await?;
    ensure!(
        canonical_staging.starts_with(&canonical_root),
        "FTPS staging must stay inside the installation data directory."
    );
    ensure!(
        !canonical_staging.starts_with(canonical_root.join("apps")),
        "FTPS staging must stay outside app folders."
    );
    clear_staging(&staging).await?;

    let listener = TcpListener::bind(&settings.bind).await.with_context(|| {
        format!(
            "Could not bind the FTPS control listener at {}",
            settings.bind
        )
    })?;
    let address = listener.local_addr()?;
    let (advertised_host, advertised_port) = advertised_endpoint(&settings, address);
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    let backend = Arc::new(BackendState {
        state: state.clone(),
        credentials: Arc::new(credentials),
        staging,
        max_upload_bytes: settings.max_upload_bytes,
    });
    tokio::spawn(async move {
        eprintln!(
            "FTPS listener is accepting explicit TLS connections at {address}, advertised as {advertised_host}:{advertised_port}."
        );
        let sessions_limit = Arc::new(Semaphore::new(MAX_SESSIONS));
        let mut sessions = JoinSet::new();
        loop {
            tokio::select! {
                _ = signal.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        let permit = match sessions_limit.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                eprintln!("FTPS connection from {peer} refused because all {MAX_SESSIONS} sessions are busy.");
                                drop(stream);
                                continue;
                            }
                        };
                        let backend = backend.clone();
                        let config = config.clone();
                        sessions.spawn(async move {
                            let _permit = permit;
                            match serve(stream, peer, config, backend, Arc::new(OperatorEvents)).await {
                                SessionEnd::Io(error) => eprintln!("FTPS connection from {peer} ended with I/O error: {error}"),
                                SessionEnd::TlsHandshakeFailed => eprintln!("FTPS TLS handshake failed for {peer}."),
                                SessionEnd::IdleTimeout => eprintln!("FTPS connection from {peer} timed out while idle."),
                                SessionEnd::LoginTimeout => eprintln!("FTPS connection from {peer} timed out before login."),
                                SessionEnd::Quit | SessionEnd::ClientClosed => {}
                                end => eprintln!("FTPS connection from {peer} ended: {end:?}."),
                            }
                        });
                    }
                    Err(error) => eprintln!("FTPS listener accept failed: {error}"),
                },
                Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                    if let Err(error) = result {
                        eprintln!("FTPS session task failed: {error}");
                    }
                }
            }
        }
        let deadline = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(deadline);
        while !sessions.is_empty() {
            tokio::select! {
                _ = &mut deadline => {
                    sessions.abort_all();
                    break;
                }
                _ = sessions.join_next() => {}
            }
        }
        eprintln!("FTPS listener stopped.");
    });
    Ok(Some(Runtime { shutdown }))
}

fn ensure_declared_source(deployment: &Deployment, source_id: &str) -> Result<()> {
    let sources = deployment
        .manifest
        .get("fileIntake")
        .and_then(|value| value.get("sources"))
        .and_then(serde_json::Value::as_array)
        .context("the app does not declare intake sources")?;
    ensure!(
        sources
            .iter()
            .any(|source| source.get("id").and_then(serde_json::Value::as_str) == Some(source_id)),
        "sourceId {source_id} is not declared by app {}",
        deployment.id
    );
    Ok(())
}

fn validate(settings: &Settings) -> Result<()> {
    ensure!(
        !settings.bind.is_empty(),
        "FTPS bind address cannot be empty."
    );
    ensure!(
        settings
            .advertised_host
            .as_deref()
            .is_none_or(|host| !host.trim().is_empty()),
        "FTPS advertisedHost cannot be empty."
    );
    ensure!(
        settings.advertised_port.is_none_or(|port| port > 0),
        "FTPS advertisedPort must be a non-zero port."
    );
    let ports = settings.passive_ports;
    ensure!(
        ports[0] > 0 && ports[0] <= ports[1],
        "FTPS passivePorts must be an ordered range of non-zero ports."
    );
    ensure!(
        settings.max_upload_bytes > 0,
        "FTPS maxUploadBytes must be greater than zero."
    );
    let mut usernames = std::collections::HashSet::new();
    for credential in &settings.credentials {
        ensure!(
            !credential.username.is_empty(),
            "FTPS usernames cannot be empty."
        );
        ensure!(
            !credential.password.is_empty(),
            "FTPS password for {} cannot be empty.",
            credential.username
        );
        ensure!(
            !credential.app_id.is_empty(),
            "FTPS appId for {} cannot be empty.",
            credential.username
        );
        ensure!(
            !credential.source_id.is_empty(),
            "FTPS sourceId for {} cannot be empty.",
            credential.username
        );
        ensure!(
            usernames.insert(&credential.username),
            "FTPS username {} is duplicated.",
            credential.username
        );
    }
    Ok(())
}

fn advertised_endpoint(settings: &Settings, bound: std::net::SocketAddr) -> (String, u16) {
    let host = settings.advertised_host.clone().unwrap_or_else(|| {
        if bound.ip().is_unspecified() {
            "localhost".to_owned()
        } else {
            bound.ip().to_string()
        }
    });
    (host, settings.advertised_port.unwrap_or(bound.port()))
}

fn installation_path(root: &Path, value: &Path) -> Result<PathBuf> {
    ensure!(
        !value.is_absolute(),
        "FTPS TLS paths must be relative to the installation data directory."
    );
    let mut path = root.to_path_buf();
    for component in value.components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => bail!("FTPS TLS paths must stay inside the installation data directory."),
        }
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_path = std::fs::canonicalize(path)?;
    ensure!(
        canonical_path.starts_with(&canonical_root),
        "FTPS TLS files must stay inside the installation data directory."
    );
    ensure!(
        !canonical_path.starts_with(canonical_root.join("apps")),
        "FTPS TLS files must stay outside app folders."
    );
    Ok(canonical_path)
}

fn ensure_private_key(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        ensure!(
            mode & 0o077 == 0,
            "FTPS private key {} must be readable only by its owner.",
            path.display()
        );
    }
    Ok(())
}

fn ensure_settings_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        ensure!(
            mode & 0o077 == 0,
            "{} must be readable only by its owner. Run chmod 600 {}.",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

async fn clear_staging(staging: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(staging).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file()
            && entry.file_name().to_string_lossy().ends_with(".part")
        {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

pub fn configured_status(data_root: &Path) -> Result<serde_json::Value> {
    let path = data_root.join(SETTINGS_FILE);
    if !path.exists() {
        return Ok(serde_json::json!({"enabled": false}));
    }
    ensure_settings_private(&path)?;
    let bytes = std::fs::read(&path)?;
    let settings: Settings = serde_json::from_slice(&bytes)?;
    validate(&settings)?;
    Ok(serde_json::json!({
        "enabled": true,
        "bind": settings.bind,
        "passivePorts": settings.passive_ports,
        "advertisedHost": settings.advertised_host,
        "advertisedPort": settings.advertised_port,
        "credentials": settings.credentials.len(),
        "maxUploadBytes": settings.max_upload_bytes,
    }))
}

fn endpoint_for_credentials(data_root: &Path) -> Result<(String, u16)> {
    let path = data_root.join(SETTINGS_FILE);
    ensure!(path.is_file(), "FTPS listener is not configured.");
    ensure_settings_private(&path)?;
    let settings: Settings = serde_json::from_slice(&std::fs::read(path)?)?;
    validate(&settings)?;
    let bound = settings
        .bind
        .parse::<std::net::SocketAddr>()
        .context("FTPS bind must be an IP socket address to derive advertised defaults.")?;
    Ok(advertised_endpoint(&settings, bound))
}

fn credential_result(issued: credentials::Issued, host: &str, port: u16) -> Value {
    let credential = issued.credential;
    let mut result = json!({
        "credentialId": credential.id,
        "host": host,
        "port": port,
        "username": credential.username,
        "password": issued.password,
    });
    if let Some(expires_at) = credential.expires_at {
        result["expiresAt"] = json!(expires_at);
    }
    if let Some(upload_id) = issued.upload_id {
        result["uploadId"] = json!(upload_id);
    }
    result
}

fn declaration_destinations(deployment: &Deployment) -> Value {
    deployment
        .manifest
        .get("ftps")
        .and_then(|ftps| ftps.get("destinations"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn owner_list(state: &AppState, deployment: &Deployment) -> Result<Value> {
    Ok(json!({
        "projectId": deployment.project_id,
        "destinations": declaration_destinations(deployment),
        "credentials": credentials::list(&state.data_root, &deployment.id)?,
    }))
}

async fn active_deployment(state: &AppState, observed: &Deployment) -> Result<Deployment> {
    state
        .deployments
        .read()
        .await
        .values()
        .find(|current| {
            current.id == observed.id
                && current.project_id == observed.project_id
                && current.artifact_version == observed.artifact_version
        })
        .cloned()
        .context("Deployment changed or stopped while handling FTPS credentials.")
}

/// Issues one short-lived FTPS ticket for an authenticated app account.
pub async fn issue_member_upload(
    state: &AppState,
    deployment: &Deployment,
    user_id: &str,
    destination: &str,
) -> Result<Value> {
    ensure!(
        !user_id.is_empty(),
        "An app account is required for FTPS uploads."
    );
    let (host, port) = endpoint_for_credentials(&state.data_root)?;
    let _lock = crate::database_lock(state, &deployment.id).await;
    let active = active_deployment(state, deployment).await?;
    crate::modules::require_on(&active.modules, "files")?;
    let target = crate::ftps_declaration::published_destination(&active, destination)
        .context("FTPS destination not found.")?;
    ensure!(
        matches!(
            target.kind,
            crate::ftps_declaration::DestinationKind::Member
        ),
        "FTPS destination requires an app user."
    );
    let _identity = crate::database_lock(state, &format!("identity:{}", active.id)).await;
    ensure!(
        crate::accounts::app_store(state, &active)?.is_active(user_id)?,
        "Only an active app account can request an FTPS upload."
    );
    let issued = credentials::issue_member(
        &state.data_root,
        &active.id,
        &active.id,
        destination,
        user_id,
    )?;
    ensure!(
        credentials::verify(
            &state.data_root,
            &active.id,
            &issued.credential.username,
            &issued.password
        )?
        .is_some(),
        "FTPS credential read-back failed."
    );
    Ok(credential_result(issued, &host, port))
}

/// Reads a member ticket's status from the Files receipt, which is authoritative after a crash.
pub async fn member_upload_status(
    state: &AppState,
    deployment: &Deployment,
    user_id: &str,
    upload_id: &str,
) -> Result<Value> {
    let _lock = crate::database_lock(state, &deployment.id).await;
    let active = active_deployment(state, deployment).await?;
    ensure!(
        credentials::ticket_status(&state.data_root, upload_id, &active.id, user_id)?.is_some(),
        "FTPS upload not found."
    );
    crate::files::reconcile_ftps_uploads(state, &active).await?;
    let (expired, consumed) =
        credentials::ticket_status(&state.data_root, upload_id, &active.id, user_id)?
            .context("FTPS upload not found.")?;
    let file_id =
        crate::files::ftps_upload_file_id(&state.data_root, &active.id, user_id, upload_id)?;
    if file_id.is_some() {
        let _ = credentials::mark_receipt(&state.data_root, &active.id, upload_id)?;
        return Ok(json!({"uploadId": upload_id, "status": "complete", "fileId": file_id}));
    }
    if consumed {
        return Ok(json!({"uploadId": upload_id, "status": "complete"}));
    }
    if expired {
        return Ok(json!({"uploadId": upload_id, "status": "expired"}));
    }
    Ok(json!({"uploadId": upload_id, "status": "pending"}))
}

/// Runs owner operations after the MCP layer has checked manage_users.
pub async fn owner_credentials_call(
    state: &AppState,
    deployment: &Deployment,
    tool: &str,
    input: &Value,
) -> Result<Value> {
    ensure!(
        input.get("projectId").and_then(Value::as_str) == Some(deployment.project_id.as_str()),
        "Project not found or access denied."
    );
    match tool {
        "ftps_credentials_list" => owner_list(state, &active_deployment(state, deployment).await?),
        "ftps_device_create" => {
            let destination = input["destination"]
                .as_str()
                .context("destination is required.")?;
            let label = input["label"].as_str().context("label is required.")?;
            ensure!(
                !label.trim().is_empty() && label.len() <= 80,
                "FTPS device label must contain 1 to 80 bytes."
            );
            let (host, port) = endpoint_for_credentials(&state.data_root)?;
            let _lock = crate::database_lock(state, &deployment.id).await;
            let active = active_deployment(state, deployment).await?;
            let target = crate::ftps_declaration::published_destination(&active, destination)
                .context("FTPS destination not found.")?;
            ensure!(
                matches!(
                    target.kind,
                    crate::ftps_declaration::DestinationKind::Device
                ),
                "FTPS destination requires an app account."
            );
            let issued = credentials::create_device(
                &state.data_root,
                &active.id,
                &active.id,
                destination,
                label,
            )?;
            let response = credential_result(issued, &host, port);
            ensure!(
                owner_list(state, &active)?["credentials"]
                    .as_array()
                    .is_some_and(|items| items
                        .iter()
                        .any(|item| item["credentialId"] == response["credentialId"])),
                "FTPS credential read-back failed."
            );
            Ok(response)
        }
        "ftps_device_rotate" => {
            let id = input["credentialId"]
                .as_str()
                .context("credentialId is required.")?;
            let (host, port) = endpoint_for_credentials(&state.data_root)?;
            let _lock = crate::database_lock(state, &deployment.id).await;
            let active = active_deployment(state, deployment).await?;
            let issued = credentials::rotate_device(&state.data_root, &active.id, &active.id, id)?;
            let response = credential_result(issued, &host, port);
            ensure!(
                owner_list(state, &active)?["credentials"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["credentialId"] == id)),
                "FTPS credential read-back failed."
            );
            Ok(response)
        }
        "ftps_credential_revoke" => {
            let id = input["credentialId"]
                .as_str()
                .context("credentialId is required.")?;
            let _lock = crate::database_lock(state, &deployment.id).await;
            let active = active_deployment(state, deployment).await?;
            ensure!(
                credentials::revoke(&state.data_root, &active.id, id)?,
                "FTPS credential not found."
            );
            let readback = owner_list(state, &active)?;
            ensure!(
                !readback["credentials"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["credentialId"] == id)),
                "FTPS credential revocation did not persist."
            );
            Ok(json!({"credentialId": id, "revoked": true}))
        }
        _ => anyhow::bail!("Unknown FTPS credential operation."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_device_paths_and_rejects_root() {
        let path = FtpPath::resolve(&FtpPath::root(), "/meter/2026/read.csv").unwrap();
        assert_eq!(normalize_device_path(&path).unwrap(), "meter/2026/read.csv");
        assert!(normalize_device_path(&FtpPath::root()).is_err());
        let path = FtpPath::resolve(&FtpPath::root(), "/../../outside.csv").unwrap();
        assert_eq!(normalize_device_path(&path).unwrap(), "outside.csv");
    }

    #[test]
    fn settings_debug_does_not_include_secrets() {
        let settings = Settings {
            bind: "127.0.0.1:2121".into(),
            passive_ports: [31000, 31010],
            passive_host: None,
            advertised_host: None,
            advertised_port: None,
            certificate: "cert.pem".into(),
            private_key: "secret-key.pem".into(),
            max_upload_bytes: 100,
            credentials: vec![Credential {
                username: "meter".into(),
                password: "password-secret".into(),
                app_id: "app_meter".into(),
                source_id: "meter-source".into(),
            }],
        };
        assert!(!format!("{settings:?}").contains("password-secret"));
        assert!(!format!("{settings:?}").contains("secret-key.pem"));
    }

    #[test]
    fn legacy_settings_allow_optional_advertised_address() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "bind": "127.0.0.1:2121",
            "passivePorts": [31000, 31010],
            "certificate": "cert.pem",
            "privateKey": "key.pem",
            "credentials": [{
                "username": "meter",
                "password": "secret",
                "appId": "app_meter",
                "sourceId": "meter-source"
            }]
        }))
        .unwrap();
        validate(&settings).unwrap();
        assert_eq!(settings.advertised_host, None);
        assert_eq!(settings.advertised_port, None);

        let invalid = Settings {
            advertised_host: Some("  ".into()),
            ..settings
        };
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn advertised_address_defaults_to_loopback_for_wildcard_bind() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "bind": "0.0.0.0:2121",
            "passivePorts": [31000, 31010],
            "certificate": "cert.pem",
            "privateKey": "key.pem",
            "credentials": [{
                "username": "meter",
                "password": "secret",
                "appId": "app_meter",
                "sourceId": "meter-source"
            }]
        }))
        .unwrap();
        assert_eq!(
            advertised_endpoint(&settings, "0.0.0.0:2121".parse().unwrap()),
            ("localhost".into(), 2121)
        );
        assert_eq!(
            advertised_endpoint(&settings, "127.0.0.1:2121".parse().unwrap()),
            ("127.0.0.1".into(), 2121)
        );
    }

    #[test]
    fn settings_allow_only_database_managed_credentials() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "bind": "127.0.0.1:2121",
            "passivePorts": [31000, 31010],
            "certificate": "cert.pem",
            "privateKey": "key.pem"
        }))
        .unwrap();
        validate(&settings).unwrap();
        assert!(settings.credentials.is_empty());
    }
}
