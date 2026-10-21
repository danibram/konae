use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::{Client as S3Client, config::Builder as S3ConfigBuilder, primitives::ByteStream};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

mod accounts;
mod action_shape;
mod admission;
mod app_owner;
mod artifact;
mod auth;
mod calls;
mod checks;
mod clerk;
mod copies;
mod domain;
mod eject;
mod files;
mod guard;
mod guide;
mod http;
mod invitations;
mod limits;
mod mail;
mod mcp;
mod migration;
mod model_rules;
mod notify;
mod oauth;
mod package;
mod provider;
mod recovery;
mod refusal;
mod schedule;
mod security;
mod sql_rules;
mod usage;
use auth::{AuthError, AuthService, AuthUser};
use limits::{Guards, trusted_proxies};

#[derive(Clone)]
struct AppState {
    deployments: Arc<RwLock<HashMap<String, Deployment>>>,
    db_locks: LockMap,
    s3: Option<S3Client>,
    artifact_bucket: String,
    data_bucket: String,
    data_root: PathBuf,
    copies: Arc<copies::CopyStatus>,
    admin_key: String,
    public_origin: String,
    host_suffix: String,
    guards: Arc<Guards>,
    accounts: accounts::AccountSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "DeploymentRecord")]
struct Deployment {
    id: String,
    project_id: String,
    project_name: String,
    url: String,
    created_at: u64,
    manifest: Value,
    hostname: String,
    actions: String,
    /// Folder under the deployment's artifact prefix that holds the live files.
    /// Deployments published before versioning keep their files at the prefix root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_version: Option<String>,
    /// Who may create an account. Records written before this field existed
    /// get the default for their visibility when they load.
    signup_policy: SignupPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    database_model: Option<DatabaseModel>,
}

/// The stored form of a Deployment, where the signup policy may be missing.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentRecord {
    id: String,
    project_id: String,
    project_name: String,
    url: String,
    created_at: u64,
    manifest: Value,
    hostname: String,
    actions: String,
    #[serde(default)]
    artifact_version: Option<String>,
    #[serde(default)]
    signup_policy: Option<SignupPolicy>,
    #[serde(default)]
    database_model: Option<DatabaseModel>,
}

