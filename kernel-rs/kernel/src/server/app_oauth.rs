use std::{
    collections::HashSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Form, Json, Router,
    extract::{Path as AxumPath, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{AppState, Deployment, auth::AuthUser, deployment_for, error, modules};

const STORE: &str = "mcp-app-oauth.sqlite";
const CLIENTS: &str = "mcp-app-clients.json";
const CODE_TTL: i64 = 600;
const TOKEN_TTL: i64 = 3600;
const RESOURCE_PATH: &str = "_konae/mcp";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientRegistration {
    pub client_id: String,
    pub app_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientFile {
    version: u8,
    clients: Vec<ClientRegistration>,
}

#[derive(Clone, Debug, Deserialize)]
struct AuthorizeRequest {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: String,
    scope: String,
}

#[derive(Deserialize)]
struct SignInForm {
    #[serde(flatten)]
    request: AuthorizeRequest,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct ConsentForm {
    #[serde(flatten)]
    request: AuthorizeRequest,
    decision: String,
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    client_id: String,
    code: String,
    redirect_uri: String,
    code_verifier: String,
    resource: String,
}

#[derive(Deserialize)]
struct RevokeForm {
    client_id: String,
    token: String,
}

pub struct OAuthCaller {
    pub actor: AuthUser,
    pub scopes: HashSet<String>,
    pub identity_guard: crate::NamedLockGuard,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource/{*resource}",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server/{*issuer}",
            get(server_metadata),
        )
        .route("/_konae/mcp/oauth/authorize", get(authorize).post(consent))
        .route("/_konae/mcp/oauth/signin", post(sign_in))
        .route("/_konae/mcp/oauth/token", post(token))
        .route("/_konae/mcp/oauth/revoke", post(revoke))
}

pub fn is_app_token(token: &str) -> bool {
    token.starts_with("kma_")
}

pub fn resource_url(state: &AppState, headers: &HeaderMap) -> String {
    format!(
        "{}{}/_konae/mcp",
        app_origin(state, headers),
        crate::path::prefix(headers).unwrap_or("")
    )
}

pub fn protected_resource_url(state: &AppState, headers: &HeaderMap) -> String {
    format!(
        "{}{}/.well-known/oauth-protected-resource/_konae/mcp",
        app_origin(state, headers),
        crate::path::prefix(headers).unwrap_or("")
    )
}

fn app_origin(state: &AppState, headers: &HeaderMap) -> String {
    let scheme = if state.public_origin.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

fn enabled(deployment: &Deployment) -> bool {
    modules::is_on(&deployment.modules, "mcp")
        && modules::is_on(&deployment.modules, "actions")
        && modules::is_on(&deployment.modules, "auth")
}

async fn locked_deployment(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(Deployment, crate::NamedLockGuard)> {
    let observed = deployment_for(state, headers).await?;
    let guard = crate::named_lock(state, &format!("publish:{}", observed.id), false).await;
    let current = crate::deployment_by_id(state, &observed.id).await?;
    Some((current, guard))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random(prefix: &str) -> anyhow::Result<String> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Secure random generation failed."))?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn store_path(state: &AppState) -> std::path::PathBuf {
    state.data_root.join(STORE)
}

pub(crate) fn revoke_app_tokens_at(root: &Path, app_id: &str) -> anyhow::Result<()> {
    revoke_tokens_at(root, app_id, None)
}

pub(crate) fn revoke_user_tokens_at(
    root: &Path,
    app_id: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    revoke_tokens_at(root, app_id, Some(user_id))
}

fn revoke_tokens_at(root: &Path, app_id: &str, user_id: Option<&str>) -> anyhow::Result<()> {
    let path = root.join(STORE);
    if !path.exists() {
        return Ok(());
    }
    let db = open_store(&path)?;
    match user_id {
        Some(user_id) => {
            db.execute("UPDATE app_oauth_tokens SET revoked_at=?1 WHERE app_id=?2 AND user_id=?3 AND revoked_at IS NULL", params![now(),app_id,user_id])?;
        }
        None => {
            db.execute(
                "UPDATE app_oauth_tokens SET revoked_at=?1 WHERE app_id=?2 AND revoked_at IS NULL",
                params![now(), app_id],
            )?;
        }
    }
    Ok(())
}

fn open_store(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS app_oauth_codes (
          code_hash TEXT PRIMARY KEY, app_id TEXT NOT NULL, client_id TEXT NOT NULL,
          user_id TEXT NOT NULL, redirect_uri TEXT NOT NULL, challenge TEXT NOT NULL,
          resource TEXT NOT NULL, scope TEXT NOT NULL, expires_at INTEGER NOT NULL,
          consumed_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS app_oauth_tokens (
          token_hash TEXT PRIMARY KEY, app_id TEXT NOT NULL, client_id TEXT NOT NULL,
          user_id TEXT NOT NULL, resource TEXT NOT NULL, scope TEXT NOT NULL,
          expires_at INTEGER NOT NULL, revoked_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS app_oauth_tokens_actor ON app_oauth_tokens(app_id, user_id, client_id);")?;
    Ok(db)
}

fn client(state: &AppState, app_id: &str, client_id: &str) -> Option<ClientRegistration> {
    let bytes = std::fs::read(state.data_root.join(CLIENTS)).ok()?;
    let parsed: ClientFile = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != 1 {
        return None;
    }
    parsed.clients.into_iter().find(|entry| {
        entry.app_id == app_id && entry.client_id == client_id && valid_client(&entry)
    })
}

fn valid_client(entry: &ClientRegistration) -> bool {
    !entry.client_id.is_empty()
        && entry.client_id.len() <= 100
        && entry
            .client_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        && !entry.name.trim().is_empty()
        && entry.name.len() <= 120
        && !entry.redirect_uris.is_empty()
        && entry.redirect_uris.len() <= 10
        && entry
            .redirect_uris
            .iter()
            .all(|redirect| valid_redirect(redirect))
}

fn valid_redirect(value: &str) -> bool {
    let Ok(uri) = url::Url::parse(value) else {
        return false;
    };
    uri.fragment().is_none()
        && uri.username().is_empty()
        && uri.password().is_none()
        && (uri.scheme() == "https"
            || (uri.scheme() == "http"
                && [Some("127.0.0.1"), Some("localhost"), Some("[::1]")].contains(&uri.host_str())))
}

fn scopes(scope: &str) -> Option<Vec<String>> {
    let values = scope
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty()
        || values.len() > 32
        || values.iter().any(|value| value.len() > 100)
        || values.iter().collect::<HashSet<_>>().len() != values.len()
    {
        return None;
    }
    Some(values)
}

fn valid_pkce(request: &AuthorizeRequest) -> bool {
    request.code_challenge_method == "S256"
        && request.code_challenge.len() == 43
        && request
            .code_challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn validate_request(
    state: &AppState,
    headers: &HeaderMap,
    deployment: &Deployment,
    request: &AuthorizeRequest,
) -> Result<(ClientRegistration, Vec<String>), &'static str> {
    if !enabled(deployment) {
        return Err("MCP is not enabled for this app.");
    }
    if request.response_type != "code"
        || request.state.is_empty()
        || request.state.len() > 500
        || !valid_pkce(request)
    {
        return Err("The authorization request must use response_type=code, state, and PKCE S256.");
    }
    let registration = client(state, &deployment.id, &request.client_id)
        .ok_or("Unknown or disabled app OAuth client.")?;
    if !registration.redirect_uris.contains(&request.redirect_uri) {
        return Err("The redirect URI is not registered for this client.");
    }
    if request.resource != resource_url(state, headers) {
        return Err("The resource does not match this app's MCP endpoint.");
    }
    let requested =
        scopes(&request.scope).ok_or("Request at least one published MCP tool scope.")?;
    let tools = crate::server::runtime::published_mcp_tools(deployment);
    if requested.iter().any(|scope| {
        !tools
            .iter()
            .any(|tool| tool["name"].as_str() == Some(scope))
    }) {
        return Err("A requested scope is not a published MCP tool.");
    }
    Ok((registration, requested))
}

async fn protected_resource(
    State(state): State<AppState>,
    AxumPath(resource): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if resource != RESOURCE_PATH || !enabled(&deployment) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let resource = resource_url(&state, &headers);
    Json(json!({"resource":resource,"authorization_servers":[resource],"scopes_supported":crate::server::runtime::published_mcp_tools(&deployment).iter().filter_map(|tool| tool["name"].as_str()).collect::<Vec<_>>(),"bearer_methods_supported":["header"]})).into_response()
}

async fn server_metadata(
    State(state): State<AppState>,
    AxumPath(issuer_path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if issuer_path != RESOURCE_PATH || !enabled(&deployment) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let issuer = resource_url(&state, &headers);
    Json(json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/oauth/authorize"),"token_endpoint":format!("{issuer}/oauth/token"),"revocation_endpoint":format!("{issuer}/oauth/revoke"),"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":crate::server::runtime::published_mcp_tools(&deployment).iter().filter_map(|tool| tool["name"].as_str()).collect::<Vec<_>>(),"authorization_response_iss_parameter_supported":true})).into_response()
}

async fn authorize(
    State(state): State<AppState>,
    Query(request): Query<AuthorizeRequest>,
    headers: HeaderMap,
) -> Response {
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_, requested) = match validate_request(&state, &headers, &deployment, &request) {
        Ok(value) => value,
        Err(message) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", message),
    };
    match crate::authenticate_deployment(&state, &deployment, &headers).await {
        Ok(Some(user)) => consent_page(&state, &headers, &deployment, &request, &user, &requested),
        Ok(None) => sign_in_page(&state, &headers, &request),
        Err(response) => response,
    }
}

async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SignInForm>,
) -> Response {
    if !crate::same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if validate_request(&state, &headers, &deployment, &form.request).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let host = headers
        .get(header::HOST)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("localhost"));
    let origin = app_origin(&state, &headers);
    let request = match Request::builder()
        .method("POST")
        .uri("/_konae/auth/login")
        .header(header::HOST, host)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"email":form.email,"password":form.password}).to_string(),
        )) {
        Ok(request) => request,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let response = crate::login(State(state.clone()), headers.clone(), request).await;
    if !response.status().is_success() {
        return response;
    }
    let query = authorize_query(&form.request);
    let location = format!("{}/oauth/authorize?{query}", resource_url(&state, &headers));
    let mut redirect = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    if let Some(cookie) = response.headers().get(header::SET_COOKIE) {
        redirect
            .headers_mut()
            .insert(header::SET_COOKIE, cookie.clone());
    }
    redirect
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    redirect
}

async fn consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConsentForm>,
) -> Response {
    if !crate::same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin authorization refused.");
    }
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (registration, requested) =
        match validate_request(&state, &headers, &deployment, &form.request) {
            Ok(value) => value,
            Err(message) => {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", message);
            }
        };
    let user = match crate::authenticate_deployment(&state, &deployment, &headers).await {
        Ok(Some(user)) => user,
        _ => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "login_required",
                "Sign in to this app before approving MCP access.",
            );
        }
    };
    if form.decision != "allow" {
        return redirect_error(
            &form.request,
            "access_denied",
            "The app user declined this request.",
        );
    }
    let code = match random("kmc_") {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let write_guard = crate::named_lock(&state, "app-oauth-store", true).await;
    let path = store_path(&state);
    let (app_id, client_id, user_id, redirect_uri, challenge, resource, scope) = (
        deployment.id.clone(),
        registration.client_id,
        user.user_id,
        form.request.redirect_uri.clone(),
        form.request.code_challenge.clone(),
        form.request.resource.clone(),
        requested.join(" "),
    );
    let hash_code = hash(&code);
    let inserted = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let _write_guard = write_guard;
        let db = open_store(&path)?;
        db.execute("INSERT INTO app_oauth_codes(code_hash,app_id,client_id,user_id,redirect_uri,challenge,resource,scope,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![hash_code,app_id,client_id,user_id,redirect_uri,challenge,resource,scope,now()+CODE_TTL])?;
        Ok(())
    }).await;
    if !matches!(inserted, Ok(Ok(()))) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    redirect_code(&form.request, &code)
}

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The app resource was not found.",
        );
    };
    let _release = crate::named_lock(&state, &format!("publish:{}", deployment.id), false).await;
    let Some(deployment) = crate::deployment_by_id(&state, &deployment.id).await else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The app release is unavailable.",
        );
    };
    if form.grant_type != "authorization_code" || !enabled(&deployment) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "This app accepts authorization_code grants while MCP is enabled.",
        );
    }
    let Some(registration) = client(&state, &deployment.id, &form.client_id) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client",
            "Unknown app OAuth client.",
        );
    };
    if !registration.redirect_uris.contains(&form.redirect_uri)
        || form.resource != resource_url(&state, &headers)
        || !valid_verifier(&form.code_verifier)
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The code, redirect URI, resource or PKCE verifier is invalid.",
        );
    }
    let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(form.code_verifier.as_bytes()));
    let access = match random("kma_") {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let _identity = crate::named_lock(&state, &format!("identity:{}", deployment.id), false).await;
    let store_guard = crate::named_lock(&state, "app-oauth-store", true).await;
    let path = store_path(&state);
    let identity_path = crate::layout::identity(&state.data_root, &deployment.id);
    let code_hash = hash(&form.code);
    let current_tools = crate::server::runtime::published_mcp_tools(&deployment)
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect::<HashSet<_>>();
    let (app_id, client_id, redirect_uri, resource) = (
        deployment.id.clone(),
        form.client_id.clone(),
        form.redirect_uri.clone(),
        form.resource.clone(),
    );
    let access_hash = hash(&access);
    let verification = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(String, String)>> {
        let _store_guard = store_guard;
        let mut db = open_store(&path)?;
        let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let code: Option<(String,String,String,String,String,String,i64,Option<i64>)> = transaction.query_row("SELECT app_id,client_id,user_id,redirect_uri,challenge,resource,expires_at,consumed_at FROM app_oauth_codes WHERE code_hash=?1", [&code_hash], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
        let Some((code_app,code_client,user_id,code_redirect,challenge,code_resource,expires,consumed)) = code else { return Ok(None); };
        if code_app != app_id || code_client != client_id || code_redirect != redirect_uri || code_resource != resource || expires <= now() || consumed.is_some()
            || challenge != expected { return Ok(None); }
        let scopes = scopes(&transaction.query_row("SELECT scope FROM app_oauth_codes WHERE code_hash=?1", [&code_hash], |row| row.get::<_,String>(0))?).ok_or_else(|| anyhow::anyhow!("Invalid scope."))?;
        if scopes.iter().any(|scope| !current_tools.contains(scope)) || crate::auth::active_user(&identity_path, &user_id)?.is_none() { return Ok(None); }
        transaction.execute("UPDATE app_oauth_codes SET consumed_at=?1 WHERE code_hash=?2 AND consumed_at IS NULL", params![now(),code_hash])?;
        transaction.execute("INSERT INTO app_oauth_tokens(token_hash,app_id,client_id,user_id,resource,scope,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![access_hash,app_id,client_id,user_id,resource,scopes.join(" "),now()+TOKEN_TTL])?;
        transaction.commit()?;
        Ok(Some((user_id, scopes.join(" "))))
    }).await;
    let (_user_id, scope) = match verification {
        Ok(Ok(Some(value))) => value,
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The authorization code is invalid, expired or already used.",
            );
        }
    };
    Json(json!({"access_token":access,"token_type":"Bearer","expires_in":TOKEN_TTL,"scope":scope}))
        .into_response()
}

