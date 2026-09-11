use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const VERSION: &str = "2025-11-25";
const CHUNK: usize = 64 * 1024;
const MAX_TRANSFER: usize = 32 * 1024 * 1024;
pub(super) fn owner_cookie_name(state: &AppState) -> &'static str {
    let mcp_https = std::env::var("LITTLEBOX_MCP_ORIGIN").is_ok_and(|origin| origin.starts_with("https://"));
    if state.public_origin.starts_with("https://") || mcp_https {
        "__Host-littlebox_owner_session"
    } else {
        "littlebox_owner_session"
    }
}
const MAX_OWNER_TRANSFERS: usize = 128;
const MAX_DEPLOYMENT_TRANSFERS: usize = 32;
/// Active transfers one owner credential may hold.
const MAX_OWNER_PRINCIPAL_TRANSFERS: usize = 8;
/// Active transfers one app user may hold. Files are at most 2 MiB, so an app
/// needs one upload or download at a time; two allows a retry.
const MAX_RUNTIME_PRINCIPAL_TRANSFERS: usize = 2;
const OWNER_TRANSFER_TTL_MS: u64 = 60 * 60 * 1000;
const RUNTIME_TRANSFER_TTL_MS: u64 = 10 * 60 * 1000;
const AUDIT_MAX_ROWS: i64 = 10_000;
const AUDIT_PRUNE_INTERVAL: i64 = 128;
const AUDIT_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const AUDIT_TEXT_CHARACTERS: usize = 128;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mcp", post(admin))
        .route("/_littlebox/mcp", post(runtime))
        .route("/_littlebox/owner/login", get(owner_page).post(owner_login))
        .route("/_littlebox/owner/signup", post(owner_signup))
        .route("/_littlebox/owner/logout", post(owner_logout))
        .route("/_littlebox/owner/config", get(owner_config))
        .route("/_littlebox/owner/clerk", post(owner_clerk))
}

