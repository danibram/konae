use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const VERSION: &str = "2025-11-25";
pub(super) const CHUNK: usize = 64 * 1024;
const MAX_TRANSFER: usize = 32 * 1024 * 1024;
pub(super) fn owner_cookie_name(state: &AppState) -> &'static str {
    let mcp_https = state.mcp_origin.as_deref().is_some_and(|origin| origin.starts_with("https://"));
    if state.public_origin.starts_with("https://") || mcp_https {
        "__Host-konae_owner_session"
    } else {
        "konae_owner_session"
    }
}
const MAX_OWNER_TRANSFERS: usize = 128;
const MAX_DEPLOYMENT_TRANSFERS: usize = 32;
/// Active transfers the signed-out visitors of one Deployment share, apart
/// from the signed-in users' pool.
const MAX_VISITOR_TRANSFERS: usize = 16;
/// Active transfers one owner credential may hold.
pub(super) const MAX_OWNER_PRINCIPAL_TRANSFERS: usize = 8;
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
        .route("/_konae/mcp", post(runtime))
        .route("/_konae/owner/login", get(owner_page).post(owner_login))
        .route("/_konae/owner/signup", post(owner_signup))
        .route("/_konae/owner/logout", post(owner_logout))
        .route("/_konae/owner/config", get(owner_config))
        .route("/_konae/owner/session", get(owner_session))
        .route("/_konae/owner/clerk", post(owner_clerk))
}

pub(super) fn store(state: &AppState) -> Result<Connection> {
    let db = Connection::open(state.data_root.join("mcp.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, sources INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);
        CREATE TABLE IF NOT EXISTS transfers(id TEXT PRIMARY KEY, principal TEXT NOT NULL, body TEXT NOT NULL, expires INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL, revision INTEGER NOT NULL, transfer TEXT NOT NULL, expires INTEGER NOT NULL, receipt TEXT);
        CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, at INTEGER NOT NULL, actor TEXT NOT NULL, tool TEXT NOT NULL, target TEXT, ok INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS audit_actor ON audit(actor, id);
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
    super::package::install_imports(&db)?;
    Ok(db)
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Clone)]
pub(super) struct Owner {
    pub(super) id: String,
    pub(super) display: Option<String>,
    pub(super) scopes: Vec<String>,
    pub(super) projects: Vec<String>,
    pub(super) session: bool,
}

impl Owner {
    pub(super) fn require(&self, scope: &str, project: Option<&str>) -> Result<()> {
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

/// The answer to an API key on the runtime endpoint.
const API_KEY_ON_RUNTIME: &str =
    "An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.";

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
                } else {
                    anyhow::ensure!(super::accounts::creator_store(state)?.is_active(&r.0)?, "Creator is not active.");
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
        super::accounts::creator_store(state)?.authenticate(cookie.as_deref())?
    };
    Ok(user
        .map(|user| Owner {
            id: user.user_id,
            display: Some(if clerk.is_some() { "Google account".to_owned() } else { user.name }),
            scopes: vec![
                "read".into(),
                "write".into(),
                "publish".into(),
                "manage_users".into(),
                "tokens".into(),
                "export_data".into(),
            ],
            projects: vec![],
            session: true,
        }))
}

pub(super) fn owned_project(state: &AppState, owner: &Owner, id: &str, scope: &str) -> Result<(Value, u64)> {
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

/// Lock names the Kernel takes besides an app's ID.
const RESERVED_IDS: [&str; 4] = ["domains", "paths", "owner-auth", "transfer-quota"];

/// A project ID names the app's folder and its lock, so it holds only
/// lowercase letters, digits, "_" and "-": two IDs that differ in case would
/// share a folder on a volume that ignores case.
pub(super) fn valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
        && !RESERVED_IDS.contains(&id)
}

pub(super) const DELETE_NAME_MISMATCH: &str = "The name does not match this project. Type the project's name exactly to delete it.";

/// Removes a project, its app folder and its Deployment. The Deployment
/// leaves the routing map first, so its address stops answering, and the
/// folder is renamed out of `apps/` before anything is deleted: a Kernel that
/// stops halfway never reloads half an app. The registry rows go last, so a
/// failure leaves a project the owner can see and delete again.
async fn delete_project(state: &AppState, id: &str) -> Result<Vec<String>> {
    let published = {
        let mut deployments = state.deployments.write().await;
        let hosts: Vec<String> = deployments.iter().filter(|(_, d)| d.id == id).map(|(host, _)| host.clone()).collect();
        hosts.into_iter().filter_map(|host| deployments.remove(&host)).count() > 0
    };
    // A Deployment the map never loaded still has its record in the folder.
    let ids: Vec<String> = if published || tokio::fs::try_exists(crate::layout::record(&state.data_root, id)).await? { vec![id.to_owned()] } else { Vec::new() };
    let trash = state.data_root.join("deleted");
    tokio::fs::create_dir_all(&trash).await?;
    let folder = crate::layout::folder(&state.data_root, id);
    for deployment in &ids {
        let _data = database_lock(state, deployment).await;
        let _identity = database_lock(state, &format!("identity:{deployment}")).await;
        super::domain::forget(state, deployment).await?;
        super::path::forget(state, deployment).await?;
        park(&folder, &trash, deployment).await?;
        // The folder held the index, so no file of this Deployment is readable
        // any more; a failure here leaves objects for files-reconcile.
        super::file_store::forget_deployment(state.files.store.as_ref(), deployment).await;
        if let Some(s3) = state.s3.as_ref() {
            let keys = list_s3_keys(s3, &state.artifact_bucket, &format!("deployments/{deployment}/")).await?;
            for key in keys {
                s3.delete_object().bucket(&state.artifact_bucket).key(key).send().await?;
            }
        }
    }
    // A project that was never published still has a folder with its sources.
    park(&folder, &trash, id).await?;
    let mut db = store(state)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (table, column) in [("plans", "project"), ("package_imports", "project")] {
        let present: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |r| r.get(0))?;
        if present {
            tx.execute(&format!("DELETE FROM {table} WHERE {column}=?1"), [id])?;
        }
    }
    tx.execute("DELETE FROM projects WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(ids)
}

/// Whether another app of this owner holds `value` as a route secret. A
/// signature names no app, so a request signed for one app would be accepted
/// by another that holds the same value. A secret for calls only verifies
/// nothing, so it may hold the value of another app's route secret: that is
/// how one app signs a call to another.
fn route_secret_reused(state: &AppState, owner: &str, project: &str, value: &str) -> Result<bool> {
    let db = store(state)?;
    let mut query = db.prepare("SELECT id FROM projects WHERE owner = ?1 AND id <> ?2")?;
    let others = query.query_map(params![owner, project], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for other in others {
        if crate::layout::secrets(&state.data_root, &other).is_file() && super::http::holds_route_secret(&super::http::secrets_store(&state.data_root, &other)?, value)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Renames the folder into the trash, then deletes it there.
async fn park(folder: &Path, trash: &Path, id: &str) -> Result<()> {
    let parked = trash.join(format!("{id}-{}", Uuid::new_v4().simple()));
    match tokio::fs::rename(folder, &parked).await {
        Ok(()) => {
            if let Err(error_value) = tokio::fs::remove_dir_all(&parked).await {
                eprintln!("Could not remove deleted app {}: {error_value}", parked.display());
            }
            Ok(())
        }
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error_value) => Err(error_value.into()),
    }
}

fn validate_sources(source: &Value, id: &str) -> Result<()> {
    let string = |value: &Value, max: usize| value.as_str().is_some_and(|s| s.len() <= max);
    // A name starting with a dot is a temporary file to the Kernel, which a copy leaves behind.
    let path = |value: &str| value.len() <= 240 && value.trim() == value
        && safe_relative_path(value) && !value.chars().any(|c| c.is_control())
        && !value.split('/').any(|part| part.starts_with('.'));
    anyhow::ensure!(source["id"] == id && string(&source["name"], 80)
        && source["name"].as_str().is_some_and(|s| !s.trim().is_empty())
        && string(&source["slug"], 100) && source["updatedAt"].as_u64().is_some(), "Invalid project metadata. Send {{ id, name, slug, files, directories, manifest, updatedAt }} with the project's own id, a name of 1 to 80 characters, a slug of up to 100 and updatedAt in milliseconds.");
    let files = source["files"].as_object().ok_or_else(|| anyhow::anyhow!("Invalid project files."))?;
    anyhow::ensure!(files.len() <= 1_000 && files.iter().all(|(name, body)| path(name) && string(body, MAX_TRANSFER)), "Invalid project files.");
    let directories = source["directories"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid project directories."))?;
    anyhow::ensure!(directories.len() <= 1_000 && directories.iter().all(|entry| entry.as_str().is_some_and(path)), "Invalid project directories.");
    let manifest = &source["manifest"];
    let actions = manifest["runtime"]["database"]["actions"].as_array();
    anyhow::ensure!(manifest["version"] == 1 && ["public", "private"].contains(&manifest["visibility"].as_str().unwrap_or(""))
        && manifest["runtime"]["database"]["mode"] == "declared-actions"
        && ["optional", "required"].contains(&manifest["runtime"]["auth"]["mode"].as_str().unwrap_or(""))
        && manifest["runtime"]["files"]["access"] == "deployment"
        && actions.is_some_and(|list| list.len() <= 50 && list.iter().all(|action|
            string(&action["name"], 120) && ["query", "mutation", "signal", "call"].contains(&action["kind"].as_str().unwrap_or(""))
            && ["anonymous", "authenticated", "granted"].contains(&action["access"].as_str().unwrap_or("")))), "Invalid project manifest. Send {{ version: 1, visibility, runtime: {{ database: {{ mode: \"declared-actions\", actions: [{{ name, kind, access }}] }}, auth: {{ mode: \"optional\" or \"required\" }}, files: {{ access: \"deployment\" }} }} }} with up to 50 actions.");
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

pub(super) async fn owned_deployment(
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

pub(super) fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"openWorldHint":false}})
}

pub(super) fn admin_tools(owner: &Owner) -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    let mut tools = vec![
        tool(
            "platform_describe",
            "Read Konae capabilities and local development instructions.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "session_inspect",
            "Read the authenticated owner's grants and a can map of the scopes this credential holds.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "mail_status",
            "Read whether this installation sends email, account email and notices: transport, sender preflight, queue depth, notice counts, last failure and limits. Set refresh to check the sender again.",
            json!({"refresh":{"type":"boolean"}}),
            &[],
            true,
        ),
        tool(
            "platform_guide",
            "Read a guide topic. Start with rules (yardstick, first run, read-back, divergences), then pieces (which Capsule declaration to use). Also credentials, development, actions, sdk, publishing, users and portability.",
            json!({"topic":{"type":"string","enum":super::guide::TOPICS}}),
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
        tool("project_sources_read", "Download the project sources the editor or project_sources_save saved last.", json!({"projectId":text}), &["projectId"], true),
        tool(
            "database_execute",
            "Administer a deployment database. Mutations require write permission.",
            json!({"deploymentId":text,"operation":{"type":"string","enum":["inspect","browse","sql","update-cell","insert-row","delete-row","list-backups","create-backup","restore-backup"]},"input":{"type":"object"}}),
            &["deploymentId", "operation"],
            false,
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
            tool("project_create", "Register a project and return a template's files, the ones konae init writes: template is tasks, notes (the default), members, form, webhook, digest or gallery, and without a manifest the project takes the template's visibility. Write the files into the folder and keep id, slug and manifest in konae.json. To save the editor's files with it, upload the project JSON without a projectId and pass sourcesTransferId to create the project and its files atomically. Repeating the same initial files is safe; existing saved files cannot be replaced here.", json!({"projectId":text,"name":text,"slug":text,"manifest":{"type":"object"},"sourcesTransferId":text,"template":{"type":"string","enum":["tasks","notes","members","form","webhook","digest","gallery"]}}), &["name"], false),
            tool("project_sources_save", "Save the project sources the editor opens if the expected source revision still matches. Upload the project JSON first: { id, name, slug, files: { path: text }, directories, manifest, updatedAt }, the format the editor saves and docs/mcp.md describes.", json!({"projectId":text,"transferId":text,"expectedRevision":{"type":"integer","minimum":0}}), &["projectId","transferId","expectedRevision"], false),
            tool("project_delete", "Delete a project for good: its registry entry, saved editor files and secrets, and, when it is published, its Deployment with the database, app accounts, uploaded files, backups and custom domain. The address stops answering at once. name must equal the project's name. Read back with projects_list.", json!({"projectId":text,"name":text}), &["projectId","name"], false),
        ]);
    }
    tools.extend([
        tool("secrets_list", "List a project's signing secrets: each one's name, use and svixKey, which says whether its value decodes as a svix whsec_ key. Values are never returned.", json!({"projectId":text}), &["projectId"], true),
        tool("api_keys_list", "List a deployment's API keys without their secrets.", json!({"deploymentId":text}), &["deploymentId"], true),
        super::domain::read_tool(),
        super::path::read_tool(),
    ]);
    if owner.scopes.iter().any(|s| s == "export_data") {
        tools.extend([
            tool(
                "project_export",
                "Export the app as its folder: konae.json, app/ and dist/ in the folder download, and a consistent copy of data/ (database, accounts without sessions or links, files, Usage) as one download per component that the manifest lists. Never secrets, owners or the installation's records. Requires export_data.",
                json!({"deploymentId":text}),
                &["deploymentId"],
                true,
            ),
        ]);
    }
    tools.extend(super::usage::admin_tools());
    tools.extend(super::calls::admin_tools());
    tools.extend(super::mail::admin_tools());
    tools.push(tool(
        "signup_policy_read",
        "Read who may create an account on a deployment.",
        json!({"deploymentId":text}),
        &["deploymentId"],
        true,
    ));
    tools.push(tool(
        "sign_in_methods_read",
        "Read how people sign in to a deployment: password, and google when the owner turned it on. googleClient says whether the app's google_client_id and google_client_secret are set, without their values, and redirectUris lists the callbacks to register in the app's Google OAuth client.",
        json!({"deploymentId":text}),
        &["deploymentId"],
        true,
    ));
    if owner.scopes.iter().any(|s| s == "manage_users") {
        tools.push(tool(
            "sign_in_methods_set",
            "Choose how people sign in to a deployment: [\"password\"] or [\"password\",\"google\"]. Password cannot be turned off. Google needs the app's own Google client: set google_client_id and google_client_secret with secret_set use google first. Read back with sign_in_methods_read.",
            json!({"deploymentId":text,"methods":{"type":"array","items":{"type":"string","enum":["password","google"]},"minItems":1,"maxItems":2}}),
            &["deploymentId", "methods"],
            false,
        ));
        tools.push(tool(
            "signup_policy_set",
            "Choose who may create an account on a deployment: open, invitation (only people invited with app_invitations_create), closed, or allowlist (exact emails or @domain entries). Omitting allowlist keeps the current entries.",
            json!({"deploymentId":text,"mode":{"type":"string","enum":["open","invitation","closed","allowlist"]},"allowlist":{"type":"array","items":{"type":"string","minLength":1,"maxLength":254}}}),
            &["deploymentId", "mode"],
            false,
        ));
        tools.extend(super::invitations::admin_tools());
    }
    if owner.scopes.iter().any(|s| s == "publish") {
        tools.extend([
            tool("secret_set", "Set a signing secret an action's http signature names, or, with use call, one a call action signs with or an API key it sends, or, with use google, the app's Google OAuth client as google_client_id and google_client_secret. The value is never returned. Read back with secrets_list, and the Google client with sign_in_methods_read.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":64},"value":{"type":"string","minLength":16,"maxLength":512},"use":{"type":"string","enum":["route","call","google"]}}), &["projectId","name","value"], false),
            tool("secret_delete", "Delete a signing secret no published action uses. A Google client secret is refused while Google sign-in is on.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":64}}), &["projectId","name"], false),
            tool("api_keys_create", "Issue an API key for one deployment, scoped to named actions with an HTTP route and to file visibilities. The key appears only in this answer.", json!({"deploymentId":text,"name":{"type":"string","minLength":1,"maxLength":80},"actions":{"type":"array","items":{"type":"string","minLength":1,"maxLength":100}},"files":{"type":"array","items":{"type":"string","enum":["public","private"]}}}), &["deploymentId","name"], false),
            tool("api_keys_revoke", "Revoke a deployment's API key by its ID.", json!({"deploymentId":text,"keyId":text}), &["deploymentId","keyId"], false),
            tool(
                "release_plan",
                "Validate a completed artifact transfer, pin the current project revision and answer the schema change the release makes: its statements, what they lose, and the confirmation release_apply needs when they lose data.",
                json!({"transferId":text}),
                &["transferId"],
                false,
            ),
            tool(
                "release_apply",
                "Apply a current release plan. Repeating an applied plan returns its receipt. When the plan's schema change loses data, pass its schema.confirmation as confirmSchema.",
                json!({"planId":text,"confirmSchema":{"type":"string","minLength":64,"maxLength":64}}),
                &["planId"],
                false,
            ),
        ]);
        tools.extend(super::domain::tools());
        tools.extend(super::path::tools());
    }
    if owner.session {
        tools.extend([
            tool("tokens_list", "List active delegated credentials without exposing their secrets.", json!({}), &[], true),
            tool("token_create", "Issue a revocable agent credential with selected scopes and project grants.", json!({"scopes":{"type":"array","items":{"type":"string","enum":["read","write","publish","manage_users","export_data"]},"minItems":1},"projectIds":{"type":"array","items":text},"ttlSeconds":{"type":"integer","minimum":60,"maximum":2592000}}), &["scopes"], false),
            tool("token_revoke", "Revoke an agent credential using its returned ID.", json!({"tokenId":text}), &["tokenId"], false),
        ]);
    }
    tools.extend(transfer_tools(false));
    tools
}