async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RevokeForm>,
) -> Response {
    let Some((deployment, _release)) = locked_deployment(&state, &headers).await else {
        return StatusCode::OK.into_response();
    };
    if client(&state, &deployment.id, &form.client_id).is_none() {
        return StatusCode::OK.into_response();
    }
    let guard = crate::named_lock(&state, "app-oauth-store", true).await;
    let path = store_path(&state);
    let token_hash = hash(&form.token);
    let app_id = deployment.id;
    let client_id = form.client_id;
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        if let Ok(db) = open_store(&path) { let _ = db.execute("UPDATE app_oauth_tokens SET revoked_at=?1 WHERE token_hash=?2 AND app_id=?3 AND client_id=?4 AND revoked_at IS NULL", params![now(),token_hash,app_id,client_id]); }
    }).await;
    StatusCode::OK.into_response()
}

pub async fn authenticate(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
    full_catalog: &[Value],
) -> Result<Option<OAuthCaller>, ()> {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Ok(None);
    };
    if !is_app_token(token) {
        return Ok(None);
    }
    if !enabled(deployment) {
        return Err(());
    }
    let resource = resource_url(state, headers);
    let identity_guard =
        crate::named_lock(state, &format!("identity:{}", deployment.id), false).await;
    let store_guard = crate::named_lock(state, "app-oauth-store", false).await;
    let path = store_path(state);
    let token_hash = hash(token);
    let app_id = deployment.id.clone();
    let db_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(String,String,String)>> {
        let _store_guard = store_guard;
        let db = open_store(&path)?;
        db.query_row("SELECT client_id,user_id,scope FROM app_oauth_tokens WHERE token_hash=?1 AND app_id=?2 AND resource=?3 AND expires_at>?4 AND revoked_at IS NULL", params![token_hash,app_id,resource,now()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(Into::into)
    }).await.map_err(|_| ())?.map_err(|_| ())?;
    let Some((client_id, user_id, scope)) = db_result else {
        return Err(());
    };
    if client(state, &deployment.id, &client_id).is_none() {
        return Err(());
    }
    let granted = scopes(&scope)
        .ok_or(())?
        .into_iter()
        .collect::<HashSet<_>>();
    if granted.is_empty()
        || granted.iter().any(|name| {
            !full_catalog
                .iter()
                .any(|tool| tool["name"].as_str() == Some(name))
        })
    {
        return Err(());
    }
    let identity_path = crate::layout::identity(&state.data_root, &deployment.id);
    let user_id_for_query = user_id.clone();
    let actor = tokio::task::spawn_blocking(move || {
        crate::auth::active_user(&identity_path, &user_id_for_query)
    })
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?
    .ok_or(())?;
    Ok(Some(OAuthCaller {
        actor,
        scopes: granted,
        identity_guard,
    }))
}

