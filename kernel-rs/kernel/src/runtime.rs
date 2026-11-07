//! The runtime endpoint the browser SDK calls: `/_konae/mcp` on an app's
//! host. It speaks the same JSON-RPC as an MCP server and offers an app's
//! visitors and app users the session, action, file and transfer tools.
//! The helpers that read a call and wrap an answer serve every MCP door.

use super::*;
use super::transfers::{CHUNK, MAX_TRANSFER, download_body, transfer, transfer_call, transfer_path};
use rusqlite::params;

pub const VERSION: &str = "2025-11-25";
/// What `initialize` tells a client of an app's runtime endpoint. The tools
/// are the app's, never an owner's: Konae's owner MCP answers on `/mcp` of
/// the installation with its own instructions.
pub const INSTRUCTIONS: &str = "This is one app's runtime endpoint. It acts as the visitor or the app account whose session the request carries. actions_invoke runs the app's Server Actions by name; tools/list names the actions and file tools this session may call. auth_session says who is signed in.";

/// Records an app user's file upload or delete: the principal, the tool,
/// the Deployment and whether it worked. Whoever starts the Kernel decides
/// where it goes; without one nothing is recorded.
pub type AuditSink = Arc<dyn Fn(&str, &str, &str, bool) + Send + Sync>;

pub fn routes() -> Router<AppState> {
    Router::new().route("/_konae/mcp", post(runtime))
}

/// Reads one JSON-RPC call of at most 200 KB.
pub async fn read_call(request: Request) -> Result<Value, Response> {
    let bytes = match axum::body::to_bytes(request.into_body(), 200_000).await {
        Ok(bytes) => bytes,
        Err(_) => return Err(error(StatusCode::PAYLOAD_TOO_LARGE, "MCP request exceeds 200 KB.")),
    };
    let call: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Err(rpc_error(Value::Null, -32700, "Invalid JSON.")),
    };
    if !call.is_object()
        || call["jsonrpc"] != "2.0"
        || !call["method"].is_string()
        || call
            .get("id")
            .is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
    {
        return Err(rpc_error(Value::Null, -32600, "Invalid JSON-RPC request."));
    }
    Ok(call)
}

/// The answer to a call without an ID: a notification is accepted, anything
/// else is refused.
pub fn without_id(method: &str) -> Response {
    if method.starts_with("notifications/") {
        StatusCode::ACCEPTED.into_response()
    } else {
        rpc_error(Value::Null, -32600, "A request ID is required.")
    }
}

/// A tool's answer as MCP carries it: the text, the structured result and
/// whether it failed.
pub fn tool_result(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"content":[{"type":"text","text":serde_json::to_string(&value).unwrap()}],"structuredContent":{"result":value},"isError":false}),
        Err(e) => tool_error(&e),
    }
}

/// The JSON-RPC answer, never cached.
pub fn answer(id: Value, result: Result<Value>) -> Response {
    let mut response = match result {
        Ok(result) => Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response(),
        Err(e) => rpc_error(id, -32603, &e.to_string()),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub fn unauthorized(state: &AppState, headers: &HeaderMap) -> Response {
    let mut response = error(StatusCode::UNAUTHORIZED, "Sign in to use Konae MCP.");
    let metadata = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        origin(state, headers)
    );
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&metadata).unwrap(),
    );
    response
}