pub(super) fn runtime_tools() -> Vec<Value> {
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
fn account_tools() -> Vec<Value> {
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
fn anonymous_runtime_tools() -> Vec<Value> {
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

async fn admin(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, false).await
}
pub(super) async fn runtime(State(state): State<AppState>, request: Request) -> Response {
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
    if !runtime && super::security::from_app_origin(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Administration refuses requests from app origins.");
    }
    let host_known = super::oauth::valid_host(&state, &headers)
        || (runtime && (super::domain::is_alias_request(&state, &headers) || super::path::on_path_host(&state, &headers)));
    if !same_origin(&headers) || !host_known {
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
    // A visitor on a private app reaches only the account tools.
    let mut account_only = false;
    if runtime {
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
        if headers.get("x-konae-owner-id").is_some_and(|expected| expected.as_bytes() != authenticated.id.as_bytes()) {
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
    if account_only
        && !(["initialize", "ping"].contains(&method)
            || method.starts_with("notifications/")
            || (method == "tools/call"
                && call["params"]["name"].as_str().is_some_and(|name| tools.iter().any(|t| t["name"] == name))))
    {
        return unauthorized(&state, &headers);
    }
    if call.get("id").is_none() {
        return if method.starts_with("notifications/") {
            StatusCode::ACCEPTED.into_response()
        } else {
            rpc_error(Value::Null, -32600, "A request ID is required.")
        };
    }
    let mut clear_cookie = false;
    let mut session_cookie: Option<HeaderValue> = None;
    let result = match method {
        "initialize" => Ok(
            json!({"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"konae","version":"0.1.0"},"instructions":"Use project-scoped credentials. Source and builds stay on your computer. Call platform_describe, then platform_guide topics rules and pieces before building. After every write, use the read-back that rules names."}),
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
                // Name the permission an administrative tool needs, so the agent knows which credential to get.
                let hint = owner.as_ref().and_then(|owner| super::guide::missing_permission(name, owner));
                return rpc_error(id, -32602, &hint.map_or("Unknown tool or insufficient permission.".to_owned(), |hint| format!("Unknown tool or insufficient permission. {hint}")));
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
            // A package export's downloads hold accounts and data. Every
            // credential of one owner shares the transfer principal, so reading
            // them needs export_data, not the read scope that finds them.
            let package_transfer = (owner.is_some() && name.starts_with("transfer_") && name != "transfer_begin")
                .then(|| transfer(&state, &principal, &arguments).ok())
                .flatten();
            let export_transfer = package_transfer.clone().filter(|t| t["exportData"] == true);
            let component_upload = name == "transfer_write" && package_transfer.is_some_and(|t| t["component"].is_string());
            let result = if name.starts_with("transfer_") {
                let permission = if let Some(owner) = owner.as_ref() {
                    let project = if name == "transfer_begin" {
                        arguments["projectId"].as_str().map(str::to_owned)
                    } else {
                        transfer(&state, &principal, &arguments)
                            .ok()
                            .and_then(|t| t["projectId"].as_str().map(str::to_owned))
                    };
                    let scope = if export_transfer.is_some() && name != "transfer_cancel" {
                        "export_data"
                    } else if ["transfer_read", "transfer_status", "transfer_cancel"]
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
                        authorize(held(&["read", "publish", "write", "export_data"]))
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
            } else if runtime && account_tools().iter().any(|t| t["name"] == name) {
                let client = state.guards.client_ip(peer, &headers);
                match super::accounts::runtime_tool(&state, &headers, client, name, &arguments).await {
                    Ok((value, cookie)) => {
                        session_cookie = cookie;
                        Ok(value)
                    }
                    Err(e) => Err(e),
                }
            } else if let Some(owner) = owner.as_ref() {
                match super::maintenance::admin_guard(&state, name, &arguments).await {
                    Ok(()) => admin_call(&state, owner, &principal, name, arguments.clone()).await,
                    Err(e) => Err(e),
                }
            } else {
                runtime_call(&state, &headers, &principal, app.as_deref(), name, arguments.clone()).await
            };
            // Each audit row costs a write to the shared mcp.sqlite, so app users
            // must not decide how fast it grows. Owner calls are audited here.
            // Of the app-user calls, only an upload that stores a new file and
            // a delete that removes one are, inside runtime_call: the transfer
            // caps bound how often uploads happen, and each file goes once.
            if !runtime {
                match export_transfer.as_ref().filter(|_| name == "transfer_read") {
                    // One row per component download, written at its first
                    // block or on a refusal, so a 256 MiB database read in
                    // 64 KiB blocks does not push the rest out of the log.
                    Some(t) if arguments["offset"] == 0 || result.is_err() => {
                        let target = json!({"deploymentId":t["deploymentId"],"component":t["component"]});
                        audit(&state, &principal, "package_download", &target, None, None, &result);
                    }
                    Some(_) => {}
                    // A package component upload is audited at its first block.
                    None if component_upload && arguments["offset"] != 0 && result.is_ok() => {}
                    None => audit(&state, &principal, name, &arguments, app.as_deref(), previous_mode, &result),
                }
            }
            // The session row is gone; the browser must drop its cookie too.
            if runtime && name == "auth_logout" && result.is_ok() {
                clear_cookie = true;
            }
            match result {
                Ok(value) => Ok(
                    json!({"content":[{"type":"text","text":serde_json::to_string(&value).unwrap()}],"structuredContent":{"result":value},"isError":false}),
                ),
                Err(e) => Ok(tool_error(&e)),
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
            .insert(header::SET_COOKIE, super::cleared_app_cookie(&state, app.as_deref().unwrap_or_default(), &headers));
    }
    if let Some(cookie) = session_cookie {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
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
        "package_download" => (
            arguments["deploymentId"].as_str().map(str::to_owned),
            arguments["component"].as_str().map(str::to_owned),
        ),
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
            db.execute("DELETE FROM audit WHERE at<?1", [now_ms().saturating_sub(AUDIT_RETENTION_MS)])?;
            // Each owner, and the app users of each Deployment together, keep
            // their own newest rows, so an app user's uploads and deletes
            // never push an owner's history out of the log.
            let actor = text(principal);
            let (low, high) = audit_partition(&actor);
            db.execute(
                "DELETE FROM audit WHERE actor>=?1 AND actor<?2 AND id<=(SELECT id FROM audit WHERE actor>=?1 AND actor<?2 ORDER BY id DESC LIMIT 1 OFFSET ?3)",
                params![low, high, AUDIT_MAX_ROWS],
            )?;
        }
        Ok(())
    });
    if let Err(error_value) = written {
        eprintln!("Could not write the audit row for {principal} {name}: {error_value}");
    }
}

/// The range of `actor` values that share one audit allowance: every app
/// user of one Deployment (`runtime:<deployment>:...`), or one owner.
fn audit_partition(actor: &str) -> (String, String) {
    let prefix = actor
        .strip_prefix("runtime:")
        .and_then(|rest| rest.split_once(':'))
        .map(|(deployment, _)| format!("runtime:{deployment}:"))
        .unwrap_or_else(|| actor.to_owned());
    // The smallest string after every string that starts with the prefix.
    let high = if prefix.starts_with("runtime:") { format!("{};", &prefix[..prefix.len() - 1]) } else { format!("{prefix}\u{1}") };
    (prefix, high)
}

fn unauthorized(state: &AppState, headers: &HeaderMap) -> Response {
    let mut response = error(StatusCode::UNAUTHORIZED, "Sign in to use Konae MCP.");
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
        .header("x-konae-admin-key", &state.admin_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_value(response: Response) -> Result<Value> {
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

async fn admin_call(
    state: &AppState,
    owner: &Owner,
    principal: &str,
    name: &str,
    a: Value,
) -> Result<Value> {
    match name {
        "platform_guide" => super::guide::topic(a["topic"].as_str().unwrap()),
        "platform_describe" => Ok(
            json!({
                "name":"Konae",
                "protocolVersion":VERSION,
                "runtimeEndpoint":"/_konae/mcp",
                "chunkBytes":CHUNK,
                "maxTransferBytes":MAX_TRANSFER,
                "files":{"quotaBytes":state.files.limits.quota_bytes,"maxFileBytes":state.files.limits.max_file_bytes},
                "yardstick":"A person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.",
                "firstRun":"platform_guide topic rules, then pieces, then credentials. Publish with the publishing topic or bun run konae -- publish, then project_inspect and one actions_invoke.",
                "workflow":[
                    "platform_guide topic rules",
                    "platform_guide topic pieces",
                    "a credential: platform_guide topic credentials",
                    "project_create",
                    "secret_set for each secret a route signs with, and google_client_id and google_client_secret when the app offers Google sign-in",
                    "build locally",
                    "transfer_begin/write/complete",
                    "release_plan",
                    "release_apply",
                    "project_inspect"
                ],
                "guideTopics":super::guide::TOPICS,
                "runtimeTools":["auth_session","actions_invoke","files_list","files_read","files_upload","files_update","files_delete"],
                "localDevelopment":"Use bun run konae -- init, dev, build, contract, login, publish, pull, sql, conform or export; the development topic of platform_guide lists them. Development uses an isolated local Kernel. All platform operations use authenticated MCP. On a public app, actions declared anonymous run without a session, and visitors can list and read files uploaded as public. Uploads and private files require sign-in. Private apps require sign-in for everything."
            }),
        ),
        "mail_status" => super::mail::status_tool(state, a["refresh"].as_bool().unwrap_or(false)).await,
        "session_inspect" => Ok(
            json!({
                "ownerId":owner.id,
                "displayName":owner.display,
                "scopes":owner.scopes,
                "can":super::guide::session_can(owner),
                "projectIds":owner.projects,
                "browserSession":owner.session
            }),
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
            anyhow::ensure!(valid_app_id(&id), "Invalid project ID. Use lowercase letters, digits, \"_\" and \"-\".");
            let initial_sources = if let Some(transfer_id) = a["sourcesTransferId"].as_str() {
                let upload = transfer(state, principal, &json!({"transferId":transfer_id}))?;
                anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload"
                    && (upload["projectId"].is_null() || upload["projectId"] == id), "Completed upload for this project required.");
                let bytes = tokio::fs::read(transfer_path(state, transfer_id)?).await?;
                let source: Value = serde_json::from_slice(&bytes)?;
                validate_sources(&source, &id)?;
                anyhow::ensure!(source["name"] == a["name"]
                    && a.get("slug").is_none_or(|slug| *slug == source["slug"])
                    && a.get("manifest").is_none_or(|manifest| *manifest == source["manifest"]), "Initial files must match the project metadata.");
                Some(source)
            } else { None };
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let db = store(state)?;
            let existing: Option<String> = db
                .query_row("SELECT owner FROM projects WHERE id=?1", [&id], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(existing) = existing {
                anyhow::ensure!(existing == owner.id, "Project not found or access denied.");
                let mut project = owned_project(state, owner, &id, "read")?.0;
                if let Some(source) = initial_sources {
                    let revision: u64 = db.query_row("SELECT sources FROM projects WHERE id=?1", [&id], |r| r.get(0))?;
                    let revision = if revision > 0 {
                        let saved = super::sources::read(&state.data_root, &id).await?;
                        anyhow::ensure!(saved.as_ref() == Some(&source), "This project already has saved sources. Reload before saving.");
                        revision
                    } else {
                        super::sources::write(&state.data_root, &id, &source).await?;
                        db.execute("UPDATE projects SET sources=1 WHERE id=?1", [&id])?;
                        1
                    };
                    project["sourcesRevision"] = json!(revision);
                }
                return Ok(project);
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
            anyhow::ensure!(
                !crate::layout::folder(&state.data_root, &id).exists(),
                "An app folder with this ID already exists on this installation. Choose another project ID."
            );
            let template = project_template(a["template"].as_str())?;
            let mut project = json!({"id":id,"name":a["name"],"slug":a["slug"].as_str().map(str::to_owned).unwrap_or_else(||safe_slug(a["name"].as_str().unwrap())),"manifest":a.get("manifest").cloned().unwrap_or(json!({"version":1,"visibility":template["visibility"]})),"files":template["files"]});
            // The app's folder exists from the start: konae.json, and app/ once sources are saved.
            // A step that fails takes the folder away again, so a retry with the same ID works.
            let created = async {
                super::Settings::new(&id, project["name"].as_str().unwrap_or_default(), project["slug"].as_str().unwrap_or_default(), &project["manifest"])
                    .write(&state.data_root)
                    .await?;
                if let Some(source) = initial_sources.as_ref() {
                    super::sources::write(&state.data_root, &id, source).await?;
                    project["sourcesRevision"] = json!(1);
                }
                Ok::<_, anyhow::Error>(())
            }
            .await
            .and_then(|()| {
                db.execute(
                    "INSERT INTO projects(id,owner,body,sources) VALUES(?1,?2,?3,?4)",
                    params![id, owner.id, {
                        let mut metadata = project.clone();
                        let object = metadata.as_object_mut().unwrap();
                        object.remove("files");
                        object.remove("sourcesRevision");
                        metadata.to_string()
                    }, u64::from(initial_sources.is_some())],
                )?;
                Ok(())
            });
            if let Err(error_value) = created {
                let _ = tokio::fs::remove_dir_all(crate::layout::folder(&state.data_root, &id)).await;
                return Err(error_value);
            }
            Ok(project)
        }
        "project_inspect" => {
            let id = a["projectId"].as_str().unwrap();
            let (project, revision) = owned_project(state, owner, id, "read")?;
            let deployment = find_project_deployment(&*state.deployments.read().await, id);
            let sources_revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            Ok(json!({"project":project,"revision":revision,"sourcesRevision":sources_revision,"deployment":deployment}))
        }
        "project_sources_read" => {
            let id = a["projectId"].as_str().unwrap();
            owned_project(state, owner, id, "read")?;
            // A save swaps app/ under this lock, so a read never sees half of one.
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            match super::sources::read(&state.data_root, id).await? {
                Some(source) if revision > 0 => Ok(json!({"revision":revision,"transfer":download(state, principal, &serde_json::to_vec(&source)?, Some(id)).await?})),
                _ => Ok(json!({"revision":0,"transfer":null})),
            }
        }
        "project_sources_save" => {
            let id = a["projectId"].as_str().unwrap();
            let transfer_id = a["transferId"].as_str().unwrap();
            let upload = transfer(state, principal, &json!({"transferId":transfer_id}))?;
            anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == id, "Completed upload for this project required.");
            let bytes = tokio::fs::read(transfer_path(state, transfer_id)?).await?;
            let source: Value = serde_json::from_slice(&bytes)?;
            validate_sources(&source, id)?;
            let _guard = database_lock(state, &format!("project:{id}")).await;
            owned_project(state, owner, id, "write")?;
            let db = store(state)?;
            let current: u64 = db.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            anyhow::ensure!(current == a["expectedRevision"].as_u64().unwrap(), "Source revision conflict. Reload the saved project before writing.");
            let revision = current.checked_add(1).ok_or_else(|| anyhow::anyhow!("Source revision exhausted."))?;
            super::sources::write(&state.data_root, id, &source).await?;
            db.execute("UPDATE projects SET sources=?2 WHERE id=?1", params![id, revision])?;
            Ok(json!({"revision":revision}))
        }
        "project_delete" => {
            let id = a["projectId"].as_str().unwrap();
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let (project, _) = owned_project(state, owner, id, "write")?;
            anyhow::ensure!(project["name"] == a["name"], "{}", DELETE_NAME_MISMATCH);
            let deleted = delete_project(state, id).await?;
            Ok(json!({"deleted":true,"projectId":id,"deployments":deleted}))
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
        "secrets_list" | "secret_set" | "secret_delete" => {
            let id = a["projectId"].as_str().unwrap();
            owned_project(state, owner, id, if name == "secrets_list" { "read" } else { "publish" })?;
            // Releases hold this lock, so a delete cannot pass between a release's check and its apply.
            let _guard = database_lock(state, &format!("project:{id}")).await;
            let deployment = find_project_deployment(&*state.deployments.read().await, id);
            if name == "secret_set" && super::http::sets_route_secret(&a) {
                anyhow::ensure!(!route_secret_reused(state, &owner.id, id, a["value"].as_str().unwrap_or(""))?, super::http::SECRET_REUSED);
            }
            super::http::secrets_call(&super::http::secrets_store(&state.data_root, id)?, id, deployment.as_ref(), name, &a)
        }
        "api_keys_list" | "api_keys_create" | "api_keys_revoke" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, if name == "api_keys_list" { "read" } else { "publish" }).await?;
            super::http::api_keys_call(state, &deployment, name, &a).await
        }
        "notices_read" => {
            let deployment = owned_deployment(state, owner, a["deploymentId"].as_str().unwrap(), "read").await?;
            super::mail::notices_tool(state, &deployment, &a).await
        }
        "calls_read" => {
            let deployment = owned_deployment(state, owner, a["deploymentId"].as_str().unwrap(), "read").await?;
            super::calls::admin_call(state, &deployment, &a).await
        }
        "usage_overview" => {
            anyhow::ensure!(owner.scopes.iter().any(|s| s == "read"), "Permission denied for read.");
            let cursor = a["cursor"].as_str().unwrap_or("");
            let rows = {
                let db = store(state)?;
                let mut q = db.prepare("SELECT id, body FROM projects WHERE owner = ?1 AND id > ?2
                    AND (?3 = '[]' OR id IN (SELECT value FROM json_each(?3))) ORDER BY id LIMIT 51")?;
                let result = q.query_map(params![owner.id, cursor, serde_json::to_string(&owner.projects)?],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                result
            };
            let next = if rows.len() > 50 { rows.get(49).map(|r| r.0.clone()) } else { None };
            let deployments = state.deployments.read().await;
            let items = rows.into_iter().take(50).map(|(id, body)| {
                let project: Value = serde_json::from_str(&body)?;
                let deployment = find_project_deployment(&deployments, &id);
                Ok((id, project["name"].clone(), deployment))
            }).collect::<Result<Vec<_>>>()?;
            drop(deployments);
            let root = state.data_root.clone();
            let from = a["from"].as_str().unwrap_or("").to_owned();
            let to = a["to"].as_str().unwrap_or("").to_owned();
            tokio::task::spawn_blocking(move || {
                // Validate the range even for an account without deployments.
                super::usage::validate_range(&from, &to)?;
                let flushed = super::usage::flush_status();
                let apps: Vec<Value> = items.into_iter().map(|(id, name, deployment)| {
                    match deployment {
                        None => json!({"projectId": id, "name": name, "summary": null}),
                        Some(d) => match super::usage::read_summary(&root, &d, &from, &to, flushed) {
                            Ok(summary) => json!({"projectId": id, "name": name, "summary": super::usage::compact_overview(summary)}),
                            Err(_) => json!({"projectId": id, "name": name, "summary": null, "error": "Usage could not be read. Try again."}),
                        }
                    }
                }).collect();
                Ok(json!({"apps": apps, "nextCursor": next, "from": from, "to": to}))
            }).await?
        }
        "usage_read" | "signals_read" | "usage_summary" => {
            let deployment = owned_deployment(state, owner, a["deploymentId"].as_str().unwrap(), "read").await?;
            super::usage::admin_call(&state.data_root, &deployment, name, &a).await
        }
        "sign_in_methods_read" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "read").await?;
            Ok(sign_in_methods_value(state, &deployment))
        }
        "sign_in_methods_set" => {
            let id = a["deploymentId"].as_str().unwrap();
            let found = owned_deployment(state, owner, id, "manage_users").await?;
            // Releases and secret_delete hold the same lock, so neither a
            // publish nor a deleted Google secret can pass this check.
            let _guard = database_lock(state, &format!("project:{}", found.project_id)).await;
            let mut updated = owned_deployment(state, owner, id, "manage_users").await?;
            let methods = requested_sign_in_methods(state, &updated, &a["methods"])?;
            let previous = updated.sign_in_methods.clone();
            updated.sign_in_methods = methods;
            persist_deployment(state, &updated).await?;
            state.deployments.write().await.insert(updated.hostname.clone(), updated.clone());
            let mut value = sign_in_methods_value(state, &updated);
            value["previousMethods"] = json!(previous);
            Ok(value)
        }
        "signup_policy_read" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "read").await?;
            Ok(signup_policy_value(&deployment))
        }
        "signup_policy_set" => {
            let id = a["deploymentId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "manage_users").await?;
            let mode: SignupMode = serde_json::from_value(a["mode"].clone())?;
            let entries: Vec<String> = match a.get("allowlist") {
                Some(list) => serde_json::from_value(list.clone())?,
                None => deployment.signup_policy.allowlist.clone(),
            };
            let policy = SignupPolicy::from_input(mode, &entries).map_err(anyhow::Error::msg)?;
            // Releases hold the same lock, so a publish cannot overwrite this change
            // with the policy it read before.
            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            let mut updated = owned_deployment(state, owner, id, "manage_users").await?;
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
        "project_export" => super::package::export(state, owner, principal, a["deploymentId"].as_str().unwrap()).await,
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
            if let Some(package) = input.initial_package.as_ref() {
                // The same package again answers with the first import.
                if let Some(replay) = super::package::replay_import(state, owner, &input.project.id, &package.manifest.package_id)? {
                    return Ok(replay);
                }
                super::package::verify(state, principal, &input.project.id, package).await?;
            }
            let schema = validate_release(state, &input).await?;
            let id = Uuid::new_v4().to_string();
            let expires = now_ms() + 900_000;
            store(state)?.execute("INSERT INTO plans(id,owner,project,revision,transfer,expires) VALUES(?1,?2,?3,?4,?5,?6)",params![id,owner.id,input.project.id,revision,a["transferId"].as_str().unwrap(),expires])?;
            if let Some(package) = input.initial_package.as_ref() {
                super::package::remember_import(state, &package.manifest.package_id, &input.project.id, &id)?;
            }
            Ok(
                json!({"planId":id,"projectId":input.project.id,"expectedRevision":revision,"artifactSha256":transfer["sha256"],"visibility":input.visibility,"expiresAt":expires,"schema":schema.to_json()}),
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
            let mut payload: Value = serde_json::from_slice(&bytes)?;
            // The owner's email is where notify to the owner sends. It comes
            // from Konae's owner record, never from the payload.
            payload["contactEmail"] = owner_email(state, &owner.id).map_or(Value::Null, Value::String);
            // A schema change that loses data applies only with the
            // confirmation release_plan gave for it.
            payload["schemaConfirmation"] = a.get("confirmSchema").cloned().unwrap_or(Value::Null);
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
        name if name.starts_with("app_") => super::invitations::admin_call(state, owner, name, &a).await,
        name if name.starts_with("domain_") => super::domain::admin_call(state, owner, name, &a).await,
        name if name.starts_with("path_") => super::path::admin_call(state, owner, name, &a).await,
        _ => anyhow::bail!("Unknown tool."),
    }
}

/// The email of the owner's active account, when Konae keeps one. An owner
/// who signs in through Clerk has none.
fn owner_email(state: &AppState, owner: &str) -> Option<String> {
    let db = Connection::open_with_flags(state.data_root.join("owners.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row("SELECT email FROM app_users WHERE id = ?1 AND state = 'active'", [owner], |row| row.get(0)).optional().ok().flatten()
}

const METHODS_INVALID: &str = "Name the methods as password and google.";

/// The methods `sign_in_methods_set` may store: password always, and
/// google only once the app's Google client ID and secret are both set.
fn requested_sign_in_methods(state: &AppState, deployment: &Deployment, requested: &Value) -> Result<Vec<SignInMethod>> {
    use super::refusal::refuse;
    let requested: Vec<SignInMethod> = serde_json::from_value(requested.clone())
        .map_err(|_| refuse("signin.methods-invalid", METHODS_INVALID))?;
    let methods = sign_in_methods_from_input(&requested).map_err(|message| refuse("signin.password-required", message))?;
    if methods.contains(&SignInMethod::Google) && !super::google::client_set(state, deployment).complete() {
        return Err(refuse("signin.google-client-unset", super::google::NEEDS_CLIENT).into());
    }
    Ok(methods)
}

/// The methods, what the sign-in page offers, whether each half of the app's
/// Google client is set (never its value) and the callbacks to register. A
/// Deployment that asks for Google without its client here, as after an
/// import, keeps the setting and offers only the password; `notice` says so.
fn sign_in_methods_value(state: &AppState, deployment: &Deployment) -> Value {
    let client = super::google::client_set(state, deployment);
    let mut offered = vec![SignInMethod::Password];
    if super::google::offered(state, deployment) {
        offered.push(SignInMethod::Google);
    }
    let mut value = json!({
        "deploymentId": deployment.id,
        "methods": deployment.sign_in_methods,
        "offered": offered,
        "googleClient": {"clientIdSet": client.id, "clientSecretSet": client.secret},
        "redirectUris": super::google::redirect_uris(state, deployment),
    });
    if deployment.sign_in_methods.contains(&SignInMethod::Google) && !client.complete() {
        value["notice"] = json!(GOOGLE_CLIENT_MISSING);
    }
    value
}

const GOOGLE_CLIENT_MISSING: &str = "This app asks for Google sign-in, but its Google client ID and client secret are not set on this installation, so its sign-in page offers only email and password. Set both, and add this app's redirect URIs to the Google client. Until then, people who signed in with Google sign in with a password, or set one with Forgot your password.";

fn signup_policy_value(deployment: &Deployment) -> Value {
    json!({"deploymentId":deployment.id,"visibility":if is_private(deployment) { "private" } else { "public" },"mode":deployment.signup_policy.mode,"allowlist":deployment.signup_policy.allowlist})
}

/// The Capsule a project is born as: one of the templates in
/// project-templates.json, which `bun run templates:render` writes from the
/// editor's templates and `konae init` reads too.
fn project_template(id: Option<&str>) -> Result<Value> {
    let all: Value = serde_json::from_str(include_str!("project-templates.json"))?;
    let id = id.or(all["default"].as_str()).unwrap_or_default();
    let template = &all["templates"][id];
    anyhow::ensure!(template.is_object(), "Unknown template {id}.");
    Ok(template.clone())
}

/// A tool's failed result. The text stays the sentence clients read today; a
/// refused Capsule also answers `{ error: { code, message, hint } }` (ADR-0020).
fn tool_error(error: &anyhow::Error) -> Value {
    let text = json!([{"type": "text", "text": error.to_string()}]);
    match super::refusal::of(error) {
        Some(refused) => json!({"content": text, "structuredContent": {"error": refused.to_json()}, "isError": true}),
        None => json!({"content": text, "isError": true}),
    }
}

/// Checks a release as the deploy would take it, and answers the schema
/// change it makes, derived on a copy of the live database.
pub(super) async fn validate_release(state: &AppState, input: &DeployInput) -> Result<super::schema_change::Change> {
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
    let program: ActionProgram = serde_json::from_str(
        input
            .project
            .files
            .get("actions.json")
            .ok_or_else(|| anyhow::anyhow!("actions.json is required."))?,
    )?;
    anyhow::ensure!(program.version == 1, "Unsupported action version.");
    super::action_shape::check_program(&program)?;
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
        if let Some(ref deployment) = existing {
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
            let result = response_value(session(State(state.clone()), headers.clone()).await).await;
            if let (Ok(_), Some(id)) = (&result, app) {
                super::usage::count(&state.data_root, id, super::usage::Operation::Session);
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
                super::usage::count(&state.data_root, id, super::usage::Operation::FileRead);
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
            store(state)?.execute("INSERT INTO file_receipts(transfer_id,principal,request_hash,body) VALUES(?1,?2,?3,?4)",params![id,principal,request_hash,result.to_string()])?;
            if let Some(deployment) = app {
                super::usage::count(&state.data_root, deployment, super::usage::Operation::Upload);
            }
            // Failed uploads and receipt replays leave no row.
            let result = Ok(result);
            audit(state, principal, name, &a, app, None, &result);
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
                audit(state, principal, name, &a, app, None, &result);
            }
            result
        }
        _ => anyhow::bail!("Unknown runtime tool."),
    }
}

pub(super) fn transfer_path(state: &AppState, id: &str) -> Result<PathBuf> {
    anyhow::ensure!(Uuid::parse_str(id).is_ok(), "Invalid transfer ID.");
    Ok(state.data_root.join("transfers").join(id))
}

pub(super) fn transfer(state: &AppState, principal: &str, a: &Value) -> Result<Value> {
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

pub(super) async fn new_transfer(state: &AppState, principal: &str, value: Value) -> Result<Value> {
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
    // Signed-out visitors of a public app, one principal per client address,
    // share a smaller pool of their own, so visitors from a handful of
    // networks cannot hold the slots signed-in users upload with.
    let (pool, excluded, limit, per_principal, ttl) = match principal.strip_prefix("runtime:") {
        Some(rest) => {
            let deployment = rest.split(':').next().unwrap_or("");
            let visitors = format!("runtime:{deployment}:anon:");
            if principal.starts_with(&visitors) {
                (visitors, None, MAX_VISITOR_TRANSFERS, MAX_RUNTIME_PRINCIPAL_TRANSFERS, RUNTIME_TRANSFER_TTL_MS)
            } else {
                (format!("runtime:{deployment}:"), Some(visitors), MAX_DEPLOYMENT_TRANSFERS, MAX_RUNTIME_PRINCIPAL_TRANSFERS, RUNTIME_TRANSFER_TTL_MS)
            }
        }
        None => ("owner:".to_owned(), None, MAX_OWNER_TRANSFERS, MAX_OWNER_PRINCIPAL_TRANSFERS, OWNER_TRANSFER_TTL_MS),
    };
    let total: usize = db.query_row(
        "SELECT COUNT(*) FROM transfers WHERE substr(principal,1,length(?1))=?1
           AND (?2 IS NULL OR substr(principal,1,length(?2))<>?2)",
        params![&pool, excluded],
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

pub(super) async fn download(
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

/// Streams a response body into a new download transfer, hashing it on the
/// way, so a file is never held in memory whole.
async fn download_body(state: &AppState, principal: &str, body: Body) -> Result<Value> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let directory = state.data_root.join("transfers");
    tokio::fs::create_dir_all(&directory).await?;
    let scratch = directory.join(format!(".download-{}", Uuid::new_v4()));
    let written = async {
        let mut output = tokio::fs::File::create(&scratch).await?;
        let mut hasher = sha2::Sha256::default();
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            sha2::Digest::update(&mut hasher, &chunk);
            output.write_all(&chunk).await?;
            size += chunk.len() as u64;
        }
        output.flush().await?;
        Ok::<_, anyhow::Error>((size, hex::encode(sha2::Digest::finalize(hasher))))
    }
    .await;
    let created = match written {
        Ok((size, sha256)) => new_transfer(state, principal, json!({"direction":"download","size":size,"sha256":sha256,"complete":true,"projectId":null})).await,
        Err(error_value) => Err(error_value),
    };
    let moved = match created {
        Ok(created) => match tokio::fs::rename(&scratch, transfer_path(state, created["transferId"].as_str().unwrap_or_default())?).await {
            Ok(()) => return Ok(created),
            Err(error_value) => Err(error_value.into()),
        },
        Err(error_value) => Err(error_value),
    };
    let _ = tokio::fs::remove_file(&scratch).await;
    moved
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
        let component = a["component"].as_str().filter(|_| !runtime);
        let limit = match component {
            _ if runtime => state.files.limits.max_file_bytes,
            Some(component) => super::package::component_limit(component, &state.files.limits)?,
            None => MAX_TRANSFER as u64,
        };
        match component {
            Some(component) => anyhow::ensure!(
                size > 0 && size <= limit,
                "Transfer size is outside the limit of {limit} bytes for the {component} component."
            ),
            None if runtime => anyhow::ensure!(size > 0 && size <= limit, "{}", state.files.limits.file_size_refused()),
            None => anyhow::ensure!(size > 0 && size <= limit, "Transfer size is outside the limit."),
        }
        anyhow::ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid SHA-256 checksum."
        );
        let mut body = json!({"direction":"upload","size":size,"sha256":hash,"complete":false,"projectId":a["projectId"]});
        if let Some(component) = component {
            super::package::ensure_upload_space(state, size)?;
            body["component"] = json!(component);
        }
        return new_transfer(state, principal, body).await;
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
            // Hashes in 64 KiB reads, so a package component never sits in memory.
            let (size, sha256) =
                tokio::task::spawn_blocking(move || super::copies::checksum_file(&path)).await??;
            anyhow::ensure!(
                size == t["size"].as_u64().unwrap() && sha256 == t["sha256"],
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

async fn owner_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
    match super::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => Json(json!({"mode":"clerk","publishableKey":config.publishable_key})).into_response(),
        Ok(None) => Json(json!({"mode":"local","admission":super::admission::mode(&state).as_str()})).into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not configured."),
    }
}

/// Who the browser is signed in as. A signed-out visit answers 200 with
/// `signedIn: false`, so the editor's first check is not a 401 in the console.
/// `session_inspect` stays the tool that lists scopes.
pub(super) async fn owner_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
    let owner = match authenticate_owner(&state, &headers) {
        Ok(owner) => owner,
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not available."),
    };
    let body = match owner {
        Some(owner) => json!({"signedIn": true, "ownerId": owner.id, "displayName": owner.display}),
        None => json!({"signedIn": false}),
    };
    let mut response = Json(body).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
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

pub(super) async fn owner_login(State(state): State<AppState>, request: Request) -> Response {
    owner_auth(state, request, false).await
}
pub(super) async fn owner_signup(State(state): State<AppState>, request: Request) -> Response {
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
    // An invitation code, or the installation key while it may still create
    // the first creator. admission.rs consumes either inside the signup.
    let admission = if signup {
        match super::admission::read(&state, &a) {
            Ok(admission) => Some(admission),
            Err(e) => return auth_error(e),
        }
    } else {
        None
    };
    let email = a["email"].as_str().unwrap_or("").to_owned();
    let password = a["password"].as_str().unwrap_or("").to_owned();
    if let Some(admission) = admission.clone().filter(|_| state.accounts.verification == auth::Verification::Required) {
        let page = super::admission::login_page(
            &super::oauth::origin(&state, &headers),
            super::admission::return_to(&a).as_deref(),
        );
        return match super::accounts::signup_creator(&state, &email, &page, admission, client).await {
            Ok(created) => {
                if let Some(user_id) = created {
                    claim_for_first_owner(&state, &user_id).await;
                }
                super::accounts::link_sent()
            }
            Err(e) => auth_error(e),
        };
    }
    // Only signup needs the lock: it spans the insert and the first-owner claim
    // below. The password hash runs before it, and logins run concurrently.
    let (result, _guard) = if let Some(admission) = admission {
        match blocking(move || auth::NewAccount::prepare(&email, &password)).await {
            Ok(account) => {
                let guard = database_lock(&state, "owner-auth").await;
                let auth = super::accounts::creator_store(&state);
                let admit = super::admission::admit(admission);
                (blocking(move || auth?.create_account_admitted(account, admit)).await, Some(guard))
            }
            Err(error_value) => (Err(error_value), None),
        }
    } else {
        let failed_email = email.clone();
        let auth = super::accounts::creator_store(&state);
        let result = blocking(move || auth?.login(&email, &password)).await;
        if matches!(result, Err(AuthError::InvalidCredentials)) {
            state.guards.auth_failed("owner", client, &failed_email);
        }
        (result, None)
    };
    match result {
        Ok(session) => {
            if signup {
                claim_for_first_owner(&state, &session.user.user_id).await;
            }
            owner_session_response(&state, session)
        }
        Err(e) => auth_error(e),
    }
}

/// The installation operator claims legacy deployments during the first setup.
async fn claim_for_first_owner(state: &AppState, user_id: &str) {
    if let Ok(db) = store(state) {
        let owner_db = Connection::open(state.data_root.join("owners.sqlite")).unwrap();
        let owners: u64 = owner_db
            .query_row("SELECT COUNT(*) FROM app_users", [], |r| r.get(0))
            .unwrap_or(0);
        if owners == 1 {
            for d in state.deployments.read().await.values() {
                let _=db.execute("INSERT OR IGNORE INTO projects(id,owner,body) VALUES(?1,?2,?3)",params![d.project_id,user_id,json!({"id":d.project_id,"name":d.project_name,"manifest":d.manifest}).to_string()]);
            }
        }
    }
}

/// The owner session cookie and body for a creator session just issued.
pub(super) fn owner_session_response(state: &AppState, session: auth::AuthSession) -> Response {
    let cookie_name = owner_cookie_name(state);
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
    if let Ok(auth) = super::accounts::creator_store(&state) {
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

    /// A Kernel route that refuses with `refusal_error` keeps its code
    /// through a tool that calls the route.
    #[tokio::test]
    async fn a_route_refusal_keeps_its_code_through_the_tool() {
        let sentence = "Action a.b has an invalid guard. Use a message of 1 to 200 characters on one line.";
        let refused = super::super::refusal::refuse("guard.invalid", sentence);
        let error = response_value(super::super::refusal_error(&refused)).await.unwrap_err();
        let answer = tool_error(&error);
        assert_eq!(answer["content"][0]["text"], sentence);
        assert_eq!(answer["structuredContent"]["error"]["code"], "guard.invalid");
        assert_eq!(answer["structuredContent"]["error"]["hint"]["fix"], "capsule");
        let plain = response_value(super::super::error(StatusCode::BAD_REQUEST, "Unknown database operation.")).await.unwrap_err();
        assert!(tool_error(&plain).get("structuredContent").is_none());
    }

    /// A refused Capsule answers its code beside the sentence; any other
    /// failure answers the sentence alone, as before.
    #[test]
    fn a_tool_refusal_answers_its_code_and_hint_beside_the_text() {
        let refused = anyhow::Error::new(super::super::refusal::refuse("guard.invalid", "Action a.b has an invalid guard. Use a message of 1 to 200 characters on one line."));
        let answer = tool_error(&refused);
        assert_eq!(answer["content"][0]["text"], "Action a.b has an invalid guard. Use a message of 1 to 200 characters on one line.");
        assert_eq!(answer["isError"], true);
        assert_eq!(answer["structuredContent"]["error"]["code"], "guard.invalid");
        assert_eq!(answer["structuredContent"]["error"]["message"], answer["content"][0]["text"]);
        assert_eq!(answer["structuredContent"]["error"]["hint"]["fix"], "capsule");
        let other = tool_error(&anyhow::anyhow!("Upload is not complete."));
        assert_eq!(other, json!({"content": [{"type": "text", "text": "Upload is not complete."}], "isError": true}));
    }

    #[tokio::test]
    async fn visitors_cannot_hold_the_transfers_signed_in_users_need() {
        let root = std::env::temp_dir().join(format!("konae-visitor-transfers-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let begin = |principal: String| { let state = state.clone(); async move { new_transfer(&state, &principal, json!({"direction": "download", "size": 1})).await } };
        for client in 0..MAX_VISITOR_TRANSFERS / MAX_RUNTIME_PRINCIPAL_TRANSFERS {
            for _ in 0..MAX_RUNTIME_PRINCIPAL_TRANSFERS {
                begin(format!("runtime:d1:anon:198.51.100.{client}")).await.unwrap();
            }
        }
        let refused = begin("runtime:d1:anon:203.0.113.9".into()).await.unwrap_err().to_string();
        assert!(refused.contains("Too many active transfers"), "{refused}");
        // Signed-in users still have their whole pool.
        for user in 0..MAX_DEPLOYMENT_TRANSFERS / MAX_RUNTIME_PRINCIPAL_TRANSFERS {
            for _ in 0..MAX_RUNTIME_PRINCIPAL_TRANSFERS {
                begin(format!("runtime:d1:user-{user}")).await.unwrap();
            }
        }
        assert!(begin("runtime:d1:user-extra".into()).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The runtime schema leaves the size bound to the handler, which knows
    /// the installation's per-file limit and names it.
    #[tokio::test]
    async fn a_runtime_upload_is_bounded_by_the_configured_per_file_limit() {
        let root = std::env::temp_dir().join(format!("konae-runtime-limit-{}", Uuid::new_v4()));
        let (mut state, _) = crate::tests::login_test_state(&root, "public").await;
        state.files.limits = crate::file_store::FileLimits::parse(Some("10485760"), Some("5242880")).unwrap();
        let schema = transfer_tools(true).into_iter().find(|tool| tool["name"] == "transfer_begin").unwrap()["inputSchema"].clone();
        let sha = "0".repeat(64);
        let three_mib = json!({"size": 3 * 1024 * 1024, "sha256": sha});
        validate_arguments(&three_mib, &schema).unwrap();
        assert!(transfer_call(&state, "runtime:d1:user-a", "transfer_begin", &three_mib, true).await.is_ok());
        let six_mib = json!({"size": 6 * 1024 * 1024, "sha256": sha});
        validate_arguments(&six_mib, &schema).unwrap();
        let refused = transfer_call(&state, "runtime:d1:user-a", "transfer_begin", &six_mib, true).await.unwrap_err().to_string();
        assert_eq!(refused, "Files must be between 1 byte and 5 MiB.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn deleting_a_project_takes_its_deployment_files_and_address_with_it() {
        let root = std::env::temp_dir().join(format!("konae-delete-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().unwrap().clone();
        persist_deployment(&state, &deployment).await.unwrap();
        tokio::fs::write(crate::layout::database(&state.data_root, &deployment.id), b"rows").await.unwrap();
        let owner = Owner { id: "o".into(), display: None, scopes: vec!["read".into(), "write".into()], projects: vec![], session: false };
        let published = deployment.project_id.clone();
        let draft = "app_draft";
        {
            let db = store(&state).unwrap();
            for (id, name) in [(published.as_str(), "Notes"), (draft, "Draft")] {
                db.execute("INSERT INTO projects(id,owner,body) VALUES(?1,'o',?2)", params![id, json!({"id": id, "name": name}).to_string()]).unwrap();
            }
            db.execute("UPDATE projects SET sources=1 WHERE id=?1", [draft]).unwrap();
        }
        let saved = json!({"id": draft, "name": "Draft", "slug": "draft", "files": {"index.html": "<main></main>"}, "directories": [], "manifest": {}, "updatedAt": 1});
        crate::sources::write(&state.data_root, draft, &saved).await.unwrap();
        let call = |name: &'static str, a: Value| { let (state, owner) = (state.clone(), owner.clone()); async move { admin_call(&state, &owner, "owner:o", name, a).await } };

        let refused = call("project_delete", json!({"projectId": published, "name": "notes"})).await.unwrap_err();
        assert_eq!(refused.to_string(), DELETE_NAME_MISMATCH);
        assert_eq!(state.deployments.read().await.len(), 1, "a refused delete keeps the app");

        let deleted = call("project_delete", json!({"projectId": published, "name": "Notes"})).await.unwrap();
        assert_eq!(deleted["deployments"], json!([deployment.id]));
        assert!(state.deployments.read().await.is_empty(), "the address stops answering");
        assert!(!crate::layout::folder(&state.data_root, &deployment.id).exists(), "its folder is gone");

        call("project_delete", json!({"projectId": draft, "name": "Draft"})).await.unwrap();
        let listed = call("projects_list", json!({})).await.unwrap();
        assert_eq!(listed["projects"], json!([]));
        assert!(!crate::layout::folder(&state.data_root, draft).exists(), "a project never published leaves no folder");
        let again = call("project_delete", json!({"projectId": draft, "name": "Draft"})).await.unwrap_err();
        assert_eq!(again.to_string(), "Project not found or access denied.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn an_app_id_is_lowercase_and_never_a_lock_the_kernel_keeps() {
        assert!(valid_app_id("app_0f2c-notes"));
        for refused in ["", "App_x", "app/x", "app.x", "domains", "paths", &"a".repeat(101)] {
            assert!(!valid_app_id(refused), "{refused}");
        }
    }

    #[tokio::test]
    async fn a_project_create_that_fails_leaves_no_folder_and_a_retry_works() {
        let root = std::env::temp_dir().join(format!("konae-create-retry-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let owner = Owner { id: "ana".into(), display: None, scopes: vec!["read".into(), "write".into()], projects: vec![], session: false };
        let manifest = json!({"version": 1, "visibility": "private", "runtime": {"database": {"mode": "declared-actions", "actions": []}, "auth": {"mode": "required"}, "files": {"access": "deployment"}}});
        let create = |files: Value| {
            let (state, owner, manifest) = (state.clone(), owner.clone(), manifest.clone());
            async move {
                let source = json!({"id": "app_retry", "name": "Retry", "slug": "retry", "files": files, "directories": [], "manifest": manifest, "updatedAt": 1});
                let upload = new_transfer(&state, "owner:ana", json!({"direction": "upload", "size": 0, "sha256": "", "complete": true, "projectId": null})).await.unwrap();
                let id = upload["transferId"].as_str().unwrap().to_owned();
                tokio::fs::write(transfer_path(&state, &id).unwrap(), serde_json::to_vec(&source).unwrap()).await.unwrap();
                admin_call(&state, &owner, "owner:ana", "project_create", json!({"projectId": "app_retry", "name": "Retry", "slug": "retry", "manifest": manifest, "sourcesTransferId": id})).await
            }
        };
        // A file and a folder of the same name cannot both be written into app/.
        assert!(create(json!({"page": "<main></main>", "page/nested.html": "<main></main>"})).await.is_err());
        assert!(!crate::layout::folder(&root, "app_retry").exists(), "the failed create left no folder");
        create(json!({"index.html": "<main></main>"})).await.unwrap();
        assert!(crate::layout::sources(&root, "app_retry").join("index.html").is_file());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn one_owner_cannot_give_two_apps_the_same_route_secret() {
        let root = std::env::temp_dir().join(format!("konae-secret-reuse-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let owner = |id: &str| Owner { id: id.into(), display: None, scopes: vec!["read".into(), "write".into(), "publish".into()], projects: vec![], session: false };
        {
            let db = store(&state).unwrap();
            for (id, who) in [("app_a", "ana"), ("app_b", "ana"), ("app_c", "ben")] {
                db.execute("INSERT INTO projects(id,owner,body) VALUES(?1,?2,?3)", params![id, who, json!({"id": id, "name": id}).to_string()]).unwrap();
            }
        }
        let set = |who: &'static str, project: &'static str, name: &'static str, usage: &'static str| {
            let state = state.clone();
            async move {
                admin_call(&state, &owner(who), &format!("owner:{who}"), "secret_set", json!({"projectId": project, "name": name, "value": "a-long-enough-secret", "use": usage})).await
            }
        };
        set("ana", "app_a", "hook", "route").await.unwrap();
        assert_eq!(set("ana", "app_b", "hook", "route").await.unwrap_err().to_string(), super::super::http::SECRET_REUSED);
        set("ana", "app_b", "outgoing", "call").await.unwrap();
        set("ben", "app_c", "hook", "route").await.unwrap();
        assert!(crate::layout::secrets(&root, "app_a").is_file(), "the secret lives in the app's data");
        let registry: i64 = store(&state).unwrap()
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%secret%'", [], |r| r.get(0)).unwrap();
        assert_eq!(registry, 0, "mcp.sqlite holds no secret");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn app_users_cannot_push_an_owners_rows_out_of_the_audit_log() {
        let root = std::env::temp_dir().join(format!("konae-audit-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let ok: Result<Value> = Ok(json!({}));
        audit(&state, "owner:ana", "project_create", &json!({"projectId": "p1"}), None, None, &ok);
        let rows = AUDIT_MAX_ROWS + 3 * AUDIT_PRUNE_INTERVAL;
        {
            let db = store(&state).unwrap();
            let insert = "INSERT INTO audit(at,actor,tool,target,ok) VALUES(?1,?2,'files_upload','d1',1)";
            for index in 0..rows - 1 {
                db.execute(insert, params![now_ms(), format!("runtime:d1:user-{}", index % 7)]).unwrap();
            }
        }
        // The next app user row lands on a prune.
        let next = store(&state).unwrap().query_row("SELECT COALESCE(MAX(id),0)+1 FROM audit", [], |row| row.get::<_, i64>(0)).unwrap();
        {
            let db = store(&state).unwrap();
            for _ in next..(next / AUDIT_PRUNE_INTERVAL + 1) * AUDIT_PRUNE_INTERVAL {
                db.execute("INSERT INTO audit(at,actor,tool,target,ok) VALUES(?1,'runtime:d1:user-x','files_delete','d1',1)", [now_ms()]).unwrap();
            }
        }
        audit(&state, "runtime:d1:user-y", "files_upload", &json!({}), Some("d1"), None, &ok);
        let db = store(&state).unwrap();
        let count = |actor: &str| db.query_row("SELECT COUNT(*) FROM audit WHERE actor LIKE ?1", [actor], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count("owner:ana"), 1, "the owner's row survives");
        assert_eq!(count("runtime:d1:%"), AUDIT_MAX_ROWS, "the Deployment's app users keep their newest rows");
        assert_eq!(audit_partition("runtime:d1:user-1"), ("runtime:d1:".to_owned(), "runtime:d1;".to_owned()));
        assert_eq!(audit_partition("owner:ana"), ("owner:ana".to_owned(), "owner:ana\u{1}".to_owned()));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

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
                "auth_signup",
                "auth_verify",
                "auth_recover",
                "auth_reset",
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
        assert!(names(&reader).contains(&"sign_in_methods_read".to_owned()));
        assert!(!names(&reader).contains(&"sign_in_methods_set".to_owned()));
        let publisher = Owner { scopes: vec!["read".into(), "publish".into()], ..reader.clone() };
        assert!(!names(&publisher).contains(&"signup_policy_set".to_owned()), "publish alone does not change who may sign up");
        assert!(!names(&publisher).iter().any(|name| name.starts_with("app_")));
        let manager = Owner { scopes: vec!["read".into(), "manage_users".into()], ..reader };
        for tool in ["signup_policy_set", "sign_in_methods_set", "app_users_list", "app_user_suspend", "app_user_resume", "app_invitations_create", "app_invitations_list", "app_invitation_resend", "app_invitation_revoke"] {
            assert!(names(&manager).contains(&tool.to_owned()), "{tool}");
        }
    }

    #[test]
    fn the_google_client_cases_refuse_with_their_sentence_and_code() {
        let file: Value = serde_json::from_str(include_str!("google-client-cases.json")).unwrap();
        let (client_id, client_secret) = (file["context"]["clientId"].as_str().unwrap(), file["context"]["clientSecret"].as_str().unwrap());
        for case in file["cases"].as_array().unwrap() {
            let name = case["case"].as_str().unwrap();
            let root = std::env::temp_dir().join(format!("konae-google-cases-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let state = AppState {
                public_origin: "http://localhost:8790".into(),
                ..AppState::for_test(&root)
            };
            let mut deployment: Deployment = serde_json::from_value(json!({
                "id": "p1", "projectId": "p1", "projectName": "Practice", "slug": "practice", "artifactVersion": "v1", "signupPolicy": {"mode": "closed"}, "signInMethods": ["password"], "url": "http://practice.localhost:8790", "createdAt": 0,
                "manifest": {"visibility": "private"}, "hostname": "practice.localhost", "actions": "{\"version\":1,\"actions\":{}}"
            }))
            .unwrap();
            let db = super::super::http::secrets_store(&state.data_root, "p1").unwrap();
            if case["clientSet"] == true {
                for (secret, value) in [("google_client_id", client_id), ("google_client_secret", client_secret)] {
                    super::super::http::secrets_call(&db, "p1", None, "secret_set", &json!({"name": secret, "value": value, "use": "google"})).unwrap();
                }
            }
            if case["googleOn"] == true {
                deployment.sign_in_methods = vec![SignInMethod::Password, SignInMethod::Google];
            }
            let answer = match case["tool"].as_str().unwrap() {
                "sign_in_methods_set" => requested_sign_in_methods(&state, &deployment, &case["input"]["methods"]).map(|_| json!(null)),
                tool => super::super::http::secrets_call(&db, "p1", Some(&deployment), tool, &case["input"]),
            };
            match (case["error"].as_str(), answer) {
                (None, Ok(_)) => {}
                (None, Err(error)) => panic!("{name}: refused with {error}"),
                (Some(_), Ok(_)) => panic!("{name}: accepted"),
                (Some(sentence), Err(error)) => {
                    let refused = super::super::refusal::of(&error).unwrap_or_else(|| panic!("{name}: {error} carries no code"));
                    assert_eq!((refused.message.as_str(), refused.code), (sentence, case["code"].as_str().unwrap()), "{name}");
                    assert_eq!(tool_error(&error)["structuredContent"]["error"]["hint"]["fix"], "owner", "{name}");
                }
            }
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn sign_in_methods_set_keeps_password_and_refuses_google_without_the_apps_client() {
        let root = std::env::temp_dir().join(format!("konae-methods-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState {
            public_origin: "http://localhost:8790".into(),
            ..AppState::for_test(&root)
        };
        let mut deployment: Deployment = serde_json::from_value(json!({
            "id": "p1", "projectId": "p1", "projectName": "Practice", "slug": "practice", "artifactVersion": "v1", "signupPolicy": {"mode": "closed"}, "signInMethods": ["password"], "url": "http://practice.localhost:8790", "createdAt": 0,
            "manifest": {"visibility": "private"}, "hostname": "practice.localhost", "actions": "{}"
        }))
        .unwrap();
        let refused = |state: &AppState, deployment: &Deployment, methods: Value| requested_sign_in_methods(state, deployment, &methods).unwrap_err().to_string();
        assert_eq!(refused(&state, &deployment, json!(["password", "google"])), "Set this app's Google client ID and client secret before turning Google on.");
        assert_eq!(refused(&state, &deployment, json!(["google"])), PASSWORD_REQUIRED);
        assert_eq!(refused(&state, &deployment, json!(["password", "facebook"])), "Name the methods as password and google.");
        assert_eq!(requested_sign_in_methods(&state, &deployment, &json!(["password"])).unwrap(), vec![SignInMethod::Password]);
        let db = super::super::http::secrets_store(&state.data_root, "p1").unwrap();
        let set = |name: &str, value: &str| super::super::http::secrets_call(&db, "p1", None, "secret_set", &json!({"name": name, "value": value, "use": "google"}));
        set("google_client_id", "1001-practice.apps.googleusercontent.com").unwrap();
        assert_eq!(refused(&state, &deployment, json!(["password", "google"])), "Set this app's Google client ID and client secret before turning Google on.");
        set("google_client_secret", "GOCSPX-test-secret-value").unwrap();
        assert_eq!(
            requested_sign_in_methods(&state, &deployment, &json!(["google", "password", "google"])).unwrap(),
            vec![SignInMethod::Password, SignInMethod::Google]
        );
        // The read-back names each half without its value, and the callback to register.
        deployment.sign_in_methods = vec![SignInMethod::Password, SignInMethod::Google];
        let value = sign_in_methods_value(&state, &deployment);
        assert_eq!(value["googleClient"], json!({"clientIdSet": true, "clientSecretSet": true}));
        assert_eq!(value["offered"], json!(["password", "google"]));
        assert_eq!(value["redirectUris"], json!(["http://practice.localhost:8790/_konae/auth/google/callback"]));
        assert!(!value.to_string().contains("GOCSPX"));
        // Deleting a Google secret while Google is on is refused.
        let deleted = super::super::http::secrets_call(&db, "p1", Some(&deployment), "secret_delete", &json!({"name": "google_client_secret"}));
        assert_eq!(deleted.unwrap_err().to_string(), "Google sign-in is on for this app and needs google_client_secret. Turn Google off before deleting it.");
        // Without its client, as after an import, the app keeps the setting and says why.
        db.execute("DELETE FROM secrets", []).unwrap();
        let value = sign_in_methods_value(&state, &deployment);
        assert_eq!((value["methods"].clone(), value["offered"].clone()), (json!(["password", "google"]), json!(["password"])));
        assert_eq!(value["notice"], GOOGLE_CLIENT_MISSING);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn app_users_cannot_take_the_transfer_slots_owners_need() {
        let root = std::env::temp_dir().join(format!("konae-quota-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = AppState::for_test(&root);
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

    /// A refused source save says what the editor's format holds, so an agent
    /// that saves sources over MCP can correct the JSON it uploaded.
    #[test]
    fn a_refused_source_save_names_the_expected_format() {
        let manifest = json!({"version":1,"visibility":"private","runtime":{"database":{"mode":"declared-actions","actions":[]},"auth":{"mode":"optional"},"files":{"access":"deployment"}}});
        let base = json!({"id":"app_a","name":"A","slug":"a","files":{"index.html":"<main></main>"},"directories":[],"manifest":manifest,"updatedAt":1});
        assert!(validate_sources(&base, "app_a").is_ok());
        let refusal = |change: &dyn Fn(&mut Value)| { let mut source = base.clone(); change(&mut source); validate_sources(&source, "app_a").unwrap_err().to_string() };
        assert!(refusal(&|s| { s.as_object_mut().unwrap().remove("directories"); }).starts_with("Invalid project directories."));
        assert!(refusal(&|s| s["manifest"] = json!({"version":1,"visibility":"private"})).starts_with("Invalid project manifest. Send { version: 1, visibility, runtime: { database: { mode: \"declared-actions\""));
        assert!(refusal(&|s| { s.as_object_mut().unwrap().remove("updatedAt"); }).starts_with("Invalid project metadata. Send { id, name, slug, files, directories, manifest, updatedAt }"));
    }

    /// The sdk topic is src/sdk/contract.ts rendered to sdk-guide.json. Every
    /// tool a call names must be one the runtime endpoint serves, and a call the
    /// contract opens to signed-out visitors must use only the tools they get.
    #[tokio::test]
    async fn platform_guide_sdk_topic_is_the_rendered_contract() {
        let root = std::env::temp_dir().join(format!("konae-guide-{}", Uuid::new_v4()));
        let state = AppState::for_test(&root);
        let owner = Owner { id: "o".into(), display: None, scopes: vec!["read".into()], projects: vec![], session: false };
        let guide = admin_call(&state, &owner, "owner:o", "platform_guide", json!({"topic":"sdk"})).await.unwrap();
        let rendered: Value = serde_json::from_str(include_str!("sdk-guide.json")).unwrap();
        assert_eq!(guide, rendered);
        let names = |tools: Vec<Value>| tools.iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let served = names(runtime_tools());
        let signed_out = names(anonymous_runtime_tools());
        let calls = guide["modules"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|module| module["calls"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
        assert!(calls.len() >= 6, "the contract lists the auth, actions and files calls");
        for call in &calls {
            let label = call["call"].as_str().unwrap();
            let tools = call["tools"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_owned()).collect::<Vec<_>>();
            for tool in &tools {
                assert!(served.contains(tool), "{label} names {tool}, which the runtime endpoint does not serve");
            }
            if call["signedOut"] == true {
                for tool in &tools {
                    assert!(signed_out.contains(tool), "{label} is open to signed-out visitors but {tool} is not");
                }
            } else {
                assert!(
                    tools.iter().any(|tool| !signed_out.contains(tool)),
                    "{label} needs a session, so one of its tools must be refused to signed-out visitors"
                );
            }
        }
    }
}