fn authorize_query(request: &AuthorizeRequest) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("response_type", &request.response_type)
        .append_pair("client_id", &request.client_id)
        .append_pair("redirect_uri", &request.redirect_uri)
        .append_pair("state", &request.state)
        .append_pair("code_challenge", &request.code_challenge)
        .append_pair("code_challenge_method", &request.code_challenge_method)
        .append_pair("resource", &request.resource)
        .append_pair("scope", &request.scope);
    query.finish()
}

fn hidden_request(request: &AuthorizeRequest) -> String {
    [
        ("response_type", &request.response_type),
        ("client_id", &request.client_id),
        ("redirect_uri", &request.redirect_uri),
        ("state", &request.state),
        ("code_challenge", &request.code_challenge),
        ("code_challenge_method", &request.code_challenge_method),
        ("resource", &request.resource),
        ("scope", &request.scope),
    ]
    .iter()
    .map(|(key, value)| {
        format!(
            "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
            key,
            escape(value)
        )
    })
    .collect::<String>()
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(body: String) -> Response {
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn sign_in_page(state: &AppState, headers: &HeaderMap, request: &AuthorizeRequest) -> Response {
    let action = format!("{}/oauth/signin", resource_url(state, headers));
    page(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>Sign in to {}</title><main><h1>Sign in to {}</h1><p>Sign in with your app account before granting access to {}.</p><form method=post action=\"{}\">{}<label>Email <input type=email name=email required autocomplete=username></label><label>Password <input type=password name=password required autocomplete=current-password></label><button>Sign in</button></form></main>",
        escape(&request.client_id),
        escape(&request.client_id),
        escape(&request.scope),
        escape(&action),
        hidden_request(request)
    ))
}

fn consent_page(
    state: &AppState,
    headers: &HeaderMap,
    deployment: &Deployment,
    request: &AuthorizeRequest,
    user: &AuthUser,
    requested: &[String],
) -> Response {
    let tools = crate::server::runtime::published_mcp_tools(deployment);
    let items = requested
        .iter()
        .map(|name| {
            let tool = tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
                .unwrap();
            format!(
                "<li><strong>{}</strong><p>{}</p></li>",
                escape(name),
                escape(tool["description"].as_str().unwrap_or_default())
            )
        })
        .collect::<String>();
    let action = format!("{}/oauth/authorize", resource_url(state, headers));
    page(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>MCP access</title><main><h1>Allow {} to read app data?</h1><p>Signed in as {}. The client requests these read-only tools:</p><ul>{}</ul><form method=post action=\"{}\">{}<button name=decision value=allow>Allow access</button><button name=decision value=deny>Deny</button></form></main>",
        escape(&request.client_id),
        escape(&user.name),
        items,
        escape(&action),
        hidden_request(request)
    ))
}