pub(super) fn store(state: &AppState) -> Result<Connection> {
    let db = Connection::open(state.data_root.join("mcp.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS managed_sources(project TEXT PRIMARY KEY REFERENCES projects(id), body BLOB NOT NULL, revision INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);
        CREATE TABLE IF NOT EXISTS transfers(id TEXT PRIMARY KEY, principal TEXT NOT NULL, body TEXT NOT NULL, expires INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL, revision INTEGER NOT NULL, transfer TEXT NOT NULL, expires INTEGER NOT NULL, receipt TEXT);
        CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, at INTEGER NOT NULL, actor TEXT NOT NULL, tool TEXT NOT NULL, target TEXT, ok INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS file_receipts(transfer_id TEXT PRIMARY KEY, principal TEXT NOT NULL, request_hash TEXT NOT NULL, body TEXT NOT NULL);")?;
    let has_audience: bool = db
        .prepare("PRAGMA table_info(credentials)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "audience");
    if !has_audience {
        db.execute("ALTER TABLE credentials ADD COLUMN audience TEXT", [])?;
    }
    let has_detail: bool = db
        .prepare("PRAGMA table_info(audit)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "detail");
    if !has_detail {
        db.execute("ALTER TABLE audit ADD COLUMN detail TEXT", [])?;
    }
    db.execute("CREATE INDEX IF NOT EXISTS audit_at ON audit(at)", [])?;
    super::oauth::install(&db)?;
    Ok(db)
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Clone)]
pub(super) struct Owner {
    pub(super) id: String,
    display: Option<String>,
    scopes: Vec<String>,
    projects: Vec<String>,
    session: bool,
}

impl Owner {
    fn require(&self, scope: &str, project: Option<&str>) -> Result<()> {
        anyhow::ensure!(
            self.scopes.iter().any(|s| s == scope),
            "Permission denied for {scope}."
        );
        if !self.projects.is_empty() {
            anyhow::ensure!(
                project.is_some_and(|p| self.projects.iter().any(|id| id == p)),
                "Project is outside this token's grants."
            );
        }
        Ok(())
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

pub(super) fn authenticate_owner(state: &AppState, headers: &HeaderMap) -> Result<Option<Owner>> {
    let clerk = super::clerk::ClerkConfig::from_env()?;
    if let Some(token) = bearer(headers) {
        let record: Option<(String, String, String, u64, Option<String>)> = store(state)?
            .query_row(
                "SELECT owner,scopes,projects,expires,audience FROM credentials WHERE hash=?1",
                [digest(token.as_bytes())],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        return record
            .filter(|r| {
                r.3 > now_ms()
                    && r.4.as_ref().is_none_or(|audience| {
                        audience == &format!("{}/mcp", super::oauth::origin(state, headers))
                    })
            })
            .map(|r| {
                if let Some(config) = &clerk {
                    let db = Connection::open(state.data_root.join("owners.sqlite"))?;
                    anyhow::ensure!(super::clerk::owner_allowed(&db, config, &r.0)?, "Creator is not admitted.");
                }
                Ok(Owner {
                    id: r.0,
                    display: None,
                    scopes: serde_json::from_str(&r.1)?,
                    projects: serde_json::from_str(&r.2)?,
                    session: false,
                })
            })
            .transpose();
    }
    let cookie = cookie_value(headers, owner_cookie_name(state));
    let user = if let Some(config) = &clerk {
        let db = Connection::open(state.data_root.join("owners.sqlite"))?;
        super::clerk::authenticate(&db, config, cookie.as_deref(), (now_ms() / 1000) as i64)?
    } else {
        AuthService::open(state.data_root.join("owners.sqlite"))?.authenticate(cookie.as_deref())?
    };
    Ok(user
        .map(|user| Owner {
            id: user.user_id,
            display: Some(if clerk.is_some() { "Google account".to_owned() } else { user.name }),
            scopes: vec![
                "read".into(),
                "write".into(),
                "publish".into(),
                "tokens".into(),
            ],
            projects: vec![],
            session: true,
        }))
}

fn owned_project(state: &AppState, owner: &Owner, id: &str, scope: &str) -> Result<(Value, u64)> {
    owner.require(scope, Some(id))?;
    let record: Option<(String, u64)> = store(state)?
        .query_row(
            "SELECT body,revision FROM projects WHERE id=?1 AND owner=?2",
            params![id, owner.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (body, revision) =
        record.ok_or_else(|| anyhow::anyhow!("Project not found or access denied."))?;
    Ok((serde_json::from_str(&body)?, revision))
}

fn validate_managed_sources(source: &Value, id: &str) -> Result<()> {
    let string = |value: &Value, max: usize| value.as_str().is_some_and(|s| s.len() <= max);
    let path = |value: &str| value.len() <= 240 && value.trim() == value
        && safe_relative_path(value) && !value.chars().any(|c| c.is_control());
    anyhow::ensure!(source["id"] == id && string(&source["name"], 80)
        && source["name"].as_str().is_some_and(|s| !s.trim().is_empty())
        && string(&source["slug"], 100) && source["updatedAt"].as_u64().is_some(), "Invalid project metadata.");
    let files = source["files"].as_object().ok_or_else(|| anyhow::anyhow!("Invalid project files."))?;
    anyhow::ensure!(files.len() <= 1_000 && files.iter().all(|(name, body)| path(name) && string(body, MAX_TRANSFER)), "Invalid project files.");
    let directories = source["directories"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid project directories."))?;
    anyhow::ensure!(directories.len() <= 1_000 && directories.iter().all(|entry| entry.as_str().is_some_and(path)), "Invalid project directories.");
    let model = &source["databaseModel"];
    anyhow::ensure!(model.is_object() && string(&model["source"], MAX_TRANSFER)
        && string(&model["acceptedSource"], MAX_TRANSFER) && string(&model["revision"], 256), "Invalid database model.");
    let manifest = &source["manifest"];
    let actions = manifest["runtime"]["database"]["actions"].as_array();
    anyhow::ensure!(manifest["version"] == 1 && ["public", "private"].contains(&manifest["visibility"].as_str().unwrap_or(""))
        && manifest["runtime"]["database"]["mode"] == "declared-actions"
        && ["optional", "required"].contains(&manifest["runtime"]["auth"]["mode"].as_str().unwrap_or(""))
        && manifest["runtime"]["files"]["access"] == "deployment"
        && actions.is_some_and(|list| list.len() <= 50 && list.iter().all(|action|
            string(&action["name"], 120) && ["query", "mutation"].contains(&action["kind"].as_str().unwrap_or(""))
            && ["anonymous", "authenticated", "granted"].contains(&action["access"].as_str().unwrap_or("")))), "Invalid project manifest.");
    let mut names = std::collections::HashSet::new();
    for action in actions.unwrap() {
        let name = action["name"].as_str().unwrap();
        let parts: Vec<_> = name.split('.').collect();
        anyhow::ensure!(parts.len() >= 2 && parts.iter().all(|part| {
            let mut chars = part.bytes();
            chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        }) && names.insert(name), "Invalid project action name.");
    }
    Ok(())
}

async fn owned_deployment(
    state: &AppState,
    owner: &Owner,
    id: &str,
    scope: &str,
) -> Result<Deployment> {
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .find(|d| d.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Deployment not found or access denied."))?;
    owned_project(state, owner, &deployment.project_id, scope)?;
    Ok(deployment)
}

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"openWorldHint":false}})
}

fn admin_tools(owner: &Owner) -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    let mut tools = vec![
        tool(
            "platform_describe",
            "Read Littlebox capabilities and local development instructions.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "session_inspect",
            "Read the authenticated owner's grants.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "platform_guide",
            "Read SDK examples, Server Action format, publishing, or local development instructions.",
            json!({"topic":{"type":"string","enum":["sdk","actions","publishing","development"]}}),
            &["topic"],
            true,
        ),
        tool(
            "projects_list",
            "List projects this credential can access.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "project_inspect",
            "Inspect a project and its deployed revision.",
            json!({"projectId":text}),
            &["projectId"],
            true,
        ),
        tool("project_sources_read", "Download the latest saved Managed project sources.", json!({"projectId":text}), &["projectId"], true),
        tool(
            "database_execute",
            "Administer a deployment database. Mutations require write permission.",
            json!({"deploymentId":text,"operation":{"type":"string","enum":["inspect","browse","sql","update-cell","insert-row","delete-row","list-backups","create-backup","restore-backup","apply-migration"]},"input":{"type":"object"}}),
            &["deploymentId", "operation"],
            false,
        ),
        tool(
            "database_export",
            "Snapshot a deployment database into a downloadable transfer.",
            json!({"deploymentId":text}),
            &["deploymentId"],
            true,
        ),
        tool(
            "project_export",
            "Export portable database, identities without sessions, and files into a transfer.",
            json!({"deploymentId":text}),
            &["deploymentId"],
            true,
        ),
        tool(
            "storage_test",
            "Check the configured artifact store.",
            json!({}),
            &[],
            true,
        ),
    ];
    if owner.scopes.iter().any(|s| s == "write") {
        tools.extend([
            tool("project_create", "Register an external project. Repeating the same project ID is idempotent.", json!({"projectId":text,"name":text,"slug":text,"manifest":{"type":"object"},"sourceAuthority":{"type":"string","enum":["managed","external"]}}), &["name"], false),
            tool("project_sources_save", "Save Managed project sources if the expected source revision still matches. Upload the project JSON first.", json!({"projectId":text,"transferId":text,"expectedRevision":{"type":"integer","minimum":0}}), &["projectId","transferId","expectedRevision"], false),
            tool("schema_plan", "Derive and validate SQL from two Drizzle models in an isolated subprocess.", json!({"acceptedSource":text,"candidateSource":text,"projectId":text}), &["acceptedSource","candidateSource"], false),
        ]);
    }
    tools.push(tool(
        "signup_policy_read",
        "Read who may create an account on a deployment.",
        json!({"deploymentId":text}),
        &["deploymentId"],
        true,
    ));
    if owner.scopes.iter().any(|s| s == "publish") {
        tools.extend([
            tool(
                "signup_policy_set",
                "Choose who may create an account on a deployment: open, allowlist (exact emails or @domain entries) or closed. Omitting allowlist keeps the current entries.",
                json!({"deploymentId":text,"mode":{"type":"string","enum":["open","allowlist","closed"]},"allowlist":{"type":"array","items":{"type":"string","minLength":1,"maxLength":254}}}),
                &["deploymentId", "mode"],
                false,
            ),
            tool(
                "release_plan",
                "Validate a completed artifact transfer and pin the current project revision.",
                json!({"transferId":text}),
                &["transferId"],
                false,
            ),
            tool(
                "release_apply",
                "Apply a current release plan. Repeating an applied plan returns its receipt.",
                json!({"planId":text}),
                &["planId"],
                false,
            ),
        ]);
    }
    if owner.session {
        tools.extend([
            tool("tokens_list", "List active delegated credentials without exposing their secrets.", json!({}), &[], true),
            tool("token_create", "Issue a revocable agent credential with selected scopes and project grants.", json!({"scopes":{"type":"array","items":{"type":"string","enum":["read","write","publish"]},"minItems":1},"projectIds":{"type":"array","items":text},"ttlSeconds":{"type":"integer","minimum":60,"maximum":2592000}}), &["scopes"], false),
            tool("token_revoke", "Revoke an agent credential using its returned ID.", json!({"tokenId":text}), &["tokenId"], false),
        ]);
    }
    tools.extend(transfer_tools(false));
    tools
}

fn runtime_tools() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "auth_session",
            "Read the current app user, or the anonymous state when no one is signed in.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "auth_logout",
            "Revoke the current app session.",
            json!({}),
            &[],
            false,
        ),
        tool(
            "actions_invoke",
            "Invoke one declared Server Action as the current app user. Without a session only actions declared anonymous run.",
            json!({"name":{"type":"string"},"input":{"type":"object"}}),
            &["name"],
            false,
        ),
        tool(
            "files_list",
            "List files visible to the current actor. Private files belong to the uploader. Public files are visible to signed-in users, and to visitors on a public deployment.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "files_read",
            "Open an immutable download transfer for a file this actor may read.",
            json!({"fileId":{"type":"string"}}),
            &["fileId"],
            true,
        ),
        tool(
            "files_upload",
            "Save a completed file transfer. The file is private to the signed-in uploader unless visibility is public.",
            json!({"transferId":{"type":"string"},"name":{"type":"string"},"type":{"type":"string"},"visibility":{"type":"string","enum":["private","public"]}}),
            &["transferId", "name"],
            false,
        ),
    ];
    tools.extend(transfer_tools(true));
    tools
}

/// Tools a visitor without a session may call on a public deployment.
/// `actions_invoke` still refuses every action not declared anonymous.
/// Uploads and the transfers that would create one stay signed-in only.
fn anonymous_runtime_tools() -> Vec<Value> {
    let allowed = [
        "auth_session",
        "actions_invoke",
        "files_list",
        "files_read",
        "transfer_read",
        "transfer_status",
        "transfer_cancel",
    ];
    runtime_tools()
        .into_iter()
        .filter(|tool| allowed.contains(&tool["name"].as_str().unwrap_or("")))
        .collect()
}

fn transfer_tools(runtime: bool) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "transfer_read",
            "Read one bounded base64 block from a completed download.",
            json!({"transferId":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
            &["transferId", "offset"],
            true,
        ),
        tool(
            "transfer_status",
            "Inspect transfer progress for resuming an upload.",
            json!({"transferId":{"type":"string"}}),
            &["transferId"],
            true,
        ),
        tool(
            "transfer_cancel",
            "Delete a transfer and its temporary bytes.",
            json!({"transferId":{"type":"string"}}),
            &["transferId"],
            false,
        ),
        tool(
            "transfer_begin",
            "Start a size-bounded upload with an expected SHA-256 checksum.",
            json!({"size":{"type":"integer","minimum":1,"maximum":if runtime {2*1024*1024} else {MAX_TRANSFER}},"sha256":{"type":"string","pattern":"^[a-f0-9]{64}$"},"projectId":{"type":"string"}}),
            &["size", "sha256"],
            false,
        ),
        tool(
            "transfer_write",
            "Append a base64 block at an exact byte offset. Identical retries are accepted.",
            json!({"transferId":{"type":"string"},"offset":{"type":"integer","minimum":0},"data":{"type":"string","maxLength":CHUNK.div_ceil(3)*4}}),
            &["transferId", "offset", "data"],
            false,
        ),
        tool(
            "transfer_complete",
            "Verify upload size and SHA-256 and make the bytes immutable.",
            json!({"transferId":{"type":"string"}}),
            &["transferId"],
            false,
        ),
    ];
    if runtime {
        for t in &mut tools {
            if t["name"] == "transfer_begin" {
                t["inputSchema"]["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("projectId");
            }
        }
    }
    tools
}

async fn admin(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, false).await
}
async fn runtime(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, true).await
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}

