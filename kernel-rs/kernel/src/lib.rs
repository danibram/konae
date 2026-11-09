use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::{Client as S3Client, config::Builder as S3ConfigBuilder, primitives::ByteStream};
use axum::{
    Json, Router, ServiceExt,
    body::Body,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower::ServiceBuilder;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

pub mod accounts;
pub mod action_shape;
pub mod app_db;
pub mod artifact;
pub mod auth;
pub mod calls;
pub mod copies;
pub mod domain;
pub mod enter;
pub mod file_store;
pub mod files;
pub mod folder;
pub mod google;
pub mod guard;
pub mod hosts;
pub mod import;
pub mod installation;
pub mod http;
pub mod invitations;
pub mod layout;
pub mod limits;
pub mod mail;
pub mod maintenance;
pub mod notify;
pub mod operator;
pub mod package;
pub mod path;
pub mod path_name;
pub mod provider;
pub mod recovery;
pub mod refusal;
pub mod runtime;
pub mod schedule;
pub mod schema_change;
pub mod security;
pub mod sources;
pub mod sql_rules;
pub mod transfers;
#[cfg(test)]
pub mod sqlite_bench;
pub mod usage;
use auth::{AuthError, AuthService, AuthUser};
use limits::{Guards, trusted_proxies};

#[derive(Clone)]
pub struct AppState {
    pub deployments: Arc<RwLock<HashMap<String, Deployment>>>,
    pub db_locks: LockMap,
    pub s3: Option<S3Client>,
    pub artifact_bucket: String,
    pub data_bucket: String,
    pub data_root: PathBuf,
    /// Where file bytes live and how many a Deployment may hold.
    pub files: file_store::Files,
    pub copies: Arc<copies::CopyStatus>,
    pub public_origin: String,
    /// The origins the installation answers on itself besides the public
    /// origin, such as where Konae's MCP answers. The first is where a
    /// client signs in.
    pub own_origins: Vec<String>,
    pub host_suffix: String,
    /// `<scheme>://<domain>` where path addresses answer; `None` without `KONAE_DOMAIN`.
    pub path_origin: Option<String>,
    pub guards: Arc<Guards>,
    pub accounts: accounts::AccountSettings,
    /// The stores at the data root besides the app folders.
    pub installation: Arc<installation::Installation>,
    /// Where an app user's file uploads and deletes are recorded.
    pub audit: Option<runtime::AuditSink>,
}

#[cfg(any(test, feature = "testing"))]
impl AppState {
    /// A Kernel on `root` with no Deployments, local files, no mail and the
    /// `localhost` names. A test changes the fields it needs with `..`.
    pub fn for_test(root: &Path) -> Self {
        Self {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            files: file_store::Files::local(root),
            data_root: root.into(),
            copies: Default::default(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            own_origins: Vec::new(),
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
            installation: Arc::new(installation::Installation::for_test()),
            audit: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    /// The app's ID. It names the app's folder, and the project in Konae.
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub slug: String,
    pub url: String,
    pub created_at: u64,
    pub manifest: Value,
    pub hostname: String,
    pub actions: String,
    /// The release whose files `dist/` holds. It tags them for browser caches.
    pub artifact_version: String,
    /// Who may create an account.
    pub signup_policy: SignupPolicy,
    /// How people sign in. Password always; Google when the owner turned it on.
    pub sign_in_methods: Vec<SignInMethod>,
    /// Where `"notify": { "to": "owner" }` sends. A publish from Konae fills
    /// it with the owner's email; the Kernel keeps no record of owners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_email: Option<String>,
}

/// `konae.json`: the app's settings, the part of an app an export hands over
/// and `konae init` writes.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub visibility: String,
    pub signup_policy: SignupPolicy,
    pub sign_in_methods: Vec<SignInMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_email: Option<String>,
    /// The version of the Kernel that wrote the file.
    pub kernel: String,
    /// The Capsule manifest without its visibility: routing, connects and the declared actions.
    pub manifest: Value,
}

/// `.konae/deployment.json`: what this installation adds to the settings to
/// serve the app. It never leaves the installation.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub hostname: String,
    pub created_at: u64,
    /// The `actions.json` of the release `dist/` holds.
    pub actions: String,
    pub artifact_version: String,
}

pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Settings {
    pub fn new(id: &str, name: &str, slug: &str, manifest: &Value) -> Self {
        let visibility = manifest["visibility"].as_str().unwrap_or("private").to_owned();
        let mut manifest = manifest.clone();
        if let Some(object) = manifest.as_object_mut() {
            object.remove("visibility");
        }
        Self {
            id: id.into(),
            name: name.into(),
            slug: slug.into(),
            signup_policy: SignupPolicy::initial(&visibility),
            visibility,
            sign_in_methods: default_sign_in_methods(),
            contact_email: None,
            kernel: KERNEL_VERSION.into(),
            manifest,
        }
    }

    pub fn of(deployment: &Deployment) -> Self {
        Self {
            signup_policy: deployment.signup_policy.clone(),
            sign_in_methods: deployment.sign_in_methods.clone(),
            contact_email: deployment.contact_email.clone(),
            ..Self::new(&deployment.id, &deployment.project_name, &deployment.slug, &deployment.manifest)
        }
    }

    pub fn read(root: &Path, id: &str) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(layout::settings(root, id))?)?)
    }

    pub async fn write(&self, root: &Path) -> Result<()> {
        write_atomic(&layout::settings(root, &self.id), &serde_json::to_vec_pretty(self)?).await?;
        Ok(())
    }

    pub fn deployment(self, installed: Installed, public_origin: &str) -> Deployment {
        let mut manifest = self.manifest;
        if let Some(object) = manifest.as_object_mut() {
            object.insert("visibility".into(), Value::String(self.visibility));
        }
        Deployment {
            project_id: self.id.clone(),
            id: self.id,
            project_name: self.name,
            slug: self.slug,
            url: deployment_url(public_origin, &installed.hostname),
            created_at: installed.created_at,
            manifest,
            hostname: installed.hostname,
            actions: installed.actions,
            artifact_version: installed.artifact_version,
            signup_policy: self.signup_policy,
            sign_in_methods: self.sign_in_methods,
            contact_email: self.contact_email,
        }
    }
}

/// A way a person signs in to a Deployment's app accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInMethod {
    Password,
    Google,
}

pub fn default_sign_in_methods() -> Vec<SignInMethod> {
    vec![SignInMethod::Password]
}

pub const PASSWORD_REQUIRED: &str = "Password sign-in cannot be turned off, so every account keeps a way back in.";

/// Builds the methods from owner input: password first, each method once.
/// Password cannot be turned off in this version.
pub fn sign_in_methods_from_input(methods: &[SignInMethod]) -> std::result::Result<Vec<SignInMethod>, String> {
    if !methods.contains(&SignInMethod::Password) {
        return Err(PASSWORD_REQUIRED.into());
    }
    let mut result = vec![SignInMethod::Password];
    if methods.contains(&SignInMethod::Google) {
        result.push(SignInMethod::Google);
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignupMode {
    #[default]
    Open,
    /// No signup without an invitation from the owner. See `invitations.rs`.
    Invitation,
    Allowlist,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignupPolicy {
    pub mode: SignupMode,
    /// Exact emails and `@domain` entries, stored trimmed and lowercased.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

pub const MAX_ALLOWLIST_ENTRIES: usize = 100;
pub const SIGNUP_REFUSED: &str = "This app is not accepting new accounts for this email address.";

impl SignupPolicy {
    /// Public apps start open. Private apps start closed until the owner
    /// chooses who may join.
    /// Open only for a Deployment that says it is public; any other value is closed.
    pub fn initial(visibility: &str) -> Self {
        Self {
            mode: if visibility == "public" {
                SignupMode::Open
            } else {
                SignupMode::Closed
            },
            allowlist: Vec::new(),
        }
    }

    /// The policy a redeploy keeps. Turning a public app private closes an
    /// open signup, since strangers who could join the public app should not
    /// get into the private one. Any other choice the owner made carries over.
    pub fn for_republish(&self, was_private: bool, visibility: &str) -> Self {
        if !was_private && visibility == "private" && self.mode == SignupMode::Open {
            Self {
                mode: SignupMode::Closed,
                allowlist: self.allowlist.clone(),
            }
        } else {
            self.clone()
        }
    }

    /// Builds a policy from owner input, normalizing and deduplicating entries.
    pub fn from_input(mode: SignupMode, entries: &[String]) -> std::result::Result<Self, String> {
        if entries.len() > MAX_ALLOWLIST_ENTRIES {
            return Err(format!(
                "The allowlist holds at most {MAX_ALLOWLIST_ENTRIES} entries."
            ));
        }
        let mut allowlist: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.trim().to_lowercase();
            if entry.is_empty() {
                continue;
            }
            let valid = match entry.strip_prefix('@') {
                Some(domain) => {
                    domain.len() <= 253
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && domain
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
                }
                None => auth::normalize_email(&entry).is_ok(),
            };
            if !valid {
                return Err(format!(
                    "{entry} is not an email address or an @domain entry."
                ));
            }
            if !allowlist.contains(&entry) {
                allowlist.push(entry);
            }
        }
        Ok(Self { mode, allowlist })
    }

    /// Decides whether this email may create an account. Domain entries match
    /// the exact domain after the `@`, never a subdomain.
    pub fn allows(&self, email: &str) -> bool {
        match self.mode {
            SignupMode::Open => true,
            SignupMode::Invitation | SignupMode::Closed => false,
            SignupMode::Allowlist => {
                let Ok(email) = auth::normalize_email(email) else {
                    return false;
                };
                let domain = email.rsplit_once('@').map(|(_, domain)| domain);
                self.allowlist.iter().any(|entry| match entry.strip_prefix('@') {
                    Some(allowed) => domain == Some(allowed),
                    None => *entry == email,
                })
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployInput {
    pub project: ProjectInput,
    pub visibility: String,
    pub artifact: BuildArtifact,
    #[serde(default)]
    pub initial_signup_policy: Option<SignupPolicy>,
    #[serde(default)]
    pub initial_sign_in_methods: Option<Vec<SignInMethod>>,
    /// A chunked package whose components were uploaded as transfers.
    #[serde(default)]
    pub initial_package: Option<package::InitialPackage>,
    /// The email `notify` to the owner reaches. Konae's release_apply sets it.
    #[serde(default)]
    pub contact_email: Option<String>,
    /// The confirmation of a schema change that loses data, as release_plan
    /// gave it. release_apply sets it from its confirmSchema argument.
    #[serde(default)]
    pub schema_confirmation: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub files: HashMap<String, String>,
    pub manifest: Value,
}

#[derive(Deserialize)]
pub struct BuildArtifact {
    pub version: u8,
    pub entrypoint: String,
    pub files: HashMap<String, String>,
    /// Paths whose value in `files` is base64; every other file is UTF-8 text.
    #[serde(default)]
    pub encodings: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBackup {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub size_bytes: u64,
    pub key: String,
    /// What a backup before an import holds: `data` (the database,
    /// accounts, files and Usage in the folder `key`) and `sources` (the
    /// saved sources). Empty for a Data panel backup of the database alone.
    #[serde(default)]
    pub holds: Vec<String>,
}

impl DatabaseBackup {
    /// The backup as `list-backups` lists it.
    pub fn view(&self) -> Value {
        json!({"id": self.id, "label": self.label, "createdAt": self.created_at, "sizeBytes": self.size_bytes, "holds": self.holds})
    }
}

#[derive(Deserialize)]
pub struct ActionProgram {
    pub version: u8,
    pub actions: HashMap<String, ActionDefinition>,
}

#[derive(Clone, Deserialize)]
pub struct ActionDefinition {
    pub kind: String,
    pub access: String,
    /// Empty for a `"kind": "signal"` action, which runs no SQL.
    #[serde(default)]
    pub statement: String,
    pub bindings: Vec<ActionBinding>,
    /// The action's HTTP route, checked by `http::binding`.
    #[serde(default)]
    pub http: Option<Value>,
    /// The Signal the action records, checked by `usage::declaration`.
    #[serde(default)]
    pub signal: Option<Value>,
    /// A mutation's refusal when it changes no row, checked by `guard::declaration`.
    #[serde(default)]
    pub guard: Option<Value>,
    /// The email a mutation queues when it changes a row, checked by `notify::declaration`.
    #[serde(default)]
    pub notify: Option<Value>,
    /// When the Kernel's timer runs a mutation, checked by `schedule::declaration`.
    #[serde(default)]
    pub schedule: Option<Value>,
    /// Refused by `schedule::check_subscribe`: the Kernel pushes nothing.
    #[serde(default)]
    pub subscribe: Option<Value>,
    /// An action of `"kind": "call"`: its url, method, body, auth and then,
    /// checked by `calls::check_kind`.
    #[serde(flatten)]
    pub call: calls::CallKeys,
    /// Every other key, refused by `action_shape::check_keys`. It comes after
    /// `call` so serde hands it only the keys no field took.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBinding {
    pub source: String,
    pub key: Option<String>,
    #[serde(rename = "type")]
    pub value_type: Option<String>,
    #[serde(default, deserialize_with = "whole_number")]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub nullable: bool,
    pub field: Option<String>,
    pub value: Option<Value>,
}

/// A whole number, also when written as `100.0`, which is the integer 100 to
/// the editor's JSON reader.
fn whole_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<usize>, D::Error> {
    Option::<f64>::deserialize(deserializer)?
        .map(|number| {
            (number.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&number))
                .then_some(number as usize)
                .ok_or_else(|| serde::de::Error::custom("expected a whole number"))
        })
        .transpose()
}

/// Runs the operator commands both binaries answer: `restore`, the file
/// store commands and `reconcile`. Returns whether one ran.
pub async fn operator_command(installation: &installation::Installation) -> Result<bool> {
    Ok(copies::operator_command(installation).await?
        || file_store::operator_command().await?
        || recovery::operator_command(installation).await?)
}

/// What a Kernel process reads from its environment before it serves.
pub struct Startup {
    pub port: u16,
    pub bind: String,
    pub hosts: hosts::Hosts,
    pub data_root: PathBuf,
}

impl Startup {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            port: env("KONAE_KERNEL_PORT", "8787").parse::<u16>()?,
            bind: env("KONAE_BIND", "127.0.0.1"),
            hosts: hosts::from_env()?,
            data_root: PathBuf::from(env("KONAE_DATA_DIR", "/data")),
        })
    }
}

/// A started Kernel: its state, and the lock that makes `files-migrate` and
/// `files-reconcile` refuse while it serves.
pub struct Running {
    pub state: AppState,
    pub _lock: std::fs::File,
}

/// Loads every app folder, builds the state and starts the workers: copies,
/// schedules, email and calls. `mail_key` seals queued links when
/// `KONAE_MAIL_SECRET` is unset. `own_origins` and `audit` are what the
/// starter adds: the origins it answers on and where app users' uploads and
/// deletes are recorded.
pub async fn start(
    startup: &Startup,
    installation: installation::Installation,
    mail_key: &str,
    own_origins: Vec<String>,
    audit: Option<runtime::AuditSink>,
) -> Result<Running> {
    tokio::fs::create_dir_all(crate::layout::apps(&startup.data_root)).await?;
    let lock = file_store::hold_kernel_lock(&startup.data_root)?;
    let schedule_clock = schedule::Clock::from_env(&startup.bind)?;
    let (state, copy_config) = state_from_env(startup, installation, mail_key, own_origins, audit).await?;
    path::load(&state);
    if let Some(config) = copy_config {
        copies::spawn_schedule(state.clone(), config);
    }
    schedule::spawn(state.clone(), schedule_clock);
    if let Some(queue) = state.accounts.queue.clone() {
        mail::spawn_worker(queue);
    }
    calls::spawn_worker(state.clone());
    Ok(Running { state, _lock: lock })
}

/// The state a Kernel serves with, read from the environment, without its
/// workers. An operator command works on the same state beside a running
/// Kernel. Also answers the copy settings the workers need.
pub async fn state_from_env(
    startup: &Startup,
    installation: installation::Installation,
    mail_key: &str,
    own_origins: Vec<String>,
    audit: Option<runtime::AuditSink>,
) -> Result<(AppState, Option<copies::CopyConfig>)> {
    let data_root = startup.data_root.clone();
    let public_origin = startup.hosts.public_origin.clone();
    tokio::fs::create_dir_all(crate::layout::apps(&data_root)).await?;
    let account_settings = accounts::AccountSettings::from_env(&data_root, mail_key, &public_origin, &installation)?;
    let s3 = s3_client_from_env().await;
    let artifact_bucket = env("KONAE_ARTIFACT_BUCKET", "konae-apps");
    let data_bucket = env("KONAE_DATA_BUCKET", "konae-data");
    let copy_config = copies::CopyConfig::from_env(&data_root, s3.is_some())?;
    let deployments = load_deployments(&data_root, &public_origin).await?;
    let files = file_store::Files::from_env(&data_root).await?;
    let state = AppState {
        deployments: Arc::new(RwLock::new(deployments)),
        db_locks: LockMap::default(),
        s3,
        artifact_bucket,
        data_bucket,
        data_root,
        files,
        copies: Arc::new(copies::CopyStatus::new(copy_config.as_ref())),
        public_origin,
        own_origins,
        host_suffix: startup.hosts.host_suffix.clone(),
        path_origin: startup.hosts.path_origin.clone(),
        guards: Arc::new(Guards::new(trusted_proxies(
            &std::env::var("KONAE_TRUSTED_PROXIES").unwrap_or_default(),
        )?)),
        accounts: account_settings,
        audit,
        installation: Arc::new(installation),
    };
    Ok((state, copy_config))
}

/// Binds the port and serves the Kernel, with `extra` beside its routes.
pub async fn listen(startup: &Startup, state: AppState, extra: Router<AppState>, name: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((startup.bind.as_str(), startup.port)).await?;
    println!("{name} listening on {}", startup.port);
    serve(listener, state, extra).await?;
    Ok(())
}

/// Serves the Kernel, and `extra` beside its own routes, on a bound listener.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState, extra: Router<AppState>) -> std::io::Result<()> {
    axum::serve(listener, ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(stack(state, extra))).await
}

/// The Kernel's whole stack. The isolation headers and the path router wrap
/// the routes, so a path address is rewritten before routing and the headers
/// cover every response, the redirects included.
/// `extra` holds the routes of whoever starts the Kernel; they answer
/// before the Kernel's catch-all and inside the same layers.
pub fn stack(state: AppState, extra: Router<AppState>) -> tower::util::BoxCloneService<Request, axum::response::Response, std::convert::Infallible> {
    let routes = Router::new()
        .route("/_konae/health", get(health))
        .route("/_konae/auth/signup", post(signup))
        .route("/_konae/auth/login", post(login))
        .route("/_konae/login", get(login_page))
        .merge(runtime::routes())
        .merge(extra)
        .merge(google::routes())
        .merge(accounts::routes())
        .merge(http::routes())
        .merge(invitations::routes())
        .merge(enter::routes())
        .merge(mail::routes())
        .route("/_konae/{*path}", axum::routing::any(|| async { error(StatusCode::GONE, "Use authenticated MCP for platform operations.") }))
        .route("/{*path}", get(http::get_route).post(http::post_route))
        .route("/", get(serve_root))
        .layer(axum::middleware::from_fn_with_state(state.clone(), maintenance::guard))
        .with_state(state.clone());
    let service = ServiceBuilder::new()
        .layer(axum::middleware::from_fn_with_state(state.clone(), security::headers))
        .layer(axum::middleware::from_fn_with_state(state, path::route))
        .service(routes);
    tower::util::BoxCloneService::new(service)
}

pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({"ok": true, "kernel": "konae-rust", "version": 1,
        "artifactStore": if state.s3.is_some() { "filesystem+s3" } else { "filesystem" },
        "copies": state.copies.health(),
        "files": {"store": state.files.store.kind(), "quotaBytes": state.files.limits.quota_bytes,
            "maxFileBytes": state.files.limits.max_file_bytes},
        "mail": state.accounts.queue.as_ref().map(|queue| queue.health())}))
}

/// The S3 client for artifacts and copies, or None when KONAE_S3_ENDPOINT is unset.
pub async fn s3_client_from_env() -> Option<S3Client> {
    let endpoint = std::env::var("KONAE_S3_ENDPOINT").ok()?;
    let credentials = Credentials::new(
        env("KONAE_S3_ACCESS_KEY", "konae"),
        env("KONAE_S3_SECRET_KEY", "konae-secret"),
        None,
        None,
        "konae-config",
    );
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(env("KONAE_S3_REGION", "us-east-1")))
        .credentials_provider(credentials)
        .endpoint_url(endpoint)
        .load()
        .await;
    let s3_config = S3ConfigBuilder::from(&sdk_config)
        .force_path_style(true)
        .build();
    Some(S3Client::from_conf(s3_config))
}