pub async fn runtime(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    let host_known = valid_host(&state, &headers)
        || super::domain::is_alias_request(&state, &headers)
        || super::path::on_path_host(&state, &headers);
    if !same_origin(&headers) || !host_known {
        return error(
            StatusCode::FORBIDDEN,
            "Cross-origin or unrecognized-host MCP request rejected.",
        );
    }
    let principal;
    let tools;
    let mut anonymous = false;
    // A visitor on a private app reaches only the account tools.
    let mut account_only = false;
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if super::path::foreign_referer(&state, &headers, &deployment.id) {
        return error(StatusCode::FORBIDDEN, "A page of another app's path address cannot call this app.");
    }
    // A key would otherwise read as an unknown session token, and the
    // program would get a visitor's answers without knowing why.
    if bearer(&headers).is_some_and(|token| token.starts_with("lbk_")) {
        return error(StatusCode::FORBIDDEN, API_KEY_ON_RUNTIME);
    }
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
        Ok(None) => {
            let client = limits::client_key(state.guards.client_ip(peer, &headers));
            principal = format!("runtime:{}:anon:{client}", deployment.id);
            tools = account_tools();
            anonymous = true;
            account_only = true;
        }
        Err(_) => return unauthorized(&state, &headers),
    }
    let app = deployment.id;
    let call = match read_call(request).await {
        Ok(call) => call,
        Err(response) => return response,
    };
    let id = call.get("id").cloned().unwrap_or(Value::Null);
    let method = call["method"].as_str().unwrap();
    if account_only
        && !(["initialize", "ping"].contains(&method)
            || method.starts_with("notifications/")
            || (method == "tools/call"
                && call["params"]["name"].as_str().is_some_and(|name| tools.iter().any(|t| t["name"] == name))))
    {
        return unauthorized(&state, &headers);
    }
    if call.get("id").is_none() {
        return without_id(method);
    }
    let mut clear_cookie = false;
    let mut session_cookie: Option<HeaderValue> = None;
    let result = match method {
        "initialize" => Ok(
            json!({"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"konae","version":"0.1.0"},"instructions":INSTRUCTIONS}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":tools})),
        "tools/call" => {
            let Some(name) = call["params"]["name"].as_str() else {
                return rpc_error(id, -32602, "Tool name is required.");
            };
            if !tools.iter().any(|t| t["name"] == name) {
                // Tell a signed-out visitor the tool needs a session, so the SDK can
                // send them to the login page.
                if runtime_tools().iter().any(|t| t["name"] == name) {
                    return Json(json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"authentication-required"}],"isError":true}})).into_response();
                }
                return rpc_error(id, -32602, "Unknown tool or insufficient permission.");
            }
            let definition = tools.iter().find(|t| t["name"] == name).unwrap();
            let arguments = call["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            if let Err(e) = validate_arguments(&arguments, &definition["inputSchema"]) {
                return rpc_error(id, -32602, &e.to_string());
            }
            if name == "actions_invoke" {
                // Signed-out visitors share a bucket per client address and
                // Deployment; each signed-in app user has their own.
                let limited = if anonymous {
                    let client = limits::client_key(state.guards.client_ip(peer, &headers));
                    let key = format!("{app}|{client}");
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
            let result = if name.starts_with("transfer_") {
                transfer_call(&state, &principal, name, &arguments, true).await
            } else if account_tools().iter().any(|t| t["name"] == name) {
                let client = state.guards.client_ip(peer, &headers);
                match super::accounts::runtime_tool(&state, &headers, client, name, &arguments).await {
                    Ok((value, cookie)) => {
                        session_cookie = cookie;
                        Ok(value)
                    }
                    Err(e) => Err(e),
                }
            } else {
                runtime_call(&state, &headers, &principal, Some(&app), name, arguments.clone()).await
            };
            // The session row is gone; the browser must drop its cookie too.
            if name == "auth_logout" && result.is_ok() {
                clear_cookie = true;
            }
            Ok(tool_result(result))
        }
        _ => return rpc_error(id, -32601, "Method not found."),
    };
    let mut response = answer(id, result);
    if clear_cookie {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, super::cleared_app_cookie(&state, &app, &headers));
    }
    if let Some(cookie) = session_cookie {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

/// Records an app user's upload or delete through the installation's sink.
fn audit(state: &AppState, principal: &str, name: &str, app: Option<&str>, result: &Result<Value>) {
    if let (Some(sink), Some(app)) = (state.audit.as_ref(), app) {
        sink(principal, name, app, result.is_ok());
    }
}

/// The answer to an API key on the runtime endpoint.
pub const API_KEY_ON_RUNTIME: &str =
    "An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.";

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}


/// Lock names the Kernel takes besides an app's ID.
const RESERVED_IDS: [&str; 3] = ["domains", "paths", "transfer-quota"];

/// A project ID names the app's folder and its lock, so it holds only
/// lowercase letters, digits, "_" and "-": two IDs that differ in case would
/// share a folder on a volume that ignores case.
pub fn valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
        && !RESERVED_IDS.contains(&id)
}