impl From<DeploymentRecord> for Deployment {
    fn from(record: DeploymentRecord) -> Self {
        let signup_policy = record.signup_policy.unwrap_or_else(|| {
            SignupPolicy::initial(
                record
                    .manifest
                    .get("visibility")
                    .and_then(Value::as_str)
                    .unwrap_or("public"),
            )
        });
        Self {
            id: record.id,
            project_id: record.project_id,
            project_name: record.project_name,
            url: record.url,
            created_at: record.created_at,
            manifest: record.manifest,
            hostname: record.hostname,
            actions: record.actions,
            artifact_version: record.artifact_version,
            signup_policy,
            database_model: record.database_model,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SignupMode {
    #[default]
    Open,
    /// No signup without an invitation from the owner. See `invitations.rs`.
    Invitation,
    Allowlist,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignupPolicy {
    mode: SignupMode,
    /// Exact emails and `@domain` entries, stored trimmed and lowercased.
    #[serde(default)]
    allowlist: Vec<String>,
}

const MAX_ALLOWLIST_ENTRIES: usize = 100;
const SIGNUP_REFUSED: &str = "This app is not accepting new accounts for this email address.";

impl SignupPolicy {
    /// Public apps start open. Private apps start closed until the owner
    /// chooses who may join.
    fn initial(visibility: &str) -> Self {
        Self {
            mode: if visibility == "private" {
                SignupMode::Closed
            } else {
                SignupMode::Open
            },
            allowlist: Vec::new(),
        }
    }

    /// The policy a redeploy keeps. Turning a public app private closes an
    /// open signup, since strangers who could join the public app should not
    /// get into the private one. Any other choice the owner made carries over.
    fn for_republish(&self, was_private: bool, visibility: &str) -> Self {
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
    fn from_input(mode: SignupMode, entries: &[String]) -> std::result::Result<Self, String> {
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
    fn allows(&self, email: &str) -> bool {
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
struct DeployInput {
    project: ProjectInput,
    visibility: String,
    artifact: BuildArtifact,
    #[serde(default)]
    initial_database: Option<String>,
    #[serde(default)]
    initial_identity: Option<String>,
    #[serde(default)]
    initial_files: Option<String>,
    #[serde(default)]
    initial_signup_policy: Option<SignupPolicy>,
    /// A chunked package whose components were uploaded as transfers.
    #[serde(default)]
    initial_package: Option<package::InitialPackage>,
    #[serde(default)]
    initial_domain: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseModel {
    source: String,
    accepted_source: String,
    revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInput {
    id: String,
    name: String,
    slug: String,
    files: HashMap<String, String>,
    manifest: Value,
    #[serde(default)]
    database_model: Option<DatabaseModel>,
}

#[derive(Deserialize)]
struct BuildArtifact {
    version: u8,
    entrypoint: String,
    files: HashMap<String, String>,
    /// Paths whose value in `files` is base64; every other file is UTF-8 text.
    #[serde(default)]
    encodings: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseBackup {
    id: String,
    label: String,
    created_at: u64,
    size_bytes: u64,
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    database_model_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_revision: Option<String>,
}

#[derive(Deserialize)]
struct ActionProgram {
    version: u8,
    actions: HashMap<String, ActionDefinition>,
}

#[derive(Clone, Deserialize)]
struct ActionDefinition {
    kind: String,
    access: String,
    /// Empty for a `"kind": "signal"` action, which runs no SQL.
    #[serde(default)]
    statement: String,
    bindings: Vec<ActionBinding>,
    /// The action's HTTP route, checked by `http::binding`.
    #[serde(default)]
    http: Option<Value>,
    /// The Signal the action records, checked by `usage::declaration`.
    #[serde(default)]
    signal: Option<Value>,
    /// A mutation's refusal when it changes no row, checked by `guard::declaration`.
    #[serde(default)]
    guard: Option<Value>,
    /// The email a mutation queues when it changes a row, checked by `notify::declaration`.
    #[serde(default)]
    notify: Option<Value>,
    /// When the Kernel's timer runs a mutation, checked by `schedule::declaration`.
    #[serde(default)]
    schedule: Option<Value>,
    /// Refused by `schedule::check_subscribe`: the Kernel pushes nothing.
    #[serde(default)]
    subscribe: Option<Value>,
    /// An action of `"kind": "call"`: its url, method, body, auth and then,
    /// checked by `calls::check_kind`.
    #[serde(flatten)]
    call: calls::CallKeys,
    /// Every other key, refused by `action_shape::check_keys`. It comes after
    /// `call` so serde hands it only the keys no field took.
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionBinding {
    source: String,
    key: Option<String>,
    #[serde(rename = "type")]
    value_type: Option<String>,
    max_length: Option<usize>,
    #[serde(default)]
    nullable: bool,
    field: Option<String>,
    value: Option<Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if clerk::operator_command()? { return Ok(()); }
    if copies::operator_command().await? { return Ok(()); }
    if recovery::operator_command().await? { return Ok(()); }
    if accounts::operator_command()? { return Ok(()); }
    if admission::operator_command()? { return Ok(()); }
    let port = env("LITTLEBOX_KERNEL_PORT", "8787").parse::<u16>()?;
    let bind = env("LITTLEBOX_BIND", "127.0.0.1");
    let public_origin = env("LITTLEBOX_PUBLIC_ORIGIN", "http://localhost:8080");
    let admin_key = admin_key_for(
        &bind,
        &public_origin,
        std::env::var("LITTLEBOX_MCP_ORIGIN").ok().as_deref(),
        std::env::var("LITTLEBOX_ADMIN_KEY").ok().as_deref(),
        std::env::var("LITTLEBOX_ALLOW_DEFAULT_KEY").as_deref() == Ok("1"),
    )?;
    let data_root = PathBuf::from(env("LITTLEBOX_DATA_DIR", "/data"));
    tokio::fs::create_dir_all(data_root.join("deployments")).await?;
    for line in copies::migrate_layout(&data_root)? {
        println!("Layout migration: moved {line}");
    }
    let account_settings = accounts::AccountSettings::from_env(&data_root, &admin_key)?;
    let s3 = s3_client_from_env().await;
    let artifact_bucket = env("LITTLEBOX_ARTIFACT_BUCKET", "littlebox-apps");
    let data_bucket = env("LITTLEBOX_DATA_BUCKET", "littlebox-data");
    let copy_config = copies::CopyConfig::from_env(&data_root, s3.is_some())?;
    let schedule_clock = schedule::Clock::from_env(&bind)?;
    let deployments = load_deployments(&data_root, &public_origin).await?;
    let state = AppState {
        deployments: Arc::new(RwLock::new(deployments)),
        db_locks: LockMap::default(),
        s3,
        artifact_bucket,
        data_bucket,
        data_root,
        copies: Arc::new(copies::CopyStatus::new(copy_config.as_ref())),
        admin_key,
        public_origin,
        host_suffix: env("LITTLEBOX_HOST_SUFFIX", "localhost"),
        guards: Arc::new(Guards::new(trusted_proxies(
            &std::env::var("LITTLEBOX_TRUSTED_PROXIES").unwrap_or_default(),
        )?)),
        accounts: account_settings,
    };
    if let Some(config) = copy_config {
        copies::spawn_schedule(state.clone(), config);
    }
    schedule::spawn(state.clone(), schedule_clock);
    if let Some(queue) = state.accounts.queue.clone() {
        mail::spawn_worker(queue);
    }
    calls::spawn_worker(state.clone());
    // Apply the verification mode to creator accounts before serving, so a
    // switch to required revokes unverified sessions at startup.
    accounts::creator_store(&state)?;

    let app = Router::new()
        .route("/_littlebox/health", get(health))
        .route("/_littlebox/auth/signup", post(signup))
        .route("/_littlebox/auth/login", post(login))
        .route("/_littlebox/login", get(login_page))
        .merge(mcp::routes())
        .merge(oauth::routes())
        .merge(accounts::routes())
        .merge(http::routes())
        .merge(invitations::routes())
        .merge(app_owner::routes())
        .merge(mail::routes())
        .route("/_littlebox/{*path}", axum::routing::any(|| async { error(StatusCode::GONE, "Use authenticated MCP for platform operations.") }))
        .route("/{*path}", get(http::get_route).post(http::post_route))
        .route("/", get(serve_root))
        .layer(axum::middleware::from_fn_with_state(state.clone(), migration::guard))
        .layer(axum::middleware::from_fn_with_state(state.clone(), security::headers))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((bind.as_str(), port)).await?;
    println!("Littlebox Rust Kernel listening on {port}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({"ok": true, "kernel": "littlebox-rust", "version": 1,
        "artifactStore": if state.s3.is_some() { "filesystem+s3" } else { "filesystem" },
        "copies": state.copies.health(),
        "mail": state.accounts.queue.as_ref().map(|queue| queue.health())}))
}

/// The S3 client for artifacts and copies, or None when LITTLEBOX_S3_ENDPOINT is unset.
async fn s3_client_from_env() -> Option<S3Client> {
    let endpoint = std::env::var("LITTLEBOX_S3_ENDPOINT").ok()?;
    let credentials = Credentials::new(
        env("LITTLEBOX_S3_ACCESS_KEY", "littlebox"),
        env("LITTLEBOX_S3_SECRET_KEY", "littlebox-secret"),
        None,
        None,
        "littlebox-config",
    );
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(env("LITTLEBOX_S3_REGION", "us-east-1")))
        .credentials_provider(credentials)
        .endpoint_url(endpoint)
        .load()
        .await;
    let s3_config = S3ConfigBuilder::from(&sdk_config)
        .force_path_style(true)
        .build();
    Some(S3Client::from_conf(s3_config))
}

/// The Deployment's folder: its record, artifact, databases, files and backups.
fn deployment_dir(state: &AppState, id: &str) -> PathBuf {
    state.data_root.join("deployments").join(id)
}

fn artifact_root(state: &AppState, id: &str) -> PathBuf {
    deployment_dir(state, id).join("artifact")
}

fn backup_dir(state: &AppState, id: &str) -> PathBuf {
    deployment_dir(state, id).join("backups")
}

async fn test_storage(State(state): State<AppState>, request: Request) -> Response {
    if !is_admin(&state, request.headers()) {
        return error(StatusCode::UNAUTHORIZED, "Admin authorization required.");
    }
    let Some(s3) = state.s3.as_ref() else {
        return Json(json!({"result": {"ok": true, "adapter": "filesystem"}})).into_response();
    };
    match s3.head_bucket().bucket(&state.artifact_bucket).send().await {
        Ok(_) => Json(json!({"result": {"ok": true, "adapter": "s3-compatible", "bucket": state.artifact_bucket}})).into_response(),
        Err(error_value) => error(StatusCode::BAD_GATEWAY, &error_value.to_string()),
    }
}

async fn deploy(State(state): State<AppState>, request: Request) -> Response {
    if !is_admin(&state, request.headers()) {
        return error(StatusCode::UNAUTHORIZED, "Admin authorization required.");
    }
    let body = match axum::body::to_bytes(request.into_body(), 26_000_000).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Deployment request is too large."),
    };
    let input: DeployInput = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error_value) => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid deployment: {error_value}"),
            );
        }
    };
    if let Err(error_value) = mcp::validate_release(&state, &input).await {
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
    let program: ActionProgram = match serde_json::from_str(&actions) {
        Ok(value) => value,
        Err(error_value) => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid actions.json: {error_value}"),
            );
        }
    };
    if program.version != 1 {
        return error(StatusCode::BAD_REQUEST, "Unsupported actions.json version.");
    }
    if let Err(refused) = action_shape::check_program(&program) {
        return refusal_error(&refused);
    }
    if let Err(error_value) = mcp::store(&state).and_then(|db| {
        calls::check_release(&domain::Rules::of(&state), &db, &input.project.id, &program)
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
        && (input.initial_database.is_some()
            || input.initial_identity.is_some()
            || input.initial_files.is_some()
            || input.initial_signup_policy.is_some()
            || input.initial_package.is_some()
            || input.initial_domain.is_some())
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
    let imported_size = [
        &input.initial_database,
        &input.initial_identity,
        &input.initial_files,
    ]
    .into_iter()
    .flatten()
    .try_fold(0usize, |total, encoded| {
        decode_base64(encoded).map(|bytes| total.saturating_add(bytes.len()))
    });
    match imported_size {
        Ok(size) if size <= 16 * 1024 * 1024 => {}
        Ok(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Portable state exceeds the 16 MiB combined import limit.",
            );
        }
        Err(error_value) => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Portable state is invalid: {error_value}"),
            );
        }
    }
    let id = existing
        .as_ref()
        .map(|item| item.id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let hostname = existing
        .as_ref()
        .map(|item| item.hostname.clone())
        .unwrap_or_else(|| {
            let short_id = id.replace('-', "").chars().take(6).collect::<String>();
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
    if let Some(model) = input.project.database_model.as_ref() {
        if let Err(refused) = model_rules::accepted(&model.source, &model.accepted_source, &model.revision) {
            return refusal_error(&refused);
        }
    }
    let mut manifest = input.project.manifest;
    manifest["visibility"] = Value::String(input.visibility.clone());
    let artifact_version = format!("v{}", Uuid::new_v4().simple());
    let mut deployment = Deployment {
        id: id.clone(),
        project_id: input.project.id,
        project_name: input.project.name,
        url,
        created_at: existing
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or_else(now_ms),
        manifest,
        hostname: hostname.clone(),
        actions,
        artifact_version: Some(artifact_version.clone()),
        database_model: input.project.database_model,
        signup_policy: match existing.as_ref() {
            Some(item) => item.signup_policy.for_republish(is_private(item), &input.visibility),
            None => initial_signup_policy.unwrap_or_else(|| SignupPolicy::initial(&input.visibility)),
        },
    };
    let directory = state.data_root.join("deployments").join(&id);
    if let Err(error_value) = tokio::fs::create_dir_all(&directory).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    let _database_guard = database_lock(&state, &id).await;
    if let Some(package) = input.initial_package.as_ref() {
        if let Err(error_value) = package::install(&state, package, &deployment.project_id, &directory)
            .await
            .and_then(|()| migration::imported(&state.data_root, &id, &deployment.project_id, &package.manifest))
        {
            let _ = tokio::fs::remove_dir_all(&directory).await;
            return error(StatusCode::BAD_REQUEST, &format!("Package import failed: {error_value:#}"));
        }
    }
    if input.initial_package.is_none() {
        if let Err(error_value) = domain::arrive(&state, &directory, input.initial_domain.as_deref()).await {
            return error(StatusCode::BAD_REQUEST, &error_value.to_string());
        }
    }
    let db_path = directory.join("database.sqlite");
    let schema = input
        .project
        .files
        .get("schema.sql")
        .cloned()
        .unwrap_or_default();
    if !db_path.exists() {
        let initialized = if let Some(snapshot) = input.initial_database.as_ref() {
            initialize_database_from_base64(&db_path, snapshot, 16 * 1024 * 1024)
        } else {
            initialize_database(&db_path)
        };
        if let Err(error_value) = initialized {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Database initialization failed: {error_value}"),
            );
        }
    }
    if let Err(error_value) = apply_additive_schema(&db_path, &schema, &program) {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Schema update failed: {error_value}"),
        );
    }
    if deployment.database_model.is_none() {
        if let Some(model) = existing.as_ref().and_then(|d| d.database_model.clone()) {
            if mcp::model_matches_path(&model.accepted_source, &db_path).await.unwrap_or(false) {
                deployment.database_model = Some(model);
            }
        }
    }
    let identity_path = directory.join("identity.sqlite");
    if !identity_path.exists() {
        if let Some(snapshot) = input.initial_identity.as_ref() {
            if let Err(error_value) =
                initialize_identity_from_base64(&identity_path, snapshot, 16 * 1024 * 1024)
            {
                return error(
                    StatusCode::BAD_REQUEST,
                    &format!("Identity import failed: {error_value}"),
                );
            }
        } else if let Err(error_value) = open_auth(&state, &deployment) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
        }
    }
    if let Some(files) = input.initial_files.as_ref() {
        let files_dir = directory.join("files");
        if let Err(error_value) = restore_initial_files(&files_dir, files, 16 * 1024 * 1024).await {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("File import failed: {error_value}"),
            );
        }
    }
    // New files go into a fresh version folder. Readers keep the previous
    // version until the deployment record switches to this one below.
    if let Err(error_value) =
        stage_local_artifact(&state, &id, &artifact_version, &artifact_files).await
    {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    if let Err(error_value) =
        upload_artifact(&state, &id, &artifact_version, artifact_files).await
    {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(
            StatusCode::BAD_GATEWAY,
            &format!("Artifact upload failed: {error_value}"),
        );
    }
    if let Err(error_value) = persist_deployment(&state, &deployment).await {
        remove_artifact_version(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    state
        .deployments
        .write()
        .await
        .insert(hostname, deployment.clone());
    http::routes_published(&deployment);
    remove_stale_artifacts(&state, &id, &artifact_version).await;
    Json(json!({"result": deployment})).into_response()
}

async fn stage_local_artifact(
    state: &AppState,
    id: &str,
    version: &str,
    files: &HashMap<String, Vec<u8>>,
) -> Result<()> {
    let root = artifact_root(state, id).join(version);
    for (path, contents) in files {
        let local_path = root.join(path);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&local_path, contents).await?;
    }
    Ok(())
}

async fn upload_artifact(
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

/// Best-effort removal of a version that never went live.
async fn remove_artifact_version(state: &AppState, id: &str, version: &str) {
    let _ = tokio::fs::remove_dir_all(artifact_root(state, id).join(version)).await;
    if let Some(s3) = state.s3.as_ref() {
        let prefix = format!("deployments/{id}/{version}/");
        if let Ok(keys) = list_s3_keys(s3, &state.artifact_bucket, &prefix).await {
            for key in keys {
                let _ = s3.delete_object().bucket(&state.artifact_bucket).key(key).send().await;
            }
        }
    }
}

/// Deletes every artifact file outside the live version, including files that
/// older deployments stored at the prefix root. Failures are logged because the
/// new version is already live.
async fn remove_stale_artifacts(state: &AppState, id: &str, version: &str) {
    let root = artifact_root(state, id);
    if let Ok(mut entries) = tokio::fs::read_dir(&root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_str() == Some(version) {
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

fn find_project_deployment(
    deployments: &HashMap<String, Deployment>,
    project_id: &str,
) -> Option<Deployment> {
    deployments
        .values()
        .find(|deployment| deployment.project_id == project_id)
        .cloned()
}

async fn invoke_action(
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
    // (ADR-0017, update of 2026-11-08).
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
        actor.as_ref().map(|user| user.role.as_str()),
    ) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    // A call is queued for the Kernel's worker and answered with its ID.
    if action.kind == "call" {
        return calls::enqueue(&state, &deployment, &name, &input).await;
    }
    let db_path = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("database.sqlite");
    // A signal action runs no SQL. The Signal is queued for the Usage store and
    // the answer does not wait for the write.
    let result = if action.kind == "signal" {
        Ok(Ran { answer: json!({"recorded": true}), rows: Vec::new() })
    } else {
        let _guard = database_lock(&state, &deployment.id).await;
        run_action(db_path, action, params).await
    };
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    match result {
        Ok(ran) => match guard::outcome(&name, action, &ran.answer) {
            guard::Outcome::Refused(message) => {
                usage::execution(&state.data_root, &deployment.id, &name, "runtime", "refused");
                usage::count(&state.data_root, &deployment.id, usage::Operation::Refused(name.clone()));
                error(StatusCode::FORBIDDEN, &message)
            }
            changed => {
                usage::execution(&state.data_root, &deployment.id, &name, "runtime", "ok");
                usage::count(&state.data_root, &deployment.id, usage::Operation::Action(name.clone()));
                if matches!(changed, guard::Outcome::Changed) {
                    usage::signal(&state.data_root, &deployment.id, &name, action, &input, actor_id);
                    let caller = notify::Caller { account: actor_id, actor_id };
                    notify::record(&state, &deployment, &name, action, &input, caller, ran.changed(now_ms())).await;
                }
                Json(json!({"result": ran.answer})).into_response()
            }
        },
        Err(error_value) => {
            usage::execution(&state.data_root, &deployment.id, &name, "runtime", "failed");
            error(StatusCode::BAD_REQUEST, &error_value.to_string())
        },
    }
}

async fn session(State(state): State<AppState>, headers: HeaderMap) -> Response {
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
struct AuthInput {
    email: String,
    /// Not read by a signup that requires verification: the person chooses
    /// the password when they answer the link.
    #[serde(default)]
    password: String,
}

async fn signup(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, true).await
}

async fn login(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, false).await
}

async fn auth_request(
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
            accounts::app_session_response(state, session)
        }
        Err(error_value) => auth_error(error_value),
    }
}

/// Runs blocking authentication work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AuthError> + Send + 'static,
) -> Result<T, AuthError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(AuthError::PasswordHash))
}

