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

mod auth;
use auth::{AuthError, AuthService, AuthUser};

#[derive(Clone)]
struct AppState {
    deployments: Arc<RwLock<HashMap<String, Deployment>>>,
    db_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    s3: Option<S3Client>,
    artifact_bucket: String,
    data_bucket: String,
    local_artifact_root: PathBuf,
    backup_root: PathBuf,
    data_root: PathBuf,
    admin_key: String,
    public_origin: String,
    host_suffix: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deployment {
    id: String,
    project_id: String,
    project_name: String,
    url: String,
    created_at: u64,
    manifest: Value,
    hostname: String,
    actions: String,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInput {
    id: String,
    name: String,
    slug: String,
    files: HashMap<String, String>,
    manifest: Value,
}

#[derive(Deserialize)]
struct BuildArtifact {
    version: u8,
    entrypoint: String,
    files: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseBackup {
    id: String,
    label: String,
    created_at: u64,
    size_bytes: u64,
    key: String,
}

#[derive(Deserialize)]
struct ActionProgram {
    version: u8,
    actions: HashMap<String, ActionDefinition>,
}

#[derive(Deserialize)]
struct ActionDefinition {
    kind: String,
    access: String,
    statement: String,
    bindings: Vec<ActionBinding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionBinding {
    source: String,
    key: Option<String>,
    #[serde(rename = "type")]
    value_type: Option<String>,
    max_length: Option<usize>,
    field: Option<String>,
    value: Option<Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let port = env("LITTLEBOX_KERNEL_PORT", "8787").parse::<u16>()?;
    let data_root = PathBuf::from(env("LITTLEBOX_DATA_DIR", "/data"));
    tokio::fs::create_dir_all(data_root.join("deployments")).await?;
    let s3 = if let Ok(endpoint) = std::env::var("LITTLEBOX_S3_ENDPOINT") {
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
    } else {
        None
    };
    let artifact_bucket = env("LITTLEBOX_ARTIFACT_BUCKET", "littlebox-apps");
    let data_bucket = env("LITTLEBOX_DATA_BUCKET", "littlebox-data");
    let deployments = load_deployments(&data_root, s3.as_ref(), &data_bucket).await?;
    let local_artifact_root = data_root.join("artifacts");
    let backup_root = data_root.join("backups");
    tokio::fs::create_dir_all(&local_artifact_root).await?;
    tokio::fs::create_dir_all(&backup_root).await?;
    let state = AppState {
        deployments: Arc::new(RwLock::new(deployments)),
        db_locks: Arc::new(Mutex::new(HashMap::new())),
        s3,
        artifact_bucket,
        data_bucket,
        local_artifact_root,
        backup_root,
        data_root,
        admin_key: env("LITTLEBOX_ADMIN_KEY", "local-development-key"),
        public_origin: env("LITTLEBOX_PUBLIC_ORIGIN", "http://localhost:8080"),
        host_suffix: env("LITTLEBOX_HOST_SUFFIX", "localhost"),
    };

    let app = Router::new()
        .route("/_littlebox/health", get(health))
        .route("/_littlebox/admin/deploy", post(deploy))
        .route("/_littlebox/admin/storage/test", post(test_storage))
        .route(
            "/_littlebox/admin/deployments/{id}/database/export",
            get(export_database),
        )
        .route(
            "/_littlebox/admin/deployments/{id}/portable-state",
            get(export_portable_state),
        )
        .route(
            "/_littlebox/admin/deployments/{id}/database",
            post(admin_database),
        )
        .route("/_littlebox/session", get(session))
        .route("/_littlebox/auth/signup", post(signup))
        .route("/_littlebox/auth/login", post(login))
        .route("/_littlebox/auth/logout", post(logout))
        .route("/_littlebox/login", get(login_page))
        .route("/_littlebox/files", get(list_files).post(upload_file))
        .route("/_littlebox/files/{id}", get(serve_file))
        .route("/_littlebox/actions/{*name}", post(invoke_action))
        .route("/{*path}", get(serve_artifact))
        .route("/", get(serve_root))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    println!("Littlebox Rust Kernel listening on {port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({"ok": true, "kernel": "littlebox-rust", "version": 1,
        "artifactStore": if state.s3.is_some() { "filesystem+s3" } else { "filesystem" }}))
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
    for (name, action) in &program.actions {
        if let Err(message) = validate_action(name, action) {
            return error(StatusCode::BAD_REQUEST, &message);
        }
    }
    let deployments = state.deployments.read().await;
    let existing = find_project_deployment(&deployments, &input.project.id);
    drop(deployments);
    if existing.is_some()
        && (input.initial_database.is_some()
            || input.initial_identity.is_some()
            || input.initial_files.is_some())
    {
        return error(
            StatusCode::CONFLICT,
            "Portable state can only be imported on the first deployment.",
        );
    }
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
    let mut manifest = input.project.manifest;
    manifest["visibility"] = Value::String(input.visibility.clone());
    let deployment = Deployment {
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
    };
    let directory = state.data_root.join("deployments").join(&id);
    if let Err(error_value) = tokio::fs::create_dir_all(&directory).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    let _database_guard = database_lock(&state, &id).await;
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
    for (path, contents) in input.artifact.files {
        if !safe_relative_path(&path) {
            return error(
                StatusCode::BAD_REQUEST,
                "Artifact contains an invalid path.",
            );
        }
        let local_path = state.local_artifact_root.join(&id).join(&path);
        if let Some(parent) = local_path.parent() {
            if let Err(error_value) = tokio::fs::create_dir_all(parent).await {
                return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
            }
        }
        if let Err(error_value) = tokio::fs::write(&local_path, contents.as_bytes()).await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
        }
        if let Some(s3) = state.s3.as_ref() {
            if let Err(error_value) = s3
                .put_object()
                .bucket(&state.artifact_bucket)
                .key(format!("deployments/{id}/{path}"))
                .body(ByteStream::from(contents.into_bytes()))
                .send()
                .await
            {
                return error(
                    StatusCode::BAD_GATEWAY,
                    &format!("Artifact upload failed: {error_value}"),
                );
            }
        }
    }
    if let Err(error_value) = persist_deployment(&state, &deployment).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    state
        .deployments
        .write()
        .await
        .insert(hostname, deployment.clone());
    Json(json!({"result": deployment})).into_response()
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
    let actor = match authenticate_deployment(&state, &deployment, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if action.access == "granted" {
        return error(StatusCode::FORBIDDEN, "action-grant-required");
    }
    if action.access != "anonymous" && actor.is_none() {
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
    let db_path = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("database.sqlite");
    let _guard = database_lock(&state, &deployment.id).await;
    if action.kind == "query" {
        return match run_query(&db_path, action, params) {
            Ok(rows) => Json(json!({"result": rows})).into_response(),
            Err(error_value) => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        };
    }
    let result = match run_mutation(&db_path, action, params) {
        Ok(value) => value,
        Err(error_value) => return error(StatusCode::BAD_REQUEST, &error_value.to_string()),
    };
    Json(json!({"result": result})).into_response()
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
    let auth_path = state
        .data_root
        .join("deployments")
        .join(&deployment.id)
        .join("identity.sqlite");
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let email = input.email;
    let password = input.password;
    let result = tokio::task::spawn_blocking(move || {
        let auth = AuthService::open(auth_path)?;
        if signup {
            auth.signup(&email, &password)
        } else {
            auth.login(&email, &password)
        }
    })
    .await;
    match result {
        Ok(Ok(session)) => {
            let secure = headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                == Some("https");
            let secure = if secure { "; Secure" } else { "" };
            let cookie = format!(
                "littlebox_app_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800{}",
                session.token, secure
            );
            let mut response =
                Json(json!({"result": {"user": session.user, "expiresAt": session.expires_at}}))
                    .into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
            response
        }
        Ok(Err(error_value)) => auth_error(error_value),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Authentication request failed.",
        ),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let token = cookie_value(&headers, "littlebox_app_session");
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
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "littlebox_app_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        ),
    );
    response
}

async fn list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        == Some("private")
        && !matches!(
            authenticate_deployment(&state, &deployment, &headers),
            Ok(Some(_))
        )
    {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let path = state
        .data_root
        .join("deployments")
        .join(deployment.id)
        .join("files")
        .join("index.json");
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(files) => Json(json!({"result": files})).into_response(),
            Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid."),
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
    if deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        == Some("private")
        && !matches!(
            authenticate_deployment(&state, &deployment, &headers),
            Ok(Some(_))
        )
    {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
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
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("file")
        .replace(['/', '\\'], "-")
        .chars()
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
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
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
    let id = Uuid::new_v4().to_string();
    let metadata = json!({"id":id,"name":name,"type":content_type,"size":data.len(),"url":format!("/_littlebox/files/{id}"),"createdAt":now_ms()});
    if let Err(e) = tokio::fs::write(directory.join(format!("{id}.blob")), &data).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    files.insert(0, metadata.clone());
    if let Err(e) = tokio::fs::write(
        directory.join("index.json"),
        serde_json::to_vec_pretty(&files).unwrap_or_default(),
    )
    .await
    {
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
    if deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        == Some("private")
        && !matches!(
            authenticate_deployment(&state, &deployment, &headers),
            Ok(Some(_))
        )
    {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
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
    let bytes = match tokio::fs::read(directory.join(format!("{id}.blob"))).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::NOT_FOUND, "File not found."),
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
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
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
        Ok(None) => private_login_page(),
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
            Ok(None) => return private_login_page(),
            Err(response) => return response,
        }
    }
    match read_artifact(&state, &deployment, &path).await {
        Ok(bytes) => artifact_response(bytes, &path),
        Err(_) if path != "index.html" => {
            match read_artifact(&state, &deployment, "index.html").await {
                Ok(bytes) => artifact_response(bytes, "index.html"),
                Err(_) => error(StatusCode::NOT_FOUND, "Artifact not found."),
            }
        }
        Err(_) => error(StatusCode::NOT_FOUND, "Artifact not found."),
    }
}

async fn read_artifact(state: &AppState, deployment: &Deployment, path: &str) -> Result<Vec<u8>> {
    if !safe_relative_path(path) {
        anyhow::bail!("invalid artifact path");
    }
    let local = state.local_artifact_root.join(&deployment.id).join(path);
    match tokio::fs::read(local).await {
        Ok(bytes) => Ok(bytes),
        Err(local_error) => {
            let Some(s3) = state.s3.as_ref() else {
                return Err(local_error.into());
            };
            let key = format!("deployments/{}/{}", deployment.id, path);
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

fn private_login_page() -> Response {
    let body = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}form,label{display:grid;gap:8px}form{gap:16px;margin-top:24px}input,button{font:inherit;border-radius:8px;padding:11px 12px}input{width:100%;border:1px solid #c9ced6}button{border:0;background:#273449;color:white;cursor:pointer}button+button{background:#edf0f4;color:#252a33}#error{min-height:1.5em;margin:0;color:#a32727}</style><main><h1>Sign in to this app</h1><p>Account creation is open for this prototype.</p><form id="auth"><label>Email <input name="email" type="email" required autocomplete="email"></label><label>Password <input name="password" type="password" minlength="8" required autocomplete="current-password"></label><button formaction="/_littlebox/auth/login">Sign in</button><button formaction="/_littlebox/auth/signup">Create account</button><p id="error" role="alert"></p></form></main><script>document.querySelector('#auth').addEventListener('submit',async e=>{e.preventDefault();const path=e.submitter?.formAction||'/_littlebox/auth/login';const form=new FormData(e.currentTarget);const response=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({email:form.get('email'),password:form.get('password')})});if(response.ok){location.replace('/');return}const body=await response.json().catch(()=>({}));document.querySelector('#error').textContent=body.error||'Sign in failed.'})</script></html>"#;
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
    let _data_guard = database_lock(&state, &deployment.id).await;
    let _identity_guard = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    match portable_state(&state, &deployment).await {
        Ok(result) => Json(json!({"result": result})).into_response(),
        Err(error_value) => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
    }
}

async fn portable_state(state: &AppState, deployment: &Deployment) -> Result<Value> {
    let backup_dir = state.backup_root.join(&deployment.id);
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
        "initialDatabase": encode_base64(&database),
        "initialIdentity": encode_base64(&identity),
        "initialFiles": encode_base64(&file_bundle),
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
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
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

async fn database_lock(state: &AppState, id: &str) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = state.db_locks.lock().await;
        locks
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
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
    let rows = statement
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
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
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
        while let Some(row) = rows.next()? {
            let mut record = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                record.insert(name.clone(), sql_json(row.get_ref(index)?));
            }
            values.push(Value::Object(record));
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
    let dir = state.backup_root.join(id);
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
        backups.push(json!({"id":backup.id,"label":backup.label,"createdAt":backup.created_at,"sizeBytes":backup.size_bytes}));
    }
    backups.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
    Ok(backups)
}

async fn create_backup(state: &AppState, id: &str, label: &str) -> Result<Value> {
    let db_path = database_path(state, id);
    let dir = state.backup_root.join(id);
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
    };
    tokio::fs::write(
        dir.join(format!("{backup_id}.json")),
        serde_json::to_vec(&backup)?,
    )
    .await?;
    Ok(
        json!({"id":backup.id,"label":backup.label,"createdAt":backup.created_at,"sizeBytes":backup.size_bytes}),
    )
}

async fn restore_backup(state: &AppState, id: &str, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }
    let backup_path = state
        .backup_root
        .join(id)
        .join(format!("{backup_id}.sqlite"));
    let bytes = tokio::fs::read(&backup_path).await?;
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
    Ok(json!({"id":backup_id,"sizeBytes":bytes.len()}))
}

async fn apply_migration(state: &AppState, id: &str, body: &Value) -> Result<Value> {
    let statements = body
        .get("statements")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Migration statements are required."))?;
    if statements.is_empty() || statements.len() > 200 {
        anyhow::bail!("Migration must contain 1 to 200 statements.");
    }
    let backup = create_backup(state, id, "Before migration").await?;
    let mut db = Connection::open(database_path(state, id))?;
    let tx = db.transaction()?;
    for statement in statements {
        let sql = statement
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Migration statement must be text."))?;
        if sql.len() > 100_000 {
            anyhow::bail!("Migration statement is too large.");
        }
        tx.execute_batch(sql)?;
    }
    let integrity: String = tx.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        anyhow::bail!("Migrated database failed SQLite integrity check.");
    }
    tx.commit()?;
    Ok(json!({"backup":backup}))
}

fn run_query(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Vec<Value>> {
    let connection = Connection::open(path)?;
    let mut statement = connection.prepare(&action.statement)?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rows = statement
        .query_map(params_from_iter(params), |row| {
            let mut object = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                object.insert(name.clone(), sql_json(row.get_ref(index)?));
            }
            Ok(Value::Object(object))
        })?
        .take(1001)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 1000 {
        anyhow::bail!("Action returned more than 1000 rows.");
    }
    Ok(rows)
}

fn run_mutation(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Value> {
    let mut connection = Connection::open(path)?;
    let transaction = connection.transaction()?;
    let changes = transaction.execute(&action.statement, params_from_iter(params))?;
    let last_insert_id = transaction.last_insert_rowid();
    transaction.commit()?;
    Ok(json!({"changes": changes, "lastInsertId": last_insert_id}))
}

fn resolve_bindings(
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
                let value = object
                    .get(key)
                    .ok_or_else(|| format!("Missing action input: {key}"))?;
                if binding.value_type.as_deref() == Some("string") {
                    let text = value
                        .as_str()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| format!("{key} must be a non-empty string."))?;
                    if text.len() > binding.max_length.unwrap_or(10_000) {
                        return Err(format!("{key} is too long."));
                    }
                    Ok(SqlValue::Text(text.to_owned()))
                } else if binding.value_type.as_deref() == Some("number") {
                    value
                        .as_f64()
                        .map(SqlValue::Real)
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

fn validate_action(name: &str, action: &ActionDefinition) -> std::result::Result<(), String> {
    if !name.contains('.') || name.len() > 100 {
        return Err(format!("Invalid action name: {name}"));
    }
    if action.access != "anonymous" && action.access != "authenticated" {
        return Err(format!(
            "Action {name} uses unsupported access policy: {}.",
            action.access
        ));
    }
    if action.kind != "query" && action.kind != "mutation" {
        return Err(format!("Action {name} has an unsupported kind."));
    }
    let sql = action.statement.trim();
    if sql.matches(';').count() > 1 || (sql.contains(';') && !sql.ends_with(';')) {
        return Err(format!("Action {name} must contain one statement."));
    }
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let allowed = if action.kind == "query" {
        first == "SELECT"
    } else {
        ["INSERT", "UPDATE", "DELETE", "REPLACE"].contains(&first.as_str())
    };
    if !allowed {
        return Err(format!("Action {name} contains disallowed SQL."));
    }
    Ok(())
}

fn initialize_database(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;")?;
    Ok(())
}

fn apply_additive_schema(path: &Path, schema: &str, program: &ActionProgram) -> Result<()> {
    let mut connection = Connection::open(path)?;
    let transaction = connection.transaction()?;
    transaction.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::CreateTable { .. } | AuthAction::CreateIndex { .. } => Authorization::Allow,
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
    for (name, action) in &program.actions {
        transaction.prepare(&action.statement).map_err(|error| {
            anyhow::anyhow!("action {name} does not match the deployed schema: {error}")
        })?;
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
    drop(check);
    if integrity != "ok" {
        let _ = std::fs::remove_file(&temp);
        anyhow::bail!("Initial database failed SQLite's integrity check.");
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
        let public_file = json!({"id":id,"name":name,"type":content_type,"size":data.len(),"url":format!("/_littlebox/files/{id}"),"createdAt":created_at});
        tokio::fs::write(directory.join(format!("{id}.blob")), data).await?;
        metadata.push(public_file);
    }
    tokio::fs::write(
        directory.join("index.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )
    .await?;
    Ok(())
}

fn valid_file_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
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
    let directory = state.data_root.join("deployments").join(&deployment.id);
    let bytes = serde_json::to_vec_pretty(deployment)?;
    tokio::fs::write(directory.join("deployment.json"), &bytes).await?;
    if let Some(s3) = state.s3.as_ref() {
        s3.put_object()
            .bucket(&state.data_bucket)
            .key(format!("deployments/{}/deployment.json", deployment.id))
            .body(ByteStream::from(bytes))
            .send()
            .await?;
    }
    Ok(())
}

async fn load_deployments(
    root: &Path,
    s3: Option<&S3Client>,
    data_bucket: &str,
) -> Result<HashMap<String, Deployment>> {
    let mut result = HashMap::new();
    let mut entries = tokio::fs::read_dir(root.join("deployments")).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("deployment.json");
        if let Ok(bytes) = tokio::fs::read(path).await {
            if let Ok(deployment) = serde_json::from_slice::<Deployment>(&bytes) {
                result.insert(deployment.hostname.clone(), deployment);
            }
        }
    }
    let Some(s3) = s3 else { return Ok(result) };
    let listed = s3
        .list_objects_v2()
        .bucket(data_bucket)
        .prefix("deployments/")
        .send()
        .await?;
    for object in listed.contents() {
        let Some(key) = object.key() else { continue };
        if !key.ends_with("/deployment.json") {
            continue;
        }
        let bytes = read_s3_bytes(s3, data_bucket, key).await?;
        let deployment: Deployment = serde_json::from_slice(&bytes)?;
        let directory = root.join("deployments").join(&deployment.id);
        tokio::fs::create_dir_all(&directory).await?;
        tokio::fs::write(directory.join("deployment.json"), &bytes).await?;
        let database_path = directory.join("database.sqlite");
        if !database_path.exists() {
            restore_database(s3, data_bucket, &deployment.id, &database_path).await?;
        }
        result.insert(deployment.hostname.clone(), deployment);
    }
    Ok(result)
}

async fn restore_database(
    s3: &S3Client,
    bucket: &str,
    deployment_id: &str,
    path: &Path,
) -> Result<()> {
    let head = read_s3_bytes(s3, bucket, &format!("databases/{deployment_id}/head.json")).await?;
    let value: Value = serde_json::from_slice(&head)?;
    let generation = value
        .get("generation")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Database head has no generation."))?;
    let bytes = read_s3_bytes(
        s3,
        bucket,
        &format!("databases/{deployment_id}/generations/{generation}.sqlite"),
    )
    .await?;
    tokio::fs::write(path, bytes).await?;
    Ok(())
}

async fn read_s3_bytes(s3: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let object = s3.get_object().bucket(bucket).key(key).send().await?;
    Ok(object.body.collect().await?.into_bytes().to_vec())
}

async fn deployment_for(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))?
        .to_str()
        .ok()?
        .split(':')
        .next()?;
    state.deployments.read().await.get(host).cloned()
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
    AuthService::open(
        state
            .data_root
            .join("deployments")
            .join(&deployment.id)
            .join("identity.sqlite"),
    )
}

fn authenticate_deployment(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
) -> std::result::Result<Option<AuthUser>, Response> {
    let auth = open_auth(state, deployment)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    auth.authenticate(cookie_value(headers, "littlebox_app_session").as_deref())
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
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
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
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
        (Some(origin), Some(host)) => url_host(origin).is_some_and(|origin_host| {
            origin_host.eq_ignore_ascii_case(host.split(':').next().unwrap_or(host))
        }),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn url_host(origin: &str) -> Option<&str> {
    origin
        .split_once("://")?
        .1
        .split('/')
        .next()?
        .split(':')
        .next()
}

fn auth_error(error_value: AuthError) -> Response {
    let status = match error_value {
        AuthError::InvalidEmail | AuthError::WeakPassword => StatusCode::BAD_REQUEST,
        AuthError::EmailTaken => StatusCode::CONFLICT,
        AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::Storage(_) | AuthError::PasswordHash => StatusCode::INTERNAL_SERVER_ERROR,
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
    if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "text/html; charset=utf-8"
    }
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
            "initialFiles": "eyJmaWxlcyI6W119"
        })).unwrap();