pub fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"openWorldHint":false}})
}


pub fn runtime_tools() -> Vec<Value> {
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
        tool(
            "files_update",
            "Make a file you uploaded public or private. Only its uploader may change it.",
            json!({"fileId":{"type":"string"},"visibility":{"type":"string","enum":["private","public"]}}),
            &["fileId", "visibility"],
            false,
        ),
        tool(
            "files_delete",
            "Delete a file you uploaded. Its size stops counting against the Deployment's quota and a later read answers File not found. Only its uploader may delete it.",
            json!({"fileId":{"type":"string"}}),
            &["fileId"],
            false,
        ),
    ];
    tools.extend(transfer_tools(true));
    tools.extend(account_tools());
    tools
}


/// Account tools any visitor may call, also on a private deployment before
/// signing in. Tokens arrive by email; no tool returns one.
pub fn account_tools() -> Vec<Value> {
    vec![
        tool(
            "auth_signup",
            "Start an app account for an email. When the installation requires verification, this sends a link to that email and signs no one in; the person chooses the password when they open it. Otherwise it creates the account with this password and signs in. The answer does not reveal whether the email already has an account.",
            json!({"email":{"type":"string","maxLength":254},"password":{"type":"string","maxLength":1024}}),
            &["email"],
            false,
        ),
        tool(
            "auth_verify",
            "Confirm an email with the token from its verification link and set the account's password. Signs in on success. A token works once.",
            json!({"token":{"type":"string","maxLength":128},"password":{"type":"string","maxLength":1024}}),
            &["token", "password"],
            false,
        ),
        tool(
            "auth_recover",
            "Send a link to choose a new password. The answer is the same whether or not the email has an account.",
            json!({"email":{"type":"string","maxLength":254}}),
            &["email"],
            false,
        ),
        tool(
            "auth_reset",
            "Set a new password with the token from a recovery link. Ends every earlier session of the account and signs in. A token works once.",
            json!({"token":{"type":"string","maxLength":128},"password":{"type":"string","maxLength":1024}}),
            &["token", "password"],
            false,
        ),
    ]
}


/// Tools a visitor without a session may call on a public deployment.
/// `actions_invoke` still refuses every action not declared anonymous.
/// Uploads and the transfers that would create one stay signed-in only.
pub fn anonymous_runtime_tools() -> Vec<Value> {
    let allowed = [
        "auth_session",
        "actions_invoke",
        "files_list",
        "files_read",
        "transfer_read",
        "transfer_status",
        "transfer_cancel",
        "auth_signup",
        "auth_verify",
        "auth_recover",
        "auth_reset",
    ];
    runtime_tools()
        .into_iter()
        .filter(|tool| allowed.contains(&tool["name"].as_str().unwrap_or("")))
        .collect()
}


pub fn transfer_tools(runtime: bool) -> Vec<Value> {
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
            if runtime {
                "Start a size-bounded upload with an expected SHA-256 checksum. A file is at most the installation's per-file limit, which a refusal names."
            } else {
                "Start a size-bounded upload with an expected SHA-256 checksum. An upload is at most 32 MiB unless it names a package component, which may be as large as that component's quota."
            },
            // The bound depends on the installation's file limits, so the
            // handler checks it and names the number.
            json!({"size":{"type":"integer","minimum":1},"sha256":{"type":"string","pattern":"^[a-f0-9]{64}$"},"projectId":{"type":"string"},"component":{"type":"string","enum":super::package::COMPONENT_NAMES}}),
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
                let properties = t["inputSchema"]["properties"].as_object_mut().unwrap();
                properties.remove("projectId");
                properties.remove("component");
            }
        }
    }
    tools
}


pub fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}


pub fn validate_arguments(value: &Value, schema: &Value) -> Result<()> {
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


pub async fn response_value(response: Response) -> Result<Value> {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_TRANSFER).await?;
    let value: Value = serde_json::from_slice(&bytes)?;
    if !status.is_success() {
        if let Some(refused) = super::refusal::from_body(&value) {
            return Err(refused.into());
        }
    }
    anyhow::ensure!(
        status.is_success(),
        "{}",
        value["error"]
            .as_str()
            .unwrap_or("Kernel operation failed.")
    );
    Ok(value["result"].clone())
}


