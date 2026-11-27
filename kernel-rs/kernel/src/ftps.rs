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

use crate::{AppState, Deployment};

const SETTINGS_FILE: &str = "ftps.json";
const DEFAULT_MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;
const STAGING_DIR: &str = ".konae/ftps/staging";
const MAX_SESSIONS: usize = 32;
const MAX_RELATIVE_PATH_BYTES: usize = 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    bind: String,
    passive_ports: [u16; 2],
    #[serde(default)]
    passive_host: Option<String>,
    certificate: PathBuf,
    private_key: PathBuf,
    #[serde(default = "default_max_upload_bytes")]
    max_upload_bytes: u64,
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
    credential: Arc<Credential>,
    deployment: Arc<Deployment>,
}

impl fmt::Debug for BoundUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundUser")
            .field("username", &self.credential.username)
            .field("app_id", &self.credential.app_id)
            .field("source_id", &self.credential.source_id)
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

fn file_error(error: std::io::Error) -> FsError {
    FsError::from(error)
}

async fn stage(
    input: &mut (dyn AsyncRead + Send + Unpin),
    staging: &Path,
    max_bytes: u64,
) -> std::result::Result<(StagedFile, u64), FsError> {
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(file_error)?;
    let path = staging.join(format!("{}.part", Uuid::new_v4()));
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(file_error)?;
    let mut staged = StagedFile {
        path,
        file: Some(file),
    };
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
        let Some(credential) = self.credentials.get(username) else {
            return Err(LoginError::Incorrect);
        };
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
        Ok(BoundUser {
            credential: credential.clone(),
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
        let (staged, bytes) = stage(data, &self.staging, self.max_upload_bytes).await?;
        crate::file_intake::accept_staged(
            &self.state,
            &user.deployment,
            &user.credential.source_id,
            &staged.path,
            &relative_path,
            None,
        )
        .await
        .map_err(|error| FsError::Local(format!("Intake did not accept the upload: {error:#}")))?;
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
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    let backend = Arc::new(BackendState {
        state: state.clone(),
        credentials: Arc::new(credentials),
        staging,
        max_upload_bytes: settings.max_upload_bytes,
    });
    tokio::spawn(async move {
        eprintln!("FTPS listener is accepting explicit TLS connections at {address}.");
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
    let ports = settings.passive_ports;
    ensure!(
        ports[0] > 0 && ports[0] <= ports[1],
        "FTPS passivePorts must be an ordered range of non-zero ports."
    );
    ensure!(
        settings.max_upload_bytes > 0,
        "FTPS maxUploadBytes must be greater than zero."
    );
    ensure!(
        !settings.credentials.is_empty(),
        "FTPS settings need at least one credential."
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
        "credentials": settings.credentials.len(),
        "maxUploadBytes": settings.max_upload_bytes,
    }))
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
}