pub fn backup_dir(state: &AppState, id: &str) -> PathBuf {
    layout::backups(&state.data_root, id)
}

/// Whether the artifact store answers.
pub async fn test_storage(state: &AppState) -> Response {
    let Some(s3) = state.s3.as_ref() else {
        return Json(json!({"result": {"ok": true, "adapter": "filesystem"}})).into_response();
    };
    match s3.head_bucket().bucket(&state.artifact_bucket).send().await {
        Ok(_) => Json(json!({"result": {"ok": true, "adapter": "s3-compatible", "bucket": state.artifact_bucket}})).into_response(),
        Err(error_value) => error(StatusCode::BAD_GATEWAY, &error_value.to_string()),
    }
}

/// Installs a release from its JSON payload: the project, the build and
/// what a first release imports.
pub async fn deploy(state: &AppState, body: &[u8]) -> Response {
    if body.len() > 26_000_000 {
        return error(StatusCode::BAD_REQUEST, "Deployment request is too large.");
    }
    let input: DeployInput = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error_value) => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid deployment: {error_value}"),
            );
        }
    };
    if let Err(error_value) = runtime::validate_release(&state, &input, None).await {
        return error(StatusCode::BAD_REQUEST, &error_value.to_string());
    }
    if input.artifact.version != 1
        || input.artifact.entrypoint != "index.html"
        || !input.artifact.files.contains_key("index.html")
    {
        return error(StatusCode::BAD_REQUEST, "Invalid Build Artifact.");
    }
    if input.visibility != "public" && input.visibility != "private" {
        return error(
            StatusCode::BAD_REQUEST,
            "Visibility must be public or private.",
        );
    }
    let actions = match input.project.files.get("actions.json") {
        Some(value) => value.clone(),
        None => return error(StatusCode::BAD_REQUEST, "actions.json is required."),
    };
    let program = match action_shape::read(&actions) {
        Ok(program) => program,
        Err(error_value) => match refusal::of(&error_value) {
            Some(refused) => return refusal_error(refused),
            None => return error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        },
    };
    if !runtime::valid_app_id(&input.project.id) {
        return error(StatusCode::BAD_REQUEST, "Invalid project ID.");
    }
    if let Err(error_value) = http::secrets_store(&state.data_root, &input.project.id).and_then(|db| {
        calls::check_release(&domain::Rules::of(&state), &db, &program)
    }) {
        return match refusal::of(&error_value) {
            Some(refused) => refusal_error(refused),
            None => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        };
    }
    if !input.artifact.files.keys().all(|path| safe_relative_path(path)) {
        return error(
            StatusCode::BAD_REQUEST,
            "Artifact contains an invalid path.",
        );
    }
    if let Err(message) = artifact::check_routing(&input.project.manifest) {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    if let Err(refused) = calls::check_connects(&input.project.manifest) {
        return refusal_error(&refused);
    }
    let artifact_files = match artifact::decode(&input.artifact) {
        Ok(files) => files,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    let deployments = state.deployments.read().await;
    let existing = find_project_deployment(&deployments, &input.project.id);
    drop(deployments);
    if existing.is_some()
        && (input.initial_signup_policy.is_some()
            || input.initial_sign_in_methods.is_some()
            || input.initial_package.is_some())
    {
        return error(
            StatusCode::CONFLICT,
            "Portable state can only be imported on the first deployment.",
        );
    }
    let package_policy = input.initial_package.as_ref().and_then(|package| package.manifest.signup_policy.as_ref());
    let initial_signup_policy = match input.initial_signup_policy.as_ref().or(package_policy) {
        Some(policy) => match SignupPolicy::from_input(policy.mode, &policy.allowlist) {
            Ok(policy) => Some(policy),
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };
    // A package keeps the methods it carries even on an installation without
    // a Google client: the sign-in page then offers only the password and the
    // owner is told why (sign_in_methods_read).
    let package_methods = input.initial_package.as_ref().and_then(|package| package.manifest.sign_in_methods.as_ref());
    let initial_sign_in_methods = match input.initial_sign_in_methods.as_ref().or(package_methods) {
        Some(methods) => match sign_in_methods_from_input(methods) {
            Ok(methods) => Some(methods),
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };
    // The app keeps one ID: its folder, its project and its Deployment share it.
    let id = input.project.id.clone();
    let hostname = existing
        .as_ref()
        .map(|item| item.hostname.clone())
        .unwrap_or_else(|| {
            // Random, so no project ID can be chosen to land on another app's address.
            let short_id = Uuid::new_v4().simple().to_string().chars().take(6).collect::<String>();
            format!(
                "{}-{}.{}",
                safe_slug(&input.project.slug),
                short_id,
                state.host_suffix
            )
        });
    let url = existing
        .as_ref()
        .map(|item| item.url.clone())
        .unwrap_or_else(|| deployment_url(&state.public_origin, &hostname));
    let mut manifest = input.project.manifest;
    manifest["visibility"] = Value::String(input.visibility.clone());
    let artifact_version = format!("v{}", Uuid::new_v4().simple());
    let deployment = Deployment {
        id: id.clone(),
        project_id: input.project.id,
        project_name: input.project.name,
        slug: input.project.slug.clone(),
        url,
        created_at: existing
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or_else(now_ms),
        manifest,
        hostname: hostname.clone(),
        actions,
        artifact_version: artifact_version.clone(),
        signup_policy: match existing.as_ref() {
            Some(item) => item.signup_policy.for_republish(is_private(item), &input.visibility),
            None => initial_signup_policy.unwrap_or_else(|| SignupPolicy::initial(&input.visibility)),
        },
        sign_in_methods: match existing.as_ref() {
            Some(item) => item.sign_in_methods.clone(),
            None => initial_sign_in_methods.unwrap_or_else(default_sign_in_methods),
        },
        contact_email: input.contact_email.clone(),
    };
    let data = layout::data(&state.data_root, &id);
    if let Err(error_value) = tokio::fs::create_dir_all(&data).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    // One release of an app at a time, so the cleanup of one never removes
    // the files another is still staging.
    let _release_guard = named_lock(&state, &format!("publish:{id}"), true).await;
    // Every step that can fail runs before the app changes. Readers keep the
    // previous files until the record names this release and dist/ switches.
    if let Err(error_value) = stage_local_artifact(&state, &id, &artifact_version, &artifact_files)
        .await
        .and_then(|()| release_step(&id, "stage_local_artifact"))
    {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    if let Err(error_value) = upload_artifact(&state, &id, &artifact_version, artifact_files)
        .await
        .and_then(|()| release_step(&id, "upload_artifact"))
    {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(
            StatusCode::BAD_GATEWAY,
            &format!("Artifact upload failed: {error_value}"),
        );
    }
    let mut notices = Vec::new();
    if let Some(package) = input.initial_package.as_ref() {
        if let Err(error_value) = package::install(&state, package, &id)
            .await
        {
            remove_artifact_version(&state, &id, &artifact_version).await;
            let _ = tokio::fs::remove_dir_all(&data).await;
            file_store::forget_deployment(state.files.store.as_ref(), &id).await;
            return error(StatusCode::BAD_REQUEST, &format!("Package import failed: {error_value:#}"));
        }
        // The path keeps the owner's choice where it can. What it changed comes back as the import notice.
        match path::arrive(&state, &deployment, package.manifest.path.as_ref()).await {
            Ok(lines) => notices = lines,
            Err(error_value) => {
                remove_artifact_version(&state, &id, &artifact_version).await;
                let _ = tokio::fs::remove_dir_all(&data).await;
                return error(StatusCode::BAD_REQUEST, &format!("Package import failed: {error_value:#}"));
            }
        }
    }
    let db_path = layout::database(&state.data_root, &id);
    let schema = input
        .project
        .files
        .get("schema.sql")
        .cloned()
        .unwrap_or_default();
    if !db_path.exists() {
        if let Err(error_value) = initialize_database(&db_path) {
            remove_artifact_version(&state, &id, &artifact_version).await;
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Database initialization failed: {error_value}"),
            );
        }
    }
    let identity_path = layout::identity(&state.data_root, &id);
    let identity = if identity_path.exists() {
        Ok(())
    } else {
        open_auth(&state, &deployment).map(drop).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))
    };
    if let Err(error_value) = identity.and_then(|()| release_step(&id, "open_auth")) {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    // From here until the new release serves, no Server Action runs on this
    // Database, so none writes a row the copy below would miss.
    let database_guard = database_lock(&state, &id).await;
    let staged_database = layout::private(&state.data_root, &id).join(format!("release-{artifact_version}.sqlite"));
    let staged = stage_database(&db_path, &staged_database, &schema, &program, input.schema_confirmation.as_deref())
        .and_then(|change| release_step(&id, "schema_change").map(|()| change));
    let change = match staged {
        Ok(change) => change,
        Err(error_value) => {
            remove_database_files(&staged_database);
            remove_artifact_version(&state, &id, &artifact_version).await;
            return match refusal::of(&error_value) {
                Some(refused) => refusal_error(refused),
                None => error(StatusCode::BAD_REQUEST, &format!("Schema update failed: {error_value}")),
            };
        }
    };
    // The database, the record, dist/ and the Deployment in memory change
    // together. A step that fails puts back the ones before it.
    let recorded = Recorded::read(&state.data_root, &id).await;
    let replaced = layout::private(&state.data_root, &id).join(format!("release-{artifact_version}-replaced.sqlite"));
    let swapped = if change.statements.is_empty() {
        remove_database_files(&staged_database);
        false
    } else if let Err(error_value) = swap_database(&db_path, &staged_database, &replaced) {
        remove_database_files(&staged_database);
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The changed database could not take the place of the live one: {error_value}"));
    } else {
        true
    };
    let undo_database = || {
        if swapped {
            if let Err(error_value) = restore_database(&db_path, &replaced) {
                eprintln!("Could not put back the database of {id} after a failed release: {error_value}");
            }
        }
    };
    if let Err(error_value) = persist_deployment(&state, &deployment)
        .await
        .and_then(|()| release_step(&id, "persist_deployment"))
    {
        recorded.put_back().await;
        undo_database();
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    if let Err(error_value) = switch_dist(&state, &id, &artifact_version).await {
        recorded.put_back().await;
        undo_database();
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The new files could not take the place of dist/: {error_value}"));
    }
    state
        .deployments
        .write()
        .await
        .insert(hostname, deployment.clone());
    drop(database_guard);
    if swapped {
        remove_database_files(&replaced);
    }
    // An app the editor never saved keeps the release's sources in app/, so its folder is whole.
    if let Err(error_value) = sources::write_release(&state.data_root, &id, &input.project.files).await {
        eprintln!("Could not write the sources of release {artifact_version} to app/: {error_value:#}");
    }
    http::routes_published(&deployment);
    remove_stale_artifacts(&state, &id, &artifact_version).await;
    let mut result = match serde_json::to_value(&deployment) {
        Ok(value) => value,
        Err(error_value) => return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
    };
    if !notices.is_empty() {
        result["notices"] = json!(notices);
    }
    Json(json!({"result": result})).into_response()
}

pub async fn stage_local_artifact(
    state: &AppState,
    id: &str,
    version: &str,
    files: &HashMap<String, Vec<u8>>,
) -> Result<()> {
    let root = staged_dist(state, id, version);
    for (path, contents) in files {
        let local_path = root.join(path);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&local_path, contents).await?;
    }
    Ok(())
}

pub async fn upload_artifact(
    state: &AppState,
    id: &str,
    version: &str,
    files: HashMap<String, Vec<u8>>,
) -> Result<()> {
    let Some(s3) = state.s3.as_ref() else {
        return Ok(());
    };
    let mut uploads = tokio::task::JoinSet::new();
    for (path, contents) in files {
        if uploads.len() >= 16 {
            if let Some(result) = uploads.join_next().await {
                result??;
            }
        }
        let request = s3
            .put_object()
            .bucket(&state.artifact_bucket)
            .key(format!("deployments/{id}/{version}/{path}"))
            .body(ByteStream::from(contents));
        uploads.spawn(async move { request.send().await.map(|_| ()).map_err(anyhow::Error::from) });
    }
    while let Some(result) = uploads.join_next().await {
        result??;
    }
    Ok(())
}

/// Where a release's files wait until they become `dist/`.
pub fn staged_dist(state: &AppState, id: &str, version: &str) -> PathBuf {
    layout::private(&state.data_root, id).join(format!("dist-{version}"))
}

/// Moves the staged files into `dist/` and the previous ones aside, for
/// `remove_stale_artifacts` to delete. When the second move fails, the
/// previous files go back to `dist/`.
pub async fn switch_dist(state: &AppState, id: &str, version: &str) -> std::io::Result<()> {
    let dist = layout::dist(&state.data_root, id);
    let retired = layout::private(&state.data_root, id).join(format!("dist-retired-{}", Uuid::new_v4().simple()));
    let had_dist = tokio::fs::try_exists(&dist).await?;
    if had_dist {
        tokio::fs::rename(&dist, &retired).await?;
    }
    let moved = match release_step(id, "switch_dist") {
        Ok(()) => tokio::fs::rename(staged_dist(state, id, version), &dist).await,
        Err(error_value) => Err(std::io::Error::other(error_value.to_string())),
    };
    if moved.is_err() && had_dist {
        if let Err(error_value) = tokio::fs::rename(&retired, &dist).await {
            eprintln!("Could not put back dist/ of {id}: {error_value}");
        }
    }
    moved
}

/// Copies the live database with `VACUUM INTO` and applies `schema` to the
/// copy, so a change that fails, or a release that fails after it, leaves the
/// live database as it was. The caller holds the database lock alone.
fn stage_database(live: &Path, staged: &Path, schema: &str, program: &ActionProgram, confirmation: Option<&str>) -> Result<schema_change::Change> {
    remove_database_files(staged);
    Connection::open(live)?.execute("VACUUM INTO ?1", [staged.to_string_lossy().as_ref()])?;
    let change = schema_change::apply(staged, schema, program, schema_change::Mode::Apply(confirmation))?;
    // The copy leaves in rollback mode. Closing this connection checkpoints
    // and removes its WAL, so only the file moves into place.
    Connection::open(staged)?.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    Ok(change)
}

fn database_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_database_files(path: &Path) {
    for file in [path.to_path_buf(), database_sidecar(path, "-wal"), database_sidecar(path, "-shm")] {
        let _ = std::fs::remove_file(file);
    }
}

/// Moves the live database and its WAL files to `replaced` and the staged
/// copy into its place. A move that fails puts back what moved before it.
fn swap_database(live: &Path, staged: &Path, replaced: &Path) -> std::io::Result<()> {
    std::fs::rename(live, replaced)?;
    for suffix in ["-wal", "-shm"] {
        let from = database_sidecar(live, suffix);
        if from.exists() {
            if let Err(error_value) = std::fs::rename(&from, database_sidecar(replaced, suffix)) {
                let _ = restore_database(live, replaced);
                return Err(error_value);
            }
        }
    }
    if let Err(error_value) = std::fs::rename(staged, live) {
        let _ = restore_database(live, replaced);
        return Err(error_value);
    }
    Ok(())
}

/// Puts the database `swap_database` moved to `replaced` back in place.
fn restore_database(live: &Path, replaced: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        let from = database_sidecar(replaced, suffix);
        if from.exists() {
            std::fs::rename(&from, database_sidecar(live, suffix))?;
        } else {
            let _ = std::fs::remove_file(database_sidecar(live, suffix));
        }
    }
    std::fs::rename(replaced, live)
}

/// `konae.json` and the record as they were before a release, to put back
/// when the release fails after writing them.
struct Recorded(Vec<(PathBuf, Option<Vec<u8>>)>);

impl Recorded {
    async fn read(root: &Path, id: &str) -> Self {
        let mut files = Vec::new();
        for path in [layout::settings(root, id), layout::record(root, id)] {
            let bytes = tokio::fs::read(&path).await.ok();
            files.push((path, bytes));
        }
        Self(files)
    }

    async fn put_back(&self) {
        for (path, bytes) in &self.0 {
            let restored = match bytes {
                Some(bytes) => write_atomic(path, bytes).await,
                None => tokio::fs::remove_file(path).await,
            };
            if let Err(error_value) = restored {
                eprintln!("Could not put back {} after a failed release: {error_value}", path.display());
            }
        }
    }
}

/// The release steps a test makes fail, by app.
#[cfg(test)]
static FAILING_RELEASE_STEPS: std::sync::Mutex<Vec<(String, &'static str)>> = std::sync::Mutex::new(Vec::new());

/// Fails a release at `step` when a test asked for it.
#[cfg(test)]
fn release_step(id: &str, step: &'static str) -> Result<()> {
    if FAILING_RELEASE_STEPS.lock().unwrap().iter().any(|(app, failing)| app == id && *failing == step) {
        anyhow::bail!("The test failed this release at {step}.");
    }
    Ok(())
}

#[cfg(not(test))]
fn release_step(_id: &str, _step: &'static str) -> Result<()> {
    Ok(())
}

/// Best-effort removal of a release that never went live.
pub async fn remove_artifact_version(state: &AppState, id: &str, version: &str) {
    let _ = tokio::fs::remove_dir_all(staged_dist(state, id, version)).await;
    if let Some(s3) = state.s3.as_ref() {
        let prefix = format!("deployments/{id}/{version}/");
        if let Ok(keys) = list_s3_keys(s3, &state.artifact_bucket, &prefix).await {
            for key in keys {
                let _ = s3.delete_object().bucket(&state.artifact_bucket).key(key).send().await;
            }
        }
    }
}

/// Deletes the files of every release but the live one: retired or staged
/// folders beside `dist/`, and S3 keys outside the live version. Failures are
/// logged because the new release is already live.
pub async fn remove_stale_artifacts(state: &AppState, id: &str, version: &str) {
    if let Ok(mut entries) = tokio::fs::read_dir(layout::private(&state.data_root, id)).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.file_name().to_str().is_some_and(|name| name.starts_with("dist-")) {
                continue;
            }
            let removed = match entry.file_type().await {
                Ok(kind) if kind.is_dir() => tokio::fs::remove_dir_all(entry.path()).await,
                _ => tokio::fs::remove_file(entry.path()).await,
            };
            if let Err(error_value) = removed {
                eprintln!("Could not remove stale artifact {}: {error_value}", entry.path().display());
            }
        }
    }
    let Some(s3) = state.s3.as_ref() else { return };
    let prefix = format!("deployments/{id}/");
    let live = format!("{prefix}{version}/");
    let keys = match list_s3_keys(s3, &state.artifact_bucket, &prefix).await {
        Ok(keys) => keys,
        Err(error_value) => {
            eprintln!("Could not list stale artifacts for {id}: {error_value}");
            return;
        }
    };
    for key in keys {
        if key.starts_with(&live) {
            continue;
        }
        if let Err(error_value) = s3.delete_object().bucket(&state.artifact_bucket).key(&key).send().await {
            eprintln!("Could not remove stale artifact {key}: {error_value}");
        }
    }
}