/// The answer when a login or signup exceeds its rate limit.
fn too_many_attempts(wait: std::time::Duration) -> Response {
    let mut response = error(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many sign-in attempts. Try again later.",
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
    response
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let token = headers.get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(&headers, app_cookie_name(&state)));
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
        .insert(header::SET_COOKIE, cleared_app_cookie(&state));
    response
}

/// The `Set-Cookie` value that removes the app session cookie from the browser.
fn cleared_app_cookie(state: &AppState) -> HeaderValue {
    let name = app_cookie_name(state);
    let secure = if name.starts_with("__Host-") { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}"))
        .unwrap()
}

async fn list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
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
    let path = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("files")
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

async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let actor = match authenticate_deployment(&state, &deployment, &headers) {
        Ok(Some(actor)) => actor,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "authentication-required"),
        Err(response) => return response,
    };
    let bytes = match axum::body::to_bytes(request.into_body(), 3_000_000).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "File upload exceeds the 2 MiB limit.",
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
    let visibility = match input.get("visibility").and_then(Value::as_str) {
        None | Some("private") => "private",
        Some("public") => "public",
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "File visibility must be private or public.",
            );
        }
    };
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("file")
        .replace(['/', '\\'], "-")
        .chars()
        .filter(|character| !character.is_control())
        .take(180)
        .collect::<String>();
    let content_type = input
        .get("type")
        .and_then(Value::as_str)
        .filter(|v| {
            v.len() < 128
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-/".contains(&c))
        })
        .unwrap_or("application/octet-stream")
        .to_owned();
    let data = match input
        .get("dataUrl")
        .and_then(Value::as_str)
        .map(decode_base64)
        .transpose()
    {
        Ok(Some(value)) if !value.is_empty() && value.len() <= 2 * 1024 * 1024 => value,
        Ok(Some(_)) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Files must be between 1 byte and 2 MiB.",
            );
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "File upload dataUrl must contain base64 data.",
            );
        }
    };
    let _guard = database_lock(&state, &deployment.id).await;
    let directory = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("files");
    if let Err(e) = tokio::fs::create_dir_all(&directory).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let mut files = match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(files) => files,
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid."),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let used = files
        .iter()
        .filter_map(|file| file["size"].as_u64())
        .sum::<u64>();
    if used + data.len() as u64 > 20 * 1024 * 1024 {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "This deployment has reached its 20 MiB file quota.",
        );
    }
    if files.len() >= files::MAX_FILES {
        return error(StatusCode::PAYLOAD_TOO_LARGE, files::TOO_MANY_FILES);
    }
    let id = Uuid::new_v4().to_string();
    let metadata = json!({
        "id": id,
        "name": name,
        "type": content_type,
        "size": data.len(),
        "visibility": visibility,
        "ownerId": actor.user_id,
        "createdAt": now_ms(),
    });
    if let Err(e) = tokio::fs::write(directory.join(format!("{id}.blob")), &data).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    files.insert(0, metadata.clone());
    let index = match serde_json::to_vec_pretty(&files) {
        Ok(bytes) => bytes,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = write_atomic(&directory.join("index.json"), &index).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({"result": metadata})).into_response()
}

async fn serve_file(
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
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return error(StatusCode::NOT_FOUND, "File not found.");
    }
    let directory = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("files");
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
    match tokio::fs::read(directory.join(format!("{id}.blob"))).await {
        Ok(bytes) => file_response(metadata, bytes),
        Err(_) => error(StatusCode::NOT_FOUND, "File not found."),
    }
}

/// A stored file's bytes with its own type. Raster images display inline;
/// everything else downloads, and nothing in it may run.
fn file_response(metadata: &Value, bytes: Vec<u8>) -> Response {
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
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(files::served_type(content_type))
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if inline { "inline" } else { "attachment" }),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

async fn serve_root(State(state): State<AppState>, headers: HeaderMap) -> Response {
    serve_key(state, headers, "index.html".into()).await
}

async fn login_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    };
    match authenticate_deployment(&state, &deployment, &headers) {
        Ok(Some(_)) => (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response(),
        Ok(None) => private_login_page(&deployment.signup_policy, state.accounts.verification),
        Err(response) => response,
    }
}

async fn serve_artifact(
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

async fn serve_key(state: AppState, headers: HeaderMap, path: String) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Littlebox deployment not found."),
    };
    if deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        == Some("private")
    {
        match authenticate_deployment(&state, &deployment, &headers) {
            Ok(Some(_)) => {}
            Ok(None) => return private_login_page(&deployment.signup_policy, state.accounts.verification),
            Err(response) => return response,
        }
    }
    if artifact::kernel_path(&path) {
        return artifact::not_found_page();
    }
    let routing = artifact::routing(&deployment.manifest);
    for candidate in artifact::candidates(&path, routing) {
        if let Ok(bytes) = read_artifact(&state, &deployment, &candidate).await {
            return artifact_response(bytes, &candidate);
        }
    }
    // Browsers ask for /favicon.ico on every page. An app without one answers "no icon", not an error.
    if path == "favicon.ico" {
        return StatusCode::NO_CONTENT.into_response();
    }
    match routing {
        artifact::Routing::Spa => match read_artifact(&state, &deployment, "index.html").await {
            Ok(bytes) => artifact_response(bytes, "index.html"),
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

async fn read_artifact(state: &AppState, deployment: &Deployment, path: &str) -> Result<Vec<u8>> {
    if !safe_relative_path(path) {
        anyhow::bail!("invalid artifact path");
    }
    let relative = match deployment.artifact_version.as_deref() {
        Some(version) => format!("{version}/{path}"),
        None => path.to_owned(),
    };
    let local = artifact_root(state, &deployment.id).join(&relative);
    match tokio::fs::read(local).await {
        Ok(bytes) => Ok(bytes),
        Err(local_error) => {
            let Some(s3) = state.s3.as_ref() else {
                return Err(local_error.into());
            };
            let key = format!("deployments/{}/{}", deployment.id, relative);
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

fn private_login_page(policy: &SignupPolicy, verification: auth::Verification) -> Response {
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
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}form,label{display:grid;gap:8px}form{gap:16px;margin-top:24px}input,button{font:inherit;border-radius:8px;padding:11px 12px}input{width:100%;border:1px solid #c9ced6}button{border:0;background:#273449;color:white;cursor:pointer}button+button{background:#edf0f4;color:#252a33}#error{min-height:1.5em;margin:0;color:#a32727}</style><main><h1>Sign in to this app</h1><p>"#,
        note,
        r#"</p><form id="auth"><label>Email <input name="email" type="email" required autocomplete="email"></label><label>Password <input name="password" type="password" minlength="8" required autocomplete="current-password"></label><button formaction="/_littlebox/auth/login">Sign in</button>"#,
        if create && verified {
            r#"<button formaction="/_littlebox/auth/signup" formnovalidate>Create account</button>"#
        } else if create {
            r#"<button formaction="/_littlebox/auth/signup">Create account</button>"#
        } else {
            ""
        },
        r#"<p id="error" role="alert"></p><a href="/_littlebox/account">Forgot your password?</a></form></main><script>document.querySelector('#auth').addEventListener('submit',async e=>{e.preventDefault();const path=e.submitter?.formAction||'/_littlebox/auth/login';const form=new FormData(e.currentTarget);const response=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({email:form.get('email'),password:form.get('password')})});const body=await response.json().catch(()=>({}));if(response.status===202){document.querySelector('#error').textContent=body.result.message;return}if(response.ok){location.replace('/');return}document.querySelector('#error').textContent=body.error||'Sign in failed.'})</script></html>"#,
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

fn artifact_response(bytes: Vec<u8>, path: &str) -> Response {
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

async fn export_database(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    request: Request,
) -> Response {
    if !is_admin(&state, request.headers()) {
        return error(StatusCode::UNAUTHORIZED, "Admin authorization required.");
    }
    let _guard = database_lock(&state, &id).await;
    let path = state
        .data_root
        .join("deployments")
        .join(id)
        .join("database.sqlite");
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/vnd.sqlite3")], bytes).into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "Database not found."),
    }
}

async fn export_portable_state(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    request: Request,
) -> Response {
    if !is_admin(&state, request.headers()) {
        return error(StatusCode::UNAUTHORIZED, "Admin authorization required.");
    }
    let Some(deployment) = state
        .deployments
        .read()
        .await
        .values()
        .find(|deployment| deployment.id == id)
        .cloned()
    else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let _project_guard = database_lock(&state, &format!("project:{}", deployment.project_id)).await;
    let deployment = state.deployments.read().await.values()
        .find(|current| current.id == id).cloned().unwrap_or(deployment);
    let _data_guard = database_lock(&state, &deployment.id).await;
    let _identity_guard = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    match portable_state(&state, &deployment).await {
        Ok(result) => Json(json!({"result": result})).into_response(),
        Err(error_value) => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
    }
}

async fn portable_state(state: &AppState, deployment: &Deployment) -> Result<Value> {
    let backup_dir = backup_dir(state, &deployment.id);
    tokio::fs::create_dir_all(&backup_dir).await?;
    let database_path = database_path(state, &deployment.id);
    let database = sqlite_snapshot(&database_path, &backup_dir, "portable-data").await?;
    if database.len() > 16 * 1024 * 1024 {
        anyhow::bail!("Database exceeds the 16 MiB portable export limit.");
    }

    let identity_path = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("identity.sqlite");
    let _ = open_auth(state, deployment)?;
    let identity_copy = sqlite_snapshot(&identity_path, &backup_dir, "portable-identity").await?;
    let identity_source = backup_dir.join(format!("identity-source-{}.sqlite", Uuid::new_v4()));
    tokio::fs::write(&identity_source, identity_copy).await?;
    let identity_sanitized =
        backup_dir.join(format!("identity-sanitized-{}.sqlite", Uuid::new_v4()));
    {
        let connection = Connection::open(&identity_source)?;
        connection.execute("DELETE FROM app_sessions", [])?;
        connection.execute("DELETE FROM account_challenges", [])?;
        invitations::forget_tokens(&connection)?;
        connection.execute(
            "VACUUM INTO ?1",
            [identity_sanitized.to_string_lossy().as_ref()],
        )?;
    }
    let identity = tokio::fs::read(&identity_sanitized).await?;
    let _ = tokio::fs::remove_file(&identity_source).await;
    let _ = tokio::fs::remove_file(&identity_sanitized).await;

    let files_dir = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("files");
    let index_path = files_dir.join("index.json");
    let file_index = match tokio::fs::read(index_path).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes)?,
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error_value) => return Err(error_value.into()),
    };
    let mut portable_files = Vec::with_capacity(file_index.len());
    for metadata in file_index {
        let file_id = metadata
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Stored file has no id."))?;
        if !valid_file_id(file_id) {
            anyhow::bail!("Stored file id is invalid.");
        }
        let bytes = tokio::fs::read(files_dir.join(format!("{file_id}.blob"))).await?;
        let mut item = metadata
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Stored file metadata is invalid."))?;
        item.insert("dataBase64".into(), Value::String(encode_base64(&bytes)));
        portable_files.push(Value::Object(item));
    }
    let file_bundle = serde_json::to_vec(&json!({"files": portable_files}))?;
    let total = database
        .len()
        .saturating_add(identity.len())
        .saturating_add(file_bundle.len());
    if total > 16 * 1024 * 1024 {
        anyhow::bail!("Portable state exceeds the 16 MiB combined export limit.");
    }
    Ok(json!({
        "domain": domain::for_package(state, &deployment.id),
        "initialDatabase": encode_base64(&database),
        "initialIdentity": encode_base64(&identity),
        "initialFiles": encode_base64(&file_bundle),
        "initialSignupPolicy": deployment.signup_policy,
        "visibility": if is_private(deployment) { "private" } else { "public" },
        "sizeBytes": total
    }))
}

async fn sqlite_snapshot(source: &Path, directory: &Path, label: &str) -> Result<Vec<u8>> {
    let snapshot = directory.join(format!("{label}-{}.sqlite", Uuid::new_v4()));
    let connection = Connection::open(source)?;
    connection.execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()])?;
    drop(connection);
    let bytes = tokio::fs::read(&snapshot).await?;
    tokio::fs::remove_file(snapshot).await?;
    Ok(bytes)
}

async fn admin_database(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    request: Request,
) -> Response {
    if !is_admin(&state, request.headers()) {
        return error(StatusCode::UNAUTHORIZED, "Admin authorization required.");
    }
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
    let bytes = match axum::body::to_bytes(request.into_body(), 200_000).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Database request is too large.",
            );
        }
    };
    let body: Value = match serde_json::from_slice(&bytes) {
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
        "apply-migration" => match apply_migration(&state, &deployment.id, &body).await {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => match refusal::of(&e) {
                Some(refused) => refusal_error(refused),
                None => error(StatusCode::BAD_REQUEST, &e.to_string()),
            },
        },
        _ => error(StatusCode::BAD_REQUEST, "Unknown database operation."),
    }
}

