use crate::refusal::{Refusal, refuse};
use crate::*;
use sha2::Digest;
use std::sync::{Arc, OnceLock};

/// The largest POST body a route reads.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// How far a signature's timestamp may sit from the Kernel's clock, both ways.
pub const SIGNATURE_WINDOW_SECONDS: u64 = 300;
const MAX_SIGNATURE_TAGS: usize = 2;
const SIGNATURE_TAG_BYTES: usize = 32;
const MAX_LIVE_KEYS: i64 = 50;
const KEY_PREFIX: &str = "lbk_";

/// The file read path. main.rs routes `/{*path}` to `get_route` and
/// `post_route` below.
pub fn routes() -> Router<AppState> {
    Router::new().route("/_konae/files/{id}", get(read_file))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpBinding {
    pub method: String,
    pub path: String,
    pub signature: Option<SignatureBinding>,
    /// The declared callers, each of "signature", "key" and "session".
    pub callers: Option<Vec<String>>,
    /// Input keys and the JSON path each reads from a POST body.
    pub fields: Option<Vec<(String, Vec<crate::provider::Step>)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureBinding {
    /// The header the MAC arrives in; `svix-signature` for the svix scheme.
    pub header: String,
    pub secret: String,
    pub scheme: crate::provider::Scheme,
}

const RESERVED_SEGMENTS: [&str; 4] = ["_konae", "oauth", ".well-known", "mcp"];
const ROUTE_CALLERS: [&str; 3] = ["signature", "key", "session"];

/// Parses and checks one action's `http` key. `Ok(None)` when it has none.
pub fn binding(name: &str, action: &ActionDefinition) -> Result<Option<HttpBinding>, Refusal> {
    let Some(value) = action.http.as_ref() else {
        return Ok(None);
    };
    if action.kind == "signal" {
        return Err(refuse(
            "signal.route",
            format!(
                "Action {name} is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route."
            ),
        ));
    }
    let invalid = || {
        refuse(
            "route.invalid",
            format!("Action {name} has an invalid http binding."),
        )
    };
    let record = value.as_object().ok_or_else(invalid)?;
    if record
        .keys()
        .any(|key| !["method", "path", "signature", "callers", "fields"].contains(&key.as_str()))
    {
        return Err(invalid());
    }
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let method = record.get("method").and_then(Value::as_str);
    let method = match (method, action.kind.as_str()) {
        (Some("GET"), "query") => "GET",
        (Some("POST"), "mutation") => "POST",
        _ => {
            return Err(refuse(
                "route.method-mismatch",
                format!("Action {name} must bind GET for a query and POST for a mutation."),
            ));
        }
    };
    if action.access != "anonymous" {
        return Err(refuse(
            "route.authenticated",
            format!(
                "Action {name} is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route."
            ),
        ));
    }
    if !valid_path(path) {
        return Err(refuse(
            "route.path-invalid",
            format!(
                r#"Action {name} has an invalid http path. Use a leading "/" and segments of letters, digits, ".", "_", "~" or "-", up to 200 characters."#
            ),
        ));
    }
    let first = path[1..]
        .split('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&first.as_str()) {
        return Err(refuse(
            "route.path-reserved",
            format!(
                "Action {name} binds the reserved path {path}. Paths under /_konae, /oauth, /.well-known and /mcp belong to the Kernel."
            ),
        ));
    }
    let signature = match record.get("signature") {
        None => None,
        Some(value) => {
            let (header, secret, scheme) = crate::provider::signature(name, method, value)?;
            Some(SignatureBinding {
                header,
                secret,
                scheme,
            })
        }
    };
    let callers = match record.get("callers") {
        None => None,
        Some(value) => {
            let invalid_callers = || {
                refuse(
                    "route.callers-invalid",
                    format!(
                        r#"Action {name} has an invalid callers list. List one or more of "signature", "key" and "session", each once."#
                    ),
                )
            };
            let items = value
                .as_array()
                .filter(|items| !items.is_empty())
                .ok_or_else(invalid_callers)?;
            let mut callers: Vec<String> = Vec::new();
            for item in items {
                let caller = item
                    .as_str()
                    .filter(|caller| ROUTE_CALLERS.contains(caller))
                    .ok_or_else(invalid_callers)?;
                if callers.iter().any(|seen| seen == caller) {
                    return Err(invalid_callers());
                }
                callers.push(caller.to_owned());
            }
            let signs = callers.iter().any(|caller| caller == "signature");
            if signs && signature.is_none() {
                return Err(refuse(
                    "route.callers-no-signature",
                    format!(
                        r#"Action {name} lists "signature" among its callers but declares no signature."#
                    ),
                ));
            }
            if !signs && signature.is_some() {
                return Err(refuse(
                    "route.callers-without-signature",
                    format!(
                        r#"Action {name} declares a signature, so its callers must include "signature"."#
                    ),
                ));
            }
            Some(callers)
        }
    };
    let fields = match record.get("fields") {
        None => None,
        Some(value) => Some(crate::provider::fields(name, method, action, value)?),
    };
    Ok(Some(HttpBinding {
        method: method.to_owned(),
        path: path.to_owned(),
        signature,
        callers,
        fields,
    }))
}

/// The callers a route accepts on a Deployment, or `None` when it answers
/// anyone. Without a `callers` list, a signed route takes the signature or a
/// key, an unsigned route on a private Deployment a key or a session, and an
/// unsigned route on a public Deployment anyone. `routeCallers` in
/// src/kernel/actions.ts is the same rule.
pub fn route_callers(http: &HttpBinding, private: bool) -> Option<Vec<String>> {
    if let Some(callers) = &http.callers {
        return Some(callers.clone());
    }
    if http.signature.is_some() {
        return Some(vec!["signature".into(), "key".into()]);
    }
    private.then(|| vec!["key".into(), "session".into()])
}

/// The route an action answers on when that is its only door: its route does
/// not answer anyone, so `actions_invoke` refuses it (ADR-0013).
/// `None` when the action has no route or its route is open.
pub fn route_only(name: &str, action: &ActionDefinition, private: bool) -> Option<String> {
    let http = binding(name, action).ok().flatten()?;
    route_callers(&http, private).map(|_| format!("{} {}", http.method, http.path))
}

fn valid_path(path: &str) -> bool {
    path.len() <= 200
        && path.starts_with('/')
        && path[1..].split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        })
}

fn valid_secret_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Refuses two actions bound to the same method and path. Names are visited
/// in sorted order, as the editor does, so both name the same pair.
pub fn validate_routes(program: &ActionProgram) -> Result<(), Refusal> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    let mut seen: HashMap<String, &str> = HashMap::new();
    for name in names {
        let Some(http) = binding(name, &program.actions[name])? else {
            continue;
        };
        let route = format!("{} {}", http.method, http.path);
        if let Some(earlier) = seen.get(&route) {
            return Err(refuse(
                "route.duplicate",
                format!("Actions {earlier} and {name} both bind {route}."),
            ));
        }
        seen.insert(route, name);
    }
    Ok(())
}

const SECRETS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS secrets(name TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL, use TEXT NOT NULL)";

/// Reads the app's secrets. A project without secrets has an empty store in memory.
pub fn read_secrets(root: &Path, project: &str) -> Result<Connection> {
    if let Some(db) = crate::app_db::read_optional(&crate::layout::secrets(root, project))? {
        return Ok(db);
    }
    let db = Connection::open_in_memory()?;
    db.execute_batch(SECRETS_SCHEMA)?;
    Ok(db)
}

/// Opens the app's secrets for writing.
pub fn secrets_store(root: &Path, project: &str) -> Result<Connection> {
    let path = crate::layout::secrets(root, project);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(SECRETS_SCHEMA)?;
    Ok(db)
}

/// A secret's value, for a call that signs with it or sends the key it holds.
pub fn project_secret(db: &Connection, name: &str) -> Result<Option<String>> {
    secret_value(db, name)
}

fn secret_for_calls_only(db: &Connection, name: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT use = 'call' FROM secrets WHERE name=?1",
            [name],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

fn secret_value(db: &Connection, name: &str) -> Result<Option<String>> {
    stored_secret(db, name, false)
}

fn stored_secret(db: &Connection, name: &str, google: bool) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT value FROM secrets WHERE name=?1 AND (use = 'google') = ?2",
            rusqlite::params![name, google],
            |row| row.get(0),
        )
        .optional()?)
}

/// Whether the app holds `value` as a route secret. Konae asks this of an
/// owner's other apps before it sets a route secret.
pub fn holds_route_secret(db: &Connection, value: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM secrets WHERE value = ?1 AND use = 'route')",
        [value],
        |row| row.get(0),
    )?)
}

/// Whether `secret_set` with these arguments sets a route secret, the only
/// kind whose value one owner may not reuse across apps.
pub fn sets_route_secret(a: &Value) -> bool {
    !crate::google::is_client_name(a["name"].as_str().unwrap_or(""))
        && matches!(a.get("use").and_then(Value::as_str), None | Some("route"))
}

/// The app's Google OAuth client ID and secret, each when the owner set it.
pub fn google_client(db: &Connection) -> Result<crate::google::StoredClient> {
    Ok(crate::google::StoredClient {
        id: stored_secret(db, crate::google::CLIENT_ID, true)?,
        secret: stored_secret(db, crate::google::CLIENT_SECRET, true)?,
    })
}