pub fn find_project_deployment(
    deployments: &HashMap<String, Deployment>,
    project_id: &str,
) -> Option<Deployment> {
    deployments
        .values()
        .find(|deployment| deployment.project_id == project_id)
        .cloned()
}

pub async fn invoke_action(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Deployment not found."),
    };
    let program: ActionProgram = match serde_json::from_str(&deployment.actions) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored action program is invalid.",
            );
        }
    };
    let action = match program.actions.get(&name) {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Server action not declared."),
    };
    // An action whose route does not answer anyone answers only there.
    if let Some(route) = http::route_only(&name, action, is_private(&deployment)) {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} answers only on its HTTP route, {route}."));
    }
    // A call's then mutation records what the call's answer said, so no one
    // else may write it.
    if let Some(call) = calls::then_of(&program, &name) {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} runs only after the call {call} answers."));
    }
    // A scheduled mutation with no route belongs to the Kernel's timer
    // (ADR-0011).
    if action.schedule.is_some() && action.http.is_none() {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} runs only on its schedule."));
    }
    let actor = match authenticate_deployment(&state, &deployment, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if action.access == "granted" {
        return error(StatusCode::FORBIDDEN, "action-grant-required");
    }
    // Private apps need a session even for anonymous actions, like their shell.
    if (action.access != "anonymous" || is_private(&deployment)) && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let params = match resolve_bindings(
        action,
        &input,
        actor.as_ref().map(|user| user.user_id.as_str()),
    ) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    // A call is queued for the Kernel's worker and answered with its ID.
    if action.kind == "call" {
        return calls::enqueue(&state, &deployment, &name, &input).await;
    }
    let db_path = crate::layout::database(&state
        .data_root, &deployment.id);
    // A signal action runs no SQL. The Signal is queued for the Usage store and
    // the answer does not wait for the write.
    let result = if action.kind == "signal" {
        Ok(Ran { answer: json!({"recorded": true}), rows: Vec::new() })
    } else {
        let _guard = action_lock(&state, &deployment.id, &action.kind).await;
        run_action(db_path, action, params).await
    };
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    // Usage and Signals do not count what someone who entered as this account does.
    let counted = !enter::entered(actor.as_ref());
    match result {
        Ok(ran) => match guard::outcome(&name, action, &ran.answer) {
            guard::Outcome::Refused(message) => {
                if counted {
                    usage::execution(&state.data_root, &deployment.id, &name, "runtime", "refused");
                    usage::count(&state.data_root, &deployment.id, usage::Operation::Refused(name.clone()));
                }
                error(StatusCode::FORBIDDEN, &message)
            }
            changed => {
                if counted {
                    usage::execution(&state.data_root, &deployment.id, &name, "runtime", "ok");
                    usage::count(&state.data_root, &deployment.id, usage::Operation::Action(name.clone()));
                }
                if matches!(changed, guard::Outcome::Changed) {
                    if counted {
                        usage::signal(&state.data_root, &deployment.id, &name, action, &input, actor_id);
                    }
                    let caller = notify::Caller { account: actor_id, actor_id };
                    notify::record(&state, &deployment, &name, action, &input, caller, ran.changed(now_ms())).await;
                }
                Json(json!({"result": ran.answer})).into_response()
            }
        },
        Err(error_value) => {
            if counted {
                usage::execution(&state.data_root, &deployment.id, &name, "runtime", "failed");
            }
            error(StatusCode::BAD_REQUEST, &error_value.to_string())
        },
    }
}

pub async fn session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    match authenticate_deployment(&state, &deployment, &headers) {
        Ok(Some(user)) => Json(json!({"result": user})).into_response(),
        Ok(None) => Json(
            json!({"result": {"userId": "anonymous", "name": "Anonymous", "role": "anonymous"}}),
        )
        .into_response(),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
pub struct AuthInput {
    pub email: String,
    /// Not read by a signup that requires verification: the person chooses
    /// the password when they answer the link.
    #[serde(default)]
    pub password: String,
}

pub async fn signup(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, true).await
}

pub async fn login(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, false).await
}

pub async fn auth_request(
    state: &AppState,
    headers: &HeaderMap,
    request: Request,
    signup: bool,
) -> Response {
    if !same_origin(headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    let bytes = match axum::body::to_bytes(request.into_body(), 4096).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Authentication request is too large.",
            );
        }
    };
    let input: AuthInput = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Authentication request must include email and password.",
            );
        }
    };
    let client = state.guards.client_ip(peer, headers);
    if let Err(wait) = state.guards.auth_attempt(&deployment.id, client, &input.email) {
        return too_many_attempts(wait);
    }
    if signup && state.accounts.verification == auth::Verification::Required {
        return match accounts::signup_app(state, &deployment, &input.email, client).await {
            Ok(()) => {
                let mut response = accounts::link_sent();
                if let Some(cookie) = accounts::signup_hint_cookie(state, &deployment, &input.email) {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                }
                response
            }
            Err(error_value) => auth_error(error_value),
        };
    }
    if signup {
        // Check the input first and give closed and unlisted the same answer,
        // so a signup attempt does not reveal who is on the allowlist.
        if let Err(error_value) = auth::normalize_email(&input.email)
            .and_then(|_| auth::validate_password(&input.password))
        {
            return auth_error(error_value);
        }
        if !deployment.signup_policy.allows(&input.email) {
            return error(StatusCode::FORBIDDEN, SIGNUP_REFUSED);
        }
    }
    // The identity lock covers only SQLite reads and writes. The password
    // hash runs between them without it, so slow hashes never queue other
    // logins, logouts or releases behind them.
    let identity = format!("identity:{}", deployment.id);
    let (email, password) = (input.email, input.password);
    let failed_email = email.clone();
    let result = if signup {
        let requested = email.clone();
        match blocking(move || auth::NewAccount::prepare(&email, &password)).await {
            Ok(account) => {
                let _identity_guard = database_lock(state, &identity).await;
                // The policy may have changed during the hash. Check the
                // current one under the identity lock before creating the account.
                let allowed = state
                    .deployments
                    .read()
                    .await
                    .values()
                    .find(|current| current.id == deployment.id)
                    .map(|current| current.signup_policy.allows(&requested));
                match allowed {
                    None => return error(StatusCode::NOT_FOUND, "Deployment not found."),
                    Some(false) => return error(StatusCode::FORBIDDEN, SIGNUP_REFUSED),
                    Some(true) => {}
                }
                let auth = accounts::app_store(state, &deployment);
                blocking(move || auth?.create_account(account)).await
            }
            Err(error_value) => Err(error_value),
        }
    } else {
        let lookup = {
            let _identity_guard = database_lock(state, &identity).await;
            let (auth, email, password) = (accounts::app_store(state, &deployment), email.clone(), password.clone());
            blocking(move || auth?.find_login(&email, &password)).await
        };
        match lookup {
            Ok(stored) => match blocking(move || auth::check_password(&password, stored)).await {
                Ok(user) => {
                    let _identity_guard = database_lock(state, &identity).await;
                    let auth = accounts::app_store(state, &deployment);
                    blocking(move || auth?.start_session(user)).await
                }
                Err(error_value) => Err(error_value),
            },
            Err(error_value) => Err(error_value),
        }
    };
    if !signup && matches!(result, Err(AuthError::InvalidCredentials)) {
        state.guards.auth_failed(&deployment.id, client, &failed_email);
        if accounts::signup_hint_applies(state, &deployment, headers, &failed_email) {
            return error(StatusCode::UNAUTHORIZED, accounts::SIGNUP_HINT);
        }
    }
    match result {
        Ok(session) => {
            let operation = if signup { usage::Operation::Signup } else { usage::Operation::Login };
            usage::count(&state.data_root, &deployment.id, operation);
            let cookie_name = app_cookie_name(state, &deployment.id, headers);
            accounts::app_session_response(&cookie_name, session)
        }
        Err(error_value) => auth_error(error_value),
    }
}

/// Runs blocking authentication work off the async runtime.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AuthError> + Send + 'static,
) -> Result<T, AuthError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(AuthError::PasswordHash))
}

/// The answer when a login or signup exceeds its rate limit.
pub fn too_many_attempts(wait: std::time::Duration) -> Response {
    let mut response = error(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many sign-in attempts. Try again later.",
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
    response
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let token = headers.get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(&headers, &app_cookie_name(&state, &deployment.id, &headers)));
    let _identity_guard = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    let auth = match open_auth(&state, &deployment) {
        Ok(auth) => auth,
        Err(error_value) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
        }
    };
    if let Err(error_value) = auth.logout(token.as_deref()) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    let mut response = Json(json!({"result": {"ok": true}})).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cleared_app_cookie(&state, &deployment.id, &headers));
    response
}

/// The `Set-Cookie` value that removes the app session cookie from the browser.
pub fn cleared_app_cookie(state: &AppState, deployment_id: &str, headers: &HeaderMap) -> HeaderValue {
    let name = app_cookie_name(state, deployment_id, headers);
    HeaderValue::from_str(&format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}", cookie_secure(&name)))
        .unwrap()
}

/// `; Secure` for a name with a prefix that requires it.
pub fn cookie_secure(name: &str) -> &'static str {
    if name.starts_with("__Host-") || name.starts_with("__Secure-") { "; Secure" } else { "" }
}

pub async fn list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let actor = match authenticate_deployment(&state, &deployment, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_private(&deployment) && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    let private_deployment = is_private(&deployment);
    let path = crate::layout::files(&state
        .data_root, &deployment.id)
        .join("index.json");
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Array(files)) => {
                let visible = files
                    .iter()
                    .filter(|file| file_readable(file, actor_id, private_deployment))
                    .map(|file| file_view(file, actor_id))
                    .collect::<Vec<_>>();
                Json(json!({"result": visible})).into_response()
            }
            Ok(_) | Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid."),
        },
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => {
            Json(json!({"result": []})).into_response()
        }
        Err(error_value) => error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
    }
}

/// The JSON upload, `{ name, type, dataUrl, visibility }`, that tests use.
/// Runtime MCP uploads arrive as transfers and go to [`upload_from_path`].
#[cfg(any(test, feature = "testing"))]
pub async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(response) = upload_actor(&state, &deployment, &headers) {
        return response;
    }
    let limits = state.files.limits;
    // Base64 takes four bytes for every three, and the JSON around it a little more.
    let body_limit = (limits.max_file_bytes / 3 * 4 + 64 * 1024).min(usize::MAX as u64) as usize;
    let bytes = match axum::body::to_bytes(request.into_body(), body_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("File upload exceeds the {} limit.", limits.max_file()),
            );
        }
    };
    let input: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "File upload must be JSON with name, type, and dataUrl.",
            );
        }
    };
    let data = match input
        .get("dataUrl")
        .and_then(Value::as_str)
        .map(decode_base64)
        .transpose()
    {
        Ok(Some(value)) => value,
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "File upload dataUrl must contain base64 data.",
            );
        }
    };
    let scratch = state.data_root.join("transfers");
    let source = scratch.join(format!(".upload-{}", Uuid::new_v4()));
    let written = async {
        tokio::fs::create_dir_all(&scratch).await?;
        tokio::fs::write(&source, &data).await
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&source).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let upload = Upload {
        name: input.get("name").and_then(Value::as_str),
        content_type: input.get("type").and_then(Value::as_str),
        visibility: input.get("visibility").and_then(Value::as_str),
    };
    let response = upload_from_path(&state, &headers, upload, &source).await;
    let _ = tokio::fs::remove_file(&source).await;
    response
}

/// What the uploader says about a file besides its bytes.
pub struct Upload<'a> {
    pub name: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub visibility: Option<&'a str>,
}

pub fn upload_actor(state: &AppState, deployment: &Deployment, headers: &HeaderMap) -> Result<AuthUser, Response> {
    match authenticate_deployment(state, deployment, headers) {
        Ok(Some(actor)) => Ok(actor),
        Ok(None) => Err(error(StatusCode::UNAUTHORIZED, "authentication-required")),
        Err(response) => Err(response),
    }
}

/// Reads the index under the Deployment's lock. A missing index is an empty one.
pub async fn read_file_index(directory: &Path) -> Result<Vec<Value>, Response> {
    match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes)
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid.")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
    }
}

/// The refusal when one more file of `size` bytes would not fit.
pub fn file_room(state: &AppState, files: &[Value], size: u64) -> Option<Response> {
    let used = files
        .iter()
        .filter_map(|file| file["size"].as_u64())
        .sum::<u64>();
    if used.saturating_add(size) > state.files.limits.quota_bytes {
        return Some(error(StatusCode::PAYLOAD_TOO_LARGE, &state.files.limits.quota_reached()));
    }
    if files.len() >= files::MAX_FILES {
        return Some(error(StatusCode::PAYLOAD_TOO_LARGE, files::TOO_MANY_FILES));
    }
    None
}

/// Stores the bytes of `source` as a new file of the signed-in user. The
/// bytes go to the store before the index names them, outside the
/// Deployment's lock, so a slow store holds up no action; the quota is
/// checked before and again when the index changes.
pub async fn upload_from_path(state: &AppState, headers: &HeaderMap, upload: Upload<'_>, source: &Path) -> Response {
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let actor = match upload_actor(state, &deployment, headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let visibility = match upload.visibility {
        None | Some("private") => "private",
        Some("public") => "public",
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "File visibility must be private or public.",
            );
        }
    };
    let name = upload
        .name
        .unwrap_or("file")
        .replace(['/', '\\'], "-")
        .chars()
        .filter(|character| !character.is_control())
        .take(180)
        .collect::<String>();
    let content_type = upload
        .content_type
        .filter(|v| {
            v.len() < 128
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-/".contains(&c))
        })
        .unwrap_or("application/octet-stream")
        .to_owned();
    let size = match tokio::fs::metadata(source).await {
        Ok(metadata) => metadata.len(),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if size == 0 || size > state.files.limits.max_file_bytes {
        return error(StatusCode::PAYLOAD_TOO_LARGE, &state.files.limits.file_size_refused());
    }
    let directory = crate::layout::files(&state.data_root, &deployment.id);
    {
        let _guard = database_lock(state, &deployment.id).await;
        match read_file_index(&directory).await {
            Ok(files) => {
                if let Some(refused) = file_room(state, &files, size) {
                    return refused;
                }
            }
            Err(response) => return response,
        }
    }
    let id = Uuid::new_v4().to_string();
    if let Err(e) = state.files.store.put(&deployment.id, &id, source).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The file could not be stored: {e:#}"));
    }
    let metadata = json!({
        "id": id,
        "name": name,
        "type": content_type,
        "size": size,
        "visibility": visibility,
        "ownerId": actor.user_id,
        "createdAt": now_ms(),
    });
    let indexed = async {
        let _guard = database_lock(state, &deployment.id).await;
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        let mut files = read_file_index(&directory).await?;
        if let Some(refused) = file_room(state, &files, size) {
            return Err(refused);
        }
        files.insert(0, metadata.clone());
        let index = serde_json::to_vec_pretty(&files)
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        write_atomic(&directory.join("index.json"), &index)
            .await
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
    }
    .await;
    match indexed {
        Ok(()) => Json(json!({"result": metadata})).into_response(),
        Err(response) => {
            // No index names the bytes, so nothing can read them.
            if let Err(e) = state.files.store.delete(&deployment.id, &id).await {
                eprintln!("konae: could not remove the bytes of refused file {id}: {e:#}");
            }
            response
        }
    }
}

pub async fn serve_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let actor = match authenticate_deployment(&state, &deployment, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let private_deployment = is_private(&deployment);
    if private_deployment && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    if !valid_file_id(&id) {
        return error(StatusCode::NOT_FOUND, "File not found.");
    }
    let directory = crate::layout::files(&&state.data_root, &deployment.id);
    let index = match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes).unwrap_or_default(),
        Err(_) => return error(StatusCode::NOT_FOUND, "File not found."),
    };
    let Some(metadata) = index
        .iter()
        .find(|file| file["id"].as_str() == Some(id.as_str()))
    else {
        return error(StatusCode::NOT_FOUND, "File not found.");
    };
    if !file_readable(metadata, actor_id, private_deployment) {
        return error(StatusCode::NOT_FOUND, "File not found.");
    }
    file_response(&state, &deployment.id, metadata, &headers).await
}

/// The one byte range a `Range` header asks for, as `start..end`. `None`
/// for no header or one this path does not serve, which then gets the whole
/// file; `Err` for a range that starts past the end.
pub fn requested_range(headers: &HeaderMap, size: u64) -> Result<Option<std::ops::Range<u64>>, ()> {
    let Some(value) = headers.get(header::RANGE).and_then(|value| value.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((first, last)) = spec.split_once('-') else { return Ok(None) };
    let (first, last) = (first.trim(), last.trim());
    let range = match (first.parse::<u64>().ok(), last.parse::<u64>().ok()) {
        (Some(start), _) if start >= size => return Err(()),
        (Some(start), Some(end)) if end >= start => start..end.saturating_add(1).min(size),
        (Some(start), None) if last.is_empty() => start..size,
        (None, Some(suffix)) if first.is_empty() && suffix > 0 => size.saturating_sub(suffix)..size,
        _ => return Ok(None),
    };
    Ok(Some(range))
}

/// A stored file's bytes with its own type, streamed from the store. Raster
/// images display inline; everything else downloads, and nothing in it may
/// run. A single `Range` gets 206 with that part.
pub async fn file_response(state: &AppState, deployment: &str, metadata: &Value, headers: &HeaderMap) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    let Some(id) = metadata["id"].as_str() else { return not_found() };
    let size = metadata["size"].as_u64().unwrap_or(0);
    let range = match requested_range(headers, size) {
        Ok(range) => range,
        Err(()) => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return response;
        }
    };
    let blob = match state.files.store.get(deployment, id, range.clone()).await {
        Ok(Some(blob)) => blob,
        Ok(None) => return not_found(),
        Err(e) => {
            eprintln!("konae: could not read file {id}: {e:#}");
            return not_found();
        }
    };
    let content_type = metadata["type"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let inline = [
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/avif",
    ]
    .contains(&content_type);
    let length = blob.length;
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::with_capacity(blob.reader, 64 * 1024)));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(range) = range {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        if let Ok(value) = HeaderValue::from_str(&format!("bytes {}-{}/{size}", range.start, range.end.saturating_sub(1))) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(files::served_type(content_type))
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if inline { "inline" } else { "attachment" }),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

pub async fn serve_root(State(state): State<AppState>, headers: HeaderMap) -> Response {
    serve_key(state, headers, "index.html".into()).await
}

pub async fn login_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Konae deployment not found.");
    };
    match authenticate_deployment(&state, &deployment, &headers) {
        Ok(Some(_)) => (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response(),
        Ok(None) => private_login_page(&deployment.signup_policy, state.accounts.verification, google::offered_here(&state, &deployment, &headers)),
        Err(response) => response,
    }
}

pub async fn serve_artifact(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    serve_key(
        state,
        headers,
        if path.is_empty() {
            "index.html".into()
        } else {
            path
        },
    )
    .await
}