async fn handle(state: AppState, request: Request, runtime: bool) -> Response {
    let headers = request.headers().clone();
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    if !same_origin(&headers) || !super::oauth::valid_host(&state, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "Cross-origin or unrecognized-host MCP request rejected.",
        );
    }
    let owner;
    let principal;
    let tools;
    let mut app: Option<String> = None;
    let mut anonymous = false;
    if runtime {
        let Some(deployment) = deployment_for(&state, &headers).await else {
            return error(StatusCode::NOT_FOUND, "Deployment not found.");
        };
        match authenticate_deployment(&state, &deployment, &headers) {
            Ok(Some(user)) => {
                principal = format!("runtime:{}:{}", deployment.id, user.user_id);
                tools = runtime_tools();
            }
            // A visitor without a session on a public app may read the anonymous
            // session, run actions declared anonymous, and read public files.
            // The transfer principal includes the client address so visitors
            // do not share one quota. Private apps require sign-in for everything.
            Ok(None) if !is_private(&deployment) => {
                let client = limits::client_key(state.guards.client_ip(peer, &headers));
                principal = format!("runtime:{}:anon:{client}", deployment.id);
                tools = anonymous_runtime_tools();
                anonymous = true;
            }
            _ => return unauthorized(&state, &headers),
        }
        app = Some(deployment.id);
        owner = None;
    } else {
        if !super::oauth::valid_owner_host(&state, &headers).await {
            return error(
                StatusCode::FORBIDDEN,
                "Administration is unavailable on app hosts.",
            );
        }
        let authenticated = match authenticate_owner(&state, &headers) {
            Ok(Some(owner)) => owner,
            _ => return unauthorized(&state, &headers),
        };
        if headers.get("x-littlebox-owner-id").is_some_and(|expected| expected.as_bytes() != authenticated.id.as_bytes()) {
            return error(StatusCode::CONFLICT, "The signed-in owner changed. Reload before continuing.");
        }
        principal = format!("owner:{}", authenticated.id);
        tools = admin_tools(&authenticated);
        owner = Some(authenticated);
    }
    let bytes = match axum::body::to_bytes(request.into_body(), 200_000).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "MCP request exceeds 200 KB."),
    };
    let call: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return rpc_error(Value::Null, -32700, "Invalid JSON."),
    };
    let id = call.get("id").cloned().unwrap_or(Value::Null);
    if !call.is_object()
        || call["jsonrpc"] != "2.0"
        || !call["method"].is_string()
        || call
            .get("id")
            .is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
    {
        return rpc_error(Value::Null, -32600, "Invalid JSON-RPC request.");
    }
    let method = call["method"].as_str().unwrap();
    if call.get("id").is_none() {
        return if method.starts_with("notifications/") {
            StatusCode::ACCEPTED.into_response()
        } else {
            rpc_error(Value::Null, -32600, "A request ID is required.")
        };
    }
    let mut clear_cookie = false;
    let result = match method {
        "initialize" => Ok(
            json!({"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"littlebox","version":"0.1.0"},"instructions":"Use project-scoped credentials. Source and builds stay on your computer. Use platform_describe for the workflow."}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":tools})),
        "tools/call" => {
            let Some(name) = call["params"]["name"].as_str() else {
                return rpc_error(id, -32602, "Tool name is required.");
            };
            let Some(definition) = tools.iter().find(|t| t["name"] == name) else {
                // Tell a signed-out visitor the tool needs a session, so the SDK can
                // send them to the login page.
                if runtime && runtime_tools().iter().any(|t| t["name"] == name) {
                    return Json(json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"authentication-required"}],"isError":true}})).into_response();
                }
                return rpc_error(id, -32602, "Unknown tool or insufficient permission.");
            };
            let arguments = call["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            if let Err(e) = validate_arguments(&arguments, &definition["inputSchema"]) {
                return rpc_error(id, -32602, &e.to_string());
            }
            if runtime && name == "actions_invoke" {
                // Signed-out visitors share a bucket per client address and
                // Deployment; each signed-in app user has their own.
                let limited = if anonymous {
                    let client = limits::client_key(state.guards.client_ip(peer, &headers));
                    let key = format!("{}|{client}", app.as_deref().unwrap_or(""));
                    state.guards.anonymous_actions.check(&key)
                } else {
                    state.guards.signed_in_actions.check(&principal)
                };
                if let Err(wait) = limited {
                    let mut response = Json(json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"rate-limited"}],"isError":true}})).into_response();
                    response.headers_mut().insert(
                        header::RETRY_AFTER,
                        HeaderValue::from(wait.as_secs().max(1)),
                    );
                    return response;
                }
            }
            let previous_mode = if name == "signup_policy_set" {
                let deployments = state.deployments.read().await;
                deployments
                    .values()
                    .find(|d| Some(d.id.as_str()) == arguments["deploymentId"].as_str())
                    .map(|d| d.signup_policy.mode)
            } else {
                None
            };
            let result = if name.starts_with("transfer_") {
                let permission = if let Some(owner) = owner.as_ref() {
                    let project = if name == "transfer_begin" {
                        arguments["projectId"].as_str().map(str::to_owned)
                    } else {
                        transfer(&state, &principal, &arguments)
                            .ok()
                            .and_then(|t| t["projectId"].as_str().map(str::to_owned))
                    };
                    let scope = if ["transfer_read", "transfer_status", "transfer_cancel"]
                        .contains(&name)
                    {
                        "read"
                    } else {
                        "publish"
                    };
                    let authorize = |scope| match project.as_deref() {
                        Some(id) => owned_project(&state, owner, id, scope).map(|_| ()),
                        None => owner.require(scope, None),
                    };
                    // Check the first accepted scope the credential holds, so a
                    // refusal names the project grant rather than a scope it never had.
                    let held = |accepted: &[&'static str]| {
                        accepted
                            .iter()
                            .copied()
                            .find(|wanted| owner.scopes.iter().any(|s| s == wanted))
                            .unwrap_or(accepted[0])
                    };
                    if name == "transfer_cancel" {
                        authorize(held(&["read", "publish", "write"]))
                    } else if scope == "publish" {
                        authorize(held(&["publish", "write"]))
                    } else {
                        authorize(scope)
                    }
                } else {
                    Ok(())
                };
                match permission {
                    Ok(()) => transfer_call(&state, &principal, name, &arguments, runtime).await,
                    Err(e) => Err(e),
                }
            } else if let Some(owner) = owner.as_ref() {
                admin_call(&state, owner, &principal, name, arguments.clone()).await
            } else {
                runtime_call(&state, &headers, &principal, app.as_deref(), name, arguments.clone()).await
            };
            // Each audit row costs a write to the shared mcp.sqlite, so app users
            // must not decide how fast it grows. Owner calls are audited here.
            // Of the app-user calls, only an upload that stores a new file is,
            // inside runtime_call: the transfer caps bound how often that happens.
            if !runtime {
                audit(&state, &principal, name, &arguments, app.as_deref(), previous_mode, &result);
            }
            // The session row is gone; the browser must drop its cookie too.
            if runtime && name == "auth_logout" && result.is_ok() {
                clear_cookie = true;
            }
            match result {
                Ok(value) => Ok(
                    json!({"content":[{"type":"text","text":serde_json::to_string(&value).unwrap()}],"structuredContent":{"result":value},"isError":false}),
                ),
                Err(e) => {
                    Ok(json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}))
                }
            }
        }
        _ => return rpc_error(id, -32601, "Method not found."),
    };
    let mut response = match result {
        Ok::<Value, anyhow::Error>(result) => {
            Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
        }
        Err(e) => rpc_error(id, -32603, &e.to_string()),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if clear_cookie {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, super::cleared_app_cookie(&state));
    }
    response
}

/// Records one tool call. The target is the project or Deployment the call
/// acted on; `detail` adds what the arguments alone do not show.
fn audit(
    state: &AppState,
    principal: &str,
    name: &str,
    arguments: &Value,
    app: Option<&str>,
    previous_mode: Option<SignupMode>,
    result: &Result<Value>,
) {
    let mode = |value: &Value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    let (target, detail): (Option<String>, Option<String>) = match name {
        _ if app.is_some() => (app.map(str::to_owned), None),
        "release_apply" => {
            let project = store(state).ok().and_then(|db| {
                db.query_row(
                    "SELECT project FROM plans WHERE id=?1",
                    [arguments["planId"].as_str().unwrap_or("")],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            });
            let deployment = result
                .as_ref()
                .ok()
                .and_then(|receipt| receipt["id"].as_str())
                .map(|id| format!("deployment {id}"));
            (project, deployment)
        }
        "signup_policy_set" => {
            let before = previous_mode.map(|m| mode(&json!(m))).unwrap_or_else(|| "unknown".into());
            let after = match result {
                Ok(value) => mode(&value["mode"]),
                Err(_) => format!("{} (refused)", mode(&arguments["mode"])),
            };
            (
                arguments["deploymentId"].as_str().map(str::to_owned),
                Some(format!("{before} to {after}")),
            )
        }
        _ => (
            arguments["projectId"]
                .as_str()
                .or(arguments["deploymentId"].as_str())
                .map(str::to_owned),
            None,
        ),
    };
    let written = store(state).and_then(|db| {
        let text = |value: &str| value.chars().take(AUDIT_TEXT_CHARACTERS).collect::<String>();
        db.execute(
            "INSERT INTO audit(at,actor,tool,target,ok,detail) VALUES(?1,?2,?3,?4,?5,?6)",
            params![now_ms(), text(principal), text(name), target.as_deref().map(text), result.is_ok(), detail.as_deref().map(text)],
        )?;
        let id = db.last_insert_rowid();
        if id % AUDIT_PRUNE_INTERVAL == 0 {
            db.execute(
                "DELETE FROM audit WHERE id<=?1 OR at<?2",
                params![id.saturating_sub(AUDIT_MAX_ROWS), now_ms().saturating_sub(AUDIT_RETENTION_MS)],
            )?;
        }
        Ok(())
    });
    if let Err(error_value) = written {
        eprintln!("Could not write the audit row for {principal} {name}: {error_value}");
    }
}

fn unauthorized(state: &AppState, headers: &HeaderMap) -> Response {
    let mut response = error(StatusCode::UNAUTHORIZED, "Sign in to use Littlebox MCP.");
    let metadata = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        super::oauth::origin(state, headers)
    );
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&metadata).unwrap(),
    );
    response
}