/// Refuses a release whose signed bindings name a secret the project does not have.
pub fn require_secrets(db: &Connection, program: &ActionProgram) -> Result<()> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    let mut actions = Vec::new();
    let mut secrets: Vec<String> = Vec::new();
    for name in names {
        let Ok(Some(HttpBinding {
            signature: Some(signature),
            ..
        })) = binding(name, &program.actions[name])
        else {
            continue;
        };
        if secret_for_calls_only(db, &signature.secret)? {
            let secret = &signature.secret;
            anyhow::bail!(refuse(
                "route.signature-secret-call-only",
                format!(
                    "Action {name} signs its route with the secret {secret}, which is set for calls only. Set a route secret with a value of its own."
                ),
            ));
        }
        match secret_value(db, &signature.secret)? {
            None => {
                actions.push(name.clone());
                if !secrets.contains(&signature.secret) {
                    secrets.push(signature.secret.clone());
                }
            }
            Some(value)
                if signature.scheme == crate::provider::Scheme::Svix
                    && crate::provider::svix_key(&value).is_none() =>
            {
                let secret = &signature.secret;
                anyhow::bail!(refuse(
                    "route.signature-svix-secret-invalid",
                    format!(
                        "Action {name} verifies svix signatures with the secret {secret}, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_."
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    unset_secrets_message(&actions, &secrets).map_or(Ok(()), |message| {
        Err(anyhow::anyhow!(refuse(
            "route.signature-secret-unset",
            message
        )))
    })
}

fn unset_secrets_message(actions: &[String], secrets: &[String]) -> Option<String> {
    let list = |items: &[String]| match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let (names, secret_names) = (list(actions), list(secrets));
    match (actions, secrets) {
        ([], _) => None,
        ([action], [secret]) => Some(format!(
            "Action {action} signs with the secret {secret}, which is not set for this project. Set it with secret_set before publishing."
        )),
        (_, [secret]) => Some(format!(
            "Actions {names} sign with the secret {secret}, which is not set for this project. Set it with secret_set before publishing."
        )),
        _ => Some(format!(
            "Actions {names} sign with the secrets {secret_names}, which are not set for this project. Set each with secret_set before publishing."
        )),
    }
}

pub const VALUE_INVALID: &str =
    "A secret value is 16 to 512 printable ASCII characters without spaces.";
pub const SECRET_REUSED: &str = "Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.";

/// `secrets_list`, `secret_set` and `secret_delete` on the app's secrets.
/// The caller has checked that the owner may act on the app, that a route
/// secret's value is not one of the owner's other apps' (`SECRET_REUSED`),
/// and holds the project lock.
pub fn secrets_call(
    db: &Connection,
    project: &str,
    deployment: Option<&Deployment>,
    name: &str,
    a: &Value,
) -> Result<Value> {
    match name {
        "secrets_list" => {
            let mut query =
                db.prepare("SELECT name, updated_at, use, value FROM secrets ORDER BY name")?;
            let secrets = query
                .query_map([], |row| {
                    Ok(json!({
                        "name": row.get::<_, String>(0)?,
                        "updatedAt": row.get::<_, i64>(1)?,
                        "use": row.get::<_, String>(2)?,
                        "svixKey": crate::provider::svix_key(&row.get::<_, String>(3)?).is_some(),
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"projectId": project, "secrets": secrets}))
        }
        "secret_set" => {
            let secret = a["name"].as_str().unwrap_or("");
            anyhow::ensure!(
                valid_secret_name(secret),
                "A secret name uses lowercase letters, digits and \"_\", starting with a letter, up to 64 characters."
            );
            let value = a["value"].as_str().unwrap_or("");
            if !((16..=512).contains(&value.len())
                && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)))
            {
                return Err(refuse("secret.value-invalid", VALUE_INVALID).into());
            }
            let google_name = crate::google::is_client_name(secret);
            let usage = match (a.get("use").and_then(Value::as_str), google_name) {
                (None | Some("google"), true) => "google",
                (Some(_), true) => {
                    return Err(refuse(
                        "secret.google-names-reserved",
                        crate::google::NAMES_RESERVED,
                    )
                    .into());
                }
                (Some("google"), false) => {
                    return Err(
                        refuse("secret.google-names-only", crate::google::NAMES_ONLY).into(),
                    );
                }
                (None | Some("route"), false) => "route",
                (Some("call"), false) => "call",
                (Some(_), false) => {
                    anyhow::bail!(r#"A secret's use is "route", "call" or "google"."#)
                }
            };
            if secret == crate::google::CLIENT_ID && value.starts_with("GOCSPX-") {
                return Err(refuse(
                    "secret.google-id-looks-like-secret",
                    crate::google::ID_LOOKS_LIKE_SECRET,
                )
                .into());
            }
            db.execute(
                "INSERT INTO secrets(name, value, updated_at, use) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(name) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at, use=excluded.use",
                rusqlite::params![secret, value, now_ms() as i64, usage],
            )?;
            Ok(json!({"projectId": project, "name": secret, "updatedAt": now_ms(), "use": usage}))
        }
        "secret_delete" => {
            let secret = a["name"].as_str().unwrap_or("");
            if let Some(deployment) = deployment {
                if crate::google::is_client_name(secret)
                    && deployment
                        .sign_in_methods
                        .contains(&crate::SignInMethod::Google)
                {
                    return Err(
                        refuse("secret.google-still-on", crate::google::still_on(secret)).into(),
                    );
                }
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
                let callers: Vec<String> = crate::calls::secrets(&program)
                    .into_iter()
                    .filter(|(_, used)| used == secret)
                    .map(|(action, _)| action)
                    .collect();
                anyhow::ensure!(
                    callers.is_empty(),
                    "The published call {} uses {secret}. Publish without that auth before deleting the secret.",
                    callers.join(", ")
                );
            }
            let removed = db.execute("DELETE FROM secrets WHERE name=?1", [secret])?;
            Ok(json!({"projectId": project, "name": secret, "deleted": removed > 0}))
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

#[derive(Clone, Debug)]
pub struct ApiKey {
    pub id: String,
    pub actions: Vec<String>,
    pub files: Vec<String>,
}

/// Why an import revoked a key, in `api_keys.revoked_reason`. A key the
/// owner revoked has no reason.
pub const REVOKED_AT_IMPORT: &str = "import";

pub(crate) const API_KEY_COLUMNS: [&str; 8] = [
    "id",
    "token_hash",
    "name",
    "actions",
    "files",
    "created_at",
    "revoked_at",
    "revoked_reason",
];

pub(crate) fn keys_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS api_keys(
           id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
           actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER,
           revoked_reason TEXT)",
    )?;
    add_revoked_reason(db)
}

/// Accounts files written before keys carried a reason lack the column.
pub fn add_revoked_reason(db: &Connection) -> rusqlite::Result<()> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('api_keys') WHERE lower(name)='revoked_reason')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        db.execute_batch("ALTER TABLE api_keys ADD COLUMN revoked_reason TEXT")?;
    }
    Ok(())
}

fn identity_connection(state: &AppState, deployment: &Deployment) -> Result<Connection> {
    open_auth(state, deployment).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))?;
    let db = Connection::open(crate::layout::identity(&state.data_root, &deployment.id))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    keys_table(&db)?;
    Ok(db)
}

fn key_hash(token: &str) -> String {
    crate::digest(token.as_bytes())
}

fn find_key(path: &Path, token: &str) -> Result<Option<ApiKey>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let db = crate::app_db::identity_reader(path)?;
    if !table_exists(&db, "api_keys")? {
        return Ok(None);
    }
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

/// Marks every live key of an accounts file that is leaving or arriving as
/// revoked at import. The rows stay, with their names, actions, files and
/// hashes, so the destination's owner sees which programs had access and
/// which keys to reissue; none of them opens anything. Keys the owner
/// revoked keep their time and have no reason. Exports written before keys
/// existed have no table, and nothing to revoke. Returns the keys revoked.
pub fn revoke_imported_keys(db: &Connection) -> Result<usize> {
    if !table_exists(db, "api_keys")? {
        return Ok(0);
    }
    add_revoked_reason(db)?;
    Ok(db.execute(
        "UPDATE api_keys SET revoked_at=?1, revoked_reason=?2 WHERE revoked_at IS NULL",
        rusqlite::params![now_ms() as i64, REVOKED_AT_IMPORT],
    )?)
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
        "revokedReason": row.get::<_, Option<String>>(6)?,
    }))
}

/// `api_keys_create`, `api_keys_list` and `api_keys_revoke`. The caller has
/// checked that the owner may act on `deployment`.
pub async fn api_keys_call(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    a: &Value,
) -> Result<Value> {
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    if name == "api_keys_list"
        && !crate::layout::identity(&state.data_root, &deployment.id).try_exists()?
    {
        return Ok(json!({"deploymentId": deployment.id, "keys": []}));
    }
    let db = identity_connection(state, deployment)?;
    match name {
        "api_keys_list" => {
            let mut query = db.prepare(
                "SELECT id, name, actions, files, created_at, revoked_at, revoked_reason FROM api_keys ORDER BY created_at DESC, id",
            )?;
            let keys = query
                .query_map([], key_view)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"deploymentId": deployment.id, "keys": keys}))
        }
        "api_keys_create" => {
            let label = a["name"].as_str().unwrap_or("").trim();
            anyhow::ensure!(
                (1..=80).contains(&label.chars().count()),
                "A key name is 1 to 80 characters."
            );
            let actions: Vec<String> =
                serde_json::from_value(a.get("actions").cloned().unwrap_or(json!([])))?;
            let mut files: Vec<String> =
                serde_json::from_value(a.get("files").cloned().unwrap_or(json!([])))?;
            files.sort();
            files.dedup();
            anyhow::ensure!(
                files
                    .iter()
                    .all(|visibility| visibility == "public" || visibility == "private"),
                "A key's files are \"public\", \"private\" or both."
            );
            anyhow::ensure!(
                !actions.is_empty() || !files.is_empty(),
                "Name at least one action or file visibility for the key."
            );
            require_api_key_modules(&deployment.modules, &actions, &files)?;
            let program: ActionProgram = serde_json::from_str(&deployment.actions)?;
            for action in &actions {
                let definition = program.actions.get(action).ok_or_else(|| {
                    anyhow::anyhow!("Action {action} is not declared on this Deployment.")
                })?;
                anyhow::ensure!(
                    matches!(binding(action, definition), Ok(Some(_))),
                    "Action {action} has no HTTP route, so a key cannot call it."
                );
            }
            let live: i64 = db.query_row(
                "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                live < MAX_LIVE_KEYS,
                "This Deployment already has {MAX_LIVE_KEYS} live keys. Revoke one first."
            );
            let id = format!("key_{}", &Uuid::new_v4().simple().to_string()[..16]);
            let token = format!(
                "{KEY_PREFIX}{}{}",
                Uuid::new_v4().simple(),
                Uuid::new_v4().simple()
            );
            let created = now_ms() as i64;
            db.execute(
                "INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, key_hash(&token), label, serde_json::to_string(&actions)?, serde_json::to_string(&files)?, created],
            )?;
            Ok(
                json!({"keyId": id, "key": token, "name": label, "actions": actions, "files": files, "createdAt": created,
                "note": "The key appears only in this answer. Send it as Authorization: Bearer <key>."}),
            )
        }
        "api_keys_revoke" => {
            let id = a["keyId"].as_str().unwrap_or("");
            let pending = {
                let tx = db.unchecked_transaction()?;
                let changed = tx.execute(
                    "UPDATE api_keys SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
                    rusqlite::params![now_ms() as i64, id],
                )?;
                anyhow::ensure!(
                    changed == 1
                        || tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM api_keys WHERE id=?1)",
                            [id],
                            |row| row.get::<_, bool>(0)
                        )?,
                    "Key not found."
                );
                let pending = crate::copies::Pending::begin(
                    &state.data_root,
                    &format!("{}/{id}", deployment.id),
                    "revoking the app key and recording its revocation",
                )?;
                tx.commit()?;
                pending
            };
            crate::recovery::record(
                state,
                crate::recovery::Entry::new("api-key", Some(&deployment.id), id, "revoked"),
            )
            .await?;
            pending.finish()?;
            Ok(json!({"keyId": id, "revoked": true}))
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