pub async fn serve_key(state: AppState, headers: HeaderMap, path: String) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Konae deployment not found."),
    };
    if is_private(&deployment) {
        match authenticate_deployment(&state, &deployment, &headers) {
            Ok(Some(_)) => {}
            Ok(None) => return private_login_page(&deployment.signup_policy, state.accounts.verification, google::offered_here(&state, &deployment, &headers)),
            Err(response) => return response,
        }
    }
    if artifact::kernel_path(&path) {
        return artifact::not_found_page();
    }
    let cache = ArtifactCache::of(&deployment);
    // A publish writes a new artifact version, so within one version an address
    // always resolves to the same bytes. An asset the browser already holds is
    // answered before the artifact is read. A navigation still resolves its file
    // so the page view counts by the file served.
    let navigation = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("document"));
    if !navigation && etag_matches(&headers, &cache.version_tag()) {
        return cache.not_modified(cache.version_tag());
    }
    let routing = artifact::routing(&deployment.manifest);
    for candidate in artifact::candidates(&path, routing) {
        if let Ok(bytes) = read_artifact(&state, &deployment, &candidate).await {
            count_page_view(&state, &deployment, &headers, &candidate);
            return cache.respond(&headers, bytes, &candidate);
        }
    }
    // A browser asks for /favicon.ico on every first visit. An app without one answers
    // with nothing instead of a 404 that every visitor's console reports as an error.
    if path == "favicon.ico" {
        return StatusCode::NO_CONTENT.into_response();
    }
    match routing {
        artifact::Routing::Spa => match read_artifact(&state, &deployment, "index.html").await {
            Ok(bytes) => {
                count_page_view(&state, &deployment, &headers, "index.html");
                cache.respond(&headers, bytes, "index.html")
            }
            Err(_) => error(StatusCode::NOT_FOUND, "Artifact not found."),
        },
        artifact::Routing::Pages => match read_artifact(&state, &deployment, "404.html").await {
            Ok(bytes) => {
                let mut response = artifact_response(bytes, "404.html");
                *response.status_mut() = StatusCode::NOT_FOUND;
                response
            }
            Err(_) => artifact::not_found_page(),
        },
    }
}

/// Counts a page view when a browser navigates to one of the artifact's HTML
/// pages. Only a navigation carries `Sec-Fetch-Dest: document`, so assets, a
/// script's fetch, a HEAD request, curl and most crawlers count nothing, and a
/// prefetch is skipped. The name is the file served, never the address asked
/// for: the artifact bounds the names, so no visitor can grow the store by
/// inventing paths. A single-page app's client-side navigation is invisible
/// here, and a view is a load, not a person (ADR-0010, page views).
pub fn count_page_view(state: &AppState, deployment: &Deployment, headers: &HeaderMap, file: &str) {
    let lower = file.to_ascii_lowercase();
    if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
        return;
    }
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or("");
    if !header("sec-fetch-dest").eq_ignore_ascii_case("document") {
        return;
    }
    if header("sec-purpose").contains("prefetch") || header("purpose").contains("prefetch") {
        return;
    }
    if headers.contains_key(header::COOKIE) && enter::entered_request(state, deployment, headers) {
        return;
    }
    usage::count(&state.data_root, &deployment.id, usage::Operation::PageView(file.to_owned()));
}

pub async fn read_artifact(state: &AppState, deployment: &Deployment, path: &str) -> Result<Vec<u8>> {
    if !safe_relative_path(path) {
        anyhow::bail!("invalid artifact path");
    }
    let local = layout::dist(&state.data_root, &deployment.id).join(path);
    match tokio::fs::read(local).await {
        Ok(bytes) => Ok(bytes),
        Err(local_error) => {
            let Some(s3) = state.s3.as_ref() else {
                return Err(local_error.into());
            };
            let key = format!("deployments/{}/{}/{path}", deployment.id, deployment.artifact_version);
            let object = s3
                .get_object()
                .bucket(&state.artifact_bucket)
                .key(key)
                .send()
                .await?;
            Ok(object.body.collect().await?.into_bytes().to_vec())
        }
    }
}

pub fn private_login_page(policy: &SignupPolicy, verification: auth::Verification, google: bool) -> Response {
    // With verification required, signup takes only the email: the password
    // is chosen on the page the emailed link opens.
    let verified = verification == auth::Verification::Required;
    let (note, create) = match policy.mode {
        SignupMode::Open if verified => (
            "New here? Enter your email and create an account. We send a link to choose your password.",
            true,
        ),
        SignupMode::Open => (
            "New here? Enter an email and password, then create an account.",
            true,
        ),
        SignupMode::Allowlist => (
            "Only invited email addresses can create an account.",
            true,
        ),
        SignupMode::Invitation => (
            "Only invited people can create an account. Open the link in your invitation email.",
            false,
        ),
        SignupMode::Closed => (
            "This app is not accepting new accounts. Sign in with an existing account.",
            false,
        ),
    };
    let body = [
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}form,label{display:grid;gap:8px}form{gap:16px;margin-top:24px}input,button{font:inherit;border-radius:8px;padding:11px 12px}input{width:100%;border:1px solid #c9ced6}button{border:0;background:#273449;color:white;cursor:pointer}button+button{background:#edf0f4;color:#252a33}#error{min-height:1.5em;margin:0;color:#a32727}@media (prefers-color-scheme:dark){body{background:#14161a;color:#e8eaed}main{background:#1d2026;border-color:#2e333b;box-shadow:none}p{color:#a9b0bc}input{background:#14161a;color:#e8eaed;border-color:#3a404a}input[readonly]{background:#23272e;color:#e8eaed}button{background:#e8eaed;color:#14161a}button+button{background:#2a2f37;color:#e8eaed}#error{color:#ff8a8a}a{color:#8ab4ff}}"#,
        if google { google::BUTTON_STYLE } else { "" },
        r#"</style><link rel="icon" href="data:,"><main><h1>Sign in to this app</h1><p>"#,
        note,
        "</p>",
        if google { google::BUTTON } else { "" },
        r#"<form id="auth"><label>Email <input name="email" type="email" required autocomplete="email"></label><label>Password <input name="password" type="password" minlength="8" required autocomplete="current-password"></label><button formaction="/_konae/auth/login">Sign in</button>"#,
        if create && verified {
            r#"<button formaction="/_konae/auth/signup" formnovalidate>Create account</button>"#
        } else if create {
            r#"<button formaction="/_konae/auth/signup">Create account</button>"#
        } else {
            ""
        },
        r#"<p id="error" role="alert"></p><a id="forgot" href="/_konae/account">Forgot your password?</a></form></main><script>{const m=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);const prefix=m?'/p/'+m[1]:'';document.querySelector('#forgot').href=prefix+'/_konae/account';document.querySelector('#auth').addEventListener('submit',async e=>{e.preventDefault();const path=prefix+new URL(e.submitter?.formAction||'/_konae/auth/login',location.href).pathname;const form=new FormData(e.currentTarget);const response=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({email:form.get('email'),password:form.get('password')})});const body=await response.json().catch(()=>({}));if(response.status===202){document.querySelector('#error').textContent=body.result.message;return}if(response.ok){location.replace(prefix+'/');return}document.querySelector('#error').textContent=body.error||'Sign in failed.'})}</script></html>"#,
    ]
    .concat();
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// How a browser may keep a Deployment's files. Every file is stored but
/// checked on each use (`no-cache`), so a publish shows at once and an
/// unchanged file costs a 304. A private app's files are `private` so no shared
/// cache holds a page that sits behind its login. File names are not
/// guaranteed to carry a content hash, so nothing is marked `immutable`.
pub struct ArtifactCache {
    pub version: String,
    pub private: bool,
}

impl ArtifactCache {
    pub fn of(deployment: &Deployment) -> Self {
        Self { version: deployment.artifact_version.clone(), private: is_private(deployment) }
    }

    pub fn version_tag(&self) -> String {
        format!("\"{}\"", self.version)
    }

    pub fn control(&self) -> HeaderValue {
        HeaderValue::from_static(if self.private { "private, no-cache" } else { "public, no-cache" })
    }

    pub fn not_modified(&self, tag: String) -> Response {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&tag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response.headers_mut().insert(header::CACHE_CONTROL, self.control());
        response
    }

    pub fn respond(&self, headers: &HeaderMap, bytes: Vec<u8>, path: &str) -> Response {
        let tag = self.version_tag();
        if etag_matches(headers, &tag) {
            return self.not_modified(tag);
        }
        let mut response = artifact_response(bytes, path);
        if let Ok(value) = HeaderValue::from_str(&tag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response.headers_mut().insert(header::CACHE_CONTROL, self.control());
        response
    }
}

/// `If-None-Match` holds a list of tags, possibly weak. A browser never sends
/// `*` on a GET, so it matches nothing here.
pub fn etag_matches(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|candidate| candidate.trim())
        .any(|candidate| candidate.strip_prefix("W/").unwrap_or(candidate) == tag)
}

pub fn artifact_response(bytes: Vec<u8>, path: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// The small export's whole budget: databases plus the base64 file bundle.

/// Runs one operation of the owner's database door on a Deployment: inspect,
/// browse, SQL, row edits and backups. `body` is its JSON input.
pub async fn admin_database(state: &AppState, id: &str, body: &[u8]) -> Response {
    let Some(deployment) = state
        .deployments
        .read()
        .await
        .values()
        .find(|d| d.id == id)
        .cloned()
    else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if body.len() > 200_000 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "Database request is too large.");
    }
    let body: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Database request must be valid JSON.",
            );
        }
    };
    let operation = body.get("operation").and_then(Value::as_str).unwrap_or("");
    let _project_guard = database_lock(&state, &format!("project:{}", deployment.project_id)).await;
    let path = database_path(&state, &deployment.id);
    let _guard = database_lock(&state, &deployment.id).await;
    match operation {
        "inspect" => match inspect_database(&path) {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        "browse" => match browse_table(&path, &body) {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        "sql" => {
            match run_admin_sql(&path, body.get("sql").and_then(Value::as_str).unwrap_or("")) {
                Ok(result) => Json(json!({"result": result})).into_response(),
                Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
            }
        }
        "update-cell" => admin_update_cell(&path, &body),
        "insert-row" => admin_insert_row(&path, &body),
        "delete-row" => admin_delete_row(&path, &body),
        "list-backups" => match list_backups(&state, &deployment.id).await {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        "create-backup" => match create_backup(
            &state,
            &deployment.id,
            body.get("label")
                .and_then(Value::as_str)
                .unwrap_or("Manual backup"),
        )
        .await
        {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        "restore-backup" => match restore_backup(
            &state,
            &deployment.id,
            body.get("backupId").and_then(Value::as_str).unwrap_or(""),
        )
        .await
        {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        _ => error(StatusCode::BAD_REQUEST, "Unknown database operation."),
    }
}

pub fn database_path(state: &AppState, id: &str) -> PathBuf {
    crate::layout::database(&state
        .data_root, &id)
}

/// Named locks on one database or transfer. Each is a read-write lock: a
/// Server Action shares its Database's lock, and work that reads or replaces
/// the whole file (a copy, a restore, an import, the owner's console) holds
/// it alone. An entry lives only while someone holds or waits for it, so
/// callers that pass many distinct names cannot grow the map.
pub type LockMap = Arc<std::sync::Mutex<HashMap<String, Arc<RwLock<()>>>>>;

/// The guard is kept only so that dropping it releases the lock.
#[allow(dead_code)]
pub enum Held {
    Shared(OwnedRwLockReadGuard<()>),
    Alone(OwnedRwLockWriteGuard<()>),
}

/// Holds one named lock. Dropping it releases the lock and removes the map
/// entry when no other task holds or waits for the same name.
pub struct NamedLockGuard {
    pub guard: Option<Held>,
    pub entry: Option<Arc<RwLock<()>>>,
    pub name: String,
    pub locks: LockMap,
}

impl Drop for NamedLockGuard {
    fn drop(&mut self) {
        let mut locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Release while holding the map lock, so no caller can insert a fresh
        // lock for this name while the old one is still held.
        self.guard.take();
        let entry = self.entry.take();
        // The map and this guard's handle are the only owners left when
        // nobody else holds or waits for the name.
        if entry.is_some_and(|entry| Arc::strong_count(&entry) == 2) {
            locks.remove(&self.name);
        }
    }
}

pub async fn named_lock(state: &AppState, name: &str, alone: bool) -> NamedLockGuard {
    let entry = {
        let mut locks = state.db_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    };
    // Built before waiting, so a caller cancelled while waiting still removes
    // the entry it may have created.
    let mut held = NamedLockGuard {
        guard: None,
        entry: Some(entry.clone()),
        name: name.to_owned(),
        locks: state.db_locks.clone(),
    };
    held.guard = Some(if alone {
        Held::Alone(entry.write_owned().await)
    } else {
        Held::Shared(entry.read_owned().await)
    });
    held
}

/// Holds the named lock alone: no Server Action runs on that Database, and
/// no one else holds the name, until the guard drops.
pub async fn database_lock(state: &AppState, id: &str) -> NamedLockGuard {
    let held = named_lock(state, id, true).await;
    // What runs under this lock may copy, replace or remove the file, so
    // no connection kept for a Server Action outlives it.
    app_db::forget(&database_path(state, id));
    held
}

/// What one Server Action holds on its Database while it runs. Queries share
/// the Database's lock, so they run side by side and beside a write: the
/// Database is in WAL mode, where a reader sees the last commit and never
/// waits for the writer. Mutations also take the Database's write lock, so
/// they reach SQLite one at a time instead of meeting `SQLITE_BUSY`.
pub struct ActionLock {
    pub _write: Option<NamedLockGuard>,
    pub _database: NamedLockGuard,
}

pub async fn action_lock(state: &AppState, id: &str, kind: &str) -> ActionLock {
    let database = named_lock(state, id, false).await;
    let write = if kind == "query" { None } else { Some(named_lock(state, &format!("write:{id}"), true).await) };
    ActionLock { _write: write, _database: database }
}

pub fn inspect_database(path: &Path) -> Result<Vec<Value>> {
    let db = Connection::open(path)?;
    let mut statement = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    names.into_iter().map(|name| {
        let columns = table_columns(&db, &name)?;
        let count_sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(&name));
        let count: i64 = db.query_row(&count_sql, [], |row| row.get(0))?;
        Ok(json!({"name": name, "columns": columns.iter().map(|column| column["name"].clone()).collect::<Vec<_>>(), "totalRows": count}))
    }).collect()
}

pub fn table_columns(db: &Connection, table: &str) -> Result<Vec<Value>> {
    if !table_exists(db, table)? {
        anyhow::bail!("Table not found: {table}");
    }
    let sql = format!("PRAGMA table_info({})", quote_identifier(table));
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map([], |row| Ok(json!({
        "name": row.get::<_, String>(1)?, "type": row.get::<_, String>(2)?,
        "notNull": row.get::<_, i64>(3)? != 0, "defaultValue": row.get::<_, Option<String>>(4)?,
        "primaryKey": row.get::<_, i64>(5)? != 0,
    })))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn table_exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn browse_table(path: &Path, body: &Value) -> Result<Value> {
    let db = Connection::open(path)?;
    restrict_database(&db)?;
    let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
    let columns = table_columns(&db, table)?;
    let names = columns
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect::<Vec<_>>();
    let page_size = body
        .get("pageSize")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 100) as i64;
    let page = body
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(i64::MAX as u64) as i64;
    let sort = body
        .get("sortColumn")
        .and_then(Value::as_str)
        .filter(|s| names.contains(s))
        .map(quote_identifier)
        .unwrap_or_else(|| "rowid".into());
    let direction = if body.get("sortDirection").and_then(Value::as_str) == Some("DESC") {
        "DESC"
    } else {
        "ASC"
    };
    let sql = format!(
        "SELECT rowid AS __konae_rowid, * FROM {} ORDER BY {sort} {direction} LIMIT ?1 OFFSET ?2",
        quote_identifier(table)
    );
    let mut statement = db.prepare(&sql)?;
    let col_names = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let rows = bounded_rows(statement
        .query_map([page_size, page.saturating_mul(page_size)], |row| {
            let mut values = serde_json::Map::new();
            let mut row_id = Value::Null;
            for (i, name) in col_names.iter().enumerate() {
                let value = sql_json(row.get_ref(i)?);
                if name == "__konae_rowid" {
                    row_id = value;
                } else {
                    values.insert(name.clone(), value);
                }
            }
            Ok(json!({"rowId": row_id, "values": values}))
        })?)?;
    let count_sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(table));
    let total: i64 = db.query_row(&count_sql, [], |row| row.get(0))?;
    Ok(
        json!({"columns": columns, "rows": rows, "totalRows": total, "page": page, "pageSize": page_size}),
    )
}

pub fn run_admin_sql(path: &Path, sql: &str) -> Result<Value> {
    if sql.trim().is_empty() {
        anyhow::bail!("Write a SQL statement first.");
    }
    if sql.len() > 100_000 {
        anyhow::bail!("SQL script is too large.");
    }
    let db = Connection::open(path)?;
    restrict_database(&db)?;
    let started = std::time::Instant::now();
    let first = sql
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("SQL")
        .to_ascii_uppercase();
    if ["SELECT", "PRAGMA", "EXPLAIN", "WITH"].contains(&first.as_str()) {
        let mut statement = db.prepare(sql)?;
        let names = statement
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        let mut result_bytes = 0;
        while let Some(row) = rows.next()? {
            let mut record = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                record.insert(name.clone(), sql_json(row.get_ref(index)?));
            }
            values.push(Value::Object(record));
            result_bytes += serde_json::to_vec(values.last().unwrap())?.len();
            if result_bytes > 2 * 1024 * 1024 {
                anyhow::bail!("SQL result exceeds 2 MiB.");
            }
            if values.len() > 1000 {
                anyhow::bail!("SQL returned more than 1000 rows.");
            }
        }
        Ok(
            json!({"columns": names, "rows": values, "changes": 0, "elapsedMs": started.elapsed().as_millis(), "statement": first}),
        )
    } else {
        let mut db = db;
        let tx = db.transaction()?;
        tx.execute_batch(sql)?;
        let changes = tx.changes();
        tx.commit()?;
        Ok(
            json!({"columns": [], "rows": [], "changes": changes, "elapsedMs": started.elapsed().as_millis(), "statement": first}),
        )
    }
}

pub fn admin_update_cell(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        let column = body.get("columnName").and_then(Value::as_str).unwrap_or("");
        let names = table_columns(&db, table)?;
        if !names.iter().any(|c| c["name"].as_str() == Some(column)) {
            anyhow::bail!("Unknown column.");
        }
        let sql = format!(
            "UPDATE {} SET {}=?1 WHERE rowid=?2",
            quote_identifier(table),
            quote_identifier(column)
        );
        db.execute(
            &sql,
            (
                value_to_sql(body.get("value"))?,
                value_to_sql(body.get("rowId"))?,
            ),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn admin_insert_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        let columns = table_columns(&db, table)?;
        let values = body
            .get("values")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let names = values.keys().cloned().collect::<Vec<_>>();
        if names
            .iter()
            .any(|n| !columns.iter().any(|c| c["name"].as_str() == Some(n)))
        {
            anyhow::bail!("Unknown column.");
        }
        if names.is_empty() {
            db.execute(
                &format!("INSERT INTO {} DEFAULT VALUES", quote_identifier(table)),
                [],
            )?;
        } else {
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_identifier(table),
                names
                    .iter()
                    .map(|n| quote_identifier(n))
                    .collect::<Vec<_>>()
                    .join(","),
                (1..=names.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let params = names
                .iter()
                .map(|n| value_to_sql(values.get(n)))
                .collect::<Result<Vec<_>>>()?;
            db.execute(&sql, params_from_iter(params))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn admin_delete_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        table_columns(&db, table)?;
        db.execute(
            &format!("DELETE FROM {} WHERE rowid=?1", quote_identifier(table)),
            [value_to_sql(body.get("rowId"))?],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn value_to_sql(value: Option<&Value>) -> Result<SqlValue> {
    let Some(value) = value else {
        return Ok(SqlValue::Null);
    };
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::String(v) => Ok(SqlValue::Text(v.clone())),
        Value::Bool(v) => Ok(SqlValue::Integer(*v as i64)),
        Value::Number(v) => v
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| v.as_f64().map(SqlValue::Real))
            .ok_or_else(|| anyhow::anyhow!("Invalid number.")),
        _ => anyhow::bail!("Unsupported SQLite value."),
    }
}

pub async fn list_backups(state: &AppState, id: &str) -> Result<Vec<Value>> {
    let dir = backup_dir(state, id);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut backups = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let backup: DatabaseBackup = serde_json::from_slice(&tokio::fs::read(entry.path()).await?)?;
        backups.push(backup.view());
    }
    backups.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
    Ok(backups)
}

pub async fn create_backup(state: &AppState, id: &str, label: &str) -> Result<Value> {
    let db_path = database_path(state, id);
    let dir = backup_dir(state, id);
    tokio::fs::create_dir_all(&dir).await?;
    let backup_id = Uuid::new_v4().to_string();
    let snapshot_path = dir.join(format!("{backup_id}.sqlite"));
    let db = Connection::open(&db_path)?;
    db.execute("VACUUM INTO ?1", [snapshot_path.to_string_lossy().as_ref()])?;
    let size = tokio::fs::metadata(&snapshot_path).await?.len();
    let backup = DatabaseBackup {
        id: backup_id.clone(),
        label: label.chars().take(120).collect(),
        created_at: now_ms(),
        size_bytes: size,
        key: format!("{backup_id}.sqlite"),
        holds: Vec::new(),
    };
    tokio::fs::write(
        dir.join(format!("{backup_id}.json")),
        serde_json::to_vec(&backup)?,
    )
    .await?;
    Ok(backup.view())
}

pub async fn restore_backup(state: &AppState, id: &str, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }
    // The metadata file proves the backup exists before anything changes.
    let record: DatabaseBackup = serde_json::from_slice(&tokio::fs::read(backup_dir(state, id).join(format!("{backup_id}.json"))).await?)?;
    if !record.holds.is_empty() {
        let deployment = state.deployments.read().await.values().find(|d| d.id == id).cloned().context("Deployment not found.")?;
        return import::restore_held(state, &deployment, &record).await;
    }
    let backup_path = backup_dir(state, id).join(format!("{backup_id}.sqlite"));
    let bytes = tokio::fs::read(&backup_path).await?;
    create_backup(state, id, "Before restore").await?;
    let temp_path = database_path(state, id).with_extension("restore.sqlite");
    tokio::fs::write(&temp_path, &bytes).await?;
    let check = Connection::open(&temp_path)?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    drop(check);
    if integrity != "ok" {
        let _ = tokio::fs::remove_file(&temp_path).await;
        anyhow::bail!("The backup failed SQLite's integrity check.");
    }
    replace_database_file(&temp_path, &database_path(state, id))?;
    Ok(json!({"id":backup_id,"sizeBytes":bytes.len()}))
}

/// Runs an action's statement on a blocking thread while the caller holds
/// the Deployment's database lock. The statement deadline is checked between
/// SQLite steps, and one step, such as `instr` or `replace` over two large
/// values, can run past it. On a blocking thread that delays only this
/// Deployment, never the async workers every other app shares.
/// What a statement did. `answer` is what the caller reads. `rows` are the
/// changed rows a mutation's notice reads back through the Kernel's own
/// RETURNING clause (ADR-0012); they never reach the answer.
pub struct Ran {
    pub answer: Value,
    pub rows: Vec<serde_json::Map<String, Value>>,
}

impl Ran {
    /// What a notice reads of this run, at `now` in milliseconds.
    pub fn changed(&self, now: u64) -> notify::Changed<'_> {
        notify::Changed { count: self.answer["changes"].as_u64().unwrap_or(0), rows: &self.rows, now }
    }
}

pub async fn run_action(path: PathBuf, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Ran> {
    let action = action.clone();
    tokio::task::spawn_blocking(move || {
        if action.kind == "query" {
            run_query(&path, &action, params).map(|rows| Ran { answer: Value::from(rows), rows: Vec::new() })
        } else {
            run_mutation(&path, &action, params)
        }
    })
    .await
    .map_err(anyhow::Error::from)?
}

pub fn run_query(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Vec<Value>> {
    let connection = app_db::open(path)?;
    let mut statement = connection.prepare_cached(&action.statement)?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rows = bounded_rows(statement
        .query_map(params_from_iter(params), |row| {
            let mut object = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                object.insert(name.clone(), sql_json(row.get_ref(index)?));
            }
            Ok(Value::Object(object))
        })?)?;
    if rows.len() > 1000 {
        anyhow::bail!("Action returned more than 1000 rows.");
    }
    Ok(rows)
}

pub fn run_mutation(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Ran> {
    let mut connection = app_db::open(path)?;
    let transaction = connection.transaction()?;
    // A notice that lists or addresses rows reads them back with a RETURNING
    // clause the Kernel appends; the author's statement cannot hold one.
    let columns = action.notify.as_ref().map(|_| notify::returning("", action)).unwrap_or_default();
    let mut rows = Vec::new();
    let changes = if columns.is_empty() {
        transaction.prepare_cached(&action.statement)?.execute(params_from_iter(params))? as u64
    } else {
        let mut statement = transaction.prepare_cached(&notify::with_returning(&action.statement, &columns))?;
        let mut cursor = statement.query(params_from_iter(params))?;
        // SQLite makes every change on the first step, so the rows past the
        // cap are skipped, not left unchanged.
        while let Some(row) = cursor.next()? {
            if rows.len() < notify::MAX_RETURNED_ROWS {
                let mut object = serde_json::Map::new();
                for (index, column) in columns.iter().enumerate() {
                    object.insert(column.clone(), sql_json(row.get_ref(index)?));
                }
                rows.push(object);
            }
        }
        drop(cursor);
        drop(statement);
        transaction.changes()
    };
    let last_insert_id = transaction.last_insert_rowid();
    transaction.commit()?;
    Ok(Ran { answer: json!({"changes": changes, "lastInsertId": last_insert_id}), rows })
}

/// Binds an action's parameters. `actor_id` is the caller's ID: a person's
/// or an API key's.
pub fn resolve_bindings(
    action: &ActionDefinition,
    input: &Value,
    actor_id: Option<&str>,
) -> std::result::Result<Vec<SqlValue>, String> {
    let object = input
        .as_object()
        .ok_or_else(|| "Action input must be an object.".to_string())?;
    let accepted = action
        .bindings
        .iter()
        .filter_map(|binding| {
            if binding.source == "input" {
                binding.key.as_ref()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if let Some(key) = object.keys().find(|key| !accepted.contains(key)) {
        return Err(format!("Unexpected action input: {key}"));
    }
    action
        .bindings
        .iter()
        .map(|binding| match binding.source.as_str() {
            "actor" if binding.field.as_deref() == Some("id") => actor_id
                .map(|id| SqlValue::Text(id.to_owned()))
                .ok_or_else(|| "This action requires an authenticated actor.".to_string()),
            "kernel" if binding.field.as_deref() == Some("now") => {
                Ok(SqlValue::Integer(now_ms() as i64))
            }
            "literal" => json_sql(binding.value.as_ref().unwrap_or(&Value::Null)),
            "input" => {
                let key = binding
                    .key
                    .as_ref()
                    .ok_or_else(|| "Input binding has no key.".to_string())?;
                if binding.nullable && object.get(key).is_none_or(Value::is_null) {
                    return Ok(SqlValue::Null);
                }
                let value = object
                    .get(key)
                    .ok_or_else(|| format!("Missing action input: {key}"))?;
                if binding.value_type.as_deref() == Some("string") {
                    let text = value
                        .as_str()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| format!("{key} must be a non-empty string."))?;
                    // Count UTF-16 code units so the limit matches the browser's String.length.
                    if text.encode_utf16().count() > binding.max_length.unwrap_or(10_000) {
                        return Err(format!("{key} is too long."));
                    }
                    Ok(SqlValue::Text(text.to_owned()))
                } else if binding.value_type.as_deref() == Some("number") {
                    // Integers bind as INTEGER like sql.js, so a TEXT column stores "5", not "5.0".
                    value
                        .as_i64()
                        .map(SqlValue::Integer)
                        .or_else(|| value.as_f64().map(SqlValue::Real))
                        .ok_or_else(|| format!("{key} must be a number."))
                } else if binding.value_type.as_deref() == Some("boolean") {
                    value
                        .as_bool()
                        .map(|value| SqlValue::Integer(value as i64))
                        .ok_or_else(|| format!("{key} must be a boolean."))
                } else {
                    Err(format!("Unsupported input type for {key}."))
                }
            }
            _ => Err(format!("Unsupported binding source: {}", binding.source)),
        })
        .collect()
}

pub fn validate_action(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    validate_declarations(name, action)?;
    action_shape::check_keys(name, action)
}

pub fn validate_declarations(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    action_shape::check_action(name, action)?;
    if calls::check_kind(name, action)?.is_some() {
        return Ok(());
    }
    usage::check_signal_action(name, action)?;
    if action.kind == "signal" {
        usage::declaration(name, action)?;
        http::binding(name, action)?;
        guard::declaration(name, action)?;
        notify::declaration(name, action)?;
        schedule::check_subscribe(name, action)?;
        schedule::declaration(name, action)?;
        return Ok(());
    }
    sql_rules::check_statement(name, action)?;
    if action.kind == "query" {
        if let Some(reason) = unbounded_query_reason(&action.statement) {
            return Err(refusal::refuse(
                "query.limit-missing",
                format!("Action {name} {reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one."),
            ));
        }
    }
    usage::declaration(name, action)?;
    http::binding(name, action)?;
    guard::declaration(name, action)?;
    notify::declaration(name, action)?;
    schedule::check_subscribe(name, action)?;
    schedule::declaration(name, action)?;
    Ok(())
}

/// Same rule as `src/lib/query-bound.ts`. A LIMIT counts only outside
/// parentheses, strings and comments. The editor and an exported Kernel refuse
/// the same query.
pub fn unbounded_query_reason(sql: &str) -> Option<&'static str> {
    let tokens = sql_tokens(sql);
    let top: Vec<&SqlTok> = tokens.iter().filter(|token| token.depth == 0).collect();
    if limit_index(&top).is_some() {
        return None;
    }
    if top.iter().any(|token| token.is_any(&["EXCEPT", "INTERSECT", "UNION"])) {
        return Some("combines more than one query and has no LIMIT on the whole result.");
    }
    if tokens.iter().any(|token| token.is_word("OVER")) {
        return Some("uses OVER, which keeps one result row per stored row, and has no LIMIT.");
    }
    let Some(from) = top.iter().position(|token| token.is_word("FROM")) else {
        return None;
    };
    for pair in top.windows(2) {
        if pair[0].is_word("GROUP") && pair[1].is_word("BY") {
            return Some("groups rows and has no LIMIT, so the result can grow with the data.");
        }
    }
    let Some(select) = top.iter().position(|token| token.is_word("SELECT")) else {
        return Some("reads rows and has no LIMIT.");
    };
    if select >= from || !aggregate_select_list(&top[select + 1..from]) {
        return Some("reads rows and has no LIMIT.");
    }
    None
}

/// Where the LIMIT clause starts among the top-level tokens. `:limit`,
/// `@limit` and `$limit` are named parameters, not the clause, as
/// `limitIndex` in src/lib/query-bound.ts reads them.
pub fn limit_index(top: &[&SqlTok]) -> Option<usize> {
    top.iter().enumerate().position(|(index, token)| {
        let parameter = index > 0 && [":", "@", "$"].iter().any(|symbol| top[index - 1].is_symbol(symbol));
        token.is_word("LIMIT") && !parameter
    })
}

pub fn aggregate_select_list(list: &[&SqlTok]) -> bool {
    let mut index = 0;
    if list.first().is_some_and(|token| token.is_any(&["ALL", "DISTINCT"])) {
        index = 1;
    }
    if index >= list.len() {
        return false;
    }
    let mut expressions = 0;
    while index < list.len() {
        if list[index].is_symbol(",") {
            index += 1;
            continue;
        }
        let Some(name) = list.get(index) else { return false; };
        let Some(open) = list.get(index + 1) else { return false; };
        if !name.is_any(&["AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL"]) || !open.is_symbol("(") {
            return false;
        }
        index += 2;
        let Some(close) = list.iter().skip(index).position(|token| token.is_symbol(")")) else {
            return false;
        };
        index += close + 1;
        if list.get(index).is_some_and(|token| token.is_word("AS")) {
            if !list.get(index + 1).is_some_and(|token| token.kind == SqlTokKind::Word || token.kind == SqlTokKind::Ident) {
                return false;
            }
            index += 2;
        } else if list.get(index).is_some_and(|token| token.kind == SqlTokKind::Word || token.kind == SqlTokKind::Ident) {
            index += 1;
        }
        expressions += 1;
        if index < list.len() && !list[index].is_symbol(",") {
            return false;
        }
    }
    expressions > 0
}

#[derive(Clone, Copy, PartialEq)]
pub enum SqlTokKind { Word, Ident, String, Symbol }

pub struct SqlTok {
    pub kind: SqlTokKind,
    pub text: String,
    pub depth: i32,
    /// Offset in chars into the statement, so a reader can quote the token as written.
    pub start: usize,
}

impl SqlTok {
    pub fn is_word(&self, word: &str) -> bool {
        self.kind == SqlTokKind::Word && self.text.eq_ignore_ascii_case(word)
    }

    pub fn is_any(&self, words: &[&str]) -> bool {
        self.kind == SqlTokKind::Word && words.iter().any(|word| self.text.eq_ignore_ascii_case(word))
    }

    pub fn is_symbol(&self, text: &str) -> bool {
        self.kind == SqlTokKind::Symbol && self.text == text
    }
}

pub fn sql_tokens(sql: &str) -> Vec<SqlTok> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut depth = 0i32;
    let mut index = 0;
    while index < chars.len() {
        let char = chars[index];
        let next = chars.get(index + 1).copied();
        if char == '-' && next == Some('-') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' { index += 1; }
            if index < chars.len() { index += 1; }
            continue;
        }
        if char == '/' && next == Some('*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') { index += 1; }
            index = (index + 2).min(chars.len());
            continue;
        }
        if char == '\'' || char == '"' || char == '`' || char == '[' {
            let close = if char == '[' { ']' } else { char };
            let mut end = index + 1;
            let mut text = String::new();
            while end < chars.len() {
                if chars[end] == close {
                    if close != ']' && chars.get(end + 1) == Some(&close) {
                        text.push(close);
                        end += 2;
                        continue;
                    }
                    break;
                }
                text.push(chars[end]);
                end += 1;
            }
            let span_end = (end + 1).min(chars.len());
            tokens.push(if char == '\'' {
                SqlTok { kind: SqlTokKind::String, text: String::new(), depth, start: index }
            } else {
                SqlTok { kind: SqlTokKind::Ident, text, depth, start: index }
            });
            index = span_end;
            continue;
        }
        if is_sql_word_start(char) {
            let mut end = index + 1;
            while end < chars.len() && is_sql_word_cont(chars[end]) { end += 1; }
            tokens.push(SqlTok { kind: SqlTokKind::Word, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '.' || chars[end] == '_') {
                end += 1;
            }
            tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_whitespace() {
            index += 1;
            continue;
        }
        if char == ')' { depth = (depth - 1).max(0); }
        tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: char.to_string(), depth, start: index });
        if char == '(' { depth += 1; }
        index += 1;
    }
    tokens
}

pub fn is_sql_word_start(char: char) -> bool {
    char.is_ascii_alphabetic() || char == '_' || (char as u32) >= 0x80
}

pub fn is_sql_word_cont(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_' || char == '$' || (char as u32) >= 0x80
}

pub fn initialize_database(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(())
}

/// Moves a Database written before WAL mode, or arrived from a snapshot or a
/// backup, into it. The mode is stored in the file, so this runs once. A
/// switch that meets another connection is left for the next one: the
/// Database still answers in its old mode meanwhile.
pub fn use_wal(db: &Connection) {
    let mode = db.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
    if !mode.is_ok_and(|mode| mode.eq_ignore_ascii_case("wal")) {
        let _ = db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
    }
}

/// Puts `replacement` where the Database at `path` was. The old file's WAL
/// and shared-memory index go first: left beside the new file, SQLite would
/// read the old file's pages into it. The caller holds the Database's lock
/// alone, so no connection has them open.
pub fn replace_database_file(replacement: &Path, path: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match std::fs::remove_file(&sidecar) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    std::fs::rename(replacement, path)
}

/// Writes a sibling temp file and renames it over `path`, so readers never see
/// a truncated file.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let temp = path.with_file_name(format!(".{name}.tmp-{}", Uuid::new_v4()));
    if let Err(error_value) = tokio::fs::write(&temp, bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    if let Err(error_value) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    Ok(())
}

pub fn valid_file_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

pub fn file_visibility(file: &Value) -> &'static str {
    if file.get("visibility").and_then(Value::as_str) == Some("public") {
        "public"
    } else {
        "private"
    }
}