/// A tool's failed result. The text stays the sentence clients read today; a
/// refused Capsule also answers `{ error: { code, message, hint } }` (ADR-0020).
pub fn tool_error(error: &anyhow::Error) -> Value {
    let text = json!([{"type": "text", "text": error.to_string()}]);
    match super::refusal::of(error) {
        Some(refused) => json!({"content": text, "structuredContent": {"error": refused.to_json()}, "isError": true}),
        None => json!({"content": text, "isError": true}),
    }
}


/// Checks a release as the deploy would take it, and answers the schema
/// change it makes, derived on a copy of the live database.
/// Checks a release and answers the schema change it makes. `against` is a
/// database an import puts in place first; without it the change is planned
/// against the live database, or a package's or an empty one for a first release.
pub async fn validate_release(state: &AppState, input: &DeployInput, against: Option<&Path>) -> Result<super::schema_change::Change> {
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
    for path in input.artifact.files.keys() {
        anyhow::ensure!(
            safe_relative_path(path)
                && !path.starts_with("_konae/")
                && !path.starts_with("oauth/")
                && !path.starts_with(".well-known/")
                && path != "mcp",
            "Invalid or reserved artifact path."
        );
    }
    super::artifact::check_routing(&input.project.manifest).map_err(anyhow::Error::msg)?;
    super::calls::check_connects(&input.project.manifest)?;
    // The limit counts the bytes the Kernel stores, after base64 is decoded.
    let total: usize = super::artifact::decode(&input.artifact).map_err(anyhow::Error::msg)?.values().map(Vec::len).sum();
    anyhow::ensure!(total <= 16 * 1024 * 1024, "Artifact exceeds 16 MiB.");
    let program = super::action_shape::read(
        input
            .project
            .files
            .get("actions.json")
            .ok_or_else(|| anyhow::anyhow!("actions.json is required."))?,
    )?;
    super::calls::check_release(&super::domain::Rules::of(state), &super::http::secrets_store(&state.data_root, &input.project.id)?, &program)?;
    let _guard = database_lock(state, &format!("release:{}", input.project.id)).await;
    let existing = find_project_deployment(&*state.deployments.read().await, &input.project.id);
    anyhow::ensure!(
        existing.is_none() || (input.initial_signup_policy.is_none()
            && input.initial_package.is_none()),
        "Portable state can only be imported on the first deployment."
    );
    if let Some(policy) = input.initial_signup_policy.as_ref() {
        SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?;
    }
    let temp = state
        .data_root
        .join(format!("validate-{}.sqlite", Uuid::new_v4()));
    let result = (|| -> Result<super::schema_change::Change> {
        if let Some(database) = against {
            std::fs::copy(database, &temp)?;
        } else if let Some(ref deployment) = existing {
            let connection = Connection::open(database_path(state, &deployment.id))?;
            connection.execute("VACUUM INTO ?1", [temp.to_string_lossy().as_ref()])?;
        } else if let Some(package) = input.initial_package.as_ref() {
            super::package::copy_database(state, package, &temp)?;
        } else {
            initialize_database(&temp)?;
        }
        super::schema_change::apply(
            &temp,
            input
                .project
                .files
                .get("schema.sql")
                .map(String::as_str)
                .unwrap_or(""),
            &program,
            super::schema_change::Mode::Plan,
        )
    })();
    let _ = tokio::fs::remove_file(temp).await;
    result
}


/// Whether Usage counts this request: not when its session came through an
/// entry link (`enter.rs`).
async fn counted(state: &AppState, app: &str, headers: &HeaderMap) -> bool {
    let deployment = state.deployments.read().await.values().find(|d| d.id == app).cloned();
    !deployment.is_some_and(|deployment| super::enter::entered_request(state, &deployment, headers))
}