fn require_api_key_modules(
    modules: &std::collections::BTreeMap<String, Value>,
    actions: &[String],
    files: &[String],
) -> Result<()> {
    if !files.is_empty() {
        crate::modules::require_on(modules, "files").map_err(anyhow::Error::new)?;
    }
    if !actions.is_empty() {
        crate::modules::require_on(modules, "actions").map_err(anyhow::Error::new)?;
        crate::modules::require_on(modules, "http").map_err(anyhow::Error::new)?;
    }
    Ok(())
}

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

fn own_origin(headers: &HeaderMap) -> bool {
    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    same_origin(headers) && !matches!(site, Some("same-site" | "cross-site"))
}

async fn resolve_caller(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
    client: std::net::IpAddr,
) -> Result<Caller, Response> {
    if let Some(token) = bearer(headers).filter(|token| token.starts_with(KEY_PREFIX)) {
        if let Err(wait) = state.guards.key_attempt(&deployment.id, client) {
            return Err(rate_limited(wait));
        }
        let path = crate::layout::identity(&state.data_root, &deployment.id);
        let token = token.to_owned();
        return match crate::app_accounts::runtime_identity(state, &deployment.id, move || {
            find_key(&path, &token)
        })
        .await?
        {
            Ok(Some(key)) => Ok(Caller::Key(key)),
            Ok(None) => {
                state.guards.key_failed(&deployment.id, client);
                Err(refusal(StatusCode::UNAUTHORIZED, "invalid-api-key"))
            }
            Err(error_value) => Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error_value.to_string(),
            )),
        };
    }
    if !own_origin(headers) {
        return Ok(Caller::Visitor);
    }
    match authenticate_deployment(state, deployment, headers).await {
        Ok(Some(user)) => Ok(Caller::Person(user)),
        Ok(None) => Ok(Caller::Visitor),
        Err(response) => Err(response),
    }
}

pub(crate) fn rate_limited(wait: std::time::Duration) -> Response {
    let mut response = refusal(StatusCode::TOO_MANY_REQUESTS, "rate-limited");
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from(wait.as_secs().max(1)),
    );
    response
}

fn refusal(status: StatusCode, message: &str) -> Response {
    let mut response = error(status, message);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if status == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}

#[derive(Debug, PartialEq, Eq)]
pub enum SignatureCheck {
    Valid,
    Invalid,
    Stale,
    Replayed,
}

/// What a signature covers besides `t` and the nonce.
pub struct SignedRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// The raw query string, without `?`.
    pub query: &'a str,
    pub body: &'a [u8],
}

/// The query string as a signature covers it: its pairs sorted by key, then
/// value, and written back as `application/x-www-form-urlencoded`, so the
/// order and the escaping a sender chose do not change the MAC.
pub fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    pairs.sort();
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

fn signed_message(timestamp: u64, nonce: &str, request: &SignedRequest<'_>) -> Vec<u8> {
    let head = format!(
        "{timestamp}\n{nonce}\n{}\n{}\n{}\n",
        request.method,
        request.path,
        canonical_query(request.query)
    );
    [head.as_bytes(), request.body].concat()
}

fn valid_nonce(nonce: &str) -> bool {
    (1..=64).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The signature header value for `request`. A call signs with it
/// (`calls.rs`), and the tests sign their requests with it.
pub fn sign(secret: &str, timestamp: u64, nonce: &str, request: &SignedRequest<'_>) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = ring::hmac::sign(&key, &signed_message(timestamp, nonce, request));
    format!("t={timestamp},n={nonce},v1={}", hex::encode(tag.as_ref()))
}

fn signatures_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS route_signatures(
           path TEXT NOT NULL, tag TEXT NOT NULL, seen_at INTEGER NOT NULL, PRIMARY KEY(path, tag));
         CREATE INDEX IF NOT EXISTS route_signatures_seen ON route_signatures(seen_at);",
    )
}

/// Checks a signature for `request` and records it in `db`, the Deployment's
/// identity store. Entries older than twice the window are pruned on the
/// way: a `t` that old is refused before the lookup.
pub fn verify_signature(
    db: &Connection,
    request: &SignedRequest<'_>,
    secret: &str,
    header_value: Option<&str>,
    now: u64,
) -> Result<SignatureCheck> {
    let Some(value) = header_value else {
        return Ok(SignatureCheck::Invalid);
    };
    let mut timestamp = None;
    let mut nonce = None;
    let mut tags = Vec::new();
    for part in value.split(',') {
        match part.trim().split_once('=') {
            Some(("t", digits))
                if !digits.is_empty()
                    && digits.len() <= 12
                    && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                timestamp = digits.parse::<u64>().ok();
            }
            Some(("n", text)) if valid_nonce(text) => nonce = Some(text.to_owned()),
            Some(("v1", tag)) => match hex::decode(tag) {
                Ok(bytes) if bytes.len() == SIGNATURE_TAG_BYTES => tags.push(bytes),
                _ => {}
            },
            _ => {}
        }
    }
    let (Some(timestamp), Some(nonce)) = (timestamp, nonce) else {
        return Ok(SignatureCheck::Invalid);
    };
    if timestamp.abs_diff(now) > SIGNATURE_WINDOW_SECONDS {
        return Ok(SignatureCheck::Stale);
    }
    // Each tag costs one MAC over the body, so a header cannot ask for more
    // than a rotation needs.
    if tags.len() > MAX_SIGNATURE_TAGS {
        return Ok(SignatureCheck::Invalid);
    }
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let message = signed_message(timestamp, &nonce, request);
    let Some(tag) = tags
        .iter()
        .find(|tag| ring::hmac::verify(&key, &message, tag).is_ok())
    else {
        return Ok(SignatureCheck::Invalid);
    };
    signatures_table(db)?;
    let cutoff = now.saturating_sub(2 * SIGNATURE_WINDOW_SECONDS) as i64;
    db.execute("DELETE FROM route_signatures WHERE seen_at < ?1", [cutoff])?;
    let inserted = db.execute(
        "INSERT INTO route_signatures(path, tag, seen_at) VALUES(?1, ?2, ?3) ON CONFLICT(path, tag) DO NOTHING",
        rusqlite::params![request.path, hex::encode(tag), now as i64],
    )?;
    Ok(if inserted == 1 {
        SignatureCheck::Valid
    } else {
        SignatureCheck::Replayed
    })
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
pub struct RouteTable {
    program: ActionProgram,
    routes: HashMap<String, (String, HttpBinding)>,
    pages: Vec<(String, String)>,
}

impl RouteTable {
    fn parse(actions: &str) -> Self {
        let program: ActionProgram = serde_json::from_str(actions).unwrap_or(ActionProgram {
            version: 1,
            actions: HashMap::new(),
        });
        let routes = program
            .actions
            .iter()
            .filter_map(|(name, action)| match binding(name, action) {
                Ok(Some(http)) => Some((
                    format!("{} {}", http.method, http.path),
                    (name.clone(), http),
                )),
                _ => None,
            })
            .collect();
        let mut pages = program
            .actions
            .iter()
            .filter_map(|(name, action)| {
                action
                    .page
                    .as_ref()?
                    .get("path")?
                    .as_str()
                    .map(|path| (name.clone(), path.to_owned()))
            })
            .collect::<Vec<_>>();
        pages.sort_by(|(left_name, left), (right_name, right)| {
            let literals = |path: &str| {
                path.split('/')
                    .filter(|segment| !segment.starts_with(':'))
                    .count()
            };
            literals(right)
                .cmp(&literals(left))
                .then_with(|| left_name.cmp(right_name))
        });
        Self {
            program,
            routes,
            pages,
        }
    }

    fn find(&self, method: &str, path: &str) -> Option<&(String, HttpBinding)> {
        self.routes.get(&format!("{method} {path}"))
    }

    fn find_page(
        &self,
        path: &str,
    ) -> Option<(&ActionDefinition, &Value, serde_json::Map<String, Value>)> {
        self.pages.iter().find_map(|(name, pattern)| {
            let action = self.program.actions.get(name)?;
            let page = action.page.as_ref()?;
            let params = match_page(pattern, path)?;
            Some((action, page, params))
        })
    }

    pub(crate) fn has_pages(&self) -> bool {
        !self.pages.is_empty()
    }

    fn allowed(&self, path: &str) -> &'static str {
        if self.find("GET", path).is_some() {
            "GET"
        } else {
            "GET, HEAD"
        }
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
    let mut cache = route_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache
        .entry(deployment.id.clone())
        .or_insert_with(|| CachedRoutes {
            revision: None,
            table: table.clone(),
            parses: 0,
        });
    entry.revision = Some(deployment.artifact_version.clone());
    entry.table = table.clone();
    entry.parses += 1;
    table
}