pub fn file_owner(file: &Value) -> Option<&str> {
    file.get("ownerId").and_then(Value::as_str)
}

pub fn accepted_owner_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Private files belong to the uploader. Public files are readable by any
/// signed-in user of the deployment, and by a visitor when the deployment
/// itself is public. A stored file with no owner stays unreadable.
pub fn file_readable(file: &Value, actor_id: Option<&str>, private_deployment: bool) -> bool {
    if file_visibility(file) == "public" {
        return !private_deployment || actor_id.is_some();
    }
    actor_id.is_some() && file_owner(file) == actor_id
}

pub fn file_view(file: &Value, actor_id: Option<&str>) -> Value {
    let mut view = file.clone();
    if let Some(object) = view.as_object_mut() {
        object.remove("url");
        if file_owner(file) != actor_id {
            object.remove("ownerId");
        }
    }
    view
}

pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        output.push(ALPHABET[a >> 2] as char);
        output.push(ALPHABET[((a & 3) << 4) | (b >> 4)] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[c & 63] as char
        } else {
            '='
        });
    }
    output
}

pub fn decode_base64(value: &str) -> Result<Vec<u8>> {
    let normalized = value
        .trim()
        .trim_start_matches("data:")
        .split_once(',')
        .map(|(_, encoded)| encoded)
        .unwrap_or(value);
    let mut output = Vec::with_capacity(normalized.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0;
    for byte in normalized
        .bytes()
        .filter(|byte| *byte != b'=' && !byte.is_ascii_whitespace())
    {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => anyhow::bail!("Initial database must be base64."),
        };
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Ok(output)
}

pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Writes the app's settings to `konae.json` and the installation's part to
/// `.konae/deployment.json`.
pub async fn persist_deployment(state: &AppState, deployment: &Deployment) -> Result<()> {
    Settings::of(deployment).write(&state.data_root).await?;
    let installed = Installed {
        hostname: deployment.hostname.clone(),
        created_at: deployment.created_at,
        actions: deployment.actions.clone(),
        artifact_version: deployment.artifact_version.clone(),
    };
    tokio::fs::create_dir_all(layout::private(&state.data_root, &deployment.id)).await?;
    write_atomic(&layout::record(&state.data_root, &deployment.id), &serde_json::to_vec_pretty(&installed)?).await?;
    Ok(())
}

/// Writes the Deployment's two files without a runtime, for tests that seed a volume.
#[cfg(any(test, feature = "testing"))]
pub fn write_record_for_test(root: &Path, deployment: &Deployment) {
    let installed = Installed {
        hostname: deployment.hostname.clone(),
        created_at: deployment.created_at,
        actions: deployment.actions.clone(),
        artifact_version: deployment.artifact_version.clone(),
    };
    std::fs::create_dir_all(layout::private(root, &deployment.id)).unwrap();
    std::fs::write(layout::settings(root, &deployment.id), serde_json::to_vec(&Settings::of(deployment)).unwrap()).unwrap();
    std::fs::write(layout::record(root, &deployment.id), serde_json::to_vec(&installed).unwrap()).unwrap();
}

/// Reads every published app on the volume. A folder without a Deployment
/// record is an app that was never published. A Kernel on a fresh volume
/// starts empty; the `restore` command installs copied folders first.
pub async fn load_deployments(root: &Path, public_origin: &str) -> Result<HashMap<String, Deployment>> {
    let mut result = HashMap::new();
    let mut entries = tokio::fs::read_dir(layout::apps(root)).await?;
    while let Some(entry) = entries.next_entry().await? {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(bytes) = tokio::fs::read(layout::record(root, &id)).await else { continue };
        let installed: Installed = serde_json::from_slice(&bytes).with_context(|| format!("{} is not a Deployment record.", layout::record(root, &id).display()))?;
        let settings = Settings::read(root, &id).with_context(|| format!("{} is not readable.", layout::settings(root, &id).display()))?;
        let deployment = settings.deployment(installed, public_origin);
        result.insert(deployment.hostname.clone(), deployment);
    }
    Ok(result)
}

pub async fn read_s3_bytes(s3: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let object = s3.get_object().bucket(bucket).key(key).send().await?;
    Ok(object.body.collect().await?.into_bytes().to_vec())
}

pub async fn list_s3_keys(s3: &S3Client, bucket: &str, prefix: &str) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = s3
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.take())
            .send()
            .await?;
        keys.extend(page.contents().iter().filter_map(|object| object.key().map(str::to_owned)));
        match page.next_continuation_token() {
            Some(token) if page.is_truncated() == Some(true) => continuation = Some(token.to_owned()),
            _ => break,
        }
    }
    Ok(keys)
}