fn database_path(state: &AppState, id: &str) -> PathBuf {
    state
        .data_root
        .join("deployments")
        .join(id)
        .join("database.sqlite")
}

/// Named locks that serialize work on one database or transfer. An entry
/// lives only while someone holds or waits for it, so callers that pass many
/// distinct names cannot grow the map.
type LockMap = Arc<std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>>;

/// Holds one named lock. Dropping it releases the lock and removes the map
/// entry when no other task holds or waits for the same name.
struct NamedLockGuard {
    guard: Option<OwnedMutexGuard<()>>,
    entry: Option<Arc<Mutex<()>>>,
    name: String,
    locks: LockMap,
}

impl Drop for NamedLockGuard {
    fn drop(&mut self) {
        let mut locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Release while holding the map lock, so no caller can insert a fresh
        // mutex for this name while the old one is still held.
        self.guard.take();
        let entry = self.entry.take();
        // The map and this guard's handle are the only owners left when
        // nobody else holds or waits for the name.
        if entry.is_some_and(|entry| Arc::strong_count(&entry) == 2) {
            locks.remove(&self.name);
        }
    }
}

async fn database_lock(state: &AppState, id: &str) -> NamedLockGuard {
    let entry = {
        let mut locks = state.db_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    // Built before waiting, so a caller cancelled while waiting still removes
    // the entry it may have created.
    let mut held = NamedLockGuard {
        guard: None,
        entry: Some(entry.clone()),
        name: id.to_owned(),
        locks: state.db_locks.clone(),
    };
    held.guard = Some(entry.lock_owned().await);
    held
}

fn inspect_database(path: &Path) -> Result<Vec<Value>> {
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

fn table_columns(db: &Connection, table: &str) -> Result<Vec<Value>> {
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

fn table_exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn browse_table(path: &Path, body: &Value) -> Result<Value> {
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
        "SELECT rowid AS __littlebox_rowid, * FROM {} ORDER BY {sort} {direction} LIMIT ?1 OFFSET ?2",
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
                if name == "__littlebox_rowid" {
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

fn run_admin_sql(path: &Path, sql: &str) -> Result<Value> {
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

fn admin_update_cell(path: &Path, body: &Value) -> Response {
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

fn admin_insert_row(path: &Path, body: &Value) -> Response {
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

fn admin_delete_row(path: &Path, body: &Value) -> Response {
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

fn value_to_sql(value: Option<&Value>) -> Result<SqlValue> {
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

async fn list_backups(state: &AppState, id: &str) -> Result<Vec<Value>> {
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
        backups.push(json!({"id":backup.id,"label":backup.label,"createdAt":backup.created_at,"sizeBytes":backup.size_bytes,"databaseModelSource":backup.database_model_source,"schemaRevision":backup.schema_revision}));
    }
    backups.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
    Ok(backups)
}

async fn create_backup(state: &AppState, id: &str, label: &str) -> Result<Value> {
    let db_path = database_path(state, id);
    let dir = backup_dir(state, id);
    tokio::fs::create_dir_all(&dir).await?;
    let backup_id = Uuid::new_v4().to_string();
    let snapshot_path = dir.join(format!("{backup_id}.sqlite"));
    let db = Connection::open(&db_path)?;
    db.execute("VACUUM INTO ?1", [snapshot_path.to_string_lossy().as_ref()])?;
    let size = tokio::fs::metadata(&snapshot_path).await?.len();
    let model = state.deployments.read().await.values().find(|d| d.id == id).and_then(|d| d.database_model.clone());
    let model = if let Some(model) = model {
        if mcp::model_matches_path(&model.accepted_source, &snapshot_path).await.unwrap_or(false) { Some(model) } else { None }
    } else { None };
    let backup = DatabaseBackup {
        database_model_source: model.as_ref().map(|m| m.accepted_source.clone()),
        schema_revision: model.map(|m| m.revision),
        id: backup_id.clone(),
        label: label.chars().take(120).collect(),
        created_at: now_ms(),
        size_bytes: size,
        key: format!("{backup_id}.sqlite"),
    };
    tokio::fs::write(
        dir.join(format!("{backup_id}.json")),
        serde_json::to_vec(&backup)?,
    )
    .await?;
    Ok(
        json!({"id":backup.id,"label":backup.label,"createdAt":backup.created_at,"sizeBytes":backup.size_bytes,"databaseModelSource":backup.database_model_source,"schemaRevision":backup.schema_revision}),
    )
}

async fn restore_backup(state: &AppState, id: &str, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }
    let metadata: DatabaseBackup = serde_json::from_slice(&tokio::fs::read(backup_dir(state, id).join(format!("{backup_id}.json"))).await?)?;
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
    tokio::fs::rename(&temp_path, database_path(state, id)).await?;
    let model = metadata.database_model_source.as_ref().zip(metadata.schema_revision.as_ref()).map(|(source, revision)| DatabaseModel {
        source: source.clone(), accepted_source: source.clone(), revision: revision.clone(),
    });
    persist_database_model(state, id, model).await?;
    Ok(json!({"id":backup_id,"sizeBytes":bytes.len(),"databaseModelSource":metadata.database_model_source,"schemaRevision":metadata.schema_revision}))
}

async fn persist_database_model(state: &AppState, id: &str, model: Option<DatabaseModel>) -> Result<()> {
    let mut deployment = state.deployments.read().await.values().find(|d| d.id == id).cloned()
        .ok_or_else(|| anyhow::anyhow!("Deployment not found."))?;
    deployment.database_model = model;
    persist_deployment(state, &deployment).await?;
    state.deployments.write().await.insert(deployment.hostname.clone(), deployment);
    Ok(())
}

async fn apply_migration(state: &AppState, id: &str, body: &Value) -> Result<Value> {
    let statements = body
        .get("statements")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Migration statements are required."))?;
    if statements.is_empty() || statements.len() > 200 {
        anyhow::bail!("Migration must contain 1 to 200 statements.");
    }
    let candidate = body["candidateModelSource"].as_str();
    let next_model = if let Some(source) = candidate {
        let previous = body["recoveryModelSource"].as_str().ok_or_else(|| anyhow::anyhow!("The migration needs its accepted model."))?;
        let deployed = state.deployments.read().await.values().find(|d| d.id == id).and_then(|d| d.database_model.clone());
        if let Some(deployed) = deployed {
            anyhow::ensure!(deployed.accepted_source == previous, "The deployed database model changed. Reload and plan the migration again.");
        }
        let planned = mcp::schema_plan(json!({"acceptedSource":previous,"candidateSource":source})).await?;
        anyhow::ensure!(planned["id"] == body["id"] && planned["fromHash"] == body["fromHash"] && planned["toHash"] == body["toHash"] && planned["statements"] == body["statements"],
            "The migration differs from its database models. Plan it again.");
        Some(DatabaseModel { source: source.into(), accepted_source: source.into(), revision: planned["toHash"].as_str().unwrap().into() })
    } else { None };
    let candidate_schema = match &next_model {
        Some(model) => Some(mcp::model_schema(&model.accepted_source).await?),
        None => None,
    };
    let mut planned = Vec::with_capacity(statements.len());
    for statement in statements {
        let sql = statement
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Migration statement must be text."))?;
        if sql.len() > 100_000 {
            anyhow::bail!("Migration statement is too large.");
        }
        planned.push(sql.to_owned());
    }
    // A rebuilt table keeps the CHECKs schema.sql gave it. Refused before the
    // backup, so a refused migration leaves no recovery point behind.
    let statements = {
        let db = Connection::open(database_path(state, id))?;
        let live = |name: &str| db.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1 COLLATE NOCASE", [name], |row| row.get::<_, String>(0)).ok();
        checks::carry_checks(&planned, live)?
    };
    let backup = create_backup(state, id, "Before migration").await?;
    {
        let mut db = Connection::open(database_path(state, id))?;
        restrict_database(&db)?;
        let tx = db.transaction()?;
        for sql in &statements {
            tx.execute_batch(sql)?;
        }
        if let Some(expected) = &candidate_schema {
            anyhow::ensure!(mcp::schema_tokens(expected)? == mcp::schema_tokens(&tx)?,
                "The migrated database does not match its candidate model. The migration was rolled back.");
        }
        let integrity: String = tx.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            anyhow::bail!("Migrated database failed SQLite integrity check.");
        }
        tx.commit()?;
    }
    persist_database_model(state, id, next_model).await?;
    Ok(json!({"backup":backup}))
}

/// Runs an action's statement on a blocking thread while the caller holds
/// the Deployment's database lock. The statement deadline is checked between
/// SQLite steps, and one step, such as `instr` or `replace` over two large
/// values, can run past it. On a blocking thread that delays only this
/// Deployment, never the async workers every other app shares.
/// What a statement did. `answer` is what the caller reads. `rows` are the
/// changed rows a mutation's notice reads back through the Kernel's own
/// RETURNING clause (ADR-0021); they never reach the answer.
struct Ran {
    answer: Value,
    rows: Vec<serde_json::Map<String, Value>>,
}

impl Ran {
    /// What a notice reads of this run, at `now` in milliseconds.
    fn changed(&self, now: u64) -> notify::Changed<'_> {
        notify::Changed { count: self.answer["changes"].as_u64().unwrap_or(0), rows: &self.rows, now }
    }
}

async fn run_action(path: PathBuf, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Ran> {
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

fn run_query(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Vec<Value>> {
    let connection = Connection::open(path)?;
    restrict_database(&connection)?;
    let mut statement = connection.prepare(&action.statement)?;
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

fn run_mutation(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Ran> {
    let mut connection = Connection::open(path)?;
    restrict_database(&connection)?;
    let transaction = connection.transaction()?;
    // A notice that lists or addresses rows reads them back with a RETURNING
    // clause the Kernel appends; the author's statement cannot hold one.
    let columns = action.notify.as_ref().map(|_| notify::returning("", action)).unwrap_or_default();
    let mut rows = Vec::new();
    let changes = if columns.is_empty() {
        transaction.execute(&action.statement, params_from_iter(params))? as u64
    } else {
        let mut statement = transaction.prepare(&notify::with_returning(&action.statement, &columns))?;
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
/// or an API key's. `actor_role` is a person's role in this app's
/// `identity.sqlite`, "owner" or "member", and None for a visitor, a key or a
/// signed request, which binds NULL rather than failing, so a guard such as
/// `?1 = 'owner'` refuses them.
fn resolve_bindings(
    action: &ActionDefinition,
    input: &Value,
    actor_id: Option<&str>,
    actor_role: Option<&str>,
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
            "actor" if binding.field.as_deref() == Some("role") => {
                Ok(actor_role.map_or(SqlValue::Null, |role| SqlValue::Text(role.to_owned())))
            }
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

fn validate_action(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    validate_declarations(name, action)?;
    action_shape::check_keys(name, action)
}

fn validate_declarations(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
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
fn unbounded_query_reason(sql: &str) -> Option<&'static str> {
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
fn limit_index(top: &[&SqlTok]) -> Option<usize> {
    top.iter().enumerate().position(|(index, token)| {
        let parameter = index > 0 && [":", "@", "$"].iter().any(|symbol| top[index - 1].is_symbol(symbol));
        token.is_word("LIMIT") && !parameter
    })
}

fn aggregate_select_list(list: &[&SqlTok]) -> bool {
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
enum SqlTokKind { Word, Ident, String, Symbol }

struct SqlTok {
    kind: SqlTokKind,
    text: String,
    depth: i32,
    /// Offsets in chars into the statement, so a reader can quote the token as written.
    start: usize,
    end: usize,
}

impl SqlTok {
    fn is_word(&self, word: &str) -> bool {
        self.kind == SqlTokKind::Word && self.text.eq_ignore_ascii_case(word)
    }

    fn is_any(&self, words: &[&str]) -> bool {
        self.kind == SqlTokKind::Word && words.iter().any(|word| self.text.eq_ignore_ascii_case(word))
    }

    fn is_symbol(&self, text: &str) -> bool {
        self.kind == SqlTokKind::Symbol && self.text == text
    }
}

fn sql_tokens(sql: &str) -> Vec<SqlTok> {
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
                SqlTok { kind: SqlTokKind::String, text: String::new(), depth, start: index, end: span_end }
            } else {
                SqlTok { kind: SqlTokKind::Ident, text, depth, start: index, end: span_end }
            });
            index = span_end;
            continue;
        }
        if is_sql_word_start(char) {
            let mut end = index + 1;
            while end < chars.len() && is_sql_word_cont(chars[end]) { end += 1; }
            tokens.push(SqlTok { kind: SqlTokKind::Word, text: chars[index..end].iter().collect(), depth, start: index, end });
            index = end;
            continue;
        }
        if char.is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '.' || chars[end] == '_') {
                end += 1;
            }
            tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: chars[index..end].iter().collect(), depth, start: index, end });
            index = end;
            continue;
        }
        if char.is_whitespace() {
            index += 1;
            continue;
        }
        if char == ')' { depth = (depth - 1).max(0); }
        tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: char.to_string(), depth, start: index, end: index + 1 });
        if char == '(' { depth += 1; }
        index += 1;
    }
    tokens
}

fn is_sql_word_start(char: char) -> bool {
    char.is_ascii_alphabetic() || char == '_' || (char as u32) >= 0x80
}

fn is_sql_word_cont(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_' || char == '$' || (char as u32) >= 0x80
}

fn initialize_database(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;")?;
    Ok(())
}

fn apply_additive_schema(path: &Path, schema: &str, program: &ActionProgram) -> Result<()> {
    let mut connection = Connection::open(path)?;
    // Runs before the authorizer below, which refuses every pragma.
    limit_database_size(&connection, MAX_APP_DATABASE_BYTES)?;
    // An expression index or a CHECK runs once per stored row while the
    // schema applies, so a value has the runtime's size limit here too, and
    // the whole schema a deadline long enough to index a full database.
    connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    connection.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline));
    if let Some(refusal) = sql_rules::schema_refusal(schema) {
        anyhow::bail!(refusal);
    }
    let transaction = connection.transaction()?;
    transaction.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::CreateTable { .. } | AuthAction::CreateIndex { .. } => Authorization::Allow,
        // CHECK constraints, expression and partial indexes, and generated
        // columns call SQL functions. The runtime denylist still applies.
        AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
        AuthAction::Function { .. } => Authorization::Allow,
        AuthAction::Insert {
            table_name: "sqlite_master",
        } => Authorization::Allow,
        AuthAction::Update {
            table_name: "sqlite_master",
            ..
        } => Authorization::Allow,
        AuthAction::Read { .. } => Authorization::Allow,
        AuthAction::Reindex { .. } => Authorization::Allow,
        _ => Authorization::Deny,
    }));
    let first_result = transaction.execute_batch(schema);
    let second_result = if first_result.is_ok() {
        transaction.execute_batch(schema)
    } else {
        Ok(())
    };
    transaction.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
    first_result
        .map_err(|error| anyhow::anyhow!("schema may only create tables and indexes: {error}"))?;
    second_result
        .map_err(|error| anyhow::anyhow!("schema must be idempotent and additive: {error}"))?;
    for (name, action) in program.actions.iter().filter(|(_, action)| action.kind != "signal") {
        let statement = transaction.prepare(&action.statement).map_err(|error| {
            anyhow::anyhow!("action {name} does not match the deployed schema: {error}")
        })?;
        if action.kind == "mutation" && statement.column_count() > 0 {
            anyhow::bail!("Action {name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.");
        }
        drop(statement);
        notify::check_columns(&transaction, name, action)?;
    }
    transaction.commit()?;
    Ok(())
}

fn initialize_database_from_base64(path: &Path, encoded: &str, maximum_bytes: usize) -> Result<()> {
    let bytes = decode_base64(encoded)?;
    if bytes.len() > maximum_bytes {
        anyhow::bail!("Initial database exceeds the 16 MiB limit.");
    }
    let temp = path.with_extension("initial.sqlite");
    std::fs::write(&temp, &bytes)?;
    let check = Connection::open(&temp)?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let stored = sql_rules::stored_trigger_or_view(&check)?;
    drop(check);
    if integrity != "ok" {
        let _ = std::fs::remove_file(&temp);
        anyhow::bail!("Initial database failed SQLite's integrity check.");
    }
    if let Some(refusal) = stored {
        let _ = std::fs::remove_file(&temp);
        anyhow::bail!(refusal);
    }
    std::fs::rename(temp, path)?;
    Ok(())
}

fn initialize_identity_from_base64(path: &Path, encoded: &str, maximum_bytes: usize) -> Result<()> {
    let bytes = decode_base64(encoded)?;
    if bytes.len() > maximum_bytes {
        anyhow::bail!("Initial identity database exceeds 16 MiB.");
    }
    let temp = path.with_extension("initial-identity.sqlite");
    std::fs::write(&temp, &bytes)?;
    let check = Connection::open(&temp)?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let has_users = check.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_users')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    drop(check);
    if integrity != "ok" || !has_users {
        let _ = std::fs::remove_file(&temp);
        anyhow::bail!("Initial identity database is invalid.");
    }
    std::fs::rename(&temp, path)?;
    let _ = AuthService::open(path)?;
    let connection = Connection::open(path)?;
    connection.execute("DELETE FROM app_sessions", [])?;
    connection.execute("DELETE FROM account_challenges", [])?;
    http::revoke_imported_keys(&connection)?;
    invitations::forget_tokens(&connection)?;
    Ok(())
}

async fn restore_initial_files(
    directory: &Path,
    encoded: &str,
    maximum_bytes: usize,
) -> Result<()> {
    let bytes = decode_base64(encoded)?;
    if bytes.len() > maximum_bytes {
        anyhow::bail!("Initial file bundle exceeds 16 MiB.");
    }
    let bundle: Value = serde_json::from_slice(&bytes)?;
    let files = bundle
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Initial file bundle has no files array."))?;
    tokio::fs::create_dir_all(directory).await?;
    let mut metadata = Vec::new();
    let mut total = 0usize;
    for file in files {
        let id = file
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Initial file has no id."))?;
        if !valid_file_id(id) {
            anyhow::bail!("Initial file id is invalid.");
        }
        let data = decode_base64(
            file.get("dataBase64")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Initial file data is missing."))?,
        )?;
        if data.len() > 2 * 1024 * 1024 {
            anyhow::bail!("An initial file exceeds the 2 MiB limit.");
        }
        total += data.len();
        if total > maximum_bytes {
            anyhow::bail!("Initial files exceed the 16 MiB limit.");
        }
        let declared_size = file
            .get("size")
            .and_then(Value::as_u64)
            .unwrap_or(data.len() as u64);
        if declared_size != data.len() as u64 {
            anyhow::bail!("Initial file size does not match its data.");
        }
        let name = file
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("file")
            .replace(['/', '\\'], "-")
            .chars()
            .take(180)
            .collect::<String>();
        let content_type = file
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        let created_at = file
            .get("createdAt")
            .and_then(Value::as_u64)
            .unwrap_or_else(now_ms);
        let visibility = file_visibility(file);
        let mut restored = json!({
            "id": id,
            "name": name,
            "type": content_type,
            "size": data.len(),
            "visibility": visibility,
            "createdAt": created_at,
        });
        if let Some(owner) = file_owner(file).filter(|owner| accepted_owner_id(owner)) {
            restored["ownerId"] = Value::String(owner.to_owned());
        }
        tokio::fs::write(directory.join(format!("{id}.blob")), data).await?;
        metadata.push(restored);
    }
    write_atomic(&directory.join("index.json"), &serde_json::to_vec_pretty(&metadata)?).await?;
    Ok(())
}

/// Writes a sibling temp file and renames it over `path`, so readers never see
/// a truncated file.
async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

fn valid_file_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn file_visibility(file: &Value) -> &'static str {
    if file.get("visibility").and_then(Value::as_str) == Some("public") {
        "public"
    } else {
        "private"
    }
}