pub async fn runtime_call(
    state: &AppState,
    headers: &HeaderMap,
    principal: &str,
    app: Option<&str>,
    name: &str,
    a: Value,
) -> Result<Value> {
    match name {
        "auth_session" => {
            let result = response_value(session(State(state.clone()), headers.clone()).await).await;
            if let (Ok(_), Some(id)) = (&result, app) {
                if counted(state, id, headers).await {
                    super::usage::count(&state.data_root, id, super::usage::Operation::Session);
                }
            }
            result
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
            // The whole file goes into the transfer, whatever range the request named.
            let mut whole = headers.clone();
            whole.remove(header::RANGE);
            let response = serve_file(
                State(state.clone()),
                AxumPath(a["fileId"].as_str().unwrap().to_owned()),
                whole,
            )
            .await;
            anyhow::ensure!(response.status().is_success(), "File not found.");
            let result = download_body(state, principal, response.into_body()).await;
            if let (Ok(_), Some(id)) = (&result, app) {
                if counted(state, id, headers).await {
                    super::usage::count(&state.data_root, id, super::usage::Operation::FileRead);
                }
            }
            result
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
            let previous: Option<(String,String)>=super::transfers::store(state)?.query_row("SELECT request_hash,body FROM file_receipts WHERE transfer_id=?1 AND principal=?2",params![id,principal],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((hash, body)) = previous {
                anyhow::ensure!(hash == request_hash, "Upload retry changed its metadata.");
                return Ok(serde_json::from_str(&body)?);
            }
            let t = transfer(state, principal, &a)?;
            anyhow::ensure!(
                t["complete"] == true && t["direction"] == "upload",
                "File upload is incomplete."
            );
            let upload = super::Upload {
                name: a["name"].as_str(),
                content_type: a.get("type").and_then(Value::as_str),
                visibility: Some(visibility),
            };
            let source = transfer_path(state, id)?;
            let mut result =
                response_value(super::upload_from_path(state, headers, upload, &source).await)
                    .await?;
            result.as_object_mut().unwrap().remove("url");
            super::transfers::store(state)?.execute("INSERT INTO file_receipts(transfer_id,principal,request_hash,body) VALUES(?1,?2,?3,?4)",params![id,principal,request_hash,result.to_string()])?;
            if let Some(deployment) = app {
                if counted(state, deployment, headers).await {
                    super::usage::count(&state.data_root, deployment, super::usage::Operation::Upload);
                }
            }
            // Failed uploads and receipt replays leave no row.
            let result = Ok(result);
            audit(state, principal, name, app, &result);
            result
        }
        "files_update" => {
            let response = super::files::update_file(state, headers, a["fileId"].as_str().unwrap(), a["visibility"].as_str()).await;
            response_value(response).await
        }
        "files_delete" => {
            let result = response_value(super::files::delete_file(state, headers, a["fileId"].as_str().unwrap()).await).await;
            // Each file is deleted once, so like uploads these rows stay bounded.
            if result.is_ok() {
                audit(state, principal, name, app, &result);
            }
            result
        }
        _ => anyhow::bail!("Unknown runtime tool."),
    }
}


pub fn valid_host(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Ok(url) = url::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let public = url::Url::parse(&state.public_origin).ok();
    let configured: Vec<url::Url> = state.own_origins.iter().filter_map(|s| url::Url::parse(s).ok()).collect();
    ["localhost", "127.0.0.1", "[::1]", "app.localhost"].contains(&host)
        || host.ends_with(".localhost")
        || public.as_ref().is_some_and(|u| u.host_str() == Some(host))
        || configured
            .iter()
            .any(|u| u.host_str() == Some(host))
        || public
            .as_ref()
            .and_then(|u| u.host_str())
            .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")))
}


pub fn origin(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(origin) = state.own_origins.first() {
        return origin.trim_end_matches('/').to_owned();
    }
    let scheme = if state.public_origin.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}


#[cfg(test)]
mod tests {
    /// Every tool the instructions name is one the runtime endpoint serves.
    #[test]
    fn the_instructions_name_only_runtime_tools() {
        let served: Vec<String> = super::runtime_tools().iter().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect();
        let named: Vec<&str> = super::INSTRUCTIONS
            .split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
            .filter(|word| word.contains('_'))
            .collect();
        assert!(!named.is_empty());
        for word in named {
            assert!(served.iter().any(|name| name == word), "{word} is not a runtime tool");
        }
    }
}
