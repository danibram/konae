//! The HTTP module: Server Actions that answer on a path of their Deployment
//! host, the owner-set secrets that sign them, API keys, and the file read
//! path `GET /_littlebox/files/<id>`.
//!
//! An `http` key in `actions.json` binds one action to one method and path.
//! The action keeps its access level, bindings, query bound and rate limit.
//! Only `anonymous` actions may bind a route, and the route answers whatever
//! the Deployment's visibility (ADR-0011, update of 2026-11-06). A route has
//! three kinds of caller, resolved before any SQL runs:
//!
//! | Caller            | Proof                                  | Actor ID       |
//! | ----------------- | -------------------------------------- | -------------- |
//! | Third-party sender| the binding's signature                | none           |
//! | Owner's program   | `Authorization: Bearer lbk_...`        | the key's ID   |
//! | Signed-in person  | the app session cookie, same origin    | the user's ID  |
//!
//! A binding that declares a signature requires a valid one unless the caller
//! presents an API key that names the action. docs/mcp.md, "HTTP routes",
//! states the signature scheme a sender computes.

use super::*;
use std::sync::{Arc, OnceLock};

/// The largest POST body a route reads.
pub(super) const MAX_BODY_BYTES: usize = 64 * 1024;
/// How far a signature's timestamp may sit from the Kernel's clock, both ways.
pub(super) const SIGNATURE_WINDOW_SECONDS: u64 = 300;
/// Live keys a Deployment may hold at once.
const MAX_LIVE_KEYS: i64 = 50;
const KEY_PREFIX: &str = "lbk_";

/// The file read path. main.rs routes `/{*path}` to `get_route` and
/// `post_route` below.
pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/_littlebox/files/{id}", get(read_file))
}

// ---------------------------------------------------------------------------
// Declarations. The same checks, order and messages as `parseHttpBinding` in
// src/kernel/actions.ts; kernel-rs/src/http-binding-cases.json holds the cases.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HttpBinding {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) signature: Option<SignatureBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SignatureBinding {
    pub(super) header: String,
    pub(super) secret: String,
}

const RESERVED_SEGMENTS: [&str; 4] = ["_littlebox", "oauth", ".well-known", "mcp"];
const UNSIGNABLE_HEADERS: [&str; 5] = ["authorization", "cookie", "host", "content-type", "content-length"];

/// Parses and checks one action's `http` key. `Ok(None)` when it has none.
pub(super) fn binding(name: &str, action: &ActionDefinition) -> Result<Option<HttpBinding>, String> {
    let Some(value) = action.http.as_ref() else { return Ok(None) };
    let invalid = || format!("Action {name} has an invalid http binding.");
    let record = value.as_object().ok_or_else(invalid)?;
    if record.keys().any(|key| !["method", "path", "signature"].contains(&key.as_str())) {
        return Err(invalid());
    }
    let path = record.get("path").and_then(Value::as_str).ok_or_else(invalid)?;
    let method = record.get("method").and_then(Value::as_str);
    let method = match (method, action.kind.as_str()) {
        (Some("GET"), "query") => "GET",
        (Some("POST"), "mutation") => "POST",
        _ => return Err(format!("Action {name} must bind GET for a query and POST for a mutation.")),
    };
    if action.access != "anonymous" {
        return Err(format!(
            "Action {name} is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route."
        ));
    }
    if !valid_path(path) {
        return Err(format!(
            r#"Action {name} has an invalid http path. Use a leading "/" and segments of letters, digits, ".", "_", "~" or "-", up to 200 characters."#
        ));
    }
    let first = path[1..].split('/').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&first.as_str()) {
        return Err(format!(
            "Action {name} binds the reserved path {path}. Paths under /_littlebox, /oauth, /.well-known and /mcp belong to the Kernel."
        ));
    }
    let signature = match record.get("signature") {
        None => None,
        Some(value) => {
            let signature = value.as_object().ok_or_else(invalid)?;
            if signature.keys().any(|key| key != "header" && key != "secret") {
                return Err(invalid());
            }
            let header = signature.get("header").and_then(Value::as_str).unwrap_or("");
            if header.is_empty()
                || header.len() > 64
                || !header.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || UNSIGNABLE_HEADERS.contains(&header.to_ascii_lowercase().as_str())
            {
                return Err(format!("Action {name} has an invalid signature header."));
            }
            let secret = signature.get("secret").and_then(Value::as_str).unwrap_or("");
            if !valid_secret_name(secret) {
                return Err(format!(
                    r#"Action {name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter."#
                ));
            }
            Some(SignatureBinding { header: header.to_owned(), secret: secret.to_owned() })
        }
    };
    Ok(Some(HttpBinding { method: method.to_owned(), path: path.to_owned(), signature }))
}

fn valid_path(path: &str) -> bool {
    path.len() <= 200
        && path.starts_with('/')
        && path[1..].split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        })
}