fn file_owner(file: &Value) -> Option<&str> {
    file.get("ownerId").and_then(Value::as_str)
}

fn accepted_owner_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Private files belong to the uploader. Public files are readable by any
/// signed-in user of the deployment, and by a visitor when the deployment
/// itself is public. A stored file with no owner stays unreadable.
fn file_readable(file: &Value, actor_id: Option<&str>, private_deployment: bool) -> bool {
    if file_visibility(file) == "public" {
        return !private_deployment || actor_id.is_some();
    }
    actor_id.is_some() && file_owner(file) == actor_id
}

fn file_view(file: &Value, actor_id: Option<&str>) -> Value {
    let mut view = file.clone();
    if let Some(object) = view.as_object_mut() {
        object.remove("url");
        if file_owner(file) != actor_id {
            object.remove("ownerId");
        }
    }
    view
}

fn encode_base64(bytes: &[u8]) -> String {
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

fn decode_base64(value: &str) -> Result<Vec<u8>> {
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

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

async fn persist_deployment(state: &AppState, deployment: &Deployment) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(deployment)?;
    write_atomic(&deployment_dir(state, &deployment.id).join("deployment.json"), &bytes).await?;
    Ok(())
}

/// Reads every Deployment record on the volume. A Kernel on a fresh volume
/// starts empty; `littlebox-kernel restore` installs copied folders first.
async fn load_deployments(root: &Path, public_origin: &str) -> Result<HashMap<String, Deployment>> {
    let mut result = HashMap::new();
    let mut entries = tokio::fs::read_dir(root.join("deployments")).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("deployment.json");
        if let Ok(bytes) = tokio::fs::read(path).await {
            if let Ok(mut deployment) = serde_json::from_slice::<Deployment>(&bytes) {
                deployment.url = deployment_url(public_origin, &deployment.hostname);
                result.insert(deployment.hostname.clone(), deployment);
            }
        }
    }
    Ok(result)
}