        assert_eq!(input.initial_database.as_deref(), Some("ZGF0YQ=="));
        assert_eq!(input.initial_identity.as_deref(), Some("aWRlbnRpdHk="));
        assert_eq!(input.initial_files.as_deref(), Some("eyJmaWxlcyI6W119"));
    }

    async fn login_test_state(root: &Path, visibility: &str) -> (AppState, HeaderMap) {
        let deployment = Deployment {
            id: Uuid::new_v4().to_string(),
            project_id: Uuid::new_v4().to_string(),
            project_name: "Login test".into(),
            url: "http://login-test.localhost".into(),
            created_at: now_ms(),
            manifest: json!({"visibility": visibility}),
            hostname: "login-test.localhost".into(),
            actions: "{}".into(),
        };
        tokio::fs::create_dir_all(root.join("deployments").join(&deployment.id))
            .await
            .unwrap();
        let state = AppState {
            deployments: Arc::new(RwLock::new(HashMap::from([(
                deployment.hostname.clone(),
                deployment,
            )]))),
            db_locks: Arc::new(Mutex::new(HashMap::new())),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            local_artifact_root: root.join("artifacts"),
            backup_root: root.join("backups"),
            data_root: root.into(),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
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
                .join(format!("littlebox-login-{visibility}-{}", Uuid::new_v4()));
            let (state, headers) = login_test_state(&root, visibility).await;
            let response = login_page(State(state), headers).await;
            assert_eq!(response.status(), StatusCode::OK, "visibility={visibility}");
            let bytes = axum::body::to_bytes(response.into_body(), 16_384)
                .await
                .unwrap();
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(html.contains("Sign in to this app"));
            let _ = tokio::fs::remove_dir_all(root).await;
        }
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
}