/// The Deployment a request is for. A generated hostname and a verified
/// alias answer first, as before. Otherwise the request is on the path host,
/// and `path::route` has put the Deployment it resolved in `RESOLVED_HEADER`,
/// which the client cannot set because the route removes it first.
pub async fn deployment_for(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
    if let Some(deployment) = deployment_for_host(state, headers).await {
        return Some(deployment);
    }
    let id = headers.get(path::RESOLVED_HEADER)?.to_str().ok()?;
    deployment_by_id(state, id).await
}

/// The Deployment for a generated hostname or a verified alias.
pub async fn deployment_for_host(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
    let host = headers
        .get(header::HOST)?
        .to_str()
        .ok()?
        .split(':')
        .next()?;
    let deployments = state.deployments.read().await;
    if let Some(deployment) = deployments.get(host) {
        return Some(deployment.clone());
    }
    // A verified custom domain answers as the Deployment's own host.
    let id = domain::serving(state, host)?;
    deployments.values().find(|deployment| deployment.id == id).cloned()
}

/// The Deployment a request answers, before `path::route` strips its prefix.
/// The security headers layer runs outside the router and reads the path itself.
pub async fn deployment_for_request(state: &AppState, headers: &HeaderMap, path: &str) -> Option<Deployment> {
    if let Some(deployment) = deployment_for_host(state, headers).await {
        return Some(deployment);
    }
    let id = path::resolve(state, headers, path).await?.deployment_id?;
    deployment_by_id(state, &id).await
}

pub async fn deployment_by_id(state: &AppState, id: &str) -> Option<Deployment> {
    state.deployments.read().await.values().find(|deployment| deployment.id == id).cloned()
}

/// Fails closed: a Deployment is public only when its manifest says "public",
/// so a record with no visibility or an unknown one needs a login.
/// The Signup Policy as `signup_policy_read` answers it.
pub fn signup_policy_value(deployment: &Deployment) -> Value {
    json!({"deploymentId":deployment.id,"visibility":if is_private(deployment) { "private" } else { "public" },"mode":deployment.signup_policy.mode,"allowlist":deployment.signup_policy.allowlist})
}

/// Sets who may create an account in the app `id`, as Konae's
/// `signup_policy_set` and the `kernel` binary's `signup-policy` both do.
/// The entries stay as they are when `entries` is `None`. The change lands
/// in `konae.json`, which a running `kernel` binary follows, and the answer
/// is `signup_policy_value` with `previousMode`.
pub async fn set_signup_policy(state: &AppState, id: &str, mode: SignupMode, entries: Option<Vec<String>>) -> Result<Value> {
    let served = |deployments: &HashMap<String, Deployment>| {
        find_project_deployment(deployments, id).ok_or_else(|| anyhow::anyhow!("The app {id} is not published."))
    };
    let current = served(&*state.deployments.read().await)?;
    let entries = entries.unwrap_or_else(|| current.signup_policy.allowlist.clone());
    let policy = SignupPolicy::from_input(mode, &entries).map_err(anyhow::Error::msg)?;
    // Releases hold the same lock, so a publish cannot overwrite this change
    // with the policy it read before.
    let _guard = database_lock(state, &format!("project:{}", current.project_id)).await;
    let mut updated = served(&*state.deployments.read().await)?;
    let previous = updated.signup_policy.mode;
    updated.signup_policy = policy;
    persist_deployment(state, &updated).await?;
    state.deployments.write().await.insert(updated.hostname.clone(), updated.clone());
    let mut value = signup_policy_value(&updated);
    value["previousMode"] = json!(previous);
    Ok(value)
}

pub fn is_private(deployment: &Deployment) -> bool {
    deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        != Some("public")
}


pub fn open_auth(
    state: &AppState,
    deployment: &Deployment,
) -> std::result::Result<AuthService, AuthError> {
    accounts::app_store(state, deployment)
}

pub fn authenticate_deployment(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
) -> std::result::Result<Option<AuthUser>, Response> {
    let auth = open_auth(state, deployment)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let token = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(headers, &app_cookie_name(state, &deployment.id, headers)));
    auth.authenticate(token.as_deref())
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// The app session cookie's name for a request to a Deployment.
///
/// A generated hostname and a verified alias keep their own origin, so the
/// name is the one they always had, with `__Host-` under HTTPS. A path address
/// shares `<domain>` with every other app and the home, so the name carries
/// the Deployment id. The cookie is `Path=/` because one app answers on two
/// addresses, `/p/<id>` and `/p/<name>`, and a `Path=/p/<segment>/` cookie set
/// on one would not reach the other. A `__Host-` name also needs `Path=/`. The
/// per-app name keeps two apps' sessions from overwriting each other; it does
/// not keep a hostile app out, since every path shares one origin (ADR-0014).
pub fn app_cookie_name(state: &AppState, deployment_id: &str, headers: &HeaderMap) -> String {
    let https = state.public_origin.starts_with("https://");
    if path::on_path_host(state, headers) {
        return format!("{}konae_app_session_{deployment_id}", if https { "__Secure-" } else { "" });
    }
    if https { "__Host-konae_app_session" } else { "konae_app_session" }.to_owned()
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

pub fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site == "cross-site" {
            return false;
        }
    }
    match (
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        host,
    ) {
        (Some(origin), Some(host)) => url::Url::parse(origin).ok().is_some_and(|origin| {
            let expected = url::Url::parse(&format!("{}://{host}", origin.scheme())).ok();
            expected.is_some_and(|expected| origin.origin() == expected.origin())
        }),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// The longest LIKE or GLOB pattern an action may compare.
pub const MAX_LIKE_PATTERN_BYTES: i32 = 500;

/// Each app database may grow to this size. Writes past it fail with
/// "database or disk is full".
pub const MAX_APP_DATABASE_BYTES: u64 = 256 * 1024 * 1024;

/// Caps the database file behind this connection. SQLite keeps the setting per
/// connection, so every connection that may write needs it.
pub fn limit_database_size(db: &Connection, max_bytes: u64) -> Result<()> {
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let pages = (max_bytes / page_size.max(1)).max(1);
    db.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))?;
    Ok(())
}

/// SQL functions no app statement or schema may call. They reach the file
/// system, load native code, or swap full-text tokenizers by pointer.
pub const DENIED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "edit",
    "fts3_tokenizer",
];

pub fn denied_function(name: &str) -> bool {
    DENIED_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str())
}

pub fn restrict_database(db: &Connection) -> Result<()> {
    // Readers share a Database with its one writer (`action_lock`), so a
    // connection waits for a lock instead of failing on the first try.
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    // In WAL mode a reader never waits for the writer, and a commit appends
    // to the log instead of rewriting a journal. NORMAL syncs at checkpoints,
    // not at every commit: a Kernel that crashes loses nothing, and a host
    // that loses power can lose the last commits but never corrupts the file.
    db.pragma_update(None, "synchronous", "NORMAL")?;
    use_wal(db);
    // Runs before the authorizer below, which refuses this pragma.
    limit_database_size(db, MAX_APP_DATABASE_BYTES)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 100_000)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    // LIKE and GLOB compare a value against the pattern inside one step, so
    // the deadline below cannot stop them; a short pattern keeps that step
    // short. SQLite's default is 50,000 bytes.
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, MAX_LIKE_PATTERN_BYTES)?;
    arm_deadline(db);
    db.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        // Konae runs no triggers or views, and Studio and the schema change
        // use this connection too, so neither can add one the Gate never saw.
        AuthAction::CreateTrigger { .. } | AuthAction::CreateTempTrigger { .. } => Authorization::Deny,
        AuthAction::CreateView { .. } | AuthAction::CreateTempView { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, .. } if !["table_info", "table_xinfo", "index_list", "index_info", "foreign_key_list", "foreign_key_check", "integrity_check", "quick_check"].contains(&pragma_name.to_ascii_lowercase().as_str()) => Authorization::Deny,
        AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    Ok(())
}

/// Interrupts any statement on `db` that runs past 2 seconds from now.
pub fn arm_deadline(db: &Connection) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    db.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline));
}

pub fn bounded_rows(rows: impl Iterator<Item = rusqlite::Result<Value>>) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    let mut bytes = 0;
    for row in rows.take(1001) {
        let row = row?;
        bytes += serde_json::to_vec(&row)?.len();
        anyhow::ensure!(bytes <= 2 * 1024 * 1024 && values.len() < 1000, "Query result exceeds 2 MiB or 1000 rows.");
        values.push(row);
    }
    Ok(values)
}

pub fn auth_error(error_value: AuthError) -> Response {
    let status = match error_value {
        AuthError::InvalidEmail | AuthError::WeakPassword => StatusCode::BAD_REQUEST,
        AuthError::EmailTaken | AuthError::AccountExists => StatusCode::CONFLICT,
        AuthError::InvitedEmailDiffers => StatusCode::FORBIDDEN,
        AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::AccountPending
        | AuthError::AccountSuspended
        | AuthError::SignupClosed
        | AuthError::AdmissionRefused(_) => {
            StatusCode::FORBIDDEN
        }
        AuthError::InvalidChallenge => StatusCode::GONE,
        AuthError::MailUnavailable | AuthError::MailBlocked(_) => StatusCode::SERVICE_UNAVAILABLE,
        AuthError::MailLimit(_) => StatusCode::TOO_MANY_REQUESTS,
        AuthError::VerificationRequired
        | AuthError::Storage(_)
        | AuthError::PasswordHash => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &error_value.to_string())
}

pub fn json_sql(value: &Value) -> std::result::Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        Value::Bool(value) => Ok(SqlValue::Integer(*value as i64)),
        Value::Number(value) => value
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| value.as_f64().map(SqlValue::Real))
            .ok_or_else(|| "Invalid number.".into()),
        _ => Err("Literal values must be scalar.".into()),
    }
}

pub fn sql_json(value: rusqlite::types::ValueRef<'_>) -> Value {
    match value {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(value) => json!(value),
        rusqlite::types::ValueRef::Real(value) => json!(value),
        rusqlite::types::ValueRef::Text(value) => {
            Value::String(String::from_utf8_lossy(value).into_owned())
        }
        rusqlite::types::ValueRef::Blob(value) => Value::String(hex::encode(value)),
    }
}

pub fn deployment_url(origin: &str, hostname: &str) -> String {
    let scheme = origin.split("://").next().unwrap_or("http");
    let port = origin
        .split("://")
        .nth(1)
        .and_then(|rest| rest.rsplit_once(':').map(|(_, port)| port));
    format!(
        "{scheme}://{hostname}{}",
        port.map(|value| format!(":{value}")).unwrap_or_default()
    )
}

pub fn safe_slug(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
pub fn content_type(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "rss" => "application/rss+xml; charset=utf-8",
        "atom" => "application/atom+xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

pub const DEFAULT_ADMIN_KEY: &str = "local-development-key";

/// True for localhost, *.localhost and loopback IP addresses.
pub fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}


pub fn base64url(value: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() % 4 != 1
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid base64url encoding."
    );
    let bytes = crate::decode_base64(value)?;
    let canonical = crate::encode_base64(&bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_");
    anyhow::ensure!(canonical == value, "Noncanonical base64url encoding.");
    Ok(bytes)
}


/// The name this process was started as, `kernel` or `konae`, for the
/// commands an answer tells the operator to run.
pub fn program() -> String {
    std::env::args()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "kernel".into())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

/// A refused Capsule: `error` stays the sentence clients read, and `code` and
/// `hint` sit beside it (ADR-0004).
pub fn refusal_error(refused: &refusal::Refusal) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": refused.message, "code": refused.code, "hint": refused.hint()}))).into_response()
}

#[cfg(any(test, feature = "testing"))]
#[allow(dead_code, unused_imports)]
pub mod tests {
    use super::*;

    #[test]
    fn deploy_input_reads_the_initial_signup_policy() {
        let input: DeployInput = serde_json::from_value(json!({
            "project": { "id": "app_1", "name": "App", "slug": "app", "files": {}, "manifest": {} },
            "visibility": "public",
            "artifact": { "version": 1, "entrypoint": "index.html", "files": { "index.html": "<main></main>" } },
            "initialSignupPolicy": { "mode": "closed", "allowlist": [] }
        })).unwrap();

        assert_eq!(input.initial_signup_policy.unwrap().mode, SignupMode::Closed);
    }

    pub async fn login_test_state(root: &Path, visibility: &str) -> (AppState, HeaderMap) {
        let app_id = Uuid::new_v4().to_string();
        let deployment = Deployment {
            id: app_id.clone(),
            project_id: app_id,
            project_name: "Login test".into(),
            slug: "app".into(),
            url: "http://login-test.localhost".into(),
            created_at: now_ms(),
            manifest: json!({"visibility": visibility}),
            hostname: "login-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: "v1".into(),
            signup_policy: SignupPolicy::default(),
            sign_in_methods: default_sign_in_methods(),
            contact_email: None,
        };
        tokio::fs::create_dir_all(crate::layout::data(&root, &deployment.id))
            .await
            .unwrap();
        let state = AppState {
            deployments: Arc::new(RwLock::new(HashMap::from([(
                deployment.hostname.clone(),
                deployment,
            )]))),
            ..AppState::for_test(&root)
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("login-test.localhost"),
        );
        (state, headers)
    }

    fn action(kind: &str, statement: &str, bindings: Vec<ActionBinding>) -> ActionDefinition {
        ActionDefinition {
            kind: kind.into(),
            access: "anonymous".into(),
            statement: statement.into(),
            bindings,
            http: None,
            signal: None,
            guard: None,
            notify: None,
            schedule: None,
            subscribe: None,
            call: Default::default(),
            extra: Default::default(),
        }
    }

    fn apply_schema(path: &Path, schema: &str, program: &ActionProgram) -> Result<schema_change::Change> {
        schema_change::apply(path, schema, program, schema_change::Mode::Apply(None))
    }

    fn action_program(actions: &[(&str, &str)]) -> ActionProgram {
        ActionProgram {
            version: 1,
            actions: actions
                .iter()
                .map(|(name, statement)| ((*name).into(), action("query", statement, Vec::new())))
                .collect(),
        }
    }