fn valid_secret_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Refuses two actions bound to the same method and path. Names are visited
/// in sorted order, as the editor does, so both name the same pair.
pub(super) fn validate_routes(program: &ActionProgram) -> Result<(), String> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    let mut seen: HashMap<String, &str> = HashMap::new();
    for name in names {
        let Some(http) = binding(name, &program.actions[name])? else { continue };
        let route = format!("{} {}", http.method, http.path);
        if let Some(earlier) = seen.get(&route) {
            return Err(format!("Actions {earlier} and {name} both bind {route}."));
        }
        seen.insert(route, name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Secrets. They belong to the Project, in mcp.sqlite next to the project
// registry, because a first publish must find them before its Deployment
// exists. Values never leave the Kernel: not in a listing, an export or a log.

fn secrets_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_secrets(
           project TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, updated_at INTEGER NOT NULL,
           PRIMARY KEY(project, name))",
    )
}

fn secret_value(db: &Connection, project: &str, name: &str) -> Result<Option<String>> {
    secrets_table(db)?;
    Ok(db
        .query_row("SELECT value FROM project_secrets WHERE project=?1 AND name=?2", params_from_iter([project, name]), |row| row.get(0))
        .optional()?)
}

/// Refuses a release whose signed bindings name a secret the project does not have.
pub(super) fn require_secrets(db: &Connection, project: &str, program: &ActionProgram) -> Result<()> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Ok(Some(HttpBinding { signature: Some(signature), .. })) = binding(name, &program.actions[name]) else {
            continue;
        };
        let secret = &signature.secret;
        anyhow::ensure!(
            secret_value(db, project, secret)?.is_some(),
            "Action {name} signs with the secret {secret}, which is not set for this project. Set it with secret_set before publishing."
        );
    }
    Ok(())
}

/// `secrets_list`, `secret_set` and `secret_delete`. The caller has checked
/// that the owner may act on `project` and holds the project lock.
pub(super) fn secrets_call(db: &Connection, project: &str, deployment: Option<&Deployment>, name: &str, a: &Value) -> Result<Value> {
    secrets_table(db)?;
    match name {
        "secrets_list" => {
            let mut query = db.prepare("SELECT name, updated_at FROM project_secrets WHERE project=?1 ORDER BY name")?;
            let secrets = query
                .query_map([project], |row| Ok(json!({"name": row.get::<_, String>(0)?, "updatedAt": row.get::<_, i64>(1)?})))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"projectId": project, "secrets": secrets}))
        }
        "secret_set" => {
            let secret = a["name"].as_str().unwrap_or("");
            anyhow::ensure!(valid_secret_name(secret), "A secret name uses lowercase letters, digits and \"_\", starting with a letter, up to 64 characters.");
            let value = a["value"].as_str().unwrap_or("");
            anyhow::ensure!(
                (16..=512).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
                "A secret value is 16 to 512 printable ASCII characters without spaces."
            );
            db.execute(
                "INSERT INTO project_secrets(project, name, value, updated_at) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(project, name) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                rusqlite::params![project, secret, value, now_ms() as i64],
            )?;
            Ok(json!({"projectId": project, "name": secret, "updatedAt": now_ms()}))
        }
        "secret_delete" => {
            let secret = a["name"].as_str().unwrap_or("");
            if let Some(deployment) = deployment {
                let program: ActionProgram = serde_json::from_str(&deployment.actions)?;
                let mut users = program
                    .actions
                    .iter()
                    .filter(|(action, definition)| {
                        matches!(binding(action, definition), Ok(Some(HttpBinding { signature: Some(ref s), .. })) if s.secret == secret)
                    })
                    .map(|(action, _)| action.as_str())
                    .collect::<Vec<_>>();
                users.sort();
                anyhow::ensure!(
                    users.is_empty(),
                    "The published action {} signs with {secret}. Publish without that signature before deleting the secret.",
                    users.join(", ")
                );
            }
            let removed = db.execute("DELETE FROM project_secrets WHERE project=?1 AND name=?2", params_from_iter([project, secret]))?;
            Ok(json!({"projectId": project, "name": secret, "deleted": removed > 0}))
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

// ---------------------------------------------------------------------------
// API keys. A key is a Runtime Actor of kind `service` on one Deployment. Its
// row lives in the Deployment's identity.sqlite, so an export carries it with
// the accounts; the import revokes every key, as it deletes sessions.

#[derive(Clone, Debug)]
pub(super) struct ApiKey {
    pub(super) id: String,
    pub(super) actions: Vec<String>,
    pub(super) files: Vec<String>,
}

fn keys_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS api_keys(
           id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
           actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER)",
    )
}