pub(crate) fn route_table(deployment: &Deployment) -> Arc<RouteTable> {
    {
        let cache = route_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.get(&deployment.id).filter(|cached| {
            cached.revision.as_deref() == Some(deployment.artifact_version.as_str())
        }) {
            return cached.table.clone();
        }
    }
    cache_table(deployment)
}

/// Replaces the cached routes with the release `deploy` just installed.
/// `release_apply` reaches `deploy`, so every publish passes here.
pub fn routes_published(deployment: &Deployment) {
    cache_table(deployment);
}

#[cfg(test)]
fn route_parses(deployment_id: &str) -> u64 {
    route_cache()
        .lock()
        .unwrap()
        .get(deployment_id)
        .map_or(0, |cached| cached.parses)
}

pub async fn get_route(
    State(state): State<AppState>,
    AxumPath(relative): AxumPath<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let path = request.uri().path().to_owned();
    let matched = deployment_for(&state, &headers)
        .await
        .map(|deployment| route_table(&deployment).find("GET", &path).is_some())
        .unwrap_or(false);
    let mut response = get_route_inner(state, relative, request).await;
    if matched {
        crate::functions::trace::attach_request_response(&headers, &mut response);
    }
    response
}

async fn get_route_inner(state: AppState, relative: String, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if let Some(deployment) = deployment_for(&state, request.headers()).await {
        let table = route_table(&deployment);
        if let Some((name, http)) = table.find("GET", &path) {
            if let Err(refused) = crate::modules::require_on(&deployment.modules, "http") {
                return crate::refusal_error(&refused);
            }
            if request.method() == axum::http::Method::HEAD {
                let mut response = error(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "A route does not answer HEAD. Send GET.",
                );
                response
                    .headers_mut()
                    .insert(header::ALLOW, HeaderValue::from_static("GET"));
                return response;
            }
            let raw_query = request.uri().query().unwrap_or("");
            let input = match query_input(&table.program, name, raw_query) {
                Ok(input) => input,
                Err(message) => return refusal(StatusCode::BAD_REQUEST, &message),
            };
            let client = state.guards.client_ip(peer(&request), request.headers());
            let mut response = run_route(
                &state,
                &deployment,
                &table,
                name,
                http,
                request.headers(),
                client,
                input,
                raw_query,
                b"",
            )
            .await;
            if route_callers(http, is_private(&deployment)).is_none() {
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
            }
            return response;
        }
        return crate::serve_key_with_routes(
            state,
            request.headers().clone(),
            relative,
            Some(table),
        )
        .await;
    }
    serve_artifact(State(state), AxumPath(relative), request.headers().clone()).await
}

pub(crate) async fn serve_page_path(
    state: &AppState,
    deployment: &Deployment,
    table: &RouteTable,
    path: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if !table.has_pages() {
        return None;
    }
    let (definition, page, params) = table.find_page(path)?;
    if is_private(deployment) {
        match authenticate_deployment(state, deployment, headers).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Some(private_login_page(
                    &deployment.project_name,
                    &deployment.signup_policy,
                    state.accounts.verification,
                    google::offered_here(state, deployment, headers),
                ));
            }
            Err(response) => return Some(response),
        }
    }
    let input = Value::Object(page_query_input(definition, &params));
    let bindings = match resolve_bindings(definition, &input, None) {
        Ok(bindings) => bindings,
        Err(_) => return Some(page_not_found(state, deployment).await),
    };
    let _database = crate::action_lock(state, &deployment.id, &definition.kind).await;
    let permit = match state.guards.statement_permit(&deployment.id).await {
        Ok(permit) => permit,
        Err(wait) => return Some(rate_limited(wait)),
    };
    let result = match run_action_in_slot(
        database_path(state, &deployment.id),
        definition,
        bindings,
        permit,
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let mut response =
                (StatusCode::INTERNAL_SERVER_ERROR, "The page query failed.").into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return Some(response);
        }
    };
    let Some(rows) = result.answer.as_array() else {
        return Some(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The page query did not return rows.",
        ));
    };
    if rows.len() > 1 {
        return Some(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The page query returned more than one row.",
        ));
    }
    let Some(row) = rows.first().and_then(Value::as_object) else {
        return Some(page_not_found(state, deployment).await);
    };
    let Some(file) = page.get("file").and_then(Value::as_str) else {
        return Some(page_not_found(state, deployment).await);
    };
    let bytes = match read_artifact(state, deployment, file).await {
        Ok(bytes) => bytes,
        Err(_) => return Some(page_not_found(state, deployment).await),
    };
    let seo_on = modules::is_on(&deployment.modules, "seo");
    let address = seo_on.then(|| seo::address(state, deployment));
    let canonical = address.as_ref().and_then(|address| {
        page.get("path")
            .and_then(Value::as_str)
            .map(|pattern| seo::page_path_canonical(&address.url, pattern, &params))
    });
    let description = row.get("description").and_then(Value::as_str).unwrap_or("");
    let image = row.get("image").and_then(Value::as_str).map(|image| {
        if image.starts_with("http://") || image.starts_with("https://") {
            image.to_owned()
        } else {
            format!(
                "{}/{}",
                address
                    .as_ref()
                    .map_or(deployment.url.as_str(), |address| address.url.as_str())
                    .trim_end_matches('/'),
                image.trim_start_matches('/')
            )
        }
    });
    let title = row
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty());
    let html = if let Some(title) = title {
        seo::page_html(
            &bytes,
            &Value::Object(params),
            title,
            description,
            image.as_deref(),
            canonical.as_deref(),
        )
    } else {
        seo::page_params_html(&bytes, &Value::Object(params))
    };
    let row_bytes = serde_json::to_vec(row).unwrap_or_default();
    let seo_state = serde_json::to_vec(&deployment.modules["seo"]).unwrap_or_default();
    let digest = sha2::Sha256::digest(
        [
            deployment.artifact_version.as_bytes(),
            row_bytes.as_slice(),
            canonical.as_deref().unwrap_or("").as_bytes(),
            seo_state.as_slice(),
        ]
        .concat(),
    );
    let etag = format!(
        "\"{}-{}\"",
        deployment.artifact_version,
        hex::encode(digest)
    );
    count_page_view(state, deployment, headers, file).await;
    if crate::etag_matches(headers, &etag) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(if is_private(deployment) {
                "private, no-cache"
            } else {
                "no-cache"
            }),
        );
        if !seo_on || title.is_none() {
            response
                .headers_mut()
                .insert("x-robots-tag", HeaderValue::from_static("noindex"));
        }
        return Some(response);
    }
    let mut response = artifact_response(html, file);
    response
        .headers_mut()
        .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if is_private(deployment) {
            "private, no-cache"
        } else {
            "no-cache"
        }),
    );
    if !seo_on || title.is_none() {
        response
            .headers_mut()
            .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    }
    Some(response)
}

async fn page_not_found(state: &AppState, deployment: &Deployment) -> Response {
    match read_artifact(state, deployment, "404.html").await {
        Ok(bytes) => {
            let mut response = artifact_response(bytes, "404.html");
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
                .headers_mut()
                .insert("x-robots-tag", HeaderValue::from_static("noindex"));
            response
        }
        Err(_) => artifact::not_found_page(),
    }
}

fn match_page(pattern: &str, path: &str) -> Option<serde_json::Map<String, Value>> {
    let pattern = pattern
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let path = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    if pattern.len() != path.len() {
        return None;
    }
    let mut params = serde_json::Map::new();
    for (pattern, value) in pattern.iter().zip(path) {
        if let Some(key) = pattern.strip_prefix(':') {
            params.insert(key.to_owned(), Value::String((*value).to_owned()));
        } else if *pattern != value {
            return None;
        }
    }
    Some(params)
}

fn page_query_input(
    action: &ActionDefinition,
    params: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut input = params.clone();
    for binding in &action.bindings {
        if binding.source != "input" {
            continue;
        }
        let Some(key) = binding.key.as_deref() else {
            continue;
        };
        let Some(Value::String(value)) = params.get(key) else {
            continue;
        };
        let typed = match binding.value_type.as_deref() {
            Some("number") => value.parse::<i64>().map(Value::from).ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
            }),
            Some("boolean") => match value.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            _ => None,
        };
        if let Some(typed) = typed {
            input.insert(key.to_owned(), typed);
        }
    }
    input
}

#[cfg(test)]
mod page_path_tests {
    use super::*;

    #[test]
    fn page_path_keeps_axum_decoded_percent_values() {
        assert_eq!(
            match_page("/posts/:slug", "/posts/100%"),
            Some(
                serde_json::json!({"slug": "100%"})
                    .as_object()
                    .unwrap()
                    .clone()
            )
        );
    }
}

pub async fn post_route(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let path = request.uri().path().to_owned();
    let matched = deployment_for(&state, &headers)
        .await
        .map(|deployment| route_table(&deployment).find("POST", &path).is_some())
        .unwrap_or(false);
    let mut response = post_route_inner(state, request).await;
    if matched {
        crate::functions::trace::attach_request_response(&headers, &mut response);
    }
    response
}

async fn post_route_inner(state: AppState, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let not_allowed = |allow: &'static str| {
        let mut response = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "No route accepts POST on this path.",
        );
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static(allow));
        response
    };
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return not_allowed("GET, HEAD");
    };
    let table = route_table(&deployment);
    let Some((name, http)) = table.find("POST", &path) else {
        return not_allowed(table.allowed(&path));
    };
    if let Err(refused) = crate::modules::require_on(&deployment.modules, "http") {
        return crate::refusal_error(&refused);
    }
    let client = state.guards.client_ip(peer(&request), request.headers());
    let headers = request.headers().clone();
    let raw_query = request.uri().query().unwrap_or("").to_owned();
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The request body exceeds 64 KiB.",
            );
        }
    };
    run_route(
        &state,
        &deployment,
        &table,
        name,
        http,
        &headers,
        client,
        Value::Null,
        &raw_query,
        &body,
    )
    .await
}