    #[test]
    fn actions_accept_one_constrained_statement_and_reject_structural_sql() {
        assert!(
            validate_action(
                "tasks.list",
                &action("query", "SELECT id FROM tasks LIMIT 10", vec![])
            )
            .is_ok()
        );
        assert!(
            validate_action(
                "tasks.create",
                &action("mutation", "INSERT INTO tasks(title) VALUES (?)", vec![])
            )
            .is_ok()
        );
        assert!(
            validate_action(
                "tasks.drop",
                &action("mutation", "DROP TABLE tasks", vec![])
            )
            .is_err()
        );
        assert!(
            validate_action("tasks.read", &action("query", "SELECT 1; SELECT 2", vec![])).is_err()
        );
        let mut granted = action("mutation", "DELETE FROM tasks", vec![]);
        granted.access = "granted".into();
        assert!(validate_action("tasks.delete", &granted).is_err());
        assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks", vec![]))
            .unwrap_err()
            .message
            .contains("reads rows and has no LIMIT"));
        assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks WHERE title = 'LIMIT 5'", vec![])).is_err());
        assert!(validate_action("tasks.count", &action("query", "SELECT count(*) AS n FROM tasks", vec![])).is_ok());
        assert!(validate_action("votes.results", &action("query", "SELECT choice, count(*) AS n FROM votes GROUP BY choice", vec![])).is_err());
        assert!(validate_action("tasks.one", &action("query", "SELECT 1", vec![])).is_ok());
    }

    #[test]
    fn action_inputs_are_named_typed_and_closed() {
        let definition = action(
            "mutation",
            "INSERT INTO tasks(title) VALUES (?)",
            vec![ActionBinding {
                source: "input".into(),
                key: Some("title".into()),
                value_type: Some("string".into()),
                max_length: Some(20),
                nullable: false,
                field: None,
                value: None,
            }],
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"title": "Ship it"}), None).unwrap(),
            vec![SqlValue::Text("Ship it".into())]
        );
        assert!(
            resolve_bindings(
                &definition,
                &json!({"title": "Ship it", "sql": "DROP TABLE tasks"}),
                None,
            )
            .unwrap_err()
            .contains("Unexpected")
        );
        assert!(
            resolve_bindings(&definition, &json!({"title": 42}), None)
                .unwrap_err()
                .contains("non-empty string")
        );
    }

    #[tokio::test]
    async fn login_route_is_available_for_public_and_private_apps() {
        for visibility in ["public", "private"] {
            let root = std::env::temp_dir()
                .join(format!("konae-login-{visibility}-{}", Uuid::new_v4()));
            let (state, headers) = login_test_state(&root, visibility).await;
            let response = login_page(State(state), headers).await;
            assert_eq!(response.status(), StatusCode::OK, "visibility={visibility}");
            let bytes = axum::body::to_bytes(response.into_body(), 16_384)
                .await
                .unwrap();
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(html.contains("Sign in to this app"));
            assert!(html.contains("/_konae/auth/signup"));
            assert!(!html.contains("Account creation is open"));
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    async fn set_policy(state: &AppState, mode: SignupMode, allowlist: &[&str]) {
        let entries = allowlist.iter().map(|e| e.to_string()).collect::<Vec<_>>();
        for deployment in state.deployments.write().await.values_mut() {
            deployment.signup_policy = SignupPolicy::from_input(mode, &entries).unwrap();
        }
    }

    async fn signup_status(state: &AppState, headers: &HeaderMap, email: &str) -> StatusCode {
        let body = json!({"email": email, "password": "correct horse battery"});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        signup(State(state.clone()), headers.clone(), request).await.status()
    }

    #[test]
    fn signup_policy_matches_exact_emails_and_whole_domains() {
        let open = SignupPolicy::default();
        assert!(open.allows("anyone@example.test"));
        let closed = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        assert!(!closed.allows("anyone@example.test"));
        let invitation: SignupPolicy = serde_json::from_value(json!({"mode": "invitation"})).unwrap();
        assert!(!invitation.allows("anyone@example.test"), "Invitation only refuses every plain signup");
        let list = SignupPolicy::from_input(
            SignupMode::Allowlist,
            &[" Carol@Example.TEST ".into(), "@Team.Example".into(), "carol@example.test".into()],
        )
        .unwrap();
        assert_eq!(list.allowlist, vec!["carol@example.test", "@team.example"]);
        assert!(list.allows("CAROL@example.test"));
        assert!(list.allows("dana@TEAM.example"));
        assert!(!list.allows("dana@sub.team.example"));
        assert!(!list.allows("dana@evilteam.example"));
        assert!(!list.allows("team.example@attacker.test"));
        assert!(!list.allows("erin@example.test"));
        assert!(!list.allows("not-an-email"));
        let empty = SignupPolicy::from_input(SignupMode::Allowlist, &[]).unwrap();
        assert!(!empty.allows("carol@example.test"));
        for invalid in ["@", "@nodot", "two@@example.test", "@bad domain.test", "plain"] {
            assert!(
                SignupPolicy::from_input(SignupMode::Allowlist, &[invalid.into()]).is_err(),
                "entry={invalid}"
            );
        }
        let too_many = (0..=MAX_ALLOWLIST_ENTRIES)
            .map(|i| format!("user{i}@example.test"))
            .collect::<Vec<_>>();
        assert!(SignupPolicy::from_input(SignupMode::Allowlist, &too_many).is_err());
    }

    #[tokio::test]
    async fn signup_follows_the_deployment_policy() {
        let root = std::env::temp_dir().join(format!("konae-signup-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        assert_eq!(signup_status(&state, &headers, "open@example.test").await, StatusCode::OK);

        set_policy(&state, SignupMode::Closed, &[]).await;
        assert_eq!(signup_status(&state, &headers, "closed@example.test").await, StatusCode::FORBIDDEN);

        set_policy(&state, SignupMode::Allowlist, &["Listed@Example.test", "@team.example"]).await;
        assert_eq!(signup_status(&state, &headers, "listed@EXAMPLE.test").await, StatusCode::OK);
        assert_eq!(signup_status(&state, &headers, "any@team.example").await, StatusCode::OK);
        assert_eq!(signup_status(&state, &headers, "other@example.test").await, StatusCode::FORBIDDEN);
        assert_eq!(signup_status(&state, &headers, "any@sub.team.example").await, StatusCode::FORBIDDEN);

        // Existing accounts still sign in when signup is closed.
        set_policy(&state, SignupMode::Closed, &[]).await;
        let body = json!({"email": "open@example.test", "password": "correct horse battery"});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        assert_eq!(login(State(state.clone()), headers.clone(), request).await.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn login_page_hides_account_creation_when_signup_is_closed() {
        let root = std::env::temp_dir().join(format!("konae-login-policy-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "private").await;
        let page = |state: AppState| {
            let headers = headers.clone();
            async move {
                let response = login_page(State(state), headers).await;
                let bytes = axum::body::to_bytes(response.into_body(), 16_384).await.unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };
        set_policy(&state, SignupMode::Closed, &[]).await;
        let html = page(state.clone()).await;
        assert!(!html.contains("/_konae/auth/signup"));
        assert!(html.contains("not accepting new accounts"));
        set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
        let html = page(state.clone()).await;
        assert!(html.contains("/_konae/auth/signup"));
        assert!(html.contains("Only invited email addresses"));
        set_policy(&state, SignupMode::Invitation, &[]).await;
        let html = page(state.clone()).await;
        assert!(!html.contains("/_konae/auth/signup"));
        assert!(html.contains("Only invited people can create an account"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn private_apps_refuse_anonymous_actions_without_a_session() {
        let root = std::env::temp_dir().join(format!("konae-private-action-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "private").await;
        let program = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n", "bindings": []}}});
        for deployment in state.deployments.write().await.values_mut() {
            deployment.actions = program.to_string();
            initialize_database(&database_path(&state, &deployment.id)).unwrap();
        }
        let call = |state: AppState| {
            let headers = headers.clone();
            async move {
                invoke_action(State(state), AxumPath("items.count".into()), headers, Json(json!({})))
                    .await
                    .status()
            }
        };
        assert_eq!(call(state.clone()).await, StatusCode::UNAUTHORIZED);
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest = json!({"visibility": "public"});
        }
        assert_eq!(call(state.clone()).await, StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_deployment_that_does_not_say_public_serves_no_asset_without_a_login() {
        let root = std::env::temp_dir().join(format!("konae-fail-closed-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let id = state.deployments.read().await.values().next().unwrap().id.clone();
        let artifact = layout::dist(&state.data_root, &id);
        tokio::fs::create_dir_all(&artifact).await.unwrap();
        tokio::fs::write(artifact.join("report.txt"), "secret").await.unwrap();
        let read = |manifest: Value| {
            let (state, headers) = (state.clone(), headers.clone());
            async move {
                for deployment in state.deployments.write().await.values_mut() {
                    deployment.manifest = manifest.clone();
                }
                let response = serve_key(state, headers, "report.txt".into()).await;
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
                String::from_utf8_lossy(&bytes).into_owned()
            }
        };
        assert_eq!(read(json!({"visibility": "public"})).await, "secret");
        for manifest in [json!({}), json!({"visibility": null}), json!({"visibility": "Public"}), json!({"visibility": "open"})] {
            let deployment = Deployment { manifest: manifest.clone(), ..state.deployments.read().await.values().next().unwrap().clone() };
            assert!(is_private(&deployment), "{manifest}");
            let body = read(manifest.clone()).await;
            assert!(!body.contains("secret") && body.contains("<form"), "{manifest}");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_scheduled_mutation_without_a_route_runs_only_on_its_schedule() {
        let root = std::env::temp_dir().join(format!("konae-scheduled-action-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let program = json!({"version": 1, "actions": {"holds.release": {
            "kind": "mutation", "access": "anonymous", "schedule": {"every": "15m"},
            "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL", "bindings": [{"source": "kernel", "field": "now"}]
        }}});
        for deployment in state.deployments.write().await.values_mut() {
            deployment.actions = program.to_string();
            deployment.manifest = json!({"visibility": "public"});
        }
        let response = invoke_action(State(state), AxumPath("holds.release".into()), headers, Json(json!({}))).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Action holds.release runs only on its schedule."));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn redeploy_adds_tables_without_losing_rows_and_rejects_destructive_schema() {
        let root = std::env::temp_dir().join(format!("konae-schema-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();

        let initial = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let original_action = action_program(&[("tasks.list", "SELECT id, title FROM tasks")]);
        apply_schema(&path, initial, &original_action).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO tasks(title) VALUES ('Keep this row')", [])
            .unwrap();

        let extended = format!(
            "{initial} CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL); CREATE INDEX IF NOT EXISTS notes_body ON notes(body);"
        );
        let extended_actions = action_program(&[
            ("tasks.list", "SELECT id, title FROM tasks"),
            ("notes.list", "SELECT id, body FROM notes"),
        ]);
        apply_schema(&path, &extended, &extended_actions).unwrap();
        apply_schema(&path, &extended, &extended_actions).unwrap();

        let connection = Connection::open(&path).unwrap();
        let title: String = connection
            .query_row("SELECT title FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Keep this row");
        assert!(connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='notes')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        drop(connection);

        assert!(apply_schema(&path, "DROP TABLE tasks;", &extended_actions).is_err());
        let connection = Connection::open(&path).unwrap();
        let title: String = connection
            .query_row("SELECT title FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Keep this row");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn schemas_may_call_safe_functions_but_not_dangerous_ones() {
        let root = std::env::temp_dir().join(format!("konae-schema-functions-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS items (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL CHECK (length(name) > 0),
              title TEXT NOT NULL DEFAULT '',
              slug TEXT GENERATED ALWAYS AS (lower(name)) VIRTUAL,
              created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS items_title ON items(lower(title));
            CREATE INDEX IF NOT EXISTS items_named ON items(name) WHERE length(name) > 0;";
        let actions = action_program(&[("items.list", "SELECT id, name, slug FROM items")]);
        apply_schema(&path, schema, &actions).unwrap();
        let connection = Connection::open(&path).unwrap();
        assert!(connection.execute("INSERT INTO items(name) VALUES ('')", []).is_err(), "the CHECK constraint runs");
        connection.execute("INSERT INTO items(name) VALUES ('Mixed')", []).unwrap();
        let slug: String = connection.query_row("SELECT slug FROM items", [], |row| row.get(0)).unwrap();
        assert_eq!(slug, "mixed");
        drop(connection);

        for function in DENIED_FUNCTIONS {
            let schema = format!(
                "CREATE TABLE IF NOT EXISTS evil_{function} (id INTEGER PRIMARY KEY, path TEXT CHECK ({function}(path) IS NULL));"
            );
            let error = apply_schema(&path, &schema, &actions).unwrap_err().to_string();
            assert_eq!(error, format!("schema.sql may only create tables and indexes, but it contains {function}()."), "{function}");
        }
        let evil = format!("{schema} CREATE INDEX IF NOT EXISTS evil_index ON items(load_extension(name));");
        let error = apply_schema(&path, &evil, &actions).unwrap_err().to_string();
        assert_eq!(error, "schema.sql may only create tables and indexes, but it contains load_extension().");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_schema_value_and_an_action_pattern_are_bounded_and_no_trigger_or_view_gets_in() {
        let root = std::env::temp_dir().join(format!("konae-sql-bounds-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let actions = action_program(&[("items.list", "SELECT id FROM items LIMIT 5")]);
        apply_schema(&path, "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT);", &actions).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection.execute("INSERT INTO items(name) VALUES ('a')", []).unwrap();
        drop(connection);
        // An index whose expression builds a 200 MB value per row hits the
        // value limit instead of the memory of the Kernel.
        let heavy = "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT); CREATE INDEX IF NOT EXISTS heavy ON items(length(hex(zeroblob(200000000))));";
        let error = apply_schema(&path, heavy, &actions).unwrap_err().to_string();
        assert!(error.contains("too big"), "{error}");

        let db = Connection::open(&path).unwrap();
        restrict_database(&db).unwrap();
        let pattern = |length: usize| format!("SELECT 'x' LIKE '{}'", "%".repeat(length));
        db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize), [], |row| row.get::<_, bool>(0)).unwrap();
        let long = db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize + 1), [], |row| row.get::<_, bool>(0)).unwrap_err().to_string();
        assert!(long.contains("pattern too complex"), "{long}");
        // Studio and the schema change open this connection too.
        for statement in ["CREATE TRIGGER t AFTER INSERT ON items BEGIN DELETE FROM items; END", "CREATE VIEW v AS SELECT 1",
            "CREATE TEMP TRIGGER t AFTER INSERT ON items BEGIN SELECT 1; END", "CREATE TEMP VIEW v AS SELECT 1"] {
            assert!(db.execute_batch(statement).unwrap_err().to_string().contains("not authorized"), "{statement}");
        }
        drop(db);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn named_locks_leave_no_entry_once_released() {
        let root = std::env::temp_dir().join(format!("konae-locks-{}", Uuid::new_v4()));
        let state = test_state(&root);
        let count = |state: &AppState| state.db_locks.lock().unwrap().len();
        for i in 0..100 {
            drop(database_lock(&state, &format!("transfer:{i}")).await);
        }
        assert_eq!(count(&state), 0);

        let first = database_lock(&state, "shared").await;
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move {
                let _second = database_lock(&state, "shared").await;
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(count(&state), 1);
        drop(first);
        waiter.await.unwrap();
        assert_eq!(count(&state), 0, "the last holder removes the entry");

        // A caller that gives up while waiting removes the entry too.
        let held = database_lock(&state, "contended").await;
        let gave_up = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            database_lock(&state, "contended"),
        )
        .await;
        assert!(gave_up.is_err());
        assert_eq!(count(&state), 1, "the holder keeps its entry");
        drop(held);
        assert_eq!(count(&state), 0);
    }

    #[test]
    fn redeploy_rolls_back_schema_when_an_action_does_not_prepare() {
        let root = std::env::temp_dir().join(format!("konae-schema-action-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let invalid_action = action_program(&[("tasks.list", "SELECT missing FROM tasks")]);
        assert!(
            apply_schema(
                &path,
                "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);",
                &invalid_action,
            )
            .is_err()
        );
        let connection = Connection::open(&path).unwrap();
        assert!(!connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    fn test_state(root: &Path) -> AppState {
        AppState::for_test(&root)
    }

    fn input_binding(key: &str, value_type: &str, max_length: Option<usize>) -> ActionBinding {
        ActionBinding {
            source: "input".into(),
            key: Some(key.into()),
            value_type: Some(value_type.into()),
            max_length,
            nullable: false,
            field: None,
            value: None,
        }
    }

    #[test]
    fn nullable_inputs_preserve_types_and_required_inputs() {
        for value_type in ["string", "number", "boolean"] {
            let mut binding = input_binding("value", value_type, Some(4));
            binding.nullable = true;
            let definition = action("query", "SELECT ?1", vec![binding]);
            assert!(validate_action("inputs.echo", &definition).is_ok());
            for input in [json!({}), json!({"value": null})] {
                assert_eq!(resolve_bindings(&definition, &input, None).unwrap(), vec![SqlValue::Null]);
            }
            assert!(resolve_bindings(&definition, &json!({"other": null}), None).unwrap_err().contains("Unexpected"));
            let wrong = match value_type {
                "string" => json!({"value": 5}),
                "number" => json!({"value": "5"}),
                _ => json!({"value": 0}),
            };
            assert!(resolve_bindings(&definition, &wrong, None).is_err());
            let required = action("query", "SELECT ?1", vec![input_binding("value", value_type, Some(4))]);
            assert!(resolve_bindings(&required, &json!({}), None).is_err());
            assert!(resolve_bindings(&required, &json!({"value": null}), None).is_err());
        }
        let mut binding = input_binding("value", "string", Some(4));
        binding.nullable = true;
        let definition = action("query", "SELECT ?1", vec![binding]);
        assert!(resolve_bindings(&definition, &json!({"value": ""}), None).is_err());
        assert!(resolve_bindings(&definition, &json!({"value": "longer"}), None).is_err());
        assert_eq!(resolve_bindings(&definition, &json!({"value": "ship"}), None).unwrap(), vec![SqlValue::Text("ship".into())]);
        for source in ["actor", "kernel", "literal"] {
            let mut binding = input_binding("value", "string", None);
            binding.source = source.into();
            binding.nullable = true;
            assert!(validate_action("inputs.echo", &action("query", "SELECT ?1", vec![binding])).is_err());
        }
    }

    #[test]
    fn mutation_returning_fails_schema_validation_without_executing() {
        let root = std::env::temp_dir().join(format!("konae-returning-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT, \"RETURNING\" INTEGER);";
        for sql in [
            "INSERT INTO notes(body) VALUES ('note') RETURNING id",
            "UPDATE notes SET body='note' returning body",
            "DELETE FROM notes RETURNING *",
        ] {
            let program = ActionProgram { version: 1, actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]) };
            assert!(apply_schema(&path, schema, &program).unwrap_err().to_string().contains("cannot use RETURNING"));
        }
        for sql in [
            "INSERT INTO notes(body) VALUES ('RETURNING')",
            "UPDATE notes SET body='note' /* RETURNING id */",
            "UPDATE notes SET \"RETURNING\"=1",
        ] {
            let program = ActionProgram { version: 1, actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]) };
            apply_schema(&path, schema, &program).unwrap();
        }
        let count: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(root);
    }




    #[test]
    fn artifact_content_types_follow_the_extension() {
        for (path, expected) in [
            ("index.html", "text/html; charset=utf-8"),
            ("docs/page.HTM", "text/html; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            ("chunk.mjs", "text/javascript; charset=utf-8"),
            ("styles.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("app.js.map", "application/json"),
            ("site.webmanifest", "application/manifest+json"),
            ("robots.txt", "text/plain; charset=utf-8"),
            ("logo.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.JPEG", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("favicon.ico", "image/x-icon"),
            ("module.wasm", "application/wasm"),
            ("font.woff2", "font/woff2"),
            ("font.woff", "font/woff"),
            ("font.ttf", "font/ttf"),
            ("font.otf", "font/otf"),
            ("sitemap.xml", "application/xml; charset=utf-8"),
            ("feed.rss", "application/rss+xml; charset=utf-8"),
            ("feed.atom", "application/atom+xml; charset=utf-8"),
            ("notes.md", "text/markdown; charset=utf-8"),
            ("rows.csv", "text/csv; charset=utf-8"),
            ("photo.avif", "image/avif"),
            ("menu.pdf", "application/pdf"),
            ("archive.zip", "application/octet-stream"),
            ("LICENSE", "application/octet-stream"),
            ("dir.html/file", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), expected, "path={path}");
        }
        let response = artifact_response(b"x".to_vec(), "x.bin");
        assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/octet-stream");
    }

    #[test]
    fn number_inputs_bind_integers_as_integer_and_fractions_as_real() {
        let definition = action(
            "mutation",
            "INSERT INTO items(label) VALUES (?)",
            vec![input_binding("value", "number", None)],
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": 5}), None).unwrap(),
            vec![SqlValue::Integer(5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": -3}), None).unwrap(),
            vec![SqlValue::Integer(-3)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": 5.5}), None).unwrap(),
            vec![SqlValue::Real(5.5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": u64::MAX}), None).unwrap(),
            vec![SqlValue::Real(u64::MAX as f64)]
        );

        let root = std::env::temp_dir().join(format!("konae-number-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE items(label TEXT)", [])
            .unwrap();
        let params = resolve_bindings(&definition, &json!({"value": 5}), None).unwrap();
        run_mutation(&path, &definition, params).unwrap();
        let stored: String = Connection::open(&path)
            .unwrap()
            .query_row("SELECT label FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, "5");
        let _ = std::fs::remove_dir_all(root);
    }

    /// ADR-0012: a notice that lists or addresses rows reads them back
    /// through the Kernel's RETURNING clause; the answer stays changes and
    /// lastInsertId.
    #[test]
    fn a_mutation_reads_back_its_changed_rows_for_the_notice_only() {
        let program: ActionProgram = serde_json::from_value(json!({"version": 1, "actions": {
            "tasks.remind": {"kind": "mutation", "access": "authenticated", "schedule": {"every": "1h"},
                "statement": "UPDATE tasks SET reminded_at = ?1 WHERE reminded_at IS NULL; -- hourly",
                "bindings": [{"source": "kernel", "field": "now"}],
                "notify": {"to": {"column": "assignee"}, "changes": ["title"], "subject": "Due", "body": "{changes}"}}
        }})).unwrap();
        let definition = &program.actions["tasks.remind"];
        let root = std::env::temp_dir().join(format!("konae-returning-rows-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, assignee TEXT, reminded_at INTEGER);
             INSERT INTO tasks(title, assignee, reminded_at) VALUES ('a', 'ana', NULL), ('b', 'ben', NULL), ('c', 'ana', 5);",
        ).unwrap();
        let ran = run_mutation(&path, definition, vec![SqlValue::Integer(9)]).unwrap();
        assert_eq!(ran.answer, json!({"changes": 2, "lastInsertId": 0}));
        let mut rows: Vec<Value> = ran.rows.iter().cloned().map(Value::Object).collect();
        rows.sort_by_key(|row| row["title"].to_string());
        assert_eq!(rows, vec![json!({"title": "a", "assignee": "ana"}), json!({"title": "b", "assignee": "ben"})]);
        let reminded: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM tasks WHERE reminded_at = 9", [], |row| row.get(0)).unwrap();
        assert_eq!(reminded, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn string_max_length_counts_utf16_code_units() {
        let two = action("mutation", "SELECT ?", vec![input_binding("title", "string", Some(2))]);
        let one = action("mutation", "SELECT ?", vec![input_binding("title", "string", Some(1))]);
        // One emoji is 4 UTF-8 bytes and 2 UTF-16 code units, like "\u{1F600}".length in a browser.
        let emoji = json!({"title": "\u{1F600}"});
        assert!(resolve_bindings(&two, &emoji, None).is_ok());
        assert!(resolve_bindings(&one, &emoji, None).unwrap_err().contains("too long"));
        assert!(resolve_bindings(&two, &json!({"title": "\u{e9}\u{e9}"}), None).is_ok());
    }

    async fn deploy_files(state: &AppState, files: Value) -> Value {
        deploy_with_visibility(state, files, "public").await
    }

    async fn deploy_with_visibility(state: &AppState, files: Value, visibility: &str) -> Value {
        let body = json!({
            "project": {
                "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
            },
            "visibility": visibility,
            "artifact": {"version": 1, "entrypoint": "index.html", "files": files}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
    }

    #[tokio::test]
    async fn a_browser_navigation_to_a_page_counts_one_page_view_by_the_file_served() {
        let root = std::env::temp_dir().join(format!("konae-page-views-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html": "<p>home</p>", "about.html": "<p>about</p>", "app.js": "1"})).await;
        let (host, deployment) = state.deployments.read().await.iter().next().map(|(h, d)| (h.clone(), d.clone())).unwrap();
        let get = |path: &str, extra: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
            for (name, value) in extra {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            serve_key(state.clone(), headers, path.into())
        };
        let navigation = [("sec-fetch-dest", "document")];
        assert_eq!(get("index.html", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);
        // None of these is a person opening a page.
        get("index.html", &[]).await;
        get("index.html", &[("sec-fetch-dest", "script")]).await;
        get("app.js", &navigation).await;
        get("index.html", &[("sec-fetch-dest", "document"), ("sec-purpose", "prefetch;prerender")]).await;
        assert_eq!(get("missing", &navigation).await.status(), StatusCode::NOT_FOUND);
        usage::flush();
        let today = usage::day(now_ms());
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        let views: Vec<_> = usage["totals"].as_array().unwrap().iter().filter(|row| row["kind"] == "page_view").cloned().collect();
        assert_eq!(views, vec![
            json!({"kind": "page_view", "name": "about.html", "count": 2}),
            json!({"kind": "page_view", "name": "index.html", "count": 1}),
        ]);
        // A single-page app answers every address with index.html. A deep link
        // counts as its first load; a missing chunk the page asks for does not.
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["routing"] = json!("spa");
        }
        assert_eq!(get("items/3", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("missing-chunk.js", &[("sec-fetch-dest", "script")]).await.status(), StatusCode::OK);
        usage::flush();
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        let index = usage["totals"].as_array().unwrap().iter().find(|row| row["kind"] == "page_view" && row["name"] == "index.html").cloned();
        assert_eq!(index, Some(json!({"kind": "page_view", "name": "index.html", "count": 2})));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_file_the_browser_holds_answers_304_until_the_next_publish() {
        let root = std::env::temp_dir().join(format!("konae-etag-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "1"})).await;
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["visibility"] = json!("public");
        }
        let (host, deployment) = state.deployments.read().await.iter().next().map(|(h, d)| (h.clone(), d.clone())).unwrap();
        let get = |path: &str, extra: Vec<(&'static str, String)>| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
            for (name, value) in extra {
                headers.insert(name, HeaderValue::from_str(&value).unwrap());
            }
            serve_key(state.clone(), headers, path.into())
        };
        let first = get("app.js", vec![]).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CACHE_CONTROL], "public, no-cache");
        let tag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert_eq!(tag, format!("\"{}\"", deployment.artifact_version.clone()));

        let again = get("app.js", vec![("if-none-match", tag.clone())]).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG], tag.as_str());
        assert!(axum::body::to_bytes(again.into_body(), 1024).await.unwrap().is_empty());
        let weak = get("app.js", vec![("if-none-match", format!("\"other\", W/{tag}"))]).await;
        assert_eq!(weak.status(), StatusCode::NOT_MODIFIED);

        // A navigation still resolves its file, so the page view counts.
        let page = get("index.html", vec![("if-none-match", tag.clone()), ("sec-fetch-dest", "document".into())]).await;
        assert_eq!(page.status(), StatusCode::NOT_MODIFIED);
        usage::flush();
        let today = usage::day(now_ms());
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        assert!(usage["totals"].as_array().unwrap().iter().any(|row| row["kind"] == "page_view" && row["name"] == "index.html"));

        // A publish changes the tag, so the old one gets the new bytes.
        deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "2"})).await;
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["visibility"] = json!("public");
        }
        let fresh = get("app.js", vec![("if-none-match", tag.clone())]).await;
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_ne!(fresh.headers()[header::ETAG], tag.as_str());
        assert_eq!(&axum::body::to_bytes(fresh.into_body(), 1024).await.unwrap()[..], b"2");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn a_private_apps_files_stay_out_of_shared_caches() {
        let deployment = Settings::new("app", "App", "app", &json!({"visibility": "private"}))
            .deployment(Installed { hostname: "app.localhost".into(), created_at: 1, actions: "{}".into(), artifact_version: "v1".into() }, "http://localhost");
        assert_eq!(ArtifactCache::of(&deployment).control(), "private, no-cache");
    }

    #[tokio::test]
    async fn republish_stops_serving_files_missing_from_the_new_artifact() {
        let root = std::env::temp_dir().join(format!("konae-republish-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(
            &state,
            json!({"index.html": "<p>one</p>", "old.js": "1", "assets/old.css": "a{}"}),
        )
        .await;
        let id = first["id"].as_str().unwrap().to_owned();
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(read_artifact(&state, &deployment, "old.js").await.unwrap(), b"1");

        deploy_files(&state, json!({"index.html": "<p>two</p>", "new.js": "2"})).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(deployment.id, id);
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>two</p>");
        assert_eq!(read_artifact(&state, &deployment, "new.js").await.unwrap(), b"2");
        assert!(read_artifact(&state, &deployment, "old.js").await.is_err());
        assert!(read_artifact(&state, &deployment, "assets/old.css").await.is_err());
        assert!(staged_or_retired(&root, &id).is_empty(), "only dist/ holds files");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn new_deployments_pick_a_policy_from_visibility_and_redeploys_keep_it() {
        for (visibility, expected) in [("public", SignupMode::Open), ("private", SignupMode::Closed)] {
            let root = std::env::temp_dir().join(format!("konae-policy-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            let first = deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), visibility).await;
            assert_eq!(first["signupPolicy"]["mode"], json!(expected), "visibility={visibility}");

            set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
            let second = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), visibility).await;
            assert_eq!(second["signupPolicy"], json!({"mode": "allowlist", "allowlist": ["@example.test"]}));
            let id = second["id"].as_str().unwrap();
            let stored = Settings::read(&root, id).unwrap();
            assert_eq!(stored.signup_policy.mode, SignupMode::Allowlist);
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn republishing_a_public_app_as_private_closes_an_open_signup() {
        // (first visibility, policy the owner set, second visibility, expected)
        let cases = [
            ("public", None, "private", json!({"mode": "closed", "allowlist": []})),
            (
                "public",
                Some((SignupMode::Allowlist, "@example.test")),
                "private",
                json!({"mode": "allowlist", "allowlist": ["@example.test"]}),
            ),
            // The owner opened a private app on purpose; a redeploy keeps that.
            ("private", Some((SignupMode::Open, "")), "private", json!({"mode": "open", "allowlist": []})),
            ("private", None, "public", json!({"mode": "closed", "allowlist": []})),
        ];
        for (first, chosen, second, expected) in cases {
            let root = std::env::temp_dir().join(format!("konae-flip-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), first).await;
            if let Some((mode, entry)) = chosen {
                let entries: Vec<&str> = if entry.is_empty() { vec![] } else { vec![entry] };
                set_policy(&state, mode, &entries).await;
            }
            let republished = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), second).await;
            assert_eq!(republished["signupPolicy"], expected, "{first} {chosen:?} -> {second}");
            let id = republished["id"].as_str().unwrap();
            let stored = Settings::read(&root, id).unwrap();
            assert_eq!(serde_json::to_value(&stored.signup_policy).unwrap(), expected);
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn signup_checks_input_first_and_gives_one_answer_for_closed_and_unlisted() {
        let root = std::env::temp_dir().join(format!("konae-signup-oracle-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let attempt = |email: &'static str, password: &'static str| {
            let state = state.clone();
            let headers = headers.clone();
            async move {
                let body = json!({"email": email, "password": password});
                let request = Request::builder().body(Body::from(body.to_string())).unwrap();
                let response = signup(State(state), headers, request).await;
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap()["error"].clone())
            }
        };
        set_policy(&state, SignupMode::Allowlist, &["listed@example.test"]).await;
        // A weak password gets the same answer whether or not the address is listed.
        let listed = attempt("listed@example.test", "x").await;
        let unlisted = attempt("other@example.test", "x").await;
        assert_eq!(listed.0, StatusCode::BAD_REQUEST);
        assert_eq!(listed, unlisted);
        assert_eq!(attempt("not-an-email", "correct horse battery").await.0, StatusCode::BAD_REQUEST);
        let refused = attempt("other@example.test", "correct horse battery").await;
        assert_eq!(refused.0, StatusCode::FORBIDDEN);
        set_policy(&state, SignupMode::Closed, &[]).await;
        assert_eq!(attempt("listed@example.test", "correct horse battery").await, refused);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn app_databases_are_capped_in_size() {
        let root = std::env::temp_dir().join(format!("konae-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let db = Connection::open(&path).unwrap();
        restrict_database(&db).unwrap();
        db.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
        let page_size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        let pages: u64 = db.query_row("PRAGMA max_page_count", [], |r| r.get(0)).unwrap();
        assert_eq!(pages * page_size, MAX_APP_DATABASE_BYTES);
        drop(db);

        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE notes(body TEXT)").unwrap();
        limit_database_size(&db, 64 * 1024).unwrap();
        let mut full = None;
        for _ in 0..100 {
            if let Err(error_value) = db.execute("INSERT INTO notes VALUES (?1)", ["x".repeat(4000)]) {
                full = Some(error_value.to_string());
                break;
            }
        }
        assert!(full.expect("the cap must stop writes").contains("full"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Release folders beside `dist/` that a publish left behind.
    fn staged_or_retired(root: &Path, id: &str) -> Vec<String> {
        std::fs::read_dir(layout::private(root, id))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("dist-"))
            .collect()
    }

    /// A release body for `id` with these tables and actions and one page.
    fn release_body(id: &str, schema: &str, actions: Value, page: &str, confirmation: Option<String>) -> Value {
        json!({
            "project": {
                "id": id, "name": "Steps", "slug": "steps", "manifest": {},
                "files": {"actions.json": json!({"version": 1, "actions": actions}).to_string(), "schema.sql": schema}
            },
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": page}},
            "schemaConfirmation": confirmation
        })
    }

    #[tokio::test]
    async fn a_release_that_fails_at_any_step_leaves_the_old_actions_on_the_old_rows() {
        let list = |table: &str, column: &str| json!({"kind": "query", "access": "anonymous", "statement": format!("SELECT {column} FROM {table} ORDER BY id LIMIT 50"), "bindings": []});
        let both = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\nCREATE TABLE IF NOT EXISTS tags(id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n";
        let notes_only = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\n";
        for step in ["stage_local_artifact", "upload_artifact", "open_auth", "schema_change", "persist_deployment", "switch_dist"] {
            let root = std::env::temp_dir().join(format!("konae-release-step-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            let id = format!("app_{step}");
            let first = release_body(&id, both, json!({"notes.list": list("notes", "body"), "tags.list": list("tags", "name")}), "<p>one</p>", None);
            let response = deploy(&state, first.to_string().as_bytes()).await;
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
            assert_eq!(status, StatusCode::OK, "{step}: {}", String::from_utf8_lossy(&bytes));
            Connection::open(database_path(&state, &id)).unwrap()
                .execute_batch("INSERT INTO notes(body) VALUES ('kept note'); INSERT INTO tags(name) VALUES ('old tag');").unwrap();
            let live = state.deployments.read().await.values().next().cloned().unwrap();

            // The next release drops tags, confirmed as release_plan would confirm it.
            let mut second = release_body(&id, notes_only, json!({"notes.list": list("notes", "body")}), "<p>two</p>", None);
            let input: DeployInput = serde_json::from_value(second.clone()).unwrap();
            let confirmation = runtime::validate_release(&state, &input, None).await.unwrap().confirmation();
            assert!(confirmation.is_some(), "dropping tags loses a row");
            second["schemaConfirmation"] = json!(confirmation);
            FAILING_RELEASE_STEPS.lock().unwrap().push((id.clone(), step));
            assert_ne!(deploy(&state, second.to_string().as_bytes()).await.status(), StatusCode::OK, "{step}");
            FAILING_RELEASE_STEPS.lock().unwrap().retain(|(app, _)| app != &id);

            let call = |name: &'static str| {
                let state = state.clone();
                let mut headers = HeaderMap::new();
                headers.insert(header::HOST, live.hostname.parse().unwrap());
                async move {
                    let response = invoke_action(State(state), AxumPath(name.into()), headers, Json(json!({}))).await;
                    let status = response.status();
                    (status, String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap())
                }
            };
            let (status, body) = call("tags.list").await;
            assert_eq!(status, StatusCode::OK, "{step}: {body}");
            assert!(body.contains("old tag"), "{step}: {body}");
            assert!(call("notes.list").await.1.contains("kept note"), "{step}");
            let serving = state.deployments.read().await.values().next().cloned().unwrap();
            assert_eq!(serving.artifact_version, live.artifact_version, "{step}");
            let record: Value = serde_json::from_slice(&std::fs::read(crate::layout::record(&root, &id)).unwrap()).unwrap();
            assert_eq!(record["artifactVersion"], json!(live.artifact_version), "{step}: the record names the release memory serves");
            assert_eq!(read_artifact(&state, &serving, "index.html").await.unwrap(), b"<p>one</p>", "{step}");
            assert!(staged_or_retired(&root, &id).is_empty(), "{step}: no staged files stay");
            let leftovers: Vec<_> = std::fs::read_dir(crate::layout::private(&root, &id)).unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.starts_with("release-")).collect();
            assert!(leftovers.is_empty(), "{step}: {leftovers:?}");

            // The same release goes through once nothing fails.
            assert_eq!(deploy(&state, second.to_string().as_bytes()).await.status(), StatusCode::OK, "{step}");
            assert_eq!(call("tags.list").await.0, StatusCode::NOT_FOUND, "{step}");
            assert!(call("notes.list").await.1.contains("kept note"), "{step}");
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn a_failed_record_write_removes_the_new_artifact_version() {
        let root = std::env::temp_dir().join(format!("konae-orphan-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        let id = first["id"].as_str().unwrap().to_owned();
        let record = crate::layout::record(&root, &id);
        tokio::fs::remove_file(&record).await.unwrap();
        tokio::fs::create_dir_all(record.join("blocker")).await.unwrap();
        let body = json!({
            "project": {
                "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
            },
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>two</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(staged_or_retired(&root, &id).is_empty(), "the staged files are gone");
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>one</p>");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restoring_a_backup_first_backs_up_the_live_database() {
        let root = std::env::temp_dir().join(format!("konae-restore-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
        let deployment_id = state.deployments.read().await.values().next().unwrap().id.clone();
        let id = deployment_id.as_str();
        let path = database_path(&state, id);
        initialize_database(&path).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('old');")
            .unwrap();
        let backup = create_backup(&state, id, "Manual backup").await.unwrap();
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO notes VALUES ('newer')", [])
            .unwrap();
        restore_backup(&state, id, backup["id"].as_str().unwrap()).await.unwrap();

        let backups = list_backups(&state, id).await.unwrap();
        let before = backups
            .iter()
            .find(|item| item["label"] == "Before restore")
            .expect("restore must keep a copy of the live database");
        let snapshot = crate::layout::backups(&root, &id)
            .join(format!("{}.sqlite", before["id"].as_str().unwrap()));
        let rows: i64 = Connection::open(snapshot)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        let rows: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    pub fn signed_in(state: &AppState, deployment_id: &str, headers: &HeaderMap, email: &str) -> (HeaderMap, String) {
        let auth = AuthService::open(
            crate::layout::identity(&state
                .data_root, &deployment_id),
        )
        .unwrap();
        let session = auth.signup(email, "correct horse battery").unwrap();
        let mut headers = headers.clone();
        headers.insert(
            header::COOKIE,
            format!("konae_app_session={}", session.token).parse().unwrap(),
        );
        (headers, session.user.user_id)
    }

    pub async fn response_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn upload_refuses_to_overwrite_an_unreadable_file_index() {
        let root = std::env::temp_dir().join(format!("konae-upload-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
        let files = crate::layout::files(&root, &deployment.id);
        tokio::fs::create_dir_all(&files).await.unwrap();
        tokio::fs::write(files.join("index.json"), "{not json").await.unwrap();
        let body = json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        let response = upload_file(State(state.clone()), headers.clone(), request).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(tokio::fs::read(files.join("index.json")).await.unwrap(), b"{not json");

        tokio::fs::remove_file(files.join("index.json")).await.unwrap();
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        let response = upload_file(State(state), headers, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let index: Vec<Value> =
            serde_json::from_slice(&tokio::fs::read(files.join("index.json")).await.unwrap()).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0]["visibility"], "private");
        let leftovers = std::fs::read_dir(&files)
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn files_are_private_to_the_uploader_unless_marked_public() {
        let root = std::env::temp_dir().join(format!("konae-files-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, alice_id) = signed_in(&state, &deployment.id, &headers, "alice@example.test");
        let (bob, _) = signed_in(&state, &deployment.id, &headers, "bob@example.test");
        let body = |visibility: Option<&str>| {
            let mut value = json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
            if let Some(visibility) = visibility {
                value["visibility"] = json!(visibility);
            }
            Request::builder().body(Body::from(value.to_string())).unwrap()
        };

        let anonymous = upload_file(State(state.clone()), headers.clone(), body(None)).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let private_file = response_json(upload_file(State(state.clone()), alice.clone(), body(None)).await).await;
        assert_eq!(private_file["result"]["visibility"], "private");
        assert_eq!(private_file["result"]["ownerId"], alice_id);
        let private_id = private_file["result"]["id"].as_str().unwrap().to_owned();

        let public_file = response_json(upload_file(State(state.clone()), alice.clone(), body(Some("public"))).await).await;
        let public_id = public_file["result"]["id"].as_str().unwrap().to_owned();

        let alice_list = response_json(list_files(State(state.clone()), alice.clone()).await).await;
        assert_eq!(alice_list["result"].as_array().unwrap().len(), 2);
        assert!(alice_list["result"].as_array().unwrap().iter().any(|file| file["ownerId"] == alice_id));

        let bob_list = response_json(list_files(State(state.clone()), bob.clone()).await).await;
        assert_eq!(bob_list["result"].as_array().unwrap().len(), 1);
        assert_eq!(bob_list["result"][0]["id"], public_id);
        assert!(bob_list["result"][0].get("ownerId").is_none());

        let visitor_list = response_json(list_files(State(state.clone()), headers.clone()).await).await;
        assert_eq!(visitor_list["result"][0]["id"], public_id);
        assert!(visitor_list["result"][0].get("ownerId").is_none());

        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id.clone()), bob.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id.clone()), headers.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id), alice.clone()).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(public_id.clone()), headers).await.status(),
            StatusCode::OK
        );

        let legacy_id = Uuid::new_v4().to_string();
        let directory = crate::layout::files(&root, &deployment.id);
        tokio::fs::write(directory.join(format!("{legacy_id}.blob")), b"hi").await.unwrap();
        let mut index: Vec<Value> = serde_json::from_slice(&tokio::fs::read(directory.join("index.json")).await.unwrap()).unwrap();
        index.push(json!({"id": legacy_id, "name": "old.txt", "type": "text/plain", "size": 2, "createdAt": 1}));
        tokio::fs::write(directory.join("index.json"), serde_json::to_vec(&index).unwrap()).await.unwrap();
        let after_legacy = response_json(list_files(State(state.clone()), alice).await).await;
        assert!(after_legacy["result"].as_array().unwrap().iter().all(|file| file["id"] != legacy_id));
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(legacy_id), bob).await.status(),
            StatusCode::NOT_FOUND
        );

        let (private_state, private_headers) = login_test_state(&root.join("private"), "private").await;
        let private_deployment = private_state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(
            list_files(State(private_state.clone()), private_headers.clone()).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let (member, _) = signed_in(&private_state, &private_deployment.id, &private_headers, "member@example.test");
        assert_eq!(list_files(State(private_state), member).await.status(), StatusCode::OK);

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