fn validate_arguments(value: &Value, schema: &Value) -> Result<()> {
    match schema["type"].as_str() {
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Arguments must be an object."))?;
            if let Some(required) = schema["required"].as_array() {
                for key in required {
                    anyhow::ensure!(
                        object.contains_key(key.as_str().unwrap()),
                        "Missing argument {}.",
                        key
                    );
                }
            }
            if let Some(properties) = schema["properties"].as_object() {
                for (key, value) in object {
                    if let Some(property) = properties.get(key) {
                        validate_arguments(value, property)?;
                    } else {
                        anyhow::ensure!(
                            schema["additionalProperties"] != false,
                            "Unknown argument {key}."
                        );
                    }
                }
            }
        }
        Some("string") => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Expected a string."))?;
            anyhow::ensure!(
                s.len() >= schema["minLength"].as_u64().unwrap_or(0) as usize
                    && s.len() <= schema["maxLength"].as_u64().unwrap_or(120_000) as usize,
                "String length is outside the allowed range."
            );
        }
        Some("integer") => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Expected a nonnegative integer."))?;
            anyhow::ensure!(
                n >= schema["minimum"].as_u64().unwrap_or(0)
                    && n <= schema["maximum"].as_u64().unwrap_or(u64::MAX),
                "Integer is outside the allowed range."
            );
        }
        Some("array") => {
            let a = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Expected an array."))?;
            anyhow::ensure!(
                a.len() >= schema["minItems"].as_u64().unwrap_or(0) as usize && a.len() <= 100,
                "Invalid array length."
            );
            for v in a {
                validate_arguments(v, &schema["items"])?;
            }
        }
        _ => {}
    }
    if let Some(allowed) = schema["enum"].as_array() {
        anyhow::ensure!(
            allowed.contains(value),
            "Value is outside the allowed choices."
        );
    }
    Ok(())
}

fn internal_request(state: &AppState, body: Value) -> Request {
    Request::builder()
        .header("x-littlebox-admin-key", &state.admin_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_value(response: Response) -> Result<Value> {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_TRANSFER).await?;
    let value: Value = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        status.is_success(),
        "{}",
        value["error"]
            .as_str()
            .unwrap_or("Kernel operation failed.")
    );
    Ok(value["result"].clone())
}