/// A call from an app of this installation to one of `deployment`'s routes,
#[allow(clippy::too_many_arguments)]
pub async fn call_route(
    state: &AppState,
    deployment: &Deployment,
    method: &str,
    path: &str,
    raw_query: &str,
    body: &[u8],
    headers: &HeaderMap,
    client: std::net::IpAddr,
) -> Response {
    if let Err(refused) = crate::modules::require_on(&deployment.modules, "http") {
        return crate::refusal_error(&refused);
    }
    let table = route_table(deployment);
    let Some((name, http)) = table.find(method, path) else {
        return refusal(
            StatusCode::NOT_FOUND,
            "No route of this app answers that method and path.",
        );
    };
    let query = if method == "GET" {
        match query_input(&table.program, name, raw_query) {
            Ok(input) => input,
            Err(message) => return refusal(StatusCode::BAD_REQUEST, &message),
        }
    } else {
        Value::Null
    };
    run_route(
        state, deployment, &table, name, http, headers, client, query, raw_query, body,
    )
    .await
}

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
            .find(|binding| {
                binding.source == "input" && binding.key.as_deref() == Some(key.as_str())
            })
            .and_then(|binding| binding.value_type.as_deref());
        let value = match declared {
            Some("number") => text
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| {
                    text.parse::<f64>()
                        .ok()
                        .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number))
                })
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
    raw_query: &str,
    body: &[u8],
) -> Response {
    if let Err(refused) = crate::modules::require_on(&deployment.modules, "http") {
        return crate::refusal_error(&refused);
    }
    let mut caller = match resolve_caller(state, deployment, headers, client).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let callers = route_callers(http, is_private(deployment));
    let accepts = |proof: &str| {
        callers
            .as_ref()
            .is_none_or(|list| list.iter().any(|caller| caller == proof))
    };
    match &caller {
        Caller::Key(_) if !accepts("key") => {
            return refusal(
                StatusCode::FORBIDDEN,
                "This route does not accept API keys.",
            );
        }
        Caller::Key(key) if !key.actions.iter().any(|action| action == name) => {
            return refusal(
                StatusCode::FORBIDDEN,
                &format!("This API key may not call {name}."),
            );
        }
        Caller::Person(_) if !accepts("session") => caller = Caller::Visitor,
        _ => {}
    }
    let limited = match &caller {
        Caller::Visitor => state.guards.anonymous_actions.check(&format!(
            "{}|{}",
            deployment.id,
            limits::client_key(client)
        )),
        Caller::Person(user) => state
            .guards
            .signed_in_actions
            .check(&format!("runtime:{}:{}", deployment.id, user.user_id)),
        Caller::Key(key) => state
            .guards
            .signed_in_actions
            .check(&format!("key:{}:{}", deployment.id, key.id)),
    };
    if let Err(wait) = limited {
        return rate_limited(wait);
    }
    let signs = matches!(caller, Caller::Visitor) && callers.is_some();
    if signs && !accepts("signature") {
        return refusal(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let delivery = if let Some(signature) = http.signature.as_ref().filter(|_| signs) {
        let (worker_state, worker_deployment) = (state.clone(), deployment.clone());
        let (signature, headers) = (signature.clone(), headers.clone());
        let (method, path, query, body) = (
            http.method.clone(),
            http.path.clone(),
            raw_query.to_owned(),
            body.to_vec(),
        );
        let proof = crate::app_accounts::runtime_identity(state, &deployment.id, move || {
            let request = SignedRequest {
                method: &method,
                path: &path,
                query: &query,
                body: &body,
            };
            check_route_signature(
                &worker_state,
                &worker_deployment,
                &signature,
                &headers,
                request,
            )
        })
        .await;
        match proof {
            Ok(Ok(delivery)) => delivery,
            Ok(Err(response)) | Err(response) => return response,
        }
    } else {
        None
    };
    let response = run_verified(state, deployment, table, name, http, &caller, query, body).await;
    if let Some(key) = delivery.filter(|_| !response.status().is_success()) {
        let guard = crate::named_lock(state, &format!("identity:{}", deployment.id), false).await;
        let (worker_state, worker_deployment, path) =
            (state.clone(), deployment.clone(), http.path.clone());
        let released = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            identity_connection(&worker_state, &worker_deployment)
                .and_then(|db| crate::provider::release(&db, &path, &key))
        })
        .await;
        if let Err(error_value) = released
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            eprintln!(
                "Could not release a delivery on {}: {error_value}",
                http.path
            );
        }
    }
    response
}

fn check_route_signature(
    state: &AppState,
    deployment: &Deployment,
    signature: &SignatureBinding,
    headers: &HeaderMap,
    request: SignedRequest<'_>,
) -> std::result::Result<Option<String>, Response> {
    let storage_error = |error_value: anyhow::Error| {
        error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string())
    };
    let secret = read_secrets(&state.data_root, &deployment.id)
        .and_then(|db| secret_value(&db, &signature.secret))
        .map_err(storage_error)?
        .ok_or_else(|| {
            refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "The route's signing secret is not set.",
            )
        })?;
    let now = now_ms() / 1000;
    if signature.scheme == crate::provider::Scheme::Konae {
        let header = headers
            .get(signature.header.as_str())
            .and_then(|value| value.to_str().ok());
        let db = identity_connection(state, deployment).map_err(storage_error)?;
        return match verify_signature(&db, &request, &secret, header, now).map_err(storage_error)? {
            SignatureCheck::Valid => Ok(None),
            SignatureCheck::Invalid => Err(refusal(StatusCode::UNAUTHORIZED, "invalid-signature")),
            SignatureCheck::Stale => Err(refusal(StatusCode::UNAUTHORIZED, "stale-timestamp")),
            SignatureCheck::Replayed => Err(refusal(StatusCode::CONFLICT, "replayed-signature")),
        };
    }
    let read = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    match crate::provider::verify(
        &signature.scheme,
        &signature.header,
        &secret,
        &read,
        request.body,
        now,
    ) {
        crate::provider::Delivery::Valid { key, expires } => {
            let db = identity_connection(state, deployment).map_err(storage_error)?;
            if crate::provider::claim(&db, request.path, &key, expires, now)
                .map_err(storage_error)?
            {
                Ok(Some(key))
            } else {
                Err(refusal(StatusCode::CONFLICT, "replayed-signature"))
            }
        }
        crate::provider::Delivery::Invalid => {
            Err(refusal(StatusCode::UNAUTHORIZED, "invalid-signature"))
        }
        crate::provider::Delivery::Stale => {
            Err(refusal(StatusCode::UNAUTHORIZED, "stale-timestamp"))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_verified(
    state: &AppState,
    deployment: &Deployment,
    table: &RouteTable,
    name: &str,
    http: &HttpBinding,
    caller: &Caller,
    query: Value,
    body: &[u8],
) -> Response {
    let input = if http.method == "POST" {
        if body.iter().all(u8::is_ascii_whitespace) {
            json!({})
        } else {
            match serde_json::from_slice::<Value>(body) {
                Ok(value) => value,
                Err(_) => {
                    return refusal(
                        StatusCode::BAD_REQUEST,
                        "The request body must be a JSON object.",
                    );
                }
            }
        }
    } else {
        query
    };
    let input = match &http.fields {
        Some(fields) => crate::provider::project(fields, &table.program.actions[name], &input),
        None => input,
    };
    let action = &table.program.actions[name];
    let params = match resolve_bindings(action, &input, caller.actor_id()) {
        Ok(params) => params,
        Err(message) => return refusal(StatusCode::BAD_REQUEST, &message),
    };
    let db_path = database_path(state, &deployment.id);
    let _guard = action_lock(state, &deployment.id, &action.kind).await;
    crate::copies::operation::action(state, &deployment.id, name, action, async {
        let result = match state.guards.statement_permit(&deployment.id).await {
            Ok(permit) if action.notify.is_some() => crate::run_action_in_slot_with_notify(db_path, action, params, permit,
                crate::notify::MutationContext { app: deployment.id.clone(), release: deployment.artifact_version.clone(), action: name.to_owned(),
                    input: input.clone(), account: match &caller { Caller::Person(user) => Some(user.user_id.clone()), _ => None },
                    actor_id: caller.actor_id().map(str::to_owned), at: crate::now_ms(), documents_rule: None }).await,
            Ok(permit) => run_action_in_slot(db_path, action, params, permit).await,
            Err(wait) => return rate_limited(wait),
        };
        drop(_guard);
        let counted = !matches!(&caller, Caller::Person(user) if user.entered_by.is_some());
        match result {
            Ok(ran) => {
                let changed = match crate::guard::outcome(name, action, &ran.answer) {
                    crate::guard::Outcome::Refused(message) => {
                        if counted {
                            crate::usage::execution(
                                &state.data_root,
                                &deployment.id,
                                name,
                                "route",
                                "refused",
                            );
                            crate::usage::count(
                                &state.data_root,
                                &deployment.id,
                                crate::usage::Operation::Refused(name.to_owned()),
                            );
                        }
                        return refusal(StatusCode::FORBIDDEN, &message);
                    }
                    outcome => matches!(outcome, crate::guard::Outcome::Changed),
                };
                if counted {
                    crate::usage::execution(&state.data_root, &deployment.id, name, "route", "ok");
                    crate::usage::count(
                        &state.data_root,
                        &deployment.id,
                        crate::usage::Operation::Route(http.method.clone(), http.path.clone()),
                    );
                }
                if changed {
                    if counted {
                        crate::usage::signal(
                            &state.data_root,
                            &deployment.id,
                            name,
                            action,
                            &input,
                            caller.actor_id(),
                        );
                    }
                    if let Some(id) = ran.notify_intent.as_deref() { crate::notify::record_intent(state, deployment, id).await; }
                }
                let mut response = Json(json!({"result": ran.answer})).into_response();
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            }
            Err(error_value) => {
                if counted {
                    crate::usage::execution(
                        &state.data_root,
                        &deployment.id,
                        name,
                        "route",
                        "failed",
                    );
                }
                refusal(StatusCode::BAD_REQUEST, &error_value.to_string())
            }
        }
    })
    .await
}

async fn read_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    request: Request,
) -> Response {
    let client = state.guards.client_ip(peer(&request), request.headers());
    let headers = request.headers().clone();
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let caller = match resolve_caller(&state, &deployment, &headers, client).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let counted = !matches!(&caller, Caller::Person(user) if user.entered_by.is_some());
    let mut response = match caller {
        Caller::Key(key) => read_file_as_key(&state, &deployment, &key, &id, &headers).await,
        Caller::Person(_) => files::serve_file(State(state.clone()), AxumPath(id), headers).await,
        Caller::Visitor => {
            let mut visitor = headers.clone();
            visitor.remove(header::COOKIE);
            visitor.remove(header::AUTHORIZATION);
            files::serve_file(State(state.clone()), AxumPath(id), visitor).await
        }
    };
    if response.status().is_success() && counted {
        crate::usage::count(
            &state.data_root,
            &deployment.id,
            crate::usage::Operation::FileRead,
        );
    }
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    if !response.headers().contains_key(header::ETAG) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }
    response
}