fn identity_connection(state: &AppState, deployment: &Deployment) -> Result<Connection> {
    // Opening the store installs the account tables an import or a fresh
    // Deployment may not have yet.
    open_auth(state, deployment).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))?;
    let db = Connection::open(deployment_dir(state, &deployment.id).join("identity.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    keys_table(&db)?;
    Ok(db)
}

fn key_hash(token: &str) -> String {
    super::mcp::digest(token.as_bytes())
}

/// The key a bearer token names, if it is a live key of this Deployment.
fn find_key(state: &AppState, deployment: &Deployment, token: &str) -> Result<Option<ApiKey>> {
    let db = identity_connection(state, deployment)?;
    let row: Option<(String, String, String)> = db
        .query_row(
            "SELECT id, actions, files FROM api_keys WHERE token_hash=?1 AND revoked_at IS NULL",
            [key_hash(token)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((id, actions, files)) => Some(ApiKey {
            id,
            actions: serde_json::from_str(&actions)?,
            files: serde_json::from_str(&files)?,
        }),
        None => None,
    })
}

/// Marks every key of an imported identity database revoked. Exports written
/// before keys existed have no table, and nothing to revoke.
pub(super) fn revoke_imported_keys(db: &Connection) -> Result<()> {
    if table_exists(db, "api_keys")? {
        db.execute("UPDATE api_keys SET revoked_at=?1 WHERE revoked_at IS NULL", [now_ms() as i64])?;
    }
    Ok(())
}

fn key_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let parse = |text: String| serde_json::from_str::<Value>(&text).unwrap_or(json!([]));
    Ok(json!({
        "keyId": row.get::<_, String>(0)?,
        "name": row.get::<_, String>(1)?,
        "actions": parse(row.get(2)?),
        "files": parse(row.get(3)?),
        "createdAt": row.get::<_, i64>(4)?,
        "revokedAt": row.get::<_, Option<i64>>(5)?,
    }))
}

/// `api_keys_create`, `api_keys_list` and `api_keys_revoke`. The caller has
/// checked that the owner may act on `deployment`.
pub(super) async fn api_keys_call(state: &AppState, deployment: &Deployment, name: &str, a: &Value) -> Result<Value> {
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let db = identity_connection(state, deployment)?;
    match name {
        "api_keys_list" => {
            let mut query = db.prepare(
                "SELECT id, name, actions, files, created_at, revoked_at FROM api_keys ORDER BY created_at DESC, id",
            )?;
            let keys = query.query_map([], key_view)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"deploymentId": deployment.id, "keys": keys}))
        }
        "api_keys_create" => {
            let label = a["name"].as_str().unwrap_or("").trim();
            anyhow::ensure!((1..=80).contains(&label.chars().count()), "A key name is 1 to 80 characters.");
            let actions: Vec<String> = serde_json::from_value(a.get("actions").cloned().unwrap_or(json!([])))?;
            let mut files: Vec<String> = serde_json::from_value(a.get("files").cloned().unwrap_or(json!([])))?;
            files.sort();
            files.dedup();
            anyhow::ensure!(
                files.iter().all(|visibility| visibility == "public" || visibility == "private"),
                "A key's files are \"public\", \"private\" or both."
            );
            anyhow::ensure!(!actions.is_empty() || !files.is_empty(), "Name at least one action or file visibility for the key.");
            let program: ActionProgram = serde_json::from_str(&deployment.actions)?;
            for action in &actions {
                let definition = program
                    .actions
                    .get(action)
                    .ok_or_else(|| anyhow::anyhow!("Action {action} is not declared on this Deployment."))?;
                anyhow::ensure!(
                    matches!(binding(action, definition), Ok(Some(_))),
                    "Action {action} has no HTTP route, so a key cannot call it."
                );
            }
            let live: i64 = db.query_row("SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL", [], |row| row.get(0))?;
            anyhow::ensure!(live < MAX_LIVE_KEYS, "This Deployment already has {MAX_LIVE_KEYS} live keys. Revoke one first.");
            let id = format!("key_{}", &Uuid::new_v4().simple().to_string()[..16]);
            let token = format!("{KEY_PREFIX}{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            let created = now_ms() as i64;
            db.execute(
                "INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, key_hash(&token), label, serde_json::to_string(&actions)?, serde_json::to_string(&files)?, created],
            )?;
            Ok(json!({"keyId": id, "key": token, "name": label, "actions": actions, "files": files, "createdAt": created,
                "note": "The key appears only in this answer. Send it as Authorization: Bearer <key>."}))
        }
        "api_keys_revoke" => {
            let id = a["keyId"].as_str().unwrap_or("");
            let changed = db.execute(
                "UPDATE api_keys SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
                rusqlite::params![now_ms() as i64, id],
            )?;
            anyhow::ensure!(
                changed == 1
                    || db.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id=?1)", [id], |row| row.get::<_, bool>(0))?,
                "Key not found."
            );
            Ok(json!({"keyId": id, "revoked": true}))
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

// ---------------------------------------------------------------------------
// Callers.

enum Caller {
    Key(ApiKey),
    Person(AuthUser),
    Visitor,
}

impl Caller {
    fn actor_id(&self) -> Option<&str> {
        match self {
            Caller::Key(key) => Some(&key.id),
            Caller::Person(user) => Some(&user.user_id),
            Caller::Visitor => None,
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

/// A session cookie counts only on a request from the app's own origin.
/// Every app host is same-site with the others, so `SameSite=Lax` alone would
/// let one app's page call another app's route as its signed-in user.
fn own_origin(headers: &HeaderMap) -> bool {
    let site = headers.get("sec-fetch-site").and_then(|value| value.to_str().ok());
    same_origin(headers) && !matches!(site, Some("same-site" | "cross-site"))
}

/// Resolves the caller, or the answer that refuses it. A bearer token with
/// the key prefix is only ever looked up as a key. A client whose unknown or
/// revoked keys used up its bucket is refused before the lookup, as a client
/// that sent too many wrong passwords is.
fn resolve_caller(state: &AppState, deployment: &Deployment, headers: &HeaderMap, client: std::net::IpAddr) -> Result<Caller, Response> {
    if let Some(token) = bearer(headers).filter(|token| token.starts_with(KEY_PREFIX)) {
        if let Err(wait) = state.guards.key_attempt(&deployment.id, client) {
            return Err(rate_limited(wait));
        }
        return match find_key(state, deployment, token) {
            Ok(Some(key)) => Ok(Caller::Key(key)),
            Ok(None) => {
                state.guards.key_failed(&deployment.id, client);
                Err(refusal(StatusCode::UNAUTHORIZED, "invalid-api-key"))
            }
            Err(error_value) => Err(error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string())),
        };
    }
    if !own_origin(headers) {
        return Ok(Caller::Visitor);
    }
    match authenticate_deployment(state, deployment, headers) {
        Ok(Some(user)) => Ok(Caller::Person(user)),
        Ok(None) => Ok(Caller::Visitor),
        Err(response) => Err(response),
    }
}

fn rate_limited(wait: std::time::Duration) -> Response {
    let mut response = refusal(StatusCode::TOO_MANY_REQUESTS, "rate-limited");
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
    response
}

fn refusal(status: StatusCode, message: &str) -> Response {
    let mut response = error(status, message);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}

// ---------------------------------------------------------------------------
// Signatures. The header carries `t=<unix seconds>,v1=<hex HMAC-SHA256>`, the
// MAC is over `<t>.<raw body>` with the secret as key, and `t` must be within
// SIGNATURE_WINDOW_SECONDS of the Kernel's clock. A verified signature is
// remembered for twice the window in the Deployment's identity.sqlite, so the
// same request is refused if replayed, across Kernel restarts too.
//
// identity.sqlite rather than usage.sqlite: Usage is written by one background
// thread that drops entries when its queue is full, and a replay check must
// be synchronous and must not drop. identity.sqlite already holds the
// Deployment's other caller proofs, sessions and API keys, and the key lookup
// opens it on the same request. The table travels in an export with the
// accounts, so the destination also refuses a request replayed from the origin.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SignatureCheck {
    Valid,
    Invalid,
    Replayed,
}

#[cfg(test)]
pub(super) fn sign(secret: &str, timestamp: u64, body: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let mut context = ring::hmac::Context::with_key(&key);
    context.update(format!("{timestamp}.").as_bytes());
    context.update(body);
    hex::encode(context.sign().as_ref())
}

fn signatures_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS route_signatures(
           path TEXT NOT NULL, tag TEXT NOT NULL, seen_at INTEGER NOT NULL, PRIMARY KEY(path, tag));
         CREATE INDEX IF NOT EXISTS route_signatures_seen ON route_signatures(seen_at);",
    )
}