async fn admin_call(
    state: &AppState,
    owner: &Owner,
    principal: &str,
    name: &str,
    a: Value,
) -> Result<Value> {
    match name {
        "platform_guide" => Ok(match a["topic"].as_str().unwrap() {
            "sdk" => {
                json!({"global":"miniapp","examples":["const user = await miniapp.auth.session();","await miniapp.auth.signIn();","const rows = await miniapp.actions.call('items.list');","const files = await miniapp.files.list();","await miniapp.files.upload(file);","await miniapp.files.upload(file, { visibility: 'public' });"],"authentication":"auth.session returns an anonymous state when no one is signed in, so the app can show a sign-in button. On a public app a visitor without a session may call actions declared anonymous, and may list and read files uploaded as public. Uploads, private files, and authenticated actions require sign-in. Private apps require sign-in for everything. The owner chooses who may create an account with signup_policy_set.","files":"File URLs are local Blob URLs created after an MCP download. There is no HTTP download path. An upload is private to that app user unless visibility is public. Another account cannot read a private file. On a public app, visitors can list and read only the public files."})
            }
            "actions" => {
                json!({"filename":"actions.json","format":{"version":1,"actions":{"items.list":{"kind":"query","access":"authenticated","statement":"SELECT id,title FROM items WHERE user_id=?1 LIMIT 50","bindings":[{"source":"actor","field":"id"}]},"items.create":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO items(title,user_id) VALUES(?1,?2)","bindings":[{"source":"input","key":"title","type":"string","maxLength":200},{"source":"actor","field":"id"}]}}},"rules":"SQL and typed bindings stay server-side. Declare schema in schema.sql for artifact publication. Runtime calls only send the declared action name and input. Add nullable:true to an input binding to bind omitted values or explicit null as SQL NULL; supplied strings remain non-empty and all supplied values retain their type checks. Mutations return changes and lastInsertId, so read rows with a query instead of mutation RETURNING, which publication rejects. A query that reads rows needs a top-level LIMIT. A total with count, sum, avg, min or max and no GROUP BY does not."})
            }
            "publishing" => {
                json!({"steps":["Create project identity with project_create. Keep its id, slug and manifest locally.","Build static HTML/CSS/JS locally. Inject the SDK using the local builder.","Upload JSON {project:{id,name,slug,files,manifest},visibility,artifact:{version:1,entrypoint:'index.html',files:{...}}} using transfer_begin with projectId, byte size and sha256, then transfer_write and transfer_complete.","Call release_plan with transferId, inspect its planId and expectedRevision, then call release_apply with planId.","Save the receipt and cancel the temporary transfer."],"concurrency":"An obsolete plan is rejected. Repeating an applied plan returns the same receipt."})
            }
            _ => {
                json!({"commands":["bun run littlebox -- init --dir /tmp/my-app --name 'My app' --url http://localhost:4173","bun run littlebox -- dev --dir /tmp/my-app","bun run littlebox -- build --dir /tmp/my-app","bun run littlebox -- publish --dir /tmp/my-app"],"credentials":"Set LITTLEBOX_MCP_TOKEN for init and publish, or use littlebox login. Never put it in app source.","isolation":"dev uses the same Rust Kernel with separate local identities, data and files under .littlebox/dev-data. It does not proxy production.","sourceAuthority":"Code and builds stay in your folder. Source is not synchronized to the browser editor."})
            }
        }),
        "platform_describe" => Ok(
            json!({"name":"Littlebox","protocolVersion":VERSION,"sourceAuthority":"external","runtimeEndpoint":"/_littlebox/mcp","chunkBytes":CHUNK,"maxTransferBytes":MAX_TRANSFER,"workflow":["project_create","build locally","transfer_begin/write/complete","release_plan","release_apply"],"runtimeTools":["auth_session","actions_invoke","files_list","files_read","files_upload"],"localDevelopment":"Use bun run littlebox -- init, build, dev, or publish. Development uses an isolated local Kernel. All platform operations use authenticated MCP. On a public app, actions declared anonymous run without a session, and visitors can list and read files uploaded as public. Uploads and private files require sign-in. Private apps require sign-in for everything."}),
        ),
        "session_inspect" => Ok(
            json!({"ownerId":owner.id,"displayName":owner.display,"scopes":owner.scopes,"projectIds":owner.projects,"browserSession":owner.session}),
        ),
        "projects_list" => {
            owner.require("read", None).or_else(|e| {
                if owner.scopes.iter().any(|s| s == "read") {
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            let db = store(state)?;
            let mut q =
                db.prepare("SELECT id,body,revision FROM projects WHERE owner=?1 ORDER BY id")?;
            let rows = q
                .query_map([&owner.id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let projects = rows
                .into_iter()
                .filter(|r| owner.projects.is_empty() || owner.projects.contains(&r.0))
                .map(|r| Ok(json!({"project":serde_json::from_str::<Value>(&r.1)?,"revision":r.2})))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"projects":projects}))
        }
        "project_create" => {
            let id = a["projectId"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("app_{}", Uuid::new_v4()));
            owner.require("write", Some(&id))?;
            anyhow::ensure!(
                id.len() <= 100
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "Invalid project ID."
            );
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let db = store(state)?;
            let existing: Option<String> = db
                .query_row("SELECT owner FROM projects WHERE id=?1", [&id], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(existing) = existing {
                anyhow::ensure!(existing == owner.id, "Project not found or access denied.");
                return Ok(owned_project(state, owner, &id, "read")?.0);
            }
            anyhow::ensure!(
                !state
                    .deployments
                    .read()
                    .await
                    .values()
                    .any(|d| d.project_id == id),
                "Existing deployments must be claimed by their original operator."
            );
            let project = json!({"id":id,"name":a["name"],"slug":a["slug"].as_str().map(str::to_owned).unwrap_or_else(||safe_slug(a["name"].as_str().unwrap())),"sourceAuthority":a["sourceAuthority"].as_str().unwrap_or("external"),"manifest":a.get("manifest").cloned().unwrap_or(json!({"version":1,"visibility":"private"})),"files":starter_files()});
            db.execute(
                "INSERT INTO projects(id,owner,body) VALUES(?1,?2,?3)",
                params![id, owner.id, {
                    let mut metadata = project.clone();
                    metadata.as_object_mut().unwrap().remove("files");
                    metadata.to_string()
                }],
            )?;
            Ok(project)
        }
        "project_inspect" => {
            let id = a["projectId"].as_str().unwrap();
            let (project, revision) = owned_project(state, owner, id, "read")?;
            let deployment = find_project_deployment(&*state.deployments.read().await, id);
            Ok(json!({"project":project,"revision":revision,"deployment":deployment}))
        }
        "project_sources_read" => {
            let id = a["projectId"].as_str().unwrap();
            let (project, _) = owned_project(state, owner, id, "read")?;
            anyhow::ensure!(project["sourceAuthority"] == "managed", "Project sources are maintained externally.");
            let row: Option<(Vec<u8>, u64)> = store(state)?.query_row(
                "SELECT body,revision FROM managed_sources WHERE project=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            match row {
                Some((bytes, revision)) => Ok(json!({"revision":revision,"transfer":download(state, principal, &bytes, Some(id)).await?})),
                None => Ok(json!({"revision":0,"transfer":null})),
            }
        }
        "project_sources_save" => {
            let id = a["projectId"].as_str().unwrap();
            let transfer_id = a["transferId"].as_str().unwrap();
            let upload = transfer(state, principal, &json!({"transferId":transfer_id}))?;
            anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == id, "Completed upload for this project required.");
            let bytes = tokio::fs::read(transfer_path(state, transfer_id)?).await?;
            let source: Value = serde_json::from_slice(&bytes)?;
            validate_managed_sources(&source, id)?;
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let (project, _) = owned_project(state, owner, id, "write")?;
            anyhow::ensure!(project["sourceAuthority"] == "managed", "Project sources are maintained externally.");
            let mut db = store(state)?;
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let current: u64 = tx.query_row("SELECT revision FROM managed_sources WHERE project=?1", [id], |r| r.get(0)).optional()?.unwrap_or(0);
            anyhow::ensure!(current == a["expectedRevision"].as_u64().unwrap(), "Source revision conflict. Reload the saved project before writing.");
            let revision = current.checked_add(1).ok_or_else(|| anyhow::anyhow!("Source revision exhausted."))?;
            tx.execute("INSERT INTO managed_sources(project,body,revision) VALUES(?1,?2,?3) ON CONFLICT(project) DO UPDATE SET body=excluded.body,revision=excluded.revision", params![id,bytes,revision])?;
            tx.commit()?;
            Ok(json!({"revision":revision}))
        }
        "tokens_list" => {
            anyhow::ensure!(
                owner.session,
                "Only owner browser sessions may list credentials."
            );
            let db = store(state)?;
            let mut query=db.prepare("SELECT hash,scopes,projects,expires FROM credentials WHERE owner=?1 AND expires>?2 ORDER BY expires DESC")?;
            let credentials=query.query_map(params![owner.id,now_ms()],|r|Ok(json!({"tokenId":r.get::<_,String>(0)?,"scopes":r.get::<_,String>(1)?,"projectIds":r.get::<_,String>(2)?,"expiresAt":r.get::<_,u64>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"credentials":credentials}))
        }
        "token_create" => {
            anyhow::ensure!(
                owner.session,
                "Only owner browser sessions may delegate credentials."
            );
            let projects: Vec<String> =
                serde_json::from_value(a.get("projectIds").cloned().unwrap_or(json!([])))?;
            for id in &projects {
                owned_project(state, owner, id, "read")?;
            }
            let token = format!("lb_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            let hash = digest(token.as_bytes());
            let ttl = a["ttlSeconds"].as_u64().unwrap_or(86400);
            let ttl = if super::clerk::ClerkConfig::from_env()?.is_some() { ttl.min(3600) } else { ttl };
            let expires = now_ms() + ttl * 1000;
            store(state)?.execute("INSERT INTO credentials(hash,owner,scopes,projects,expires) VALUES(?1,?2,?3,?4,?5)",params![hash,owner.id,a["scopes"].to_string(),serde_json::to_string(&projects)?,expires])?;
            Ok(json!({"token":token,"tokenId":hash,"expiresAt":expires}))
        }
        "token_revoke" => {
            anyhow::ensure!(
                owner.session,
                "Only owner browser sessions may revoke credentials."
            );
            store(state)?.execute(
                "DELETE FROM credentials WHERE hash=?1 AND owner=?2",
                params![a["tokenId"].as_str().unwrap(), owner.id],
            )?;
            Ok(json!({"ok":true}))
        }
        "signup_policy_read" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "read").await?;
            Ok(signup_policy_value(&deployment))
        }
        "signup_policy_set" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "publish").await?;
            let mode: SignupMode = serde_json::from_value(a["mode"].clone())?;
            let entries: Vec<String> = match a.get("allowlist") {
                Some(list) => serde_json::from_value(list.clone())?,
                None => deployment.signup_policy.allowlist.clone(),
            };
            let policy = SignupPolicy::from_input(mode, &entries).map_err(anyhow::Error::msg)?;
            // Releases hold the same lock, so a publish cannot overwrite this change
            // with the policy it read before.
            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            let mut updated = owned_deployment(state, owner, id, "publish").await?;
            let previous = updated.signup_policy.mode;
            updated.signup_policy = policy;
            persist_deployment(state, &updated).await?;
            state
                .deployments
                .write()
                .await
                .insert(updated.hostname.clone(), updated.clone());
            let mut value = signup_policy_value(&updated);
            value["previousMode"] = json!(previous);
            Ok(value)
        }
        "storage_test" => {
            owner.require("read", None)?;
            response_value(
                test_storage(State(state.clone()), internal_request(state, json!({}))).await,
            )
            .await
        }
        "database_execute" => {
            let id = a["deploymentId"].as_str().unwrap();
            let op = a["operation"].as_str().unwrap();
            let scope = if ["inspect", "browse", "list-backups"].contains(&op) {
                "read"
            } else {
                "write"
            };
            owned_deployment(state, owner, id, scope).await?;
            let mut input = a.get("input").cloned().unwrap_or(json!({}));
            anyhow::ensure!(input.is_object(), "Input must be an object.");
            input["operation"] = json!(op);
            response_value(
                admin_database(
                    State(state.clone()),
                    AxumPath(id.to_owned()),
                    internal_request(state, input),
                )
                .await,
            )
            .await
        }
        "database_export" | "project_export" => {
            let id = a["deploymentId"].as_str().unwrap();
            let d = owned_deployment(state, owner, id, "read").await?;
            let response = if name == "database_export" {
                export_database(
                    State(state.clone()),
                    AxumPath(id.to_owned()),
                    internal_request(state, json!({})),
                )
                .await
            } else {
                export_portable_state(
                    State(state.clone()),
                    AxumPath(id.to_owned()),
                    internal_request(state, json!({})),
                )
                .await
            };
            anyhow::ensure!(
                response.status().is_success(),
                "Could not export deployment."
            );
            let bytes = axum::body::to_bytes(response.into_body(), MAX_TRANSFER).await?;
            download(state, principal, &bytes, Some(&d.project_id)).await
        }
        "schema_plan" => {
            if let Some(id) = a["projectId"].as_str() {
                owned_project(state, owner, id, "write")?;
            } else {
                owner.require("write", None)?;
            }
            schema_plan(a).await
        }
        "release_plan" => {
            let transfer = transfer(state, principal, &json!({"transferId":a["transferId"]}))?;
            anyhow::ensure!(
                transfer["complete"] == true && transfer["direction"] == "upload",
                "Upload is not complete."
            );
            let bytes =
                tokio::fs::read(transfer_path(state, a["transferId"].as_str().unwrap())?).await?;
            anyhow::ensure!(
                bytes.len() <= 26_000_000,
                "Deployment payload exceeds 26 MB."
            );
            let payload: Value = serde_json::from_slice(&bytes)?;
            let input: DeployInput = serde_json::from_value(payload.clone())?;
            let (_, revision) = owned_project(state, owner, &input.project.id, "publish")?;
            anyhow::ensure!(
                transfer["projectId"] == input.project.id,
                "Artifact belongs to another project."
            );
            validate_release(state, &input).await?;
            let id = Uuid::new_v4().to_string();
            let expires = now_ms() + 900_000;
            store(state)?.execute("INSERT INTO plans(id,owner,project,revision,transfer,expires) VALUES(?1,?2,?3,?4,?5,?6)",params![id,owner.id,input.project.id,revision,a["transferId"].as_str().unwrap(),expires])?;
            Ok(
                json!({"planId":id,"projectId":input.project.id,"expectedRevision":revision,"artifactSha256":transfer["sha256"],"visibility":input.visibility,"expiresAt":expires}),
            )
        }
        "release_apply" => {
            let id = a["planId"].as_str().unwrap();
            let row: (String,u64,String,u64,Option<String>)=store(state)?.query_row("SELECT project,revision,transfer,expires,receipt FROM plans WHERE id=?1 AND owner=?2",params![id,owner.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            let _guard = database_lock(state, &format!("project:{}", row.0)).await;
            let (mut metadata, revision) = owned_project(state, owner, &row.0, "publish")?;
            let receipt: Option<String> =
                store(state)?
                    .query_row("SELECT receipt FROM plans WHERE id=?1", [id], |r| r.get(0))?;
            if let Some(receipt) = receipt {
                return Ok(serde_json::from_str(&receipt)?);
            }
            anyhow::ensure!(row.3 > now_ms(), "Release plan expired.");
            anyhow::ensure!(
                revision == row.1,
                "Project revision changed. Create a new release plan."
            );
            transfer(state, principal, &json!({"transferId":row.2}))?;
            let bytes = tokio::fs::read(transfer_path(state, &row.2)?).await?;
            let payload: Value = serde_json::from_slice(&bytes)?;
            for key in ["name", "slug", "manifest"] {
                if let Some(value) = payload["project"].get(key) {
                    metadata[key] = value.clone();
                }
            }
            let receipt = response_value(
                deploy(State(state.clone()), internal_request(state, payload)).await,
            )
            .await?;
            let mut db = store(state)?;
            let tx = db.transaction()?;
            tx.execute("UPDATE projects SET revision=revision+1,body=?4 WHERE id=?1 AND owner=?2 AND revision=?3",params![row.0,owner.id,revision,metadata.to_string()])?;
            tx.execute(
                "UPDATE plans SET receipt=?1 WHERE id=?2",
                params![receipt.to_string(), id],
            )?;
            tx.commit()?;
            Ok(receipt)
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

fn signup_policy_value(deployment: &Deployment) -> Value {
    json!({"deploymentId":deployment.id,"visibility":deployment.manifest.get("visibility").cloned().unwrap_or(json!("public")),"mode":deployment.signup_policy.mode,"allowlist":deployment.signup_policy.allowlist})
}

fn starter_files() -> Value {
    json!({"index.html":"<main><h1>My app</h1><button id=login>Sign in</button><pre id=result></pre></main>","styles.css":"body { font: 16px system-ui; max-width: 48rem; margin: 3rem auto; padding: 1rem; }","app.js":"document.querySelector('#login').onclick = () => miniapp.auth.signIn();\nminiapp.auth.session().then(user => document.querySelector('#result').textContent = user.name);\n","schema.sql":"CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);","actions.json":"{\"version\":1,\"actions\":{\"items.list\":{\"kind\":\"query\",\"access\":\"authenticated\",\"statement\":\"SELECT id,title FROM items WHERE user_id = ?1 LIMIT 50\",\"bindings\":[{\"source\":\"actor\",\"field\":\"id\"}]}}}"})
}

async fn validate_release(state: &AppState, input: &DeployInput) -> Result<()> {
    anyhow::ensure!(
        input.artifact.version == 1
            && input.artifact.entrypoint == "index.html"
            && input.artifact.files.contains_key("index.html"),
        "Invalid Build Artifact."
    );
    anyhow::ensure!(
        ["public", "private"].contains(&input.visibility.as_str()),
        "Invalid visibility."
    );
    anyhow::ensure!(
        input.artifact.files.len() <= 1000,
        "Artifact has too many files."
    );
    let mut total = 0;
    for (path, content) in &input.artifact.files {
        anyhow::ensure!(
            safe_relative_path(path)
                && !path.starts_with("_littlebox/")
                && !path.starts_with("oauth/")
                && !path.starts_with(".well-known/")
                && path != "mcp",
            "Invalid or reserved artifact path."
        );
        total += content.len();
    }
    anyhow::ensure!(total <= 16 * 1024 * 1024, "Artifact exceeds 16 MiB.");
    let program: ActionProgram = serde_json::from_str(
        input
            .project
            .files
            .get("actions.json")
            .ok_or_else(|| anyhow::anyhow!("actions.json is required."))?,
    )?;
    anyhow::ensure!(program.version == 1, "Unsupported action version.");
    anyhow::ensure!(
        program.actions.len() <= 100,
        "A project may declare at most 100 actions."
    );
    for (name, action) in &program.actions {
        validate_action(name, action).map_err(anyhow::Error::msg)?;
    }
    let _guard = database_lock(state, &format!("release:{}", input.project.id)).await;
    let existing = find_project_deployment(&*state.deployments.read().await, &input.project.id);
    anyhow::ensure!(
        existing.is_none() || (input.initial_database.is_none()
            && input.initial_identity.is_none()
            && input.initial_files.is_none()
            && input.initial_signup_policy.is_none()),
        "Portable state can only be imported on the first deployment."
    );
    if let Some(policy) = input.initial_signup_policy.as_ref() {
        SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?;
    }
    let temp = state
        .data_root
        .join(format!("validate-{}.sqlite", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        if let Some(ref deployment) = existing {
            let connection = Connection::open(database_path(state, &deployment.id))?;
            connection.execute("VACUUM INTO ?1", [temp.to_string_lossy().as_ref()])?;
        } else if let Some(snapshot) = input.initial_database.as_deref() {
            initialize_database_from_base64(&temp, snapshot, 16 * 1024 * 1024)?;
        } else {
            initialize_database(&temp)?;
        }
        apply_additive_schema(
            &temp,
            input
                .project
                .files
                .get("schema.sql")
                .map(String::as_str)
                .unwrap_or(""),
            &program,
        )
    })();
    let _ = tokio::fs::remove_file(temp).await;
    result
}

async fn runtime_call(
    state: &AppState,
    headers: &HeaderMap,
    principal: &str,
    app: Option<&str>,
    name: &str,
    a: Value,
) -> Result<Value> {
    match name {
        "auth_session" => {
            response_value(session(State(state.clone()), headers.clone()).await).await
        }
        "auth_logout" => response_value(logout(State(state.clone()), headers.clone()).await).await,
        "actions_invoke" => {
            response_value(
                invoke_action(
                    State(state.clone()),
                    AxumPath(a["name"].as_str().unwrap().to_owned()),
                    headers.clone(),
                    Json(a.get("input").cloned().unwrap_or(json!({}))),
                )
                .await,
            )
            .await
        }
        "files_list" => {
            let mut files =
                response_value(list_files(State(state.clone()), headers.clone()).await).await?;
            if let Some(files) = files.as_array_mut() {
                for file in files {
                    file.as_object_mut().unwrap().remove("url");
                }
            }
            Ok(files)
        }
        "files_read" => {
            let response = serve_file(
                State(state.clone()),
                AxumPath(a["fileId"].as_str().unwrap().to_owned()),
                headers.clone(),
            )
            .await;
            anyhow::ensure!(response.status().is_success(), "File not found.");
            let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
            download(state, principal, &bytes, None).await
        }
        "files_upload" => {
            let visibility = match a.get("visibility").and_then(Value::as_str) {
                None | Some("private") => "private",
                Some("public") => "public",
                Some(_) => anyhow::bail!("File visibility must be private or public."),
            };
            let id = a["transferId"].as_str().unwrap();
            // Validate before locking: the lock name comes from the caller.
            transfer_path(state, id)?;
            let _guard = database_lock(state, &format!("transfer:{id}")).await;
            let request_hash = digest(a.to_string().as_bytes());
            let previous: Option<(String,String)>=store(state)?.query_row("SELECT request_hash,body FROM file_receipts WHERE transfer_id=?1 AND principal=?2",params![id,principal],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((hash, body)) = previous {
                anyhow::ensure!(hash == request_hash, "Upload retry changed its metadata.");
                return Ok(serde_json::from_str(&body)?);
            }
            let t = transfer(state, principal, &a)?;
            anyhow::ensure!(
                t["complete"] == true && t["direction"] == "upload",
                "File upload is incomplete."
            );
            let bytes =
                tokio::fs::read(transfer_path(state, a["transferId"].as_str().unwrap())?).await?;
            let body = json!({"name":a["name"],"type":a.get("type").cloned().unwrap_or(json!("application/octet-stream")),"dataUrl":encode_base64(&bytes),"visibility":visibility});
            let request = Request::builder().body(Body::from(body.to_string()))?;
            let mut result =
                response_value(upload_file(State(state.clone()), headers.clone(), request).await)
                    .await?;
            result.as_object_mut().unwrap().remove("url");
            store(state)?.execute("INSERT INTO file_receipts(transfer_id,principal,request_hash,body) VALUES(?1,?2,?3,?4)",params![id,principal,request_hash,result.to_string()])?;
            // Failed uploads and receipt replays leave no row.
            let result = Ok(result);
            audit(state, principal, name, &a, app, None, &result);
            result
        }
        _ => anyhow::bail!("Unknown runtime tool."),
    }
}

fn transfer_path(state: &AppState, id: &str) -> Result<PathBuf> {
    anyhow::ensure!(Uuid::parse_str(id).is_ok(), "Invalid transfer ID.");
    Ok(state.data_root.join("transfers").join(id))
}

fn transfer(state: &AppState, principal: &str, a: &Value) -> Result<Value> {
    let id = a["transferId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Transfer ID required."))?;
    transfer_path(state, id)?;
    let body: Option<String> = store(state)?
        .query_row(
            "SELECT body FROM transfers WHERE id=?1 AND principal=?2 AND expires>?3",
            params![id, principal, now_ms()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(&body.ok_or_else(|| {
        anyhow::anyhow!("Transfer not found or expired.")
    })?)?)
}

async fn cleanup_transfers(state: &AppState) -> Result<()> {
    let ids = {
        let db = store(state)?;
        let mut query = db.prepare("SELECT id FROM transfers WHERE expires<=?1")?;
        query
            .query_map([now_ms()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        let _ = tokio::fs::remove_file(transfer_path(state, &id)?).await;
        store(state)?.execute(
            "DELETE FROM transfers WHERE id=?1 AND expires<=?2",
            params![id, now_ms()],
        )?;
    }
    Ok(())
}

async fn new_transfer(state: &AppState, principal: &str, value: Value) -> Result<Value> {
    cleanup_transfers(state).await?;
    let _guard = database_lock(state, "transfer-quota").await;
    let db = store(state)?;
    let count: usize = db.query_row(
        "SELECT COUNT(*) FROM transfers WHERE principal=?1",
        [principal],
        |r| r.get(0),
    )?;
    // App users share one pool per deployment and owners share another, so
    // runtime traffic cannot exhaust the slots owners need for releases and exports.
    // App user transfers also expire sooner, so abandoned ones free the
    // Deployment's pool quickly.
    let (pool, limit, per_principal, ttl) = match principal.strip_prefix("runtime:") {
        Some(rest) => (
            format!("runtime:{}:", rest.split(':').next().unwrap_or("")),
            MAX_DEPLOYMENT_TRANSFERS,
            MAX_RUNTIME_PRINCIPAL_TRANSFERS,
            RUNTIME_TRANSFER_TTL_MS,
        ),
        None => (
            "owner:".to_owned(),
            MAX_OWNER_TRANSFERS,
            MAX_OWNER_PRINCIPAL_TRANSFERS,
            OWNER_TRANSFER_TTL_MS,
        ),
    };
    let total: usize = db.query_row(
        "SELECT COUNT(*) FROM transfers WHERE substr(principal,1,length(?1))=?1",
        [&pool],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        count < per_principal && total < limit,
        "Too many active transfers. Cancel completed transfers before starting another."
    );
    let id = Uuid::new_v4().to_string();
    let expires = now_ms() + ttl;
    tokio::fs::create_dir_all(state.data_root.join("transfers")).await?;
    tokio::fs::write(transfer_path(state, &id)?, []).await?;
    db.execute(
        "INSERT INTO transfers(id,principal,body,expires) VALUES(?1,?2,?3,?4)",
        params![id, principal, value.to_string(), expires],
    )?;
    Ok(
        json!({"transferId":id,"size":value["size"],"sha256":value["sha256"],"chunkBytes":CHUNK,"expiresAt":expires}),
    )
}

async fn download(
    state: &AppState,
    principal: &str,
    bytes: &[u8],
    project: Option<&str>,
) -> Result<Value> {
    anyhow::ensure!(
        bytes.len() <= MAX_TRANSFER,
        "Download exceeds transfer limit."
    );
    let result=new_transfer(state,principal,json!({"direction":"download","size":bytes.len(),"sha256":digest(bytes),"complete":true,"projectId":project})).await?;
    tokio::fs::write(
        transfer_path(state, result["transferId"].as_str().unwrap())?,
        bytes,
    )
    .await?;
    Ok(result)
}

async fn transfer_call(
    state: &AppState,
    principal: &str,
    name: &str,
    a: &Value,
    runtime: bool,
) -> Result<Value> {
    if name == "transfer_begin" {
        let size = a["size"].as_u64().unwrap_or(0);
        let hash = a["sha256"].as_str().unwrap_or("");
        anyhow::ensure!(
            size > 0
                && size
                    <= if runtime {
                        2 * 1024 * 1024
                    } else {
                        MAX_TRANSFER as u64
                    },
            "Transfer size is outside the limit."
        );
        anyhow::ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid SHA-256 checksum."
        );
        return new_transfer(state,principal,json!({"direction":"upload","size":size,"sha256":hash,"complete":false,"projectId":a["projectId"]})).await;
    }
    let id = a["transferId"].as_str().unwrap();
    let path = transfer_path(state, id)?;
    let _guard = database_lock(state, &format!("transfer:{id}")).await;
    let mut t = transfer(state, principal, a)?;
    match name {
        "transfer_cancel" => {
            store(state)?.execute(
                "DELETE FROM transfers WHERE id=?1 AND principal=?2",
                params![id, principal],
            )?;
            let _ = tokio::fs::remove_file(path).await;
            Ok(json!({"ok":true}))
        }
        "transfer_status" => Ok(
            json!({"transferId":id,"offset":tokio::fs::metadata(path).await?.len(),"complete":t["complete"],"size":t["size"],"sha256":t["sha256"]}),
        ),
        "transfer_read" => {
            anyhow::ensure!(t["complete"] == true, "Transfer is incomplete.");
            let offset = a["offset"].as_u64().unwrap();
            let size = t["size"].as_u64().unwrap();
            anyhow::ensure!(offset <= size, "Offset exceeds transfer size.");
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut data = vec![0; CHUNK.min((size - offset) as usize)];
            file.read_exact(&mut data).await?;
            Ok(
                json!({"data":encode_base64(&data),"offset":offset,"nextOffset":offset+data.len() as u64,"size":size,"sha256":t["sha256"]}),
            )
        }
        "transfer_write" => {
            anyhow::ensure!(
                t["direction"] == "upload" && t["complete"] != true,
                "Transfer is immutable."
            );
            let encoded = a["data"].as_str().unwrap();
            let bytes = decode_base64(encoded)?;
            anyhow::ensure!(
                !bytes.is_empty() && bytes.len() <= CHUNK && encode_base64(&bytes) == encoded,
                "Invalid base64 block."
            );
            let offset = a["offset"].as_u64().unwrap();
            let size = t["size"].as_u64().unwrap();
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .await?;
            let length = file.metadata().await?.len();
            anyhow::ensure!(
                offset <= length && offset + bytes.len() as u64 <= size,
                "Invalid block offset or size."
            );
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            if offset < length {
                anyhow::ensure!(
                    offset + bytes.len() as u64 <= length,
                    "Retry overlaps the upload head."
                );
                let mut previous = vec![0; bytes.len()];
                file.read_exact(&mut previous).await?;
                anyhow::ensure!(previous == bytes, "Retry does not match the stored block.");
            } else {
                file.write_all(&bytes).await?;
                file.sync_data().await?;
            }
            Ok(json!({"nextOffset":file.metadata().await?.len()}))
        }
        "transfer_complete" => {
            anyhow::ensure!(t["direction"] == "upload", "Not an upload.");
            let bytes = tokio::fs::read(path).await?;
            anyhow::ensure!(
                bytes.len() as u64 == t["size"].as_u64().unwrap() && digest(&bytes) == t["sha256"],
                "Upload size or SHA-256 mismatch."
            );
            t["complete"] = json!(true);
            store(state)?.execute(
                "UPDATE transfers SET body=?1 WHERE id=?2",
                params![t.to_string(), id],
            )?;
            Ok(json!({"transferId":id,"complete":true,"sha256":t["sha256"]}))
        }
        _ => anyhow::bail!("Unknown transfer tool."),
    }
}

async fn schema_plan(input: Value) -> Result<Value> {
    let root = std::env::var("LITTLEBOX_SOURCE_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let helper = std::env::var("LITTLEBOX_DERIVE_HELPER")
        .ok()
        .filter(|value| !value.is_empty());
    let bind = std::env::var("LITTLEBOX_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let public_origin = std::env::var("LITTLEBOX_PUBLIC_ORIGIN").unwrap_or_default();
    let mcp_origin = std::env::var("LITTLEBOX_MCP_ORIGIN").unwrap_or_default();
    if helper.is_none() && super::derive_must_isolate(&bind, &public_origin, &mcp_origin) {
        anyhow::bail!(
            "Schema derivation needs LITTLEBOX_DERIVE_HELPER on a Kernel that is not loopback-only."
        );
    }
    let mut command = if let Some(helper) = helper {
        let mut command = tokio::process::Command::new(helper);
        command.current_dir("/tmp");
        command
    } else {
        let bun = std::env::var("LITTLEBOX_BUN_PATH").unwrap_or_else(|_| "bun".into());
        let mut command = tokio::process::Command::new(bun);
        command
            .arg("run")
            .arg(root.join("database-lifecycle/derive-migration.ts"))
            .current_dir(root);
        command
    };
    let mut child = command
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.to_string().as_bytes()).await?;
    drop(stdin);
    let mut stdout = child.stdout.take().unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        (&mut stdout)
            .take(4_000_001)
            .read_to_end(&mut output)
            .await?;
        anyhow::ensure!(
            output.len() <= 4_000_000,
            "Migration reply exceeds size limit."
        );
        child.wait().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    let output = String::from_utf8(output)?;
    let reply: Value = serde_json::from_str(
        output
            .rsplit("<<littlebox-schema-reply>>")
            .next()
            .unwrap_or(""),
    )?;
    anyhow::ensure!(
        reply["ok"] == true,
        "{}",
        reply["error"]
            .as_str()
            .unwrap_or("Migration derivation failed.")
    );
    let statements: Vec<String> = serde_json::from_value(reply["statements"].clone())?;
    anyhow::ensure!(
        !statements.is_empty(),
        "The Drizzle model has no schema changes to migrate."
    );
    let sql = statements.join("\n--> statement-breakpoint\n");
    let from = digest(input["acceptedSource"].as_str().unwrap().as_bytes());
    let to = digest(input["candidateSource"].as_str().unwrap().as_bytes());
    let destructive = statements.iter().any(|s| {
        let s = s.to_uppercase();
        s.contains("DROP ") || s.contains("DELETE FROM")
    });
    Ok(
        json!({"id":digest(format!("{from}:{to}:{sql}").as_bytes()),"fromHash":from,"toHash":to,"sql":sql,"statements":statements,"destructive":destructive}),
    )
}

async fn owner_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
    match super::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => Json(json!({"mode":"clerk","publishableKey":config.publishable_key})).into_response(),
        Ok(None) => Json(json!({"mode":"local"})).into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not configured."),
    }
}

async fn owner_clerk(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    if !same_origin(&headers) || !super::oauth::valid_owner_host(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Cross-origin sign-in rejected.");
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return error(StatusCode::FORBIDDEN, "Sign-in origin is required.");
    };
    let config = match super::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => config,
        _ => return error(StatusCode::SERVICE_UNAVAILABLE, "Clerk sign-in is not configured."),
    };
    let client = state.guards.client_ip(request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|info| info.0), &headers);
    if let Err(wait) = state.guards.auth_attempt("owner", client, "clerk") { return too_many_attempts(wait); }
    let bytes = match axum::body::to_bytes(request.into_body(), 20_000).await { Ok(bytes) => bytes, Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response() };
    let body: Value = match serde_json::from_slice(&bytes) { Ok(body) => body, Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid sign-in request.") };
    let Some(token) = body["token"].as_str() else { return error(StatusCode::BAD_REQUEST, "Sign-in token required."); };
    let mut db = match Connection::open(state.data_root.join("owners.sqlite")) { Ok(db) => db, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    let session = match super::clerk::exchange(&mut db, &config, token, origin, (now_ms() / 1000) as i64) {
        Ok(session) => session,
        Err(_) => return error(StatusCode::UNAUTHORIZED, "Sign-in failed or this account has not been admitted to the beta."),
    };
    let cookie_name = owner_cookie_name(&state);
    let secure = if cookie_name.starts_with("__Host-") { "; Secure" } else { "" };
    let mut response = Json(json!({"result":{"user":session.user,"expiresAt":session.expires_at}})).into_response();
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!("{cookie_name}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600{secure}", session.token)).unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn owner_login(State(state): State<AppState>, request: Request) -> Response {
    owner_auth(state, request, false).await
}
async fn owner_signup(State(state): State<AppState>, request: Request) -> Response {
    owner_auth(state, request, true).await
}

async fn owner_auth(state: AppState, request: Request, signup: bool) -> Response {
    match super::clerk::ClerkConfig::from_env() {
        Ok(None) => {},
        Ok(Some(_)) => return error(StatusCode::FORBIDDEN, "Local owner sign-in is disabled on this installation."),
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not configured."),
    }
    let headers = request.headers().clone();
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    if !same_origin(&headers)
        || !super::oauth::valid_owner_host(&state, &headers).await
    {
        return error(
            StatusCode::FORBIDDEN,
            "Owner authentication is unavailable on this origin.",
        );
    }
    let bytes = match axum::body::to_bytes(request.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Authentication request exceeds limit.",
            );
        }
    };
    let a: Value = match serde_json::from_slice(&bytes) {
        Ok(a) => a,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Expected JSON."),
    };
    let client = state.guards.client_ip(peer, &headers);
    // Counted before the setup key check, so the limit also slows guessing it.
    if let Err(wait) = state
        .guards
        .auth_attempt("owner", client, a["email"].as_str().unwrap_or(""))
    {
        return too_many_attempts(wait);
    }
    if signup
        && digest(a["setupKey"].as_str().unwrap_or("").as_bytes())
            != digest(state.admin_key.as_bytes())
    {
        return error(
            StatusCode::FORBIDDEN,
            "A valid installation setup key is required to create an owner.",
        );
    }
    let email = a["email"].as_str().unwrap_or("").to_owned();
    let password = a["password"].as_str().unwrap_or("").to_owned();
    let path = state.data_root.join("owners.sqlite");
    // Only signup needs the lock: it spans the insert and the first-owner claim
    // below. The password hash runs before it, and logins run concurrently.
    let (result, _guard) = if signup {
        match blocking(move || auth::NewAccount::prepare(&email, &password)).await {
            Ok(account) => {
                let guard = database_lock(&state, "owner-auth").await;
                (blocking(move || AuthService::open(path)?.create_account(account)).await, Some(guard))
            }
            Err(error_value) => (Err(error_value), None),
        }
    } else {
        let failed_email = email.clone();
        let result = blocking(move || AuthService::open(path)?.login(&email, &password)).await;
        if matches!(result, Err(AuthError::InvalidCredentials)) {
            state.guards.auth_failed("owner", client, &failed_email);
        }
        (result, None)
    };
    match result {
        Ok(session) => {
            if signup {
                // The installation operator claims legacy deployments during the first setup.
                if let Ok(db) = store(&state) {
                    let owner_db = Connection::open(state.data_root.join("owners.sqlite")).unwrap();
                    let owners: u64 = owner_db
                        .query_row("SELECT COUNT(*) FROM app_users", [], |r| r.get(0))
                        .unwrap_or(0);
                    if owners == 1 {
                        for d in state.deployments.read().await.values() {
                            let _=db.execute("INSERT OR IGNORE INTO projects(id,owner,body) VALUES(?1,?2,?3)",params![d.project_id,session.user.user_id,json!({"id":d.project_id,"name":d.project_name,"sourceAuthority":"managed","manifest":d.manifest}).to_string()]);
                        }
                    }
                }
            }
            let cookie_name = owner_cookie_name(&state);
            let secure = if cookie_name.starts_with("__Host-") {
                "; Secure"
            } else {
                ""
            };
            let mut response =
                Json(json!({"result":{"user":session.user,"expiresAt":session.expires_at}}))
                    .into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{cookie_name}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800{secure}",
                    session.token
                ))
                .unwrap(),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => auth_error(e),
    }
}

async fn owner_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers)
        || !super::oauth::valid_owner_host(&state, &headers).await
    {
        return error(StatusCode::FORBIDDEN, "Cross-origin logout rejected.");
    }
    let cookie_name = owner_cookie_name(&state);
    if super::clerk::ClerkConfig::from_env().ok().flatten().is_some() {
        if let Ok(db) = Connection::open(state.data_root.join("owners.sqlite")) {
            let _ = super::clerk::revoke(&db, cookie_value(&headers, cookie_name).as_deref());
        }
    }
    if let Ok(auth) = AuthService::open(state.data_root.join("owners.sqlite")) {
        let _ = auth.logout(cookie_value(&headers, cookie_name).as_deref());
    }
    let secure = if cookie_name.starts_with("__Host-") { "; Secure" } else { "" };
    let mut response = Json(json!({"result":{"ok":true}})).into_response();
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!(
        "{cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"
    )).unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn owner_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    let html = include_str!("owner-login.html");
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_principals_never_see_owner_tools_and_anonymous_visitors_cannot_upload() {
        let anonymous = anonymous_runtime_tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            anonymous,
            vec![
                "auth_session",
                "actions_invoke",
                "files_list",
                "files_read",
                "transfer_read",
                "transfer_status",
                "transfer_cancel",
            ]
        );
        for t in runtime_tools() {
            let name = t["name"].as_str().unwrap();
            assert!(!name.starts_with("signup_policy"), "runtime tool {name}");
        }
        let reader = Owner { id: "o".into(), display: None, scopes: vec!["read".into()], projects: vec![], session: false };
        let names = |owner: &Owner| admin_tools(owner).iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert!(names(&reader).contains(&"signup_policy_read".to_owned()));
        assert!(!names(&reader).contains(&"signup_policy_set".to_owned()));
        let publisher = Owner { scopes: vec!["read".into(), "publish".into()], ..reader };
        assert!(names(&publisher).contains(&"signup_policy_set".to_owned()));
    }

    #[tokio::test]
    async fn app_users_cannot_take_the_transfer_slots_owners_need() {
        let root = std::env::temp_dir().join(format!("littlebox-quota-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            local_artifact_root: root.join("artifacts"),
            backup_root: root.join("backups"),
            data_root: root.clone(),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        };
        let body = || json!({"direction":"download","size":1,"sha256":"00","complete":true});
        let per_user = MAX_RUNTIME_PRINCIPAL_TRANSFERS;
        for _ in 0..per_user {
            new_transfer(&state, "runtime:app-a:user-0", body()).await.unwrap();
        }
        assert!(
            new_transfer(&state, "runtime:app-a:user-0", body()).await.is_err(),
            "one app user holds at most {per_user} transfers"
        );
        for user in 1..MAX_DEPLOYMENT_TRANSFERS / per_user {
            for _ in 0..per_user {
                new_transfer(&state, &format!("runtime:app-a:user-{user}"), body()).await.unwrap();
            }
        }
        assert!(new_transfer(&state, "runtime:app-a:another", body()).await.is_err());
        new_transfer(&state, "runtime:app-b:user", body()).await.unwrap();
        let owner = new_transfer(&state, "owner:alice", body()).await.unwrap();
        let runtime = new_transfer(&state, "runtime:app-c:user", body()).await.unwrap();
        let ttl = |t: &Value| t["expiresAt"].as_u64().unwrap() - now_ms();
        assert!(ttl(&runtime) <= RUNTIME_TRANSFER_TTL_MS);
        assert!(ttl(&owner) > RUNTIME_TRANSFER_TTL_MS);
        for _ in 1..MAX_OWNER_PRINCIPAL_TRANSFERS {
            new_transfer(&state, "owner:alice", body()).await.unwrap();
        }
        assert!(new_transfer(&state, "owner:alice", body()).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