fn key_reads(key: &ApiKey, visibility: &str, private_deployment: bool) -> bool {
    (visibility == "public" && !private_deployment)
        || key.files.iter().any(|named| named == visibility)
}

async fn read_file_as_key(
    state: &AppState,
    deployment: &Deployment,
    key: &ApiKey,
    id: &str,
    headers: &HeaderMap,
) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    if let Err(refused) = crate::modules::require_on(&deployment.modules, "files") {
        return error(StatusCode::FORBIDDEN, &refused.message);
    }
    if files::app_user_files(deployment) || !files::valid_file_id(id) {
        return not_found();
    }
    let Some(metadata) = files::find_file_metadata(state, &deployment.id, id).await else {
        return not_found();
    };
    if !key_reads(
        key,
        files::file_visibility(&metadata),
        is_private(deployment),
    ) {
        return not_found();
    }
    files::file_response(state, &deployment.id, &metadata, headers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_scope_requires_only_the_modules_it_uses() {
        let mut modules = crate::modules::defaults();
        modules.get_mut("files").unwrap()["on"] = json!(true);
        assert!(require_api_key_modules(&modules, &[], &["public".into()]).is_ok());
        modules.get_mut("actions").unwrap()["on"] = json!(true);
        let refused =
            require_api_key_modules(&modules, &["hooks.receive".into()], &[]).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "Module http is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
        );
        modules.get_mut("http").unwrap()["on"] = json!(true);
        modules.get_mut("actions").unwrap()["on"] = json!(false);
        assert_eq!(
            require_api_key_modules(&modules, &["hooks.receive".into()], &[])
                .unwrap_err()
                .to_string(),
            "Module actions is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
        );
    }

    fn program(actions: &Value) -> ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    #[test]
    fn bindings_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../http-binding-cases.json")).unwrap();
        assert!(cases.len() > 15);
        for case in cases {
            let program = program(&case["actions"]);
            let mut names = program.actions.keys().collect::<Vec<_>>();
            names.sort();
            let outcome = names
                .into_iter()
                .try_for_each(|name| validate_action(name, &program.actions[name]))
                .and_then(|_| validate_routes(&program));
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn a_signature_verifies_once_inside_its_window_and_the_store_remembers_it() {
        let root = std::env::temp_dir().join(format!("konae-signatures-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("identity.sqlite");
        let open = || Connection::open(&file).unwrap();
        let secret = "orders-secret-0123";
        let body = br#"{"id":"A-1","total":12}"#;
        let post = |path: &'static str| SignedRequest {
            method: "POST",
            path,
            query: "",
            body,
        };
        let now = 1_800_000_000;
        let header = sign(secret, now, "n1", &post("/hooks/orders"));
        let verify =
            |db: &Connection,
             request: &SignedRequest<'_>,
             secret: &str,
             header: Option<&str>,
             at: u64| { verify_signature(db, request, secret, header, at).unwrap() };
        assert_eq!(
            verify(
                &open(),
                &post("/hooks/orders"),
                secret,
                Some(&header),
                now + 10
            ),
            SignatureCheck::Valid
        );
        assert_eq!(
            verify(
                &open(),
                &post("/hooks/orders"),
                secret,
                Some(&header),
                now + 20
            ),
            SignatureCheck::Replayed
        );
        let retry = sign(secret, now, "n2", &post("/hooks/orders"));
        assert_eq!(
            verify(
                &open(),
                &post("/hooks/orders"),
                secret,
                Some(&retry),
                now + 20
            ),
            SignatureCheck::Valid
        );
        let db = open();
        assert_eq!(
            verify(&db, &post("/hooks/other"), secret, Some(&header), now),
            SignatureCheck::Invalid,
            "the path is signed"
        );
        let get = SignedRequest {
            method: "GET",
            path: "/hooks/orders",
            query: "",
            body,
        };
        assert_eq!(
            verify(&db, &get, secret, Some(&header), now),
            SignatureCheck::Invalid,
            "the method is signed"
        );
        let fresh = |n: &str| sign(secret, now, n, &post("/hooks/fresh"));
        assert_eq!(
            verify(
                &db,
                &post("/hooks/fresh"),
                secret,
                Some(&fresh("a")),
                now + 301
            ),
            SignatureCheck::Stale,
            "stale"
        );
        assert_eq!(
            verify(
                &db,
                &post("/hooks/fresh"),
                secret,
                Some(&fresh("a")),
                now - 301
            ),
            SignatureCheck::Stale,
            "future"
        );
        assert_eq!(
            verify(
                &db,
                &post("/hooks/fresh"),
                "another-secret-0123",
                Some(&fresh("a")),
                now + 301
            ),
            SignatureCheck::Stale,
            "a stale t is refused before the MAC"
        );
        assert_eq!(
            verify(
                &db,
                &post("/hooks/fresh"),
                "another-secret-0123",
                Some(&fresh("a")),
                now
            ),
            SignatureCheck::Invalid,
            "wrong secret"
        );
        let tampered = SignedRequest {
            body: br#"{"id":"A-1","total":99}"#,
            ..post("/hooks/fresh")
        };
        assert_eq!(
            verify(&db, &tampered, secret, Some(&fresh("a")), now),
            SignatureCheck::Invalid,
            "tampered body"
        );
        let moved = fresh("a").replacen(&format!("t={now}"), &format!("t={}", now + 1), 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&moved), now),
            SignatureCheck::Invalid,
            "t is signed"
        );
        let renonced = fresh("a").replacen("n=a", "n=b", 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&renonced), now),
            SignatureCheck::Invalid,
            "the nonce is signed"
        );
        let no_nonce = fresh("a").replacen("n=a,", "", 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&no_nonce), now),
            SignatureCheck::Invalid,
            "a nonce is required"
        );
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, None, now),
            SignatureCheck::Invalid,
            "missing"
        );
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some("n=a,v1=00"), now),
            SignatureCheck::Invalid,
            "no timestamp"
        );
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&fresh("a")), now),
            SignatureCheck::Valid
        );
        let crowded = fresh("c").replacen("v1=", &format!("{}v1=", "v1=,v1=00,".repeat(5000)), 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&crowded), now),
            SignatureCheck::Valid,
            "short values are ignored"
        );
        let third = fresh("d").replacen("v1=", &format!("v1={0},v1={0},v1=", "ab".repeat(32)), 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&third), now),
            SignatureCheck::Invalid,
            "three tags"
        );
        let rotated = fresh("d").replacen("v1=", &format!("v1={},v1=", "ab".repeat(32)), 1);
        assert_eq!(
            verify(&db, &post("/hooks/fresh"), secret, Some(&rotated), now),
            SignatureCheck::Valid,
            "two tags, the second one valid"
        );
        let stored = |db: &Connection| {
            db.query_row("SELECT COUNT(*) FROM route_signatures", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(stored(&db), 5);
        let later = now + 2 * SIGNATURE_WINDOW_SECONDS + 30;
        let late = sign(secret, later, "n3", &post("/hooks/orders"));
        assert_eq!(
            verify(&db, &post("/hooks/orders"), secret, Some(&late), later),
            SignatureCheck::Valid
        );
        assert_eq!(stored(&db), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_signature_covers_the_query_string_in_any_order_and_escaping() {
        let db = Connection::open_in_memory().unwrap();
        let secret = "feed-secret-0123456";
        let now = 1_800_000_000;
        let get = |query: &'static str| SignedRequest {
            method: "GET",
            path: "/api/events",
            query,
            body: b"",
        };
        let header = sign(secret, now, "q1", &get("after=1&severity=critical"));
        assert_eq!(
            verify_signature(
                &db,
                &get("after=4&severity=critical"),
                secret,
                Some(&header),
                now
            )
            .unwrap(),
            SignatureCheck::Invalid
        );
        assert_eq!(
            verify_signature(
                &db,
                &get("severity=critical&after=1"),
                secret,
                Some(&header),
                now
            )
            .unwrap(),
            SignatureCheck::Valid
        );
        assert_eq!(
            canonical_query("b=caf%C3%A9+au+lait&a=x%2Ay~"),
            "a=x*y%7E&b=caf%C3%A9+au+lait"
        );
    }

    /// The signer in src/lib/route-signature.ts computes the same headers.
    #[test]
    fn the_signature_vectors_match_the_editors_signer() {
        let vectors: Vec<Value> =
            serde_json::from_str(include_str!("../signature-cases.json")).unwrap();
        assert!(vectors.len() >= 3);
        for vector in vectors {
            let request = SignedRequest {
                method: vector["method"].as_str().unwrap(),
                path: vector["path"].as_str().unwrap(),
                query: vector["query"].as_str().unwrap(),
                body: vector["body"].as_str().unwrap().as_bytes(),
            };
            assert_eq!(
                canonical_query(request.query),
                vector["canonicalQuery"].as_str().unwrap(),
                "{}",
                vector["case"]
            );
            let header = sign(
                vector["secret"].as_str().unwrap(),
                vector["t"].as_u64().unwrap(),
                vector["n"].as_str().unwrap(),
                &request,
            );
            assert_eq!(
                header,
                vector["header"].as_str().unwrap(),
                "{}",
                vector["case"]
            );
        }
    }

    fn test_state(root: &std::path::Path) -> AppState {
        AppState::for_test(&root)
    }

    /// Publishes a public app whose one GET route is `path`, with a static
    /// `app.js` next to it.
    async fn publish_route(state: &AppState, project_id: &str, path: &str) -> Deployment {
        let actions = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous",
            "statement": "SELECT 1 AS n LIMIT 1", "bindings": [], "http": {"method": "GET", "path": path}}}});
        let body = json!({
            "project": {"id": project_id, "name": "Route cache", "slug": project_id, "manifest": {},
                "files": {"actions.json": actions.to_string(), "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>", "app.js": "app"}}
        });
        assert_eq!(
            deploy(&state, body.to_string().as_bytes()).await.status(),
            StatusCode::OK
        );
        state
            .deployments
            .read()
            .await
            .values()
            .next()
            .cloned()
            .unwrap()
    }

    async fn get(state: &AppState, deployment: &Deployment, path: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .uri(path)
            .header(header::HOST, &deployment.hostname)
            .body(Body::empty())
            .unwrap();
        let response = get_route(
            State(state.clone()),
            AxumPath(path.trim_start_matches('/').to_owned()),
            request,
        )
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn a_static_request_does_not_parse_actions_and_a_publish_replaces_the_routes() {
        let root = std::env::temp_dir().join(format!("konae-route-cache-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        let first = publish_route(&state, "route_cache_test", "/api/count").await;
        assert_eq!(
            route_parses(&first.id),
            1,
            "the publish parses the actions once"
        );
        for _ in 0..3 {
            assert_eq!(
                get(&state, &first, "/app.js").await,
                (StatusCode::OK, "app".into())
            );
        }
        assert_eq!(
            get(&state, &first, "/api/count").await,
            (StatusCode::OK, r#"{"result":[{"n":1}]}"#.into())
        );
        assert_eq!(
            route_parses(&first.id),
            1,
            "static files and the route reuse the parsed table"
        );
        // HEAD on the route refuses before SQL. Running a query opens, and so
        // creates, the database file; after a HEAD it is still missing.
        let head = Request::builder()
            .method(axum::http::Method::HEAD)
            .uri("/api/count")
            .header(header::HOST, &first.hostname)
            .body(Body::empty())
            .unwrap();
        tokio::fs::remove_file(database_path(&state, &first.id))
            .await
            .unwrap();
        let refused = get_route(State(state.clone()), AxumPath("api/count".into()), head).await;
        assert_eq!(refused.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(refused.headers()[header::ALLOW], "GET");
        assert!(
            !database_path(&state, &first.id).exists(),
            "HEAD opened no database"
        );
        assert_eq!(get(&state, &first, "/api/count").await.0, StatusCode::OK);
        assert!(
            database_path(&state, &first.id).exists(),
            "GET ran the query"
        );

        let second = publish_route(&state, "route_cache_test", "/api/total").await;
        assert_eq!(second.id, first.id);
        assert_ne!(second.artifact_version, first.artifact_version);
        assert_eq!(
            route_parses(&second.id),
            2,
            "a publish parses the new release"
        );
        assert_eq!(get(&state, &second, "/api/total").await.0, StatusCode::OK);
        // The old path is no route and no artifact file, so it answers 404.
        assert_eq!(
            get(&state, &second, "/api/count").await.0,
            StatusCode::NOT_FOUND,
            "the old route is gone"
        );
        assert_eq!(route_parses(&second.id), 2);

        // A table cached for another revision is not used: a Kernel that
        // loads the record without a publish parses on the first request.
        let mut restarted = second.clone();
        restarted.artifact_version = "v-restarted".into();
        state
            .deployments
            .write()
            .await
            .insert(restarted.hostname.clone(), restarted.clone());
        assert_eq!(
            get(&state, &restarted, "/api/total").await.0,
            StatusCode::OK
        );
        assert_eq!(route_parses(&second.id), 3);
        assert_eq!(
            get(&state, &restarted, "/api/total").await.0,
            StatusCode::OK
        );
        assert_eq!(route_parses(&second.id), 3);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn every_http_route_entry_refuses_when_http_is_off() {
        let root = std::env::temp_dir().join(format!("konae-http-module-off-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        let deployment = publish_route(&state, "route_http_off", "/hooks/incoming").await;
        state
            .deployments
            .write()
            .await
            .get_mut(&deployment.hostname)
            .unwrap()
            .modules
            .get_mut("http")
            .unwrap()["on"] = json!(false);
        let get_response = get(&state, &deployment, "/hooks/incoming").await;
        let get_refusal: Value = serde_json::from_str(&get_response.1).unwrap();
        assert_eq!(get_refusal["code"], "module.off");
        assert_eq!(
            get_refusal["error"],
            "Module http is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
        );
        let head = Request::builder()
            .method(axum::http::Method::HEAD)
            .uri("/hooks/incoming")
            .header(header::HOST, &deployment.hostname)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            get_route(
                State(state.clone()),
                AxumPath("hooks/incoming".into()),
                head
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        let actions = json!({"version": 1, "actions": {"hooks.signed": {"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO events (name) VALUES (?1)", "bindings": [{"source":"input","key":"name","type":"string"}],
            "http": {"method":"POST","path":"/hooks/signed","signature":{"header":"X-Signature","secret":"hook_secret"}}}}});
        secrets_call(
            &secrets_store(&state.data_root, "route_signed").unwrap(),
            "route_signed",
            None,
            "secret_set",
            &json!({"name":"hook_secret","value":"test-secret-123456"}),
        )
        .unwrap();
        let body = json!({"project":{"id":"route_signed","name":"Signed route","slug":"route-signed","manifest":{},"files":{"actions.json":actions.to_string(),"schema.sql":"CREATE TABLE IF NOT EXISTS events(name TEXT);"}},
            "visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<p>shell</p>"}}});
        let deployed = deploy(&state, body.to_string().as_bytes()).await;
        assert_eq!(
            deployed.status(),
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(
                &axum::body::to_bytes(deployed.into_body(), 100_000)
                    .await
                    .unwrap()
            )
        );
        let mut post_deployment = state
            .deployments
            .read()
            .await
            .values()
            .find(|deployment| deployment.id == "route_signed")
            .cloned()
            .unwrap();
        post_deployment.modules.get_mut("http").unwrap()["on"] = json!(false);
        state
            .deployments
            .write()
            .await
            .insert(post_deployment.hostname.clone(), post_deployment.clone());
        let post = Request::builder()
            .method("POST")
            .uri("/hooks/signed")
            .header(header::HOST, &post_deployment.hostname)
            .header("x-signature", "invalid")
            .body(Body::from("{}"));
        let response = post_route(State(state.clone()), post.unwrap()).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "signature verification never runs with http off"
        );
        let signature_refusal: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 100_000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(signature_refusal["code"], "module.off");
        assert_eq!(
            signature_refusal["error"],
            "Module http is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
        );
        let token = format!("{KEY_PREFIX}http-off-test-key");
        let db = identity_connection(&state, &post_deployment).unwrap();
        db.execute(
            "INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["key_http_off", key_hash(&token), "HTTP off", "[\"hooks.signed\"]", "[]", now_ms() as i64],
        ).unwrap();
        let keyed = Request::builder()
            .method("POST")
            .uri("/hooks/signed")
            .header(header::HOST, &post_deployment.hostname)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from("{}"));
        let keyed = post_route(State(state.clone()), keyed.unwrap()).await;
        assert_eq!(
            keyed.status(),
            StatusCode::BAD_REQUEST,
            "the API key path refuses before running the route"
        );
        let keyed_refusal: Value = serde_json::from_slice(
            &axum::body::to_bytes(keyed.into_body(), 100_000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(keyed_refusal["code"], "module.off");
        assert_eq!(keyed_refusal["error"], signature_refusal["error"]);
        let response = call_route(
            &state,
            &post_deployment,
            "POST",
            "/hooks/signed",
            "",
            b"{}",
            &HeaderMap::new(),
            "127.0.0.1".parse().unwrap(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "the internal route path refuses too"
        );
        let internal_refusal: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 100_000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(internal_refusal["code"], "module.off");
        assert_eq!(internal_refusal["error"], signature_refusal["error"]);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A provider's delivery on a live route: the fields map reads the nested
    #[tokio::test]
    async fn a_provider_delivery_runs_once_and_a_failed_one_can_be_retried() {
        let root = std::env::temp_dir().join(format!("konae-provider-route-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        let secret = "github-route-secret-0123";
        secrets_call(
            &secrets_store(&state.data_root, "provider_route").unwrap(),
            "provider_route",
            None,
            "secret_set",
            &json!({"name": "github_webhook", "value": secret}),
        )
        .unwrap();
        let actions = json!({"version": 1, "actions": {"issues.receive": {"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO issues (node_id, title) VALUES (?1, ?2)",
            "bindings": [{"source": "input", "key": "id", "type": "string"}, {"source": "input", "key": "title", "type": "string"}],
            "http": {"method": "POST", "path": "/hooks/github",
                "signature": {"scheme": "hmac-body", "header": "X-Hub-Signature-256", "secret": "github_webhook", "algorithm": "sha256", "encoding": "hex", "prefix": "sha256="},
                "fields": {"id": "$.issue.node_id", "title": "$.issue.title"}}}}});
        let body = json!({
            "project": {"id": "provider_route", "name": "Provider route", "slug": "provider-route", "manifest": {},
                "files": {"actions.json": actions.to_string(), "schema.sql": "CREATE TABLE IF NOT EXISTS issues (node_id TEXT NOT NULL, title TEXT NOT NULL);"}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>"}}
        });
        let deployed = deploy(&state, body.to_string().as_bytes()).await;
        let status = deployed.status();
        let text = axum::body::to_bytes(deployed.into_body(), 100_000)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&text));
        let deployment = state
            .deployments
            .read()
            .await
            .values()
            .next()
            .cloned()
            .unwrap();
        let sign = |payload: &str| {
            format!(
                "sha256={}",
                hex::encode(
                    ring::hmac::sign(
                        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()),
                        payload.as_bytes()
                    )
                    .as_ref()
                )
            )
        };
        let post = |payload: String, signature: String| {
            let state = state.clone();
            let host = deployment.hostname.clone();
            async move {
                let request = Request::builder()
                    .method("POST")
                    .uri("/hooks/github")
                    .header(header::HOST, host)
                    .header("x-hub-signature-256", signature)
                    .body(Body::from(payload))
                    .unwrap();
                let response = post_route(State(state), request).await;
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
                    .await
                    .unwrap();
                (status, String::from_utf8_lossy(&bytes).into_owned())
            }
        };
        let event = r#"{"action":"opened","issue":{"node_id":"I_1","title":"Broken link","user":{"login":"ana"}},"repository":{"full_name":"acme/site"}}"#;
        assert_eq!(
            post(event.into(), sign(event)).await,
            (
                StatusCode::OK,
                r#"{"result":{"changes":1,"lastInsertId":1}}"#.into()
            ),
            "undeclared keys are left out"
        );
        assert_eq!(
            post(event.into(), sign(event)).await,
            (
                StatusCode::CONFLICT,
                r#"{"error":"replayed-signature"}"#.into()
            )
        );
        let tampered = event.replace("Broken link", "Fixed");
        assert_eq!(
            post(tampered, sign(event)).await,
            (
                StatusCode::UNAUTHORIZED,
                r#"{"error":"invalid-signature"}"#.into()
            )
        );
        assert_eq!(
            post(event.into(), sign("{}")).await.0,
            StatusCode::UNAUTHORIZED,
            "another body's MAC"
        );
        let untitled = r#"{"action":"opened","issue":{"node_id":"I_2"}}"#;
        let missing = (
            StatusCode::BAD_REQUEST,
            r#"{"error":"Missing action input: title"}"#.to_owned(),
        );
        assert_eq!(post(untitled.into(), sign(untitled)).await, missing);
        assert_eq!(
            post(untitled.into(), sign(untitled)).await,
            missing,
            "a failed delivery is not remembered, so its retry runs"
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn a_key_reads_what_a_visitor_reads_and_the_visibilities_it_names() {
        let key = |files: &[&str]| ApiKey {
            id: "key".into(),
            actions: Vec::new(),
            files: files.iter().map(|f| f.to_string()).collect(),
        };
        assert!(key_reads(&key(&[]), "public", false));
        assert!(!key_reads(&key(&[]), "private", false));
        assert!(!key_reads(&key(&[]), "public", true));
        assert!(key_reads(&key(&["public"]), "public", true));
        assert!(!key_reads(&key(&["public"]), "private", true));
        assert!(key_reads(&key(&["private"]), "private", true));
        assert!(!key_reads(&key(&["private"]), "public", true));
    }

    #[tokio::test]
    async fn an_api_key_gets_the_module_off_refusal_for_a_file_url() {
        let root =
            std::env::temp_dir().join(format!("konae-key-files-off-{}", uuid::Uuid::new_v4()));
        let mut state = AppState::for_test(&root);
        let mut deployment =
            crate::Settings::new("app", "App", "app", &json!({"visibility":"public"})).deployment(
                crate::Installed {
                    hostname: "app.localhost".into(),
                    created_at: 1,
                    released_at: Some(2),
                    source_digest: None,
                    actions: "{}".into(),
                    artifact_version: "v1".into(),
                    released: None,
                },
                "http://localhost",
            );
        deployment.modules.get_mut("files").unwrap()["on"] = json!(false);
        state.deployments =
            std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
                (deployment.hostname.clone(), deployment.clone()),
            ])));
        let key = ApiKey {
            id: "key".into(),
            actions: Vec::new(),
            files: vec!["public".into()],
        };
        let response = read_file_as_key(
            &state,
            &deployment,
            &key,
            "018f2b2c-4a10-7a01-8000-000000000001",
            &HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"],
            "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn the_apps_google_client_has_fixed_names_and_no_route_or_call_reads_it() {
        let root =
            std::env::temp_dir().join(format!("konae-google-secrets-{}", uuid::Uuid::new_v4()));
        let store = |project: &str| secrets_store(&root, project).unwrap();
        let set = |project: &str, name: &str, value: &str, usage: Option<&str>| {
            let mut input = json!({"name": name, "value": value});
            if let Some(usage) = usage {
                input["use"] = json!(usage);
            }
            secrets_call(&store(project), project, None, "secret_set", &input)
                .map_err(|error| error.to_string())
        };
        let id = "1001-practice.apps.googleusercontent.com";
        assert_eq!(
            set("project_a", "google_client_id", id, Some("route")).unwrap_err(),
            crate::google::NAMES_RESERVED
        );
        assert_eq!(
            set("project_a", "client_id", id, Some("google")).unwrap_err(),
            crate::google::NAMES_ONLY
        );
        assert_eq!(
            set(
                "project_a",
                "google_client_id",
                "GOCSPX-pasted-in-the-wrong-field",
                None
            )
            .unwrap_err(),
            crate::google::ID_LOOKS_LIKE_SECRET
        );
        assert_eq!(
            set("project_a", "google_client_id", id, None).unwrap()["use"],
            "google"
        );
        set(
            "project_a",
            "google_client_secret",
            "GOCSPX-shared-secret-value",
            Some("google"),
        )
        .unwrap();
        set(
            "project_b",
            "google_client_secret",
            "GOCSPX-shared-secret-value",
            Some("google"),
        )
        .unwrap();
        let client = google_client(&store("project_a")).unwrap();
        assert_eq!(
            (client.id.as_deref(), client.secret.as_deref()),
            (Some(id), Some("GOCSPX-shared-secret-value"))
        );
        let client = google_client(&store("project_b")).unwrap();
        assert_eq!(
            (client.id.as_deref(), client.secret.as_deref()),
            (None, Some("GOCSPX-shared-secret-value"))
        );
        let listed = secrets_call(
            &store("project_a"),
            "project_a",
            None,
            "secrets_list",
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            listed["secrets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["use"].clone())
                .collect::<Vec<_>>(),
            vec![json!("google"), json!("google")]
        );
        assert!(!listed.to_string().contains("GOCSPX"));
        assert_eq!(
            project_secret(&store("project_a"), "google_client_secret").unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unset_secret_refuses_the_release() {
        let root = std::env::temp_dir().join(format!("konae-secrets-{}", uuid::Uuid::new_v4()));
        let db = secrets_store(&root, "project_a").unwrap();
        let other = secrets_store(&root, "project_b").unwrap();
        let signed = program(
            &json!({"orders.receive": {"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO orders (external_id) VALUES (?1)", "bindings": [{"source": "input", "key": "id", "type": "string"}],
            "http": {"method": "POST", "path": "/hooks/orders", "signature": {"header": "X-Signature", "secret": "orders_webhook"}}}}),
        );
        assert_eq!(
            require_secrets(&db, &signed).unwrap_err().to_string(),
            "Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing."
        );
        secrets_call(
            &db,
            "project_a",
            None,
            "secret_set",
            &json!({"name": "orders_webhook", "value": "a-long-enough-secret"}),
        )
        .unwrap();
        require_secrets(&db, &signed).unwrap();
        let route = |path: &str, secret: &str| {
            json!({"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO orders (external_id) VALUES (?1)", "bindings": [{"source": "input", "key": "id", "type": "string"}],
            "http": {"method": "POST", "path": path, "signature": {"header": "X-Signature", "secret": secret}}})
        };
        let shared =
            program(&json!({"a.one": route("/one", "hook"), "b.two": route("/two", "hook")}));
        assert_eq!(
            require_secrets(&db, &shared).unwrap_err().to_string(),
            "Actions a.one and b.two sign with the secret hook, which is not set for this project. Set it with secret_set before publishing."
        );
        let three = program(
            &json!({"a.one": route("/one", "feed"), "b.two": route("/two", "orders_webhook"),
            "c.three": route("/three", "hook"), "d.four": route("/four", "feed")}),
        );
        assert_eq!(
            require_secrets(&db, &three).unwrap_err().to_string(),
            "Actions a.one, c.three and d.four sign with the secrets feed and hook, which are not set for this project. Set each with secret_set before publishing.",
            "one refusal names every unset secret and leaves out the one that is set"
        );
        assert!(
            require_secrets(&other, &signed).is_err(),
            "secrets belong to one app"
        );
        let listed = secrets_call(&db, "project_a", None, "secrets_list", &json!({})).unwrap();
        assert!(
            !listed.to_string().contains("a-long-enough-secret"),
            "a listing never shows a value"
        );
        assert!(
            secrets_call(
                &db,
                "project_a",
                None,
                "secret_set",
                &json!({"name": "orders_webhook", "value": "short"})
            )
            .is_err()
        );
        assert!(
            holds_route_secret(&db, "a-long-enough-secret").unwrap(),
            "Konae asks this before another app of the owner takes the value"
        );
        assert!(!holds_route_secret(&other, "a-long-enough-secret").unwrap());
        let svix = program(
            &json!({"emails.receive": {"kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO orders (external_id) VALUES (?1)", "bindings": [{"source": "input", "key": "id", "type": "string"}],
            "http": {"method": "POST", "path": "/hooks/email", "signature": {"scheme": "svix", "secret": "resend_webhook"}}}}),
        );
        secrets_call(
            &db,
            "project_a",
            None,
            "secret_set",
            &json!({"name": "resend_webhook", "value": "whsec_not*base64*at*all"}),
        )
        .unwrap();
        assert_eq!(
            require_secrets(&db, &svix).unwrap_err().to_string(),
            "Action emails.receive verifies svix signatures with the secret resend_webhook, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_."
        );
        secrets_call(
            &db,
            "project_a",
            None,
            "secret_set",
            &json!({"name": "resend_webhook", "value": "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"}),
        )
        .unwrap();
        require_secrets(&db, &svix).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