/// Checks a signature for the route at `path` and records it in `db`, the
/// Deployment's identity store. Entries older than twice the window are
/// pruned on the way: a `t` that old is refused before the lookup.
pub(super) fn verify_signature(
    db: &Connection,
    path: &str,
    secret: &str,
    header_value: Option<&str>,
    body: &[u8],
    now: u64,
) -> Result<SignatureCheck> {
    let Some(value) = header_value else { return Ok(SignatureCheck::Invalid) };
    let mut timestamp = None;
    let mut tags = Vec::new();
    for part in value.split(',') {
        match part.trim().split_once('=') {
            Some(("t", digits)) if !digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|b| b.is_ascii_digit()) => {
                timestamp = digits.parse::<u64>().ok();
            }
            Some(("v1", tag)) => {
                if let Ok(bytes) = hex::decode(tag) {
                    tags.push(bytes);
                }
            }
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else { return Ok(SignatureCheck::Invalid) };
    if timestamp.abs_diff(now) > SIGNATURE_WINDOW_SECONDS {
        return Ok(SignatureCheck::Invalid);
    }
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let message = [format!("{timestamp}.").as_bytes(), body].concat();
    let Some(tag) = tags.iter().find(|tag| ring::hmac::verify(&key, &message, tag).is_ok()) else {
        return Ok(SignatureCheck::Invalid);
    };
    signatures_table(db)?;
    let cutoff = now.saturating_sub(2 * SIGNATURE_WINDOW_SECONDS) as i64;
    db.execute("DELETE FROM route_signatures WHERE seen_at < ?1", [cutoff])?;
    let inserted = db.execute(
        "INSERT INTO route_signatures(path, tag, seen_at) VALUES(?1, ?2, ?3) ON CONFLICT(path, tag) DO NOTHING",
        rusqlite::params![path, hex::encode(tag), now as i64],
    )?;
    Ok(if inserted == 1 { SignatureCheck::Valid } else { SignatureCheck::Replayed })
}

// ---------------------------------------------------------------------------
// Routes.

fn peer(request: &Request) -> Option<std::net::SocketAddr> {
    request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0)
}