fn valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
}

fn redirect_code(request: &AuthorizeRequest, code: &str) -> Response {
    let mut uri = url::Url::parse(&request.redirect_uri).unwrap();
    uri.query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", &request.state)
        .append_pair("iss", &request.resource);
    let mut response =
        (StatusCode::SEE_OTHER, [(header::LOCATION, uri.to_string())]).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn redirect_error(request: &AuthorizeRequest, code: &str, description: &str) -> Response {
    let mut uri = match url::Url::parse(&request.redirect_uri) {
        Ok(uri) => uri,
        Err(_) => return oauth_error(StatusCode::BAD_REQUEST, code, description),
    };
    uri.query_pairs_mut()
        .append_pair("error", code)
        .append_pair("error_description", description)
        .append_pair("state", &request.state);
    (StatusCode::SEE_OTHER, [(header::LOCATION, uri.to_string())]).into_response()
}

fn oauth_error(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (
        status,
        Json(json!({"error":code,"error_description":message})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

#[cfg(test)]
fn store_signout(path: &Path) -> anyhow::Result<()> {
    let db = open_store(path)?;
    db.execute_batch("DELETE FROM app_oauth_codes; DELETE FROM app_oauth_tokens;")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_requires_exact_safe_redirect_uris() {
        let valid = ClientRegistration {
            client_id: "notes-client".into(),
            app_id: "notes".into(),
            name: "Notes client".into(),
            redirect_uris: vec!["http://127.0.0.1:8765/callback".into()],
        };
        assert!(valid_client(&valid));
        for redirect in [
            "http://example.com/callback",
            "javascript:alert(1)",
            "http://127.0.0.1/cb#fragment",
        ] {
            let mut invalid = valid.clone();
            invalid.redirect_uris = vec![redirect.into()];
            assert!(!valid_client(&invalid));
        }
    }

    #[test]
    fn resource_metadata_uses_the_request_host_and_path_mount() {
        let root =
            std::env::temp_dir().join(format!("app-oauth-resource-{}", uuid::Uuid::new_v4()));
        let mut state = AppState::for_test(&root);
        state.public_origin = "https://apps.example.com".into();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "notes.example.net".parse().unwrap());
        headers.insert(crate::path::PREFIX_HEADER, "/p/notes".parse().unwrap());
        assert_eq!(
            resource_url(&state, &headers),
            "https://notes.example.net/p/notes/_konae/mcp"
        );
        assert_eq!(
            protected_resource_url(&state, &headers),
            "https://notes.example.net/p/notes/.well-known/oauth-protected-resource/_konae/mcp"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn app_oauth_store_keeps_only_hashed_codes_and_tokens_and_can_revoke_them() {
        let root = std::env::temp_dir().join(format!("app-oauth-store-{}", uuid::Uuid::new_v4()));
        let path = root.join(STORE);
        let code = "kmc_secret-code";
        let token = "kma_secret-token";
        let db = open_store(&path).unwrap();
        db.execute("INSERT INTO app_oauth_codes(code_hash,app_id,client_id,user_id,redirect_uri,challenge,resource,scope,expires_at) VALUES(?1,'app','client','user','http://127.0.0.1/cb','challenge','http://app/_konae/mcp','notes.mine',?2)", params![hash(code),now()+600]).unwrap();
        db.execute("INSERT INTO app_oauth_tokens(token_hash,app_id,client_id,user_id,resource,scope,expires_at) VALUES(?1,'app','client','user','http://app/_konae/mcp','notes.mine',?2)", params![hash(token),now()+600]).unwrap();
        db.execute("INSERT INTO app_oauth_tokens(token_hash,app_id,client_id,user_id,resource,scope,expires_at) VALUES(?1,'app','client','other-user','http://app/_konae/mcp','notes.mine',?2)", params![hash("kma_other-user"),now()+600]).unwrap();
        let other_app_lookup: Option<String> = db
            .query_row(
                "SELECT user_id FROM app_oauth_tokens WHERE token_hash=?1 AND app_id='other-app'",
                [hash(token)],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(other_app_lookup, None);
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(code.len())
                .any(|window| window == code.as_bytes())
        );
        assert!(
            !bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes())
        );
        revoke_user_tokens_at(&root, "app", "user").unwrap();
        let db = open_store(&path).unwrap();
        let revoked_at: Option<i64> = db
            .query_row(
                "SELECT revoked_at FROM app_oauth_tokens WHERE token_hash=?1",
                [hash(token)],
                |row| row.get(0),
            )
            .unwrap();
        assert!(revoked_at.is_some());
        drop(db);
        revoke_app_tokens_at(&root, "app").unwrap();
        let db = open_store(&path).unwrap();
        let app_revoked_at: Option<i64> = db
            .query_row(
                "SELECT revoked_at FROM app_oauth_tokens WHERE token_hash=?1",
                [hash("kma_other-user")],
                |row| row.get(0),
            )
            .unwrap();
        assert!(app_revoked_at.is_some());
        drop(db);
        store_signout(&path).unwrap();
        let db = open_store(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM app_oauth_tokens", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