async fn read_s3_bytes(s3: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let object = s3.get_object().bucket(bucket).key(key).send().await?;
    Ok(object.body.collect().await?.into_bytes().to_vec())
}

async fn list_s3_keys(s3: &S3Client, bucket: &str, prefix: &str) -> Result<Vec<String>> {
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

async fn deployment_for(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
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

fn is_private(deployment: &Deployment) -> bool {
    deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        == Some("private")
}

fn is_admin(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get("x-littlebox-admin-key")
        .and_then(|value| value.to_str().ok())
        == Some(state.admin_key.as_str())
}

fn open_auth(
    state: &AppState,
    deployment: &Deployment,
) -> std::result::Result<AuthService, AuthError> {
    accounts::app_store(state, deployment)
}

fn authenticate_deployment(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
) -> std::result::Result<Option<AuthUser>, Response> {
    let auth = open_auth(state, deployment)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let token = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(headers, app_cookie_name(state)));
    auth.authenticate(token.as_deref())
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

fn app_cookie_name(state: &AppState) -> &'static str {
    if state.public_origin.starts_with("https://") {
        "__Host-littlebox_app_session"
    } else {
        "littlebox_app_session"
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

fn same_origin(headers: &HeaderMap) -> bool {
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
const MAX_LIKE_PATTERN_BYTES: i32 = 500;

/// Each app database may grow to this size. Writes past it fail with
/// "database or disk is full".
const MAX_APP_DATABASE_BYTES: u64 = 256 * 1024 * 1024;

/// Caps the database file behind this connection. SQLite keeps the setting per
/// connection, so every connection that may write needs it.
fn limit_database_size(db: &Connection, max_bytes: u64) -> Result<()> {
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let pages = (max_bytes / page_size.max(1)).max(1);
    db.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))?;
    Ok(())
}

/// SQL functions no app statement or schema may call. They reach the file
/// system, load native code, or swap full-text tokenizers by pointer.
const DENIED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "edit",
    "fts3_tokenizer",
];

fn denied_function(name: &str) -> bool {
    DENIED_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str())
}

fn restrict_database(db: &Connection) -> Result<()> {
    // Runs before the authorizer below, which refuses this pragma.
    limit_database_size(db, MAX_APP_DATABASE_BYTES)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 100_000)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    // LIKE and GLOB compare a value against the pattern inside one step, so
    // the deadline below cannot stop them; a short pattern keeps that step
    // short. SQLite's default is 50,000 bytes.
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, MAX_LIKE_PATTERN_BYTES)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    db.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline));
    db.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        // Littlebox runs no triggers or views, and Studio and migrations use
        // this connection too, so neither can add one the Gate never saw.
        AuthAction::CreateTrigger { .. } | AuthAction::CreateTempTrigger { .. } => Authorization::Deny,
        AuthAction::CreateView { .. } | AuthAction::CreateTempView { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, .. } if !["table_info", "table_xinfo", "index_list", "index_info", "foreign_key_list", "foreign_key_check", "integrity_check", "quick_check"].contains(&pragma_name.to_ascii_lowercase().as_str()) => Authorization::Deny,
        AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    Ok(())
}