/// A Deployment's routes, parsed once from its `actions.json`.
pub(super) struct RouteTable {
    program: ActionProgram,
    /// `"<method> <path>"` to the action's name and binding.
    routes: HashMap<String, (String, HttpBinding)>,
}

impl RouteTable {
    fn parse(actions: &str) -> Self {
        let program: ActionProgram =
            serde_json::from_str(actions).unwrap_or(ActionProgram { version: 1, actions: HashMap::new() });
        let routes = program
            .actions
            .iter()
            .filter_map(|(name, action)| match binding(name, action) {
                Ok(Some(http)) => Some((format!("{} {}", http.method, http.path), (name.clone(), http))),
                _ => None,
            })
            .collect();
        Self { program, routes }
    }

    fn find(&self, method: &str, path: &str) -> Option<&(String, HttpBinding)> {
        self.routes.get(&format!("{method} {path}"))
    }

    /// The `Allow` header for a path. A GET route does not answer HEAD.
    fn allowed(&self, path: &str) -> &'static str {
        if self.find("GET", path).is_some() { "GET" } else { "GET, HEAD" }
    }
}

/// The parsed table of one Deployment and the revision it was parsed from.
/// Every publish writes a fresh `artifact_version`, so a table built from an
/// earlier release never answers for a later one.
struct CachedRoutes {
    revision: Option<String>,
    table: Arc<RouteTable>,
    parses: u64,
}

fn route_cache() -> &'static std::sync::Mutex<HashMap<String, CachedRoutes>> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, CachedRoutes>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cache_table(deployment: &Deployment) -> Arc<RouteTable> {
    let table = Arc::new(RouteTable::parse(&deployment.actions));
    let mut cache = route_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache.entry(deployment.id.clone()).or_insert_with(|| CachedRoutes {
        revision: None,
        table: table.clone(),
        parses: 0,
    });
    entry.revision = deployment.artifact_version.clone();
    entry.table = table.clone();
    entry.parses += 1;
    table
}

/// The routes of the Deployment's current release. Only the first request
/// after a Kernel start parses `actions.json`; a publish parses it in
/// `routes_published`.
fn route_table(deployment: &Deployment) -> Arc<RouteTable> {
    {
        let cache = route_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.get(&deployment.id).filter(|cached| cached.revision == deployment.artifact_version) {
            return cached.table.clone();
        }
    }
    cache_table(deployment)
}

/// Replaces the cached routes with the release `deploy` just installed.
/// `release_apply` reaches `deploy`, so every publish passes here.
pub(super) fn routes_published(deployment: &Deployment) {
    cache_table(deployment);
}

#[cfg(test)]
fn route_parses(deployment_id: &str) -> u64 {
    route_cache().lock().unwrap().get(deployment_id).map_or(0, |cached| cached.parses)
}

pub(super) async fn get_route(State(state): State<AppState>, AxumPath(relative): AxumPath<String>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if let Some(deployment) = deployment_for(&state, request.headers()).await {
        let table = route_table(&deployment);
        if let Some((name, http)) = table.find("GET", &path) {
            // HEAD would run the query only to drop its rows, and its headers
            // depend on the answer, so a route refuses it before any check.
            if request.method() == axum::http::Method::HEAD {
                let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "A route does not answer HEAD. Send GET.");
                response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET"));
                return response;
            }
            let input = match query_input(&table.program, name, request.uri().query().unwrap_or("")) {
                Ok(input) => input,
                Err(message) => return refusal(StatusCode::BAD_REQUEST, &message),
            };
            let client = state.guards.client_ip(peer(&request), request.headers());
            return run_route(&state, &deployment, &table, name, http, request.headers(), client, input, b"").await;
        }
    }
    // Every other GET keeps serving the Build Artifact.
    serve_artifact(State(state), AxumPath(relative), request.headers().clone()).await
}

pub(super) async fn post_route(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let not_allowed = |allow: &'static str| {
        let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "No route accepts POST on this path.");
        response.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
        response
    };
    let Some(deployment) = deployment_for(&state, request.headers()).await else { return not_allowed("GET, HEAD") };
    let table = route_table(&deployment);
    let Some((name, http)) = table.find("POST", &path) else { return not_allowed(table.allowed(&path)) };
    let client = state.guards.client_ip(peer(&request), request.headers());
    let headers = request.headers().clone();
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return refusal(StatusCode::PAYLOAD_TOO_LARGE, "The request body exceeds 64 KiB."),
    };
    run_route(&state, &deployment, &table, name, http, &headers, client, Value::Null, &body).await
}

/// Turns a query string into action input, typed by the action's bindings, so
/// `?since=5` binds the number 5. A value that does not parse as its type is
/// passed on as text and refused by the usual binding check.
fn query_input(program: &ActionProgram, name: &str, query: &str) -> Result<Value, String> {
    let action = &program.actions[name];
    let mut input = serde_json::Map::new();
    for (key, text) in url::form_urlencoded::parse(query.as_bytes()) {
        let key = key.into_owned();
        if input.contains_key(&key) {
            return Err(format!("Repeated query parameter: {key}"));
        }
        let declared = action
            .bindings
            .iter()
            .find(|binding| binding.source == "input" && binding.key.as_deref() == Some(key.as_str()))
            .and_then(|binding| binding.value_type.as_deref());
        let value = match declared {
            Some("number") => text
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| text.parse::<f64>().ok().and_then(|n| serde_json::Number::from_f64(n).map(Value::Number)))
                .unwrap_or_else(|| Value::String(text.into_owned())),
            Some("boolean") => match text.as_ref() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::String(text.into_owned()),
            },
            _ => Value::String(text.into_owned()),
        };
        input.insert(key, value);
    }
    Ok(Value::Object(input))
}

#[allow(clippy::too_many_arguments)]
async fn run_route(
    state: &AppState,
    deployment: &Deployment,
    table: &RouteTable,
    name: &str,
    http: &HttpBinding,
    headers: &HeaderMap,
    client: std::net::IpAddr,
    query: Value,
    body: &[u8],
) -> Response {
    let caller = match resolve_caller(state, deployment, headers, client) {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    if let Caller::Key(key) = &caller {
        if !key.actions.iter().any(|action| action == name) {
            return refusal(StatusCode::FORBIDDEN, &format!("This API key may not call {name}."));
        }
    }
    // Rate limits match the runtime endpoint: one bucket per client address
    // for callers with no account, one per user or key otherwise.
    let limited = match &caller {
        Caller::Visitor => state.guards.anonymous_actions.check(&format!("{}|{}", deployment.id, limits::client_key(client))),
        Caller::Person(user) => state.guards.signed_in_actions.check(&format!("runtime:{}:{}", deployment.id, user.user_id)),
        Caller::Key(key) => state.guards.signed_in_actions.check(&format!("key:{}:{}", deployment.id, key.id)),
    };
    if let Err(wait) = limited {
        return rate_limited(wait);
    }
    if let (Some(signature), false) = (&http.signature, matches!(caller, Caller::Key(_))) {
        let secret = match super::mcp::store(state).and_then(|db| secret_value(&db, &deployment.project_id, &signature.secret)) {
            Ok(Some(secret)) => secret,
            Ok(None) => return refusal(StatusCode::SERVICE_UNAVAILABLE, "The route's signing secret is not set."),
            Err(error_value) => return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
        };
        let header_value = headers.get(signature.header.as_str()).and_then(|value| value.to_str().ok());
        let checked = identity_connection(state, deployment)
            .and_then(|db| verify_signature(&db, &http.path, &secret, header_value, body, now_ms() / 1000));
        match checked {
            Ok(SignatureCheck::Valid) => {}
            Ok(SignatureCheck::Invalid) => return refusal(StatusCode::UNAUTHORIZED, "invalid-signature"),
            Ok(SignatureCheck::Replayed) => return refusal(StatusCode::CONFLICT, "replayed-signature"),
            Err(error_value) => return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
        }
    }
    let input = if http.method == "POST" {
        if body.iter().all(u8::is_ascii_whitespace) {
            json!({})
        } else {
            match serde_json::from_slice::<Value>(body) {
                Ok(value) => value,
                Err(_) => return refusal(StatusCode::BAD_REQUEST, "The request body must be a JSON object."),
            }
        }
    } else {
        query
    };
    let action = &table.program.actions[name];
    let params = match resolve_bindings(action, &input, caller.actor_id()) {
        Ok(params) => params,
        Err(message) => return refusal(StatusCode::BAD_REQUEST, &message),
    };
    let db_path = database_path(state, &deployment.id);
    let _guard = database_lock(state, &deployment.id).await;
    let result = if action.kind == "query" {
        run_query(&db_path, action, params).map(Value::from)
    } else {
        run_mutation(&db_path, action, params)
    };
    match result {
        Ok(value) => {
            // A route counts under its method and path, not again as an action.
            super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Route(http.method.clone(), http.path.clone()));
            super::usage::signal(&state.data_root, &deployment.id, name, action, &input);
            let mut response = Json(json!({"result": value})).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error_value) => refusal(StatusCode::BAD_REQUEST, &error_value.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The file read path. A key reads the visibilities it names; a person reads
// what `file_readable` allows; a visitor reads public files of a public
// Deployment. A file another caller may not read answers 404, as a missing one.

async fn read_file(State(state): State<AppState>, AxumPath(id): AxumPath<String>, request: Request) -> Response {
    let client = state.guards.client_ip(peer(&request), request.headers());
    let headers = request.headers().clone();
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let caller = match resolve_caller(&state, &deployment, &headers, client) {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let mut response = match caller {
        Caller::Key(key) => read_file_as_key(&state, &deployment, &key, &id).await,
        Caller::Person(_) => serve_file(State(state.clone()), AxumPath(id), headers).await,
        // A cookie from another origin must not reach the file, so the
        // visitor's request goes on without it.
        Caller::Visitor => {
            let mut visitor = headers.clone();
            visitor.remove(header::COOKIE);
            visitor.remove(header::AUTHORIZATION);
            serve_file(State(state.clone()), AxumPath(id), visitor).await
        }
    };
    if response.status().is_success() {
        super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::FileRead);
    }
    // Only the app's own pages may embed the file; programs are unaffected.
    response.headers_mut().insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

async fn read_file_as_key(state: &AppState, deployment: &Deployment, key: &ApiKey, id: &str) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    if !valid_file_id(id) {
        return not_found();
    }
    let directory = deployment_dir(state, &deployment.id).join("files");
    let index = match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes).unwrap_or_default(),
        Err(_) => return not_found(),
    };
    let Some(metadata) = index.iter().find(|file| file["id"].as_str() == Some(id)) else { return not_found() };
    if !key.files.iter().any(|visibility| visibility == file_visibility(metadata)) {
        return not_found();
    }
    match tokio::fs::read(directory.join(format!("{id}.blob"))).await {
        Ok(bytes) => file_response(metadata, bytes),
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(actions: &Value) -> ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    /// The editor runs the same file in src/kernel/actions.test.ts.
    #[test]
    fn bindings_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("http-binding-cases.json")).unwrap();
        assert!(cases.len() > 15);
        for case in cases {
            let program = program(&case["actions"]);
            let mut names = program.actions.keys().collect::<Vec<_>>();
            names.sort();
            let outcome = names
                .into_iter()
                .try_for_each(|name| validate_action(name, &program.actions[name]))
                .and_then(|_| validate_routes(&program));
            match case["error"].as_str() {
                None => assert_eq!(outcome, Ok(()), "{}", case["case"]),
                Some(message) => assert_eq!(outcome, Err(message.to_owned()), "{}", case["case"]),
            }
        }
    }

    #[test]
    fn a_signature_verifies_once_inside_its_window_and_the_store_remembers_it() {
        let root = std::env::temp_dir().join(format!("littlebox-signatures-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("identity.sqlite");
        let open = || Connection::open(&file).unwrap();
        let body = br#"{"id":"A-1","total":12}"#;
        let now = 1_800_000_000;
        let header = format!("t={now},v1={}", sign("orders-secret-0123", now, body));
        let verify = |db: &Connection, path: &str, secret: &str, header: Option<&str>, body: &[u8], at: u64| {
            verify_signature(db, path, secret, header, body, at).unwrap()
        };
        assert_eq!(verify(&open(), "/hooks/orders", "orders-secret-0123", Some(&header), body, now + 10), SignatureCheck::Valid);
        // A new connection stands in for a restarted Kernel.
        assert_eq!(verify(&open(), "/hooks/orders", "orders-secret-0123", Some(&header), body, now + 20), SignatureCheck::Replayed);
        assert_eq!(verify(&open(), "/hooks/other", "orders-secret-0123", Some(&header), body, now + 20), SignatureCheck::Valid, "each path keeps its own");
        let db = open();
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some(&header), body, now + 301), SignatureCheck::Invalid, "stale");
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some(&header), body, now - 301), SignatureCheck::Invalid, "future");
        assert_eq!(verify(&db, "/hooks/fresh", "another-secret-0123", Some(&header), body, now), SignatureCheck::Invalid, "wrong secret");
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some(&header), br#"{"id":"A-1","total":99}"#, now), SignatureCheck::Invalid, "tampered body");
        let moved = format!("t={},v1={}", now + 1, sign("orders-secret-0123", now, body));
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some(&moved), body, now), SignatureCheck::Invalid, "timestamp is signed");
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", None, body, now), SignatureCheck::Invalid, "missing");
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some("v1=00"), body, now), SignatureCheck::Invalid, "no timestamp");
        assert_eq!(verify(&db, "/hooks/fresh", "orders-secret-0123", Some(&header), body, now), SignatureCheck::Valid);
        let stored = |db: &Connection| db.query_row("SELECT COUNT(*) FROM route_signatures", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(stored(&db), 3);
        // A verification past twice the window prunes what came before it.
        let later = now + 2 * SIGNATURE_WINDOW_SECONDS + 30;
        let fresh = format!("t={later},v1={}", sign("orders-secret-0123", later, body));
        assert_eq!(verify(&db, "/hooks/orders", "orders-secret-0123", Some(&fresh), body, later), SignatureCheck::Valid);
        assert_eq!(stored(&db), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    fn test_state(root: &std::path::Path) -> AppState {
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(super::super::copies::CopyStatus::default()),
            admin_key: "test-admin".into(),
            accounts: super::super::accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        }
    }

    /// Publishes a public app whose one GET route is `path`, with a static
    /// `app.js` next to it.
    async fn publish_route(state: &AppState, path: &str) -> Deployment {
        let actions = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous",
            "statement": "SELECT 1 AS n LIMIT 1", "bindings": [], "http": {"method": "GET", "path": path}}}});
        let body = json!({
            "project": {"id": "route_cache", "name": "Route cache", "slug": "route-cache", "manifest": {},
                "files": {"actions.json": actions.to_string(), "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>", "app.js": "app"}}
        });
        let request = Request::builder().header("x-littlebox-admin-key", "test-admin").body(Body::from(body.to_string())).unwrap();
        assert_eq!(deploy(State(state.clone()), request).await.status(), StatusCode::OK);
        state.deployments.read().await.values().next().cloned().unwrap()
    }

    async fn get(state: &AppState, deployment: &Deployment, path: &str) -> (StatusCode, String) {
        let request = Request::builder().uri(path).header(header::HOST, &deployment.hostname).body(Body::empty()).unwrap();
        let response = get_route(State(state.clone()), AxumPath(path.trim_start_matches('/').to_owned()), request).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn a_static_request_does_not_parse_actions_and_a_publish_replaces_the_routes() {
        let root = std::env::temp_dir().join(format!("littlebox-route-cache-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
        let state = test_state(&root);
        let first = publish_route(&state, "/api/count").await;
        assert_eq!(route_parses(&first.id), 1, "the publish parses the actions once");
        for _ in 0..3 {
            assert_eq!(get(&state, &first, "/app.js").await, (StatusCode::OK, "app".into()));
        }
        assert_eq!(get(&state, &first, "/api/count").await, (StatusCode::OK, r#"{"result":[{"n":1}]}"#.into()));
        assert_eq!(route_parses(&first.id), 1, "static files and the route reuse the parsed table");
        // HEAD on the route refuses before SQL. Running a query opens, and so
        // creates, the database file; after a HEAD it is still missing.
        let head = Request::builder().method(axum::http::Method::HEAD).uri("/api/count").header(header::HOST, &first.hostname).body(Body::empty()).unwrap();
        tokio::fs::remove_file(database_path(&state, &first.id)).await.unwrap();
        let refused = get_route(State(state.clone()), AxumPath("api/count".into()), head).await;
        assert_eq!(refused.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(refused.headers()[header::ALLOW], "GET");
        assert!(!database_path(&state, &first.id).exists(), "HEAD opened no database");
        assert_eq!(get(&state, &first, "/api/count").await.0, StatusCode::OK);
        assert!(database_path(&state, &first.id).exists(), "GET ran the query");

        let second = publish_route(&state, "/api/total").await;
        assert_eq!(second.id, first.id);
        assert_ne!(second.artifact_version, first.artifact_version);
        assert_eq!(route_parses(&second.id), 2, "a publish parses the new release");
        assert_eq!(get(&state, &second, "/api/total").await.0, StatusCode::OK);
        assert_eq!(get(&state, &second, "/api/count").await, (StatusCode::OK, "<p>shell</p>".into()), "the old route is gone");
        assert_eq!(route_parses(&second.id), 2);

        // A table cached for another revision is not used: a Kernel that
        // loads the record without a publish parses on the first request.
        let mut restarted = second.clone();
        restarted.artifact_version = Some("v-restarted".into());
        state.deployments.write().await.insert(restarted.hostname.clone(), restarted.clone());
        assert_eq!(get(&state, &restarted, "/api/total").await.0, StatusCode::OK);
        assert_eq!(route_parses(&second.id), 3);
        assert_eq!(get(&state, &restarted, "/api/total").await.0, StatusCode::OK);
        assert_eq!(route_parses(&second.id), 3);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn an_unset_secret_refuses_the_release() {
        let db = Connection::open_in_memory().unwrap();
        let signed = program(&json!({"orders.receive": {"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO orders (external_id) VALUES (?1)", "bindings": [{"source": "input", "key": "id", "type": "string"}],
            "http": {"method": "POST", "path": "/hooks/orders", "signature": {"header": "X-Signature", "secret": "orders_webhook"}}}}));
        assert_eq!(
            require_secrets(&db, "project_a", &signed).unwrap_err().to_string(),
            "Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing."
        );
        secrets_call(&db, "project_a", None, "secret_set", &json!({"name": "orders_webhook", "value": "a-long-enough-secret"})).unwrap();
        require_secrets(&db, "project_a", &signed).unwrap();
        assert!(require_secrets(&db, "project_b", &signed).is_err(), "secrets belong to one project");
        let listed = secrets_call(&db, "project_a", None, "secrets_list", &json!({})).unwrap();
        assert!(!listed.to_string().contains("a-long-enough-secret"), "a listing never shows a value");
        assert!(secrets_call(&db, "project_a", None, "secret_set", &json!({"name": "orders_webhook", "value": "short"})).is_err());
    }
}