fn bounded_rows(rows: impl Iterator<Item = rusqlite::Result<Value>>) -> Result<Vec<Value>> {
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

fn auth_error(error_value: AuthError) -> Response {
    let status = match error_value {
        AuthError::InvalidEmail | AuthError::WeakPassword => StatusCode::BAD_REQUEST,
        AuthError::EmailTaken | AuthError::AccountExists => StatusCode::CONFLICT,
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

fn json_sql(value: &Value) -> std::result::Result<SqlValue, String> {
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

fn sql_json(value: rusqlite::types::ValueRef<'_>) -> Value {
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

fn deployment_url(origin: &str, hostname: &str) -> String {
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

fn safe_slug(value: &str) -> String {
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
fn content_type(path: &str) -> &'static str {
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

const DEFAULT_ADMIN_KEY: &str = "local-development-key";

/// Returns the installation setup key. The well-known default only works on a
/// loopback bind with a local public origin, unless the operator opts in with
/// LITTLEBOX_ALLOW_DEFAULT_KEY=1. A proxy can expose a loopback bind, so the
/// public origin counts too.
fn admin_key_for(
    bind: &str,
    public_origin: &str,
    mcp_origin: Option<&str>,
    configured: Option<&str>,
    allow_default: bool,
) -> Result<String> {
    let key = configured
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ADMIN_KEY);
    if key != DEFAULT_ADMIN_KEY || allow_default {
        return Ok(key.to_owned());
    }
    if !is_local_host(bind) {
        anyhow::bail!(
            "LITTLEBOX_BIND={bind} is reachable from other machines. Set LITTLEBOX_ADMIN_KEY to a secret value before starting the Kernel."
        );
    }
    let origin_host = url::Url::parse(public_origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    if !origin_host.as_deref().is_some_and(is_local_host) {
        anyhow::bail!(
            "LITTLEBOX_PUBLIC_ORIGIN={public_origin} is not a local address. Set LITTLEBOX_ADMIN_KEY to a secret value before starting the Kernel."
        );
    }
    // MCP and owner sign-in also answer on this origin's host.
    if let Some(mcp_origin) = mcp_origin {
        let mcp_host = url::Url::parse(mcp_origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        if !mcp_host.as_deref().is_some_and(is_local_host) {
            anyhow::bail!(
                "LITTLEBOX_MCP_ORIGIN={mcp_origin} is not a local address. Set LITTLEBOX_ADMIN_KEY to a secret value before starting the Kernel."
            );
        }
    }
    Ok(key.to_owned())
}

/// A reachable Kernel must not evaluate a Drizzle model as its own user.
/// Loopback with a local origin may spawn the worker in-process.
pub(crate) fn derive_must_isolate(bind: &str, public_origin: &str, mcp_origin: &str) -> bool {
    !loopback_bind(bind)
        || !origin_is_local_or_empty(public_origin)
        || !origin_is_local_or_empty(mcp_origin)
}

fn loopback_bind(bind: &str) -> bool {
    let bind = bind.trim().trim_matches(['[', ']']);
    bind == "127.0.0.1" || bind == "::1" || bind.eq_ignore_ascii_case("localhost")
}

fn origin_is_local_or_empty(origin: &str) -> bool {
    if origin.is_empty() {
        return true;
    }
    url::Url::parse(origin)
        .ok()
        .and_then(|parsed| parsed.host_str().map(is_local_host))
        .unwrap_or(false)
}

/// True for localhost, *.localhost and loopback IP addresses.
fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

/// A refused Capsule: `error` stays the sentence clients read, and `code` and
/// `hint` sit beside it (ADR-0020).
fn refusal_error(refused: &refusal::Refusal) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": refused.message, "code": refused.code, "hint": refused.hint()}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_input_reads_camel_case_portable_state_fields() {
        let input: DeployInput = serde_json::from_value(json!({
            "project": { "id": "app_1", "name": "App", "slug": "app", "files": {}, "manifest": {} },
            "visibility": "public",
            "artifact": { "version": 1, "entrypoint": "index.html", "files": { "index.html": "<main></main>" } },
            "initialDatabase": "ZGF0YQ==",
            "initialIdentity": "aWRlbnRpdHk=",
            "initialFiles": "eyJmaWxlcyI6W119",
            "initialSignupPolicy": { "mode": "closed", "allowlist": [] }
        })).unwrap();

        assert_eq!(input.initial_database.as_deref(), Some("ZGF0YQ=="));
        assert_eq!(input.initial_identity.as_deref(), Some("aWRlbnRpdHk="));
        assert_eq!(input.initial_files.as_deref(), Some("eyJmaWxlcyI6W119"));
        assert_eq!(input.initial_signup_policy.unwrap().mode, SignupMode::Closed);
    }

    pub(crate) async fn login_test_state(root: &Path, visibility: &str) -> (AppState, HeaderMap) {
        let deployment = Deployment {
            id: Uuid::new_v4().to_string(),
            project_id: Uuid::new_v4().to_string(),
            project_name: "Login test".into(),
            url: "http://login-test.localhost".into(),
            created_at: now_ms(),
            manifest: json!({"visibility": visibility}),
            hostname: "login-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: None,
            database_model: None,
            signup_policy: SignupPolicy::default(),
        };
        tokio::fs::create_dir_all(root.join("deployments").join(&deployment.id))
            .await
            .unwrap();
        let state = AppState {
            deployments: Arc::new(RwLock::new(HashMap::from([(
                deployment.hostname.clone(),
                deployment,
            )]))),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(copies::CopyStatus::default()),
            admin_key: "test-admin".into(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
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
            resolve_bindings(&definition, &json!({"title": "Ship it"}), None, None).unwrap(),
            vec![SqlValue::Text("Ship it".into())]
        );
        assert!(
            resolve_bindings(
                &definition,
                &json!({"title": "Ship it", "sql": "DROP TABLE tasks"}),
                None,
                None,
            )
            .unwrap_err()
            .contains("Unexpected")
        );
        assert!(
            resolve_bindings(&definition, &json!({"title": 42}), None, None)
                .unwrap_err()
                .contains("non-empty string")
        );
    }

    #[test]
    fn the_actor_role_binds_a_persons_role_and_null_for_anyone_else() {
        let definition: ActionDefinition = serde_json::from_value(json!({"kind": "mutation", "access": "authenticated",
            "statement": "UPDATE profiles SET role = 'admin' WHERE user_id = ?2 AND ?1 = 'owner'",
            "bindings": [{"source": "actor", "field": "role"}, {"source": "actor", "field": "id"}]})).unwrap();
        assert_eq!(
            resolve_bindings(&definition, &json!({}), Some("olga"), Some("owner")).unwrap(),
            vec![SqlValue::Text("owner".into()), SqlValue::Text("olga".into())]
        );
        assert_eq!(resolve_bindings(&definition, &json!({}), Some("ana"), Some("member")).unwrap()[0], SqlValue::Text("member".into()));
        // An API key has an ID and no role; the role binds NULL and the guard's WHERE refuses.
        assert_eq!(resolve_bindings(&definition, &json!({}), Some("key-1"), None).unwrap()[0], SqlValue::Null);
        let anonymous: ActionDefinition = serde_json::from_value(json!({"kind": "query", "access": "anonymous",
            "statement": "SELECT ?1 = 'owner' AS owner LIMIT 1", "bindings": [{"source": "actor", "field": "role"}]})).unwrap();
        assert_eq!(resolve_bindings(&anonymous, &json!({}), None, None).unwrap(), vec![SqlValue::Null], "a visitor binds NULL instead of failing");
        assert!(resolve_bindings(&anonymous, &json!({"role": "owner"}), None, None).unwrap_err().contains("Unexpected action input: role"), "an input cannot supply it");
    }

    /// The refusal keeps its code out of apply_migration, so the route answers
    /// it with `refusal_error`, and it comes before the recovery point.
    #[tokio::test]
    async fn a_migration_that_drops_a_checked_column_refuses_with_its_code_and_no_backup() {
        let root = std::env::temp_dir().join(format!("littlebox-carry-refusal-{}", Uuid::new_v4()));
        let state = test_state(&root);
        let path = database_path(&state, "dep");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE habits(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, note TEXT, CONSTRAINT short_note CHECK (note IS NULL OR length(note) < 20))").unwrap();
        let body = json!({"statements": ["ALTER TABLE `habits` DROP COLUMN `note`;"]});
        let error = apply_migration(&state, "dep", &body).await.unwrap_err();
        let refused = refusal::of(&error).expect("a refusal with a code");
        assert_eq!(refused.code, "schema.check-names-removed-column");
        assert_eq!(refused.message, "Table habits has a CHECK that names column note, which this migration removes. Rebuild habits without that CHECK in the SQL console, or keep note, then apply the migration again.");
        assert!(!backup_dir(&state, "dep").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn login_route_is_available_for_public_and_private_apps() {
        for visibility in ["public", "private"] {
            let root = std::env::temp_dir()
                .join(format!("littlebox-login-{visibility}-{}", Uuid::new_v4()));
            let (state, headers) = login_test_state(&root, visibility).await;
            let response = login_page(State(state), headers).await;
            assert_eq!(response.status(), StatusCode::OK, "visibility={visibility}");
            let bytes = axum::body::to_bytes(response.into_body(), 16_384)
                .await
                .unwrap();
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(html.contains("Sign in to this app"));
            assert!(html.contains("/_littlebox/auth/signup"));
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

    #[test]
    fn deployment_records_without_a_policy_load_with_the_default_for_their_visibility() {
        let legacy = |manifest: Value| -> Deployment {
            serde_json::from_value(json!({
                "id": "d", "projectId": "p", "projectName": "Legacy", "url": "http://legacy.localhost",
                "createdAt": 1, "manifest": manifest, "hostname": "legacy.localhost",
                "actions": "{}"
            }))
            .unwrap()
        };
        let private = legacy(json!({"visibility": "private"}));
        assert_eq!(private.signup_policy.mode, SignupMode::Closed);
        let saved = serde_json::to_value(&private).unwrap();
        assert_eq!(saved["signupPolicy"], json!({"mode": "closed", "allowlist": []}));
        assert_eq!(legacy(json!({"visibility": "public"})).signup_policy.mode, SignupMode::Open);
        assert_eq!(legacy(json!({})).signup_policy.mode, SignupMode::Open);

        // A stored policy always wins over the visibility default.
        let stored: Deployment = serde_json::from_value(json!({
            "id": "d", "projectId": "p", "projectName": "Kept", "url": "http://kept.localhost",
            "createdAt": 1, "manifest": {"visibility": "private"}, "hostname": "kept.localhost",
            "actions": "{}", "signupPolicy": {"mode": "open", "allowlist": []}
        }))
        .unwrap();
        assert_eq!(stored.signup_policy.mode, SignupMode::Open);
    }

    #[tokio::test]
    async fn signup_follows_the_deployment_policy() {
        let root = std::env::temp_dir().join(format!("littlebox-signup-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("littlebox-login-policy-{}", Uuid::new_v4()));
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
        assert!(!html.contains("/_littlebox/auth/signup"));
        assert!(html.contains("not accepting new accounts"));
        set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
        let html = page(state.clone()).await;
        assert!(html.contains("/_littlebox/auth/signup"));
        assert!(html.contains("Only invited email addresses"));
        set_policy(&state, SignupMode::Invitation, &[]).await;
        let html = page(state.clone()).await;
        assert!(!html.contains("/_littlebox/auth/signup"));
        assert!(html.contains("Only invited people can create an account"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn private_apps_refuse_anonymous_actions_without_a_session() {
        let root = std::env::temp_dir().join(format!("littlebox-private-action-{}", Uuid::new_v4()));
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
    async fn a_scheduled_mutation_without_a_route_runs_only_on_its_schedule() {
        let root = std::env::temp_dir().join(format!("littlebox-scheduled-action-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("littlebox-schema-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();

        let initial = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let original_action = action_program(&[("tasks.list", "SELECT id, title FROM tasks")]);
        apply_additive_schema(&path, initial, &original_action).unwrap();
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
        apply_additive_schema(&path, &extended, &extended_actions).unwrap();
        apply_additive_schema(&path, &extended, &extended_actions).unwrap();

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

        assert!(apply_additive_schema(&path, "DROP TABLE tasks;", &extended_actions).is_err());
        let connection = Connection::open(&path).unwrap();
        let title: String = connection
            .query_row("SELECT title FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Keep this row");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn schemas_may_call_safe_functions_but_not_dangerous_ones() {
        let root = std::env::temp_dir().join(format!("littlebox-schema-functions-{}", Uuid::new_v4()));
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
        apply_additive_schema(&path, schema, &actions).unwrap();
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
            let error = apply_additive_schema(&path, &schema, &actions).unwrap_err().to_string();
            assert!(error.contains("schema may only"), "{function}: {error}");
        }
        let schema = "CREATE INDEX IF NOT EXISTS evil_index ON items(load_extension(name));";
        assert!(apply_additive_schema(&path, schema, &actions).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_schema_value_and_an_action_pattern_are_bounded_and_no_trigger_or_view_gets_in() {
        let root = std::env::temp_dir().join(format!("littlebox-sql-bounds-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let actions = action_program(&[("items.list", "SELECT id FROM items LIMIT 5")]);
        apply_additive_schema(&path, "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT);", &actions).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection.execute("INSERT INTO items(name) VALUES ('a')", []).unwrap();
        drop(connection);
        // An index whose expression builds a 200 MB value per row hits the
        // value limit instead of the memory of the Kernel.
        let heavy = "CREATE INDEX IF NOT EXISTS heavy ON items(length(hex(zeroblob(200000000))));";
        let error = apply_additive_schema(&path, heavy, &actions).unwrap_err().to_string();
        assert!(error.contains("too big"), "{error}");

        let db = Connection::open(&path).unwrap();
        restrict_database(&db).unwrap();
        let pattern = |length: usize| format!("SELECT 'x' LIKE '{}'", "%".repeat(length));
        db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize), [], |row| row.get::<_, bool>(0)).unwrap();
        let long = db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize + 1), [], |row| row.get::<_, bool>(0)).unwrap_err().to_string();
        assert!(long.contains("pattern too complex"), "{long}");
        // Studio and migrations open this connection too.
        for statement in ["CREATE TRIGGER t AFTER INSERT ON items BEGIN DELETE FROM items; END", "CREATE VIEW v AS SELECT 1",
            "CREATE TEMP TRIGGER t AFTER INSERT ON items BEGIN SELECT 1; END", "CREATE TEMP VIEW v AS SELECT 1"] {
            assert!(db.execute_batch(statement).unwrap_err().to_string().contains("not authorized"), "{statement}");
        }
        drop(db);

        // A database that arrives with a trigger is refused before it is installed.
        let source = root.join("source.sqlite");
        let db = Connection::open(&source).unwrap();
        db.execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY); CREATE TRIGGER copy AFTER INSERT ON items BEGIN DELETE FROM items; END;").unwrap();
        drop(db);
        let encoded = encode_base64(&std::fs::read(&source).unwrap());
        let target = root.join("imported.sqlite");
        let refused = initialize_database_from_base64(&target, &encoded, 16 * 1024 * 1024).unwrap_err().to_string();
        assert_eq!(refused, "The database holds CREATE TRIGGER copy, and Littlebox runs no triggers or views. Drop it before importing.");
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn named_locks_leave_no_entry_once_released() {
        let root = std::env::temp_dir().join(format!("littlebox-locks-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("littlebox-schema-action-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let invalid_action = action_program(&[("tasks.list", "SELECT missing FROM tasks")]);
        assert!(
            apply_additive_schema(
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
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(copies::CopyStatus::default()),
            admin_key: "test-admin".into(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        }
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
                assert_eq!(resolve_bindings(&definition, &input, None, None).unwrap(), vec![SqlValue::Null]);
            }
            assert!(resolve_bindings(&definition, &json!({"other": null}), None, None).unwrap_err().contains("Unexpected"));
            let wrong = match value_type {
                "string" => json!({"value": 5}),
                "number" => json!({"value": "5"}),
                _ => json!({"value": 0}),
            };
            assert!(resolve_bindings(&definition, &wrong, None, None).is_err());
            let required = action("query", "SELECT ?1", vec![input_binding("value", value_type, Some(4))]);
            assert!(resolve_bindings(&required, &json!({}), None, None).is_err());
            assert!(resolve_bindings(&required, &json!({"value": null}), None, None).is_err());
        }
        let mut binding = input_binding("value", "string", Some(4));
        binding.nullable = true;
        let definition = action("query", "SELECT ?1", vec![binding]);
        assert!(resolve_bindings(&definition, &json!({"value": ""}), None, None).is_err());
        assert!(resolve_bindings(&definition, &json!({"value": "longer"}), None, None).is_err());
        assert_eq!(resolve_bindings(&definition, &json!({"value": "ship"}), None, None).unwrap(), vec![SqlValue::Text("ship".into())]);
        for source in ["actor", "kernel", "literal"] {
            let mut binding = input_binding("value", "string", None);
            binding.source = source.into();
            binding.nullable = true;
            assert!(validate_action("inputs.echo", &action("query", "SELECT ?1", vec![binding])).is_err());
        }
    }

    #[test]
    fn mutation_returning_fails_schema_validation_without_executing() {
        let root = std::env::temp_dir().join(format!("littlebox-returning-{}", Uuid::new_v4()));
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
            assert!(apply_additive_schema(&path, schema, &program).unwrap_err().to_string().contains("cannot use RETURNING"));
        }
        for sql in [
            "INSERT INTO notes(body) VALUES ('RETURNING')",
            "UPDATE notes SET body='note' /* RETURNING id */",
            "UPDATE notes SET \"RETURNING\"=1",
        ] {
            let program = ActionProgram { version: 1, actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]) };
            apply_additive_schema(&path, schema, &program).unwrap();
        }
        let count: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_setup_key_is_refused_off_loopback() {
        let local = "http://localhost:8080";
        for bind in ["127.0.0.1", "::1", "[::1]", "localhost"] {
            assert_eq!(admin_key_for(bind, local, None, None, false).unwrap(), DEFAULT_ADMIN_KEY, "bind={bind}");
        }
        assert!(admin_key_for("0.0.0.0", local, None, None, false).is_err());
        assert!(admin_key_for("0.0.0.0", local, None, Some(""), false).is_err());
        assert!(admin_key_for("0.0.0.0", local, None, Some(DEFAULT_ADMIN_KEY), false).is_err());
        assert!(admin_key_for("192.168.1.4", local, None, Some(DEFAULT_ADMIN_KEY), false).is_err());
        assert_eq!(admin_key_for("0.0.0.0", local, None, Some("s3cret"), false).unwrap(), "s3cret");
        assert_eq!(
            admin_key_for("0.0.0.0", local, None, Some(DEFAULT_ADMIN_KEY), true).unwrap(),
            DEFAULT_ADMIN_KEY
        );
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_origin() {
        for origin in ["http://localhost:8790", "http://127.0.0.1:18900", "http://[::1]:8080", "http://editor.localhost"] {
            assert!(admin_key_for("127.0.0.1", origin, None, None, false).is_ok(), "origin={origin}");
        }
        for origin in ["https://apps.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(admin_key_for("127.0.0.1", origin, None, None, false).is_err(), "origin={origin}");
            assert!(admin_key_for("127.0.0.1", origin, None, None, true).is_ok(), "origin={origin}");
            assert!(admin_key_for("127.0.0.1", origin, None, Some("s3cret"), false).is_ok(), "origin={origin}");
        }
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_mcp_origin() {
        let local = "http://localhost:8080";
        for mcp in ["http://localhost:8790", "http://127.0.0.1:9000", "http://editor.localhost"] {
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, false).is_ok(), "mcp={mcp}");
        }
        for mcp in ["https://mcp.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, false).is_err(), "mcp={mcp}");
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, true).is_ok(), "mcp={mcp}");
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), Some("s3cret"), false).is_ok(), "mcp={mcp}");
        }
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
            resolve_bindings(&definition, &json!({"value": 5}), None, None).unwrap(),
            vec![SqlValue::Integer(5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": -3}), None, None).unwrap(),
            vec![SqlValue::Integer(-3)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": 5.5}), None, None).unwrap(),
            vec![SqlValue::Real(5.5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": u64::MAX}), None, None).unwrap(),
            vec![SqlValue::Real(u64::MAX as f64)]
        );

        let root = std::env::temp_dir().join(format!("littlebox-number-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE items(label TEXT)", [])
            .unwrap();
        let params = resolve_bindings(&definition, &json!({"value": 5}), None, None).unwrap();
        run_mutation(&path, &definition, params).unwrap();
        let stored: String = Connection::open(&path)
            .unwrap()
            .query_row("SELECT label FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, "5");
        let _ = std::fs::remove_dir_all(root);
    }

    /// ADR-0021: a notice that lists or addresses rows reads them back
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
        let root = std::env::temp_dir().join(format!("littlebox-returning-rows-{}", Uuid::new_v4()));
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
        assert!(resolve_bindings(&two, &emoji, None, None).is_ok());
        assert!(resolve_bindings(&one, &emoji, None, None).unwrap_err().contains("too long"));
        assert!(resolve_bindings(&two, &json!({"title": "\u{e9}\u{e9}"}), None, None).is_ok());
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
        let request = Request::builder()
            .header("x-littlebox-admin-key", "test-admin")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = deploy(State(state.clone()), request).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
    }

    #[tokio::test]
    async fn republish_stops_serving_files_missing_from_the_new_artifact() {
        let root = std::env::temp_dir().join(format!("littlebox-republish-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
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
        let mut entries = std::fs::read_dir(root.join("deployments").join(&id).join("artifact"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, vec![deployment.artifact_version.clone().unwrap()]);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn new_deployments_pick_a_policy_from_visibility_and_redeploys_keep_it() {
        for (visibility, expected) in [("public", SignupMode::Open), ("private", SignupMode::Closed)] {
            let root = std::env::temp_dir().join(format!("littlebox-policy-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
            let state = test_state(&root);
            let first = deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), visibility).await;
            assert_eq!(first["signupPolicy"]["mode"], json!(expected), "visibility={visibility}");

            set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
            let second = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), visibility).await;
            assert_eq!(second["signupPolicy"], json!({"mode": "allowlist", "allowlist": ["@example.test"]}));
            let id = second["id"].as_str().unwrap();
            let stored: Deployment = serde_json::from_slice(
                &tokio::fs::read(root.join("deployments").join(id).join("deployment.json")).await.unwrap(),
            )
            .unwrap();
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
            let root = std::env::temp_dir().join(format!("littlebox-flip-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
            let state = test_state(&root);
            deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), first).await;
            if let Some((mode, entry)) = chosen {
                let entries: Vec<&str> = if entry.is_empty() { vec![] } else { vec![entry] };
                set_policy(&state, mode, &entries).await;
            }
            let republished = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), second).await;
            assert_eq!(republished["signupPolicy"], expected, "{first} {chosen:?} -> {second}");
            let id = republished["id"].as_str().unwrap();
            let stored: Deployment = serde_json::from_slice(
                &tokio::fs::read(root.join("deployments").join(id).join("deployment.json")).await.unwrap(),
            )
            .unwrap();
            assert_eq!(serde_json::to_value(&stored.signup_policy).unwrap(), expected);
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn signup_checks_input_first_and_gives_one_answer_for_closed_and_unlisted() {
        let root = std::env::temp_dir().join(format!("littlebox-signup-oracle-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("littlebox-cap-{}", Uuid::new_v4()));
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

    #[tokio::test]
    async fn a_failed_record_write_removes_the_new_artifact_version() {
        let root = std::env::temp_dir().join(format!("littlebox-orphan-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        let id = first["id"].as_str().unwrap().to_owned();
        let record = root.join("deployments").join(&id).join("deployment.json");
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
        let request = Request::builder()
            .header("x-littlebox-admin-key", "test-admin")
            .body(Body::from(body.to_string()))
            .unwrap();
        assert_eq!(deploy(State(state.clone()), request).await.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let versions = std::fs::read_dir(root.join("deployments").join(&id).join("artifact")).unwrap().count();
        assert_eq!(versions, 1, "only the live version remains");
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>one</p>");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn legacy_artifacts_without_a_version_are_still_served() {
        let root = std::env::temp_dir().join(format!("littlebox-legacy-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let directory = root.join("deployments").join(&deployment.id).join("artifact");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        tokio::fs::write(directory.join("index.html"), "legacy").await.unwrap();
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"legacy");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restoring_a_backup_first_backs_up_the_live_database() {
        let root = std::env::temp_dir().join(format!("littlebox-restore-{}", Uuid::new_v4()));
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
        let snapshot = root
            .join("deployments")
            .join(id)
            .join("backups")
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

    pub(crate) fn signed_in(state: &AppState, deployment_id: &str, headers: &HeaderMap, email: &str) -> (HeaderMap, String) {
        let auth = AuthService::open(
            state
                .data_root
                .join("deployments")
                .join(deployment_id)
                .join("identity.sqlite"),
        )
        .unwrap();
        let session = auth.signup(email, "correct horse battery").unwrap();
        let mut headers = headers.clone();
        headers.insert(
            header::COOKIE,
            format!("littlebox_app_session={}", session.token).parse().unwrap(),
        );
        (headers, session.user.user_id)
    }

    pub(crate) async fn response_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn upload_refuses_to_overwrite_an_unreadable_file_index() {
        let root = std::env::temp_dir().join(format!("littlebox-upload-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
        let files = root.join("deployments").join(&deployment.id).join("files");
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
        let root = std::env::temp_dir().join(format!("littlebox-files-{}", Uuid::new_v4()));
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
        let directory = root.join("deployments").join(&deployment.id).join("files");
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

        let restored = root.join("restored");
        let kept = Uuid::new_v4().to_string();
        let orphan = Uuid::new_v4().to_string();
        let bundle = json!({"files": [
            {"id": kept, "name": "kept.txt", "type": "text/plain", "size": 2, "visibility": "public", "ownerId": "user-1", "createdAt": 1, "dataBase64": "aGk="},
            {"id": orphan, "name": "orphan.txt", "type": "text/plain", "size": 2, "visibility": "public", "createdAt": 1, "dataBase64": "aGk="}
        ]});
        restore_initial_files(&restored, &encode_base64(bundle.to_string().as_bytes()), 16 * 1024 * 1024).await.unwrap();
        let restored_index: Vec<Value> = serde_json::from_slice(&tokio::fs::read(restored.join("index.json")).await.unwrap()).unwrap();
        assert_eq!(restored_index[0]["ownerId"], "user-1");
        assert_eq!(restored_index[0]["visibility"], "public");
        assert!(restored_index[1].get("ownerId").is_none());
        assert_eq!(restored_index[1]["visibility"], "public");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn reachable_kernels_isolate_schema_derivation() {
        assert!(!derive_must_isolate("127.0.0.1", "http://localhost:8787", ""));
        assert!(!derive_must_isolate("::1", "", ""));
        assert!(derive_must_isolate("0.0.0.0", "http://localhost:8787", ""));
        assert!(derive_must_isolate("127.0.0.1", "https://apps.example", ""));
        assert!(derive_must_isolate("127.0.0.1", "http://localhost:8787", "https://mcp.example"));
    }
}
