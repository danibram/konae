use super::*;
use super::runtime::*;
use super::transfers::{ADMIN_CHUNK, ADMIN_MCP_BODY_BYTES, MAX_TRANSFER, download, transfer, transfer_call, transfer_path};
use rusqlite::params;


fn admin_body_refused() -> String {
    format!("MCP request exceeds {ADMIN_MCP_BODY_BYTES} bytes. Write a transfer in blocks of at most chunkBytes ({ADMIN_CHUNK} bytes).")
}

pub(super) fn owner_cookie_name(state: &Konae) -> &'static str {
    let mcp_https = state.mcp_origin.as_deref().is_some_and(|origin| origin.starts_with("https://"));
    if state.public_origin.starts_with("https://") || mcp_https {
        "__Host-konae_owner_session"
    } else {
        "konae_owner_session"
    }
}
const AUDIT_MAX_ROWS: i64 = 10_000;
const AUDIT_PRUNE_INTERVAL: i64 = 128;
const AUDIT_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const AUDIT_TEXT_CHARACTERS: usize = 128;

pub fn routes() -> Router<Konae> {
    Router::new()
        .route("/mcp", post(admin))
        .route("/_konae/owner/login", get(owner_page).post(owner_login))
        .route("/_konae/owner/signup", post(owner_signup))
        .route("/_konae/owner/logout", post(owner_logout))
        .route("/_konae/owner/config", get(owner_config))
        .route("/_konae/owner/session", get(owner_session))
        .route("/_konae/owner/clerk", post(owner_clerk))
}

/// Remembers which export each project imported, so a retried import
/// returns its first receipt.
fn install_imports(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS package_imports(package_id TEXT NOT NULL, project TEXT NOT NULL, plan TEXT NOT NULL, PRIMARY KEY(package_id, project));",
    )?;
    Ok(())
}


/// A second `release_plan` with an export this project already imported
/// gets the first plan and its receipt instead of a refusal.
fn replay_import(state: &Konae, owner: &Owner, project: &str, package_id: &str) -> Result<Option<Value>> {
    let db = store(state)?;
    let replay: Option<(String, String)> = db
        .query_row(
            "SELECT i.plan, p.receipt FROM package_imports i JOIN plans p ON p.id=i.plan
             WHERE i.package_id=?1 AND i.project=?2 AND p.owner=?3 AND p.receipt IS NOT NULL",
            params![package_id, project, owner.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match replay {
        Some((plan, receipt)) => Some(json!({
            "planId": plan,
            "projectId": project,
            "applied": true,
            "packageId": package_id,
            "receipt": serde_json::from_str::<Value>(&receipt)?,
        })),
        None => None,
    })
}


fn remember_import(state: &Konae, package_id: &str, project: &str, plan: &str) -> Result<()> {
    store(state)?.execute(
        "INSERT INTO package_imports(package_id,project,plan) VALUES(?1,?2,?3)
         ON CONFLICT(package_id,project) DO UPDATE SET plan=excluded.plan",
        params![package_id, project, plan],
    )?;
    Ok(())
}


/// Tools that change nothing a paused app serves. Every other tool that
/// names a paused Deployment, its project or one of its plans is refused.
const OPEN_TOOLS: [&str; 19] = [
    "platform_guide",
    "platform_describe",
    "mail_status",
    "session_inspect",
    "projects_list",
    "project_create",
    "project_inspect",
    "project_sources_read",
    "project_sources_save",
    "tokens_list",
    "token_create",
    "token_revoke",
    "secrets_list",
    "api_keys_list",
    "usage_summary",
    "signals_read",
    "sign_in_methods_read",
    "project_export",
    "release_plan",
];
/// What `initialize` tells an owner's agent on `/mcp`.
const OWNER_INSTRUCTIONS: &str = "A delegated credential can access only projects that already exist; it cannot create a project. Source and builds stay on your computer. Run konae init and konae dev, then konae login --browser and konae publish. After a write, read its effect through the door named by platform_guide rules.";


const PAUSED_TOOLS: [&str; 2] = ["maintenance_set", "project_import"];
const READ_TOOLS_SUFFIX: [&str; 2] = ["_list", "_read"];


async fn admin_guard(state: &Konae, name: &str, a: &Value) -> Result<()> {
    if name.starts_with("transfer_")
        || OPEN_TOOLS.contains(&name)
        || READ_TOOLS_SUFFIX.iter().any(|suffix| name.ends_with(suffix))
        || (name == "database_execute" && ["inspect", "browse", "list-backups"].contains(&a["operation"].as_str().unwrap_or("")))
    {
        return Ok(());
    }
    if let Some(message) = super::recovery::hold(&state.data_root) {
        anyhow::bail!(message);
    }
    if PAUSED_TOOLS.contains(&name) {
        return Ok(());
    }
    let deployment = {
        let project = match (a["projectId"].as_str(), a["planId"].as_str()) {
            (Some(project), _) => Some(project.to_owned()),
            (None, Some(plan)) if name == "release_apply" => {
                let row: Option<(String, Option<String>)> = store(state)?
                    .query_row("SELECT project,receipt FROM plans WHERE id=?1", [plan], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                match row {

                    Some((_, Some(_))) => return Ok(()),
                    Some((project, None)) => Some(project),
                    None => None,
                }
            }
            _ => None,
        };
        match project {
            Some(project) => find_project_deployment(&*state.deployments.read().await, &project).map(|d| d.id),
            None => None,
        }
    };
    if let Some(message) = deployment.and_then(|id| super::maintenance::pause(&state.data_root, &id)) {
        anyhow::bail!(message);
    }
    Ok(())
}





pub(super) fn installation() -> super::installation::Installation {
    use super::installation::{AccountDatabase, RootDatabase};
    let mut installation = super::installation::Installation::kernel();
    installation.databases.splice(0..0, [
        RootDatabase { file: "owners.sqlite", sign_out: &["app_sessions", "creator_sessions", "account_challenges"] },
        RootDatabase { file: "mcp.sqlite", sign_out: &["credentials", "oauth_codes", "plans"] },
    ]);
    installation.accounts.push(AccountDatabase { scope: super::owners::CREATOR_SCOPE, kind: "creator", file: "owners.sqlite" });
    installation.mail_group = project_owner;
    installation
}


fn project_owner(root: &Path, project_id: &str) -> Option<String> {
    let connection = Connection::open_with_flags(root.join("mcp.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection
        .query_row("SELECT owner FROM projects WHERE id = ?1", [project_id], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
}

pub(super) fn store(state: &Konae) -> Result<Connection> {
    store_at(&state.data_root)
}

fn source_revision_refusal(observed: u64) -> anyhow::Error {
    super::refusal::refuse("sources.revision-conflict", format!("Sources changed since revision {observed}. Run konae pull --dir <folder>, then publish again.")).into()
}

fn store_at(root: &Path) -> Result<Connection> {
    let db = Connection::open(root.join("mcp.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, sources INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);
        CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL, revision INTEGER NOT NULL, transfer TEXT NOT NULL, expires INTEGER NOT NULL, receipt TEXT, source_revision INTEGER NOT NULL DEFAULT 0, cleanup_uploads TEXT NOT NULL DEFAULT '[]');
        CREATE TABLE IF NOT EXISTS source_read_requests(owner TEXT NOT NULL, request_id TEXT NOT NULL, project TEXT NOT NULL, transfer TEXT, cancelled INTEGER NOT NULL DEFAULT 0, expires INTEGER NOT NULL, PRIMARY KEY(owner, request_id));
        CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, at INTEGER NOT NULL, actor TEXT NOT NULL, tool TEXT NOT NULL, target TEXT, ok INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS audit_actor ON audit(actor, id);")?;
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

    let has_sources: bool = db
        .prepare("PRAGMA table_info(plans)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "sources");
    if !has_sources {
        db.execute("ALTER TABLE plans ADD COLUMN sources TEXT", [])?;
    }
    let has_source_revision = db.prepare("PRAGMA table_info(plans)")?.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?.iter().any(|name| name == "source_revision");
    if !has_source_revision { db.execute("ALTER TABLE plans ADD COLUMN source_revision INTEGER NOT NULL DEFAULT 0", [])?; }
    let has_cleanup_uploads = db.prepare("PRAGMA table_info(plans)")?.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?.iter().any(|name| name == "cleanup_uploads");
    if !has_cleanup_uploads { db.execute("ALTER TABLE plans ADD COLUMN cleanup_uploads TEXT NOT NULL DEFAULT '[]'", [])?; }
    db.execute("CREATE INDEX IF NOT EXISTS audit_at ON audit(at)", [])?;
    super::oauth::install(&db)?;
    install_imports(&db)?;
    Ok(db)
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


pub(super) fn authenticate_owner(state: &Konae, headers: &HeaderMap) -> Result<Option<Owner>> {
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
                        audience == &format!("{}/mcp", origin(state, headers))
                    })
            })
            .map(|r| {
                if let Some(config) = &clerk {
                    let db = Connection::open(state.data_root.join("owners.sqlite"))?;
                    anyhow::ensure!(super::clerk::owner_allowed(&db, config, &r.0)?, "Owner is not admitted.");
                } else {
                    anyhow::ensure!(super::owners::creator_store(state)?.is_active(&r.0)?, "Owner is not active.");
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
        super::owners::creator_store(state)?.authenticate(cookie.as_deref())?
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






fn owner_unauthorized(state: &Konae, headers: &HeaderMap) -> Response {
    let refused = unauthorized(state, headers);
    let Some(token) = bearer(headers) else { return refused };
    let record: Option<(u64, Option<String>, Option<String>)> = store(state)
        .and_then(|db| {
            Ok(db
                .query_row(
                    "SELECT expires,audience,client FROM credentials WHERE hash=?1",
                    [digest(token.as_bytes())],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?)
        })
        .ok()
        .flatten();
    let resource = format!("{}/mcp", origin(state, headers));
    let Some((expires, _, client)) = record
        .filter(|(expires, audience, _)| *expires <= now_ms() && audience.as_ref().is_none_or(|audience| audience == &resource))
    else {
        return refused;
    };
    let login = format!("{}{}", origin(state, headers), super::oauth::owner_login_path());
    let when = kernel::notify::format_now(expires);
    let message = if client.is_some() {
        format!("This Konae sign-in expired at {when}. Run konae login --browser for the CLI, or reconnect your MCP client; either opens {login}.")
    } else {
        format!("This Konae token expired at {when}. Run konae login for a new CLI credential, reconnect the MCP client, or sign in as the owner at {login}.")
    };
    let mut response = error(StatusCode::UNAUTHORIZED, &message);
    if let Some(value) = refused.headers().get(header::WWW_AUTHENTICATE) {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, value.clone());
    }
    response
}

pub(super) fn owned_project(state: &Konae, owner: &Owner, id: &str, scope: &str) -> Result<(Value, u64)> {
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


pub(super) const DELETE_NAME_MISMATCH: &str = "The name does not match this project. Type the project's name exactly to delete it.";






async fn delete_project(state: &Konae, id: &str) -> Result<Vec<String>> {
    let published = {
        let mut deployments = state.deployments.write().await;
        let hosts: Vec<String> = deployments.iter().filter(|(_, d)| d.id == id).map(|(host, _)| host.clone()).collect();
        hosts.into_iter().filter_map(|host| deployments.remove(&host)).count() > 0
    };

    let ids: Vec<String> = if published || tokio::fs::try_exists(crate::layout::record(&state.data_root, id)).await? { vec![id.to_owned()] } else { Vec::new() };
    let trash = state.data_root.join("deleted");
    tokio::fs::create_dir_all(&trash).await?;
    let folder = crate::layout::folder(&state.data_root, id);
    for deployment in &ids {
        let _data = database_lock(state, deployment).await;
        let _identity = database_lock(state, &format!("identity:{deployment}")).await;
        super::domain::forget(state, deployment).await?;
        super::path::forget(state, deployment).await?;
        super::hostname::forget(state, deployment).await?;
        park(&folder, &trash, deployment).await?;


        super::file_store::forget_deployment(state.files.store.as_ref(), deployment).await;
    }

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






fn route_secret_reused(state: &Konae, owner: &str, project: &str, value: &str) -> Result<bool> {
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
    anyhow::ensure!(source["id"] == id && string(&source["name"], 80)
        && source["name"].as_str().is_some_and(|s| !s.trim().is_empty())
        && string(&source["slug"], 100) && source["updatedAt"].as_u64().is_some(), "Invalid project metadata. Send {{ id, name, slug, files, directories, manifest, updatedAt }} with the project's own id, a name of 1 to 80 characters, a slug of up to 100 and updatedAt in milliseconds.");
    anyhow::ensure!(source["files"].is_object(), "Invalid project files. Send files as an object of each path inside app/ and its contents.");
    super::sources::check_files(&super::sources::decode_document(source)?)?;
    let directories = source["directories"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid project directories."))?;
    anyhow::ensure!(directories.len() <= super::sources::MAX_FILES && directories.iter().all(|entry| entry.as_str().is_some_and(super::sources::holds)), "Invalid project directories.");
    let manifest = &source["manifest"];
    artifact::check_file_access(manifest).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(manifest["version"] == 1 && ["public", "private"].contains(&manifest["visibility"].as_str().unwrap_or("")), "Invalid project manifest. Send version 1 and visibility \"public\" or \"private\".");
    Ok(())
}



fn merge_source_delta(current: &Value, delta: &Value, id: &str) -> Result<Value> {
    anyhow::ensure!(delta["format"] == "delta", "Invalid source update format.");
    let mut merged = current.clone();
    let mut files = merged["files"].as_object().cloned().ok_or_else(|| anyhow::anyhow!("Saved project files are unavailable."))?;
    let mut encodings = merged["encodings"].as_object().cloned().unwrap_or_default();
    let changed = delta["files"].as_object().ok_or_else(|| anyhow::anyhow!("Invalid changed project files."))?;
    let changed_encodings = delta["encodings"].as_object();
    if let Some(encodings) = changed_encodings {
        anyhow::ensure!(encodings.iter().all(|(path, encoding)| changed.contains_key(path) && encoding == "base64"), "Invalid changed project file encoding.");
    }
    for (path, contents) in changed {
        anyhow::ensure!(contents.is_string(), "Invalid changed project file.");
        files.insert(path.clone(), contents.clone());
        if changed_encodings.is_some_and(|items| items.get(path).is_some()) {
            encodings.insert(path.clone(), json!("base64"));
        } else {
            encodings.remove(path);
        }
    }
    let removals = delta["removedPaths"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid removed project paths."))?;
    for path in removals {
        let path = path.as_str().ok_or_else(|| anyhow::anyhow!("Invalid removed project path."))?;
        files.remove(path);
        encodings.remove(path);
    }
    merged["files"] = Value::Object(files);
    if encodings.is_empty() { merged.as_object_mut().unwrap().remove("encodings"); }
    else { merged["encodings"] = Value::Object(encodings); }
    let metadata = delta["metadata"].as_object().ok_or_else(|| anyhow::anyhow!("Invalid project metadata update."))?;
    for key in ["name", "slug", "directories", "manifest", "updatedAt"] {
        let value = metadata.get(key).ok_or_else(|| anyhow::anyhow!("Source metadata is missing {key}."))?;
        merged[key] = value.clone();
    }
    merged["id"] = json!(id);
    Ok(merged)
}



fn ensure_no_component(upload: &Value) -> Result<()> {
    match upload["component"].as_str() {
        Some(component) => anyhow::bail!("This upload names the {component} component of an export. Upload sources and releases without a component."),
        None => Ok(()),
    }
}



async fn release_bytes(state: &Konae, upload: &Value, transfer_id: &str) -> Result<Vec<u8>> {
    ensure_no_component(upload)?;
    let size = upload["size"].as_u64().unwrap_or(0) as usize;
    anyhow::ensure!(size <= super::size::RELEASE_BYTES, "{}", super::size::release_refused(size));
    Ok(tokio::fs::read(transfer_path(state, transfer_id)?).await?)
}



async fn release_upload_ids(
    state: &Konae,
    principal: &str,
    release_id: &str,
    sources_id: Option<&str>,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    if let Ok(upload) = transfer(state, principal, &json!({"transferId": release_id})) {
        let payload: Value = serde_json::from_slice(&release_bytes(state, &upload, release_id).await?)?;
        if let Some(components) = payload["initialPackage"]["components"].as_object() {
            ids.extend(components.values().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    if let Some(sources) = sources_id {
        ids.push(sources.to_owned());
    }
    ids.push(release_id.to_owned());
    Ok(ids)
}

async fn cleanup_release_uploads(state: &Konae, owner: &Owner, principal: &str, plan_id: &str) -> Result<()> {
    let (release_id, sources_id, recorded_ids): (String, Option<String>, String) = {
        let db = store(state)?;
        db.query_row("SELECT transfer,sources,cleanup_uploads FROM plans WHERE id=?1 AND owner=?2", params![plan_id, owner.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
    };


    let mut ids: Vec<String> = serde_json::from_str(&recorded_ids)?;
    if ids.is_empty() {
        ids = release_upload_ids(state, principal, &release_id, sources_id.as_deref()).await?;
    }
    for transfer_id in ids {
        let still_available = super::transfers::completed_upload(&state.kernel, principal, &transfer_id)?;
        if still_available {
            transfer_call(state, principal, "transfer_cancel", &json!({"transferId":transfer_id}), false).await?;
        }
    }
    Ok(())
}

fn transfer_project_for_owner(state: &Konae, owner: &Owner, principal: &str, transfer_id: &str) -> Result<(Option<String>, bool)> {
    if let Some(upload) = super::transfers::owned_transfer(&state.kernel, principal, &json!({"transferId":transfer_id}))? {
        return Ok((upload["projectId"].as_str().map(str::to_owned), true));
    }
    let db = store(state)?;
    let mut query = db.prepare("SELECT project,transfer,sources,cleanup_uploads FROM plans WHERE owner=?1")?;
    let plans = query.query_map([&owner.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (project, release_id, sources_id, cleanup_json) in plans {
        let cleanup_ids: Vec<String> = serde_json::from_str(&cleanup_json)?;
        if release_id == transfer_id || sources_id.as_deref() == Some(transfer_id) || cleanup_ids.iter().any(|id| id == transfer_id) {
            return Ok((Some(project), false));
        }
    }
    Ok((None, false))
}


#[derive(serde::Deserialize)]
struct ReleaseSources {
    files: HashMap<String, String>,
    #[serde(default)]
    encodings: HashMap<String, String>,
    #[serde(default)]
    directories: Vec<String>,
}






async fn release_input(state: &Konae, principal: &str, transfer_id: &str, sources_id: Option<&str>, artifact_id: Option<&str>) -> Result<DeployInput> {
    let upload = transfer(state, principal, &json!({"transferId": transfer_id}))?;
    anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload", "Upload is not complete.");
    let mut payload: Value = serde_json::from_slice(&release_bytes(state, &upload, transfer_id).await?)?;
    if let Some(artifact_id) = artifact_id {
        anyhow::ensure!(payload.get("artifact").is_none(), "Send artifact in the release JSON or artifactTransferId, not both.");
        let project_id = payload["project"]["id"].as_str().ok_or_else(|| anyhow::anyhow!("Release has no project ID."))?;
        let artifact_upload = transfer(state, principal, &json!({"transferId": artifact_id}))?;
        anyhow::ensure!(artifact_upload["complete"] == true && artifact_upload["direction"] == "upload" && artifact_upload["projectId"] == project_id, "Completed artifact upload for this project required.");
        ensure_no_component(&artifact_upload)?;
        payload["artifact"] = serde_json::from_slice(&tokio::fs::read(transfer_path(state, artifact_id)?).await?)?;
    }
    let mut input: DeployInput = serde_json::from_value(payload)?;
    anyhow::ensure!(upload["projectId"] == input.project.id, "Artifact belongs to another project.");
    if let Some(sources_id) = sources_id {
        anyhow::ensure!(input.project.files.is_empty() && input.project.encodings.is_empty(), "Send the files of app/ in sourcesTransferId or in project.files, not in both.");
        let upload = transfer(state, principal, &json!({"transferId": sources_id}))?;
        anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == input.project.id, "Completed upload of the sources for this project required.");
        ensure_no_component(&upload)?;
        let bytes = tokio::fs::read(transfer_path(state, sources_id)?).await?;
        let sources: ReleaseSources = serde_json::from_slice(&bytes).map_err(|error_value| anyhow::anyhow!("The sources upload must be the JSON {{ files, encodings }}: {error_value}"))?;
        input.project.files = sources.files;
        input.project.encodings = sources.encodings;
        input.project.directories = sources.directories;
    }
    Ok(input)
}

pub(super) async fn owned_deployment(
    state: &Konae,
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
            "Read a guide topic. Start with rules (yardstick, first run, read-back, divergences), then pieces (which Capsule declaration to use). Also credentials, development, actions, sdk, publishing, users, portability and refusals.",
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
        tool("project_sources_read", "Download the saved project sources, or cancel a pending source read with cancelRequestId. metadataOnly returns id, name, slug and manifest without creating a source transfer. Pass requestId to make an in-progress download cancelable on page departure.", json!({"projectId":text,"metadataOnly":{"type":"boolean"},"requestId":text,"cancelRequestId":text}), &["projectId"], true),
        tool(
            "database_execute",
            "Administer a deployment database. Mutations require write permission.",
            json!({"projectId":text,"operation":{"type":"string","enum":["inspect","browse","sql","update-cell","insert-row","delete-row","list-backups","create-backup","restore-backup"]},"input":{"type":"object"}}),
            &["projectId", "operation"],
            false,
        ),
    ];
    if owner.scopes.iter().any(|s| s == "write") {
        tools.extend([
            tool("project_create", "Register a project and return a template's files, the ones konae init writes: template is tasks, notes (the default), members, form, webhook, digest or gallery, and without a manifest the project takes the template's visibility. Write the files into the folder and keep id, slug and manifest in konae.json. To save the editor's files with it, upload the project JSON without a projectId and pass sourcesTransferId to create the project and its files atomically. Repeating the same initial files is safe; existing saved files cannot be replaced here.", json!({"projectId":text,"name":text,"slug":text,"manifest":{"type":"object"},"sourcesTransferId":text,"template":{"type":"string","enum":["tasks","notes","members","form","webhook","digest","gallery"]}}), &["name"], false),
        tool("project_sources_save", "Save project sources if expectedRevision still matches. Upload the full document for compatibility, or { format: \"delta\", metadata: { name, slug, directories, manifest, updatedAt }, files: { changedPath: contents }, encodings, removedPaths } to send changed files only. Base64 files use encodings: { path: \"base64\" }.", json!({"projectId":text,"transferId":text,"expectedRevision":{"type":"integer","minimum":0}}), &["projectId","transferId","expectedRevision"], false),
            tool("project_delete", "Delete a project for good: its registry entry, saved editor files and secrets, and, when it is published, its Deployment with the database, app accounts, uploaded files, backups and custom domain. The address stops answering at once. name must equal the project's name. Read back with projects_list.", json!({"projectId":text,"name":text}), &["projectId","name"], false),
        ]);
    }
    tools.extend([
        tool("secrets_list", "List a project's signing secrets: each one's name, use and svixKey, which says whether its value decodes as a svix whsec_ key. Values are never returned.", json!({"projectId":text}), &["projectId"], true),
        tool("api_keys_list", "List a deployment's API keys without their secrets.", json!({"projectId":text}), &["projectId"], true),
        domain_read_tool(),
        path_read_tool(),
        tool("maintenance_read", "Read whether a deployment is in maintenance and since when. installation names a hold of the whole installation that the app's own state cannot lift.", json!({"projectId":text}), &["projectId"], true),
        tool("contact_email_read", "Read the app's contact email, where \"notify\": { \"to\": \"owner\" } sends, and ownerNotices, the actions that send there. A contact email of null with ownerNotices listed means those notices are not sent.", json!({"projectId":text}), &["projectId"], true),
        tool("modules_read", "Read every module's state and options. Read back module_set with this tool.", json!({"projectId":text}), &["projectId"], true),
    ]);
    if owner.scopes.iter().any(|scope| ["publish", "write", "manage_users"].contains(&scope.as_str())) {
        tools.push(tool("module_set", "Turn a module on or off, or change its options. Pass the state returned by modules_read as expected. Read the result back with modules_read.", json!({"projectId":text,"module":text,"on":{"type":"boolean"},"options":{"type":"object"},"expected":{"type":"object"}}), &["projectId", "module"], false));
    }
    if owner.scopes.iter().any(|s| s == "export_data") {
        tools.extend([
            tool(
                "project_export",
                "Export the app as its folder: konae.json, app/ and dist/ in the folder download, and a consistent copy of data/ (database, accounts without sessions or links, files, Usage) as one download per component that the manifest lists. Never secrets, owners or the installation's records. Requires export_data.",
                json!({"projectId":text}),
                &["projectId"],
                true,
            ),
        ]);
    }
    tools.extend(usage_tools());
    tools.extend(calls_tools());
    tools.extend(mail_tools());
    tools.push(tool(
        "sign_in_methods_read",
        "Read how people sign in to a deployment: password, and google when the owner turned it on. googleClient says whether the app's google_client_id and google_client_secret are set, without their values, and redirectUris lists the callbacks to register in the app's Google OAuth client.",
        json!({"projectId":text}),
        &["projectId"],
        true,
    ));
    if owner.scopes.iter().any(|s| s == "manage_users") {
        tools.push(tool(
            "sign_in_methods_set",
            "Choose how people sign in to a deployment: [\"password\"] or [\"password\",\"google\"]. Password cannot be turned off. Google needs the app's own Google client: set google_client_id and google_client_secret with secret_set use google first. Read back with sign_in_methods_read.",
            json!({"projectId":text,"methods":{"type":"array","items":{"type":"string","enum":["password","google"]},"minItems":1,"maxItems":2}}),
            &["projectId", "methods"],
            false,
        ));
        tools.push(tool(
            "signup_policy_set",
            "Choose who may create an account on a deployment: open, invitation (only people invited with app_invitations_create), closed, or allowlist (exact emails or @domain entries). Omitting allowlist keeps the current entries.",
            json!({"projectId":text,"mode":{"type":"string","enum":["open","invitation","closed","allowlist"]},"allowlist":{"type":"array","items":{"type":"string","minLength":1,"maxLength":254}}}),
            &["projectId", "mode"],
            false,
        ));
        tools.extend(invitations_tools());
        if owner.session {
            tools.push(tool(
                "app_user_enter",
                "Enter a deployment as one of its active app accounts. Returns a link on the app's own address that works once, for two minutes, and signs the browser that opens it in as that account. The owner's audit log records who entered as whom, and Usage and Signals do not count what that session does. Needs manage_users and the owner's own browser session; a delegated token cannot call it.",
                json!({"projectId":text,"userId":text}),
                &["projectId", "userId"],
                false,
            ));
        }
    }
    if owner.scopes.iter().any(|s| s == "publish") {
        tools.extend([
            tool("secret_set", "Set a signing secret an action's http signature names, or, with use call, one a call action signs with or an API key it sends, or, with use google, the app's Google OAuth client as google_client_id and google_client_secret. The value is never returned. Read back with secrets_list, and the Google client with sign_in_methods_read.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":64},"value":{"type":"string","minLength":16,"maxLength":512},"use":{"type":"string","enum":["route","call","google"]}}), &["projectId","name","value"], false),
            tool("secret_delete", "Delete a signing secret no published action uses. A Google client secret is refused while Google sign-in is on.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":64}}), &["projectId","name"], false),
            tool("api_keys_create", "Issue an API key for one deployment, scoped to named actions with an HTTP route and to file visibilities. The key appears only in this answer.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":80},"actions":{"type":"array","items":{"type":"string","minLength":1,"maxLength":100}},"files":{"type":"array","items":{"type":"string","enum":["public","private"]}}}), &["projectId","name"], false),
            tool("api_keys_revoke", "Revoke a deployment's API key by its ID.", json!({"projectId":text,"keyId":text}), &["projectId","keyId"], false),
            tool(
                "release_plan",
                "Validate a release against the source revision this caller read. Existing apps need observedSourcesRevision from project_inspect or project_sources_read. If sources changed, run konae pull --dir <folder> and publish again. transferId is the release JSON; sourcesTransferId is app/ as { files, encodings }.",
                json!({"transferId":text,"sourcesTransferId":text,"observedSourcesRevision":{"type":"integer","minimum":0}}),
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
        tools.extend(domain_tools());
        tools.extend(path_tools());
        tools.push(tool(
            "project_import",
            "Replace an existing deployment's sources, data or both with those of an exported app folder. It backs up what it replaces first; database_execute list-backups shows the backup and restore-backup brings it back. Rows are never merged. replace sources or both takes transferId, the release JSON, and may take sourcesTransferId for app/ and artifactTransferId for { version, entrypoint, files, encodings }. The app keeps its Signup Policy, sign-in methods, address, secrets, sessions and API keys. When the schema change loses data, the answer is { imported: false, schema } and nothing changed; call again with schema.confirmation as confirmSchema after the owner agrees. Works in maintenance. Requires publish. Read back with project_inspect and database_execute list-backups.",
            json!({"projectId":text,"replace":{"type":"string","enum":["sources","data","both"]},"transferId":text,"sourcesTransferId":text,"artifactTransferId":text,"package":{"type":"object"},"confirmSchema":{"type":"string","minLength":64,"maxLength":64}}),
            &["projectId", "replace"],
            false,
        ));
        tools.push(tool(
            "contact_email_set",
            "Set the app's contact email in konae.json, where \"notify\": { \"to\": \"owner\" } sends, or clear it with email null. A publish of an app with none fills it with the owner's email when Konae knows it; an owner who signs in through Clerk sets it here. A later publish keeps it. Pass expected from contact_email_read to refuse a change made since that read. Requires publish. Read back with contact_email_read.",
            json!({"projectId":text,"email":{"type":["string","null"],"maxLength":254},"expected":{"type":["string","null"],"maxLength":254}}),
            &["projectId", "email"],
            false,
        ));
        tools.push(tool(
            "maintenance_set",
            "Put a deployment in maintenance (on true) or take it out (on false). In maintenance the app serves a maintenance page, refuses writes, signups and sign-ins, holds its email, calls and schedule, and refuses every administrative tool that would change it except maintenance_set and project_import. Moving an app with no lost write is maintenance, project_export, then an import on the other installation. Pass expected from maintenance_read to refuse a change made since that read. Requires publish. Read back with maintenance_read.",
            json!({"projectId":text,"on":{"type":"boolean"},"expected":{"type":"boolean"}}),
            &["projectId", "on"],
            false,
        ));
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



fn calls_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    vec![tool(
        "calls_read",
        "Read a deployment's outbox: the calls its call actions queued, newest first, 50 at a time, with state (pending, delivered or failed), attempts, method, URL, HTTP status, error kind, the first 1 KiB of the answer and what the then mutation did. Pass nextCursor back as cursor for the next page.",
        json!({"projectId": text, "cursor": {"type": "string", "minLength": 1, "maxLength": 20}}),
        &["projectId"],
        true,
    )]
}


fn domain_read_tool() -> Value {
    tool(
        "domain_read",
        "Read a deployment's custom domain: the hostname, whether it serves yet, and the TXT record that proves control of it.",
        json!({"projectId":{"type":"string","minLength":1}}),
        &["projectId"],
        true,
    )
}


fn domain_tools() -> Vec<Value> {
    vec![
        tool(
            "domain_set",
            "Serve a deployment from a hostname you control as well as its generated one. The first call returns a TXT record, _konae.<hostname>, and the token it must hold; add it at your DNS provider, point the hostname at this installation's proxy, and call domain_set again with the same hostname. The Kernel looks the record up itself and the alias serves once it holds the token. The proxy terminates TLS and must forward Host unchanged. Pass expected from domain_read to refuse a change made since that read. Requires publish.",
            json!({"projectId":{"type":"string","minLength":1},"hostname":{"type":"string","minLength":1,"maxLength":260},"expected":{"type":["string","null"],"maxLength":260}}),
            &["projectId", "hostname"],
            false,
        ),
        tool(
            "domain_clear",
            "Stop serving a deployment from its custom domain. Pass expected from domain_read to refuse a change made since that read. The generated hostname keeps answering. Requires publish.",
            json!({"projectId":{"type":"string","minLength":1},"expected":{"type":["string","null"],"maxLength":260}}),
            &["projectId"],
            false,
        ),
        tool(
            "hostname_set",
            "Move a deployment to another generated hostname, <name>.<app suffix>, that no other app on this installation holds or held. The names it held before answer with a permanent redirect to the new one and stay reserved for it. The new name is a new origin: app users sign in again there, and an app with Google sign-in needs the googleCallback the answer names registered in its Google client. Pass expected from project_inspect's deployment.hostname to refuse a change made since that read. Requires publish.",
            json!({"projectId":{"type":"string","minLength":1},"name":{"type":"string","minLength":1,"maxLength":260},"expected":{"type":"string","minLength":1,"maxLength":260}}),
            &["projectId", "name"],
            false,
        ),
    ]
}




fn invitations_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    let key = json!({"type": "string", "minLength": 8, "maxLength": 128});
    let page = json!({"type": "integer", "minimum": 1, "maximum": super::invitations::MAX_PAGE});
    vec![
        tool(
            "app_users_list",
            "List a deployment's app accounts with their state and whether they verified their email, ordered by email. Pass nextCursor back as cursor for the next page.",
            json!({"projectId": text, "limit": page, "cursor": text, "state": {"type": "string", "enum": ["pending", "active", "suspended"]}}),
            &["projectId"],
            true,
        ),
        tool(
            "app_user_suspend",
            "Suspend an app account. Its sessions and open links end at once; its ID, password and rows stay.",
            json!({"projectId": text, "userId": text}),
            &["projectId", "userId"],
            false,
        ),
        tool(
            "app_user_resume",
            "Lift a suspension. The account is active again if it ever verified its email, otherwise pending until it does.",
            json!({"projectId": text, "userId": text}),
            &["projectId", "userId"],
            false,
        ),
        tool(
            "app_invitations_create",
            "Invite people to a deployment by email. Each recipient gets an email with a link valid for seven days, so call this only when the owner asked for those invitations. Up to 50 recipients, each with its own result. Retrying with the same idempotencyKey returns the first answer and sends nothing.",
            json!({"projectId": text, "emails": {"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 254}, "minItems": 1}, "idempotencyKey": key}),
            &["projectId", "emails", "idempotencyKey"],
            false,
        ),
        tool(
            "app_invitations_list",
            "List a deployment's invitations, newest first, with status (pending, expired, needs-resend, already-member, accepted, revoked), expiry and whether the mail transport took the message. Pass nextCursor back as cursor for the next page.",
            json!({"projectId": text, "limit": page, "cursor": text}),
            &["projectId"],
            true,
        ),
        tool(
            "app_invitation_resend",
            "Email a pending, expired or imported invitation again with a new seven-day link. The earlier link stops working. Retrying with the same idempotencyKey sends nothing more.",
            json!({"projectId": text, "invitationId": text, "idempotencyKey": key}),
            &["projectId", "invitationId", "idempotencyKey"],
            false,
        ),
        tool(
            "app_invitation_revoke",
            "Revoke a pending invitation so its link stops working.",
            json!({"projectId": text, "invitationId": text}),
            &["projectId", "invitationId"],
            false,
        ),
    ]
}






fn mail_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    vec![tool(
        "notices_read",
        "Read the email notices a deployment's mutations queued, newest first, 50 at a time: the action, state (pending, accepted, delivered or failed), attempts, failure kind and times. Names no recipient. Notices the Kernel did not queue are counted under notify by usage_summary. Pass nextCursor back as cursor for the next page.",
        json!({"projectId": text, "cursor": {"type": "string", "minLength": 1, "maxLength": 20}}),
        &["projectId"],
        true,
    )]
}


fn path_read_tool() -> Value {
    tool(
        "path_read",
        "Read a deployment's path address: whether it is on, its name, whether the owner accepted the shared origin, and the addresses it answers on.",
        json!({"projectId":{"type":"string","minLength":1}}),
        &["projectId"],
        true,
    )
}


fn path_tools() -> Vec<Value> {
    vec![
        tool(
            "path_set",
            "Answer a deployment on <domain>/p/<id>, and on <domain>/p/<name> when it has a name. The path shares its origin with the home and every other path on this installation, so an app with accounts or a private app needs acceptSharedOrigin true before its path turns on. A name is 3 to 40 letters, digits and hyphens, and no other Deployment may hold it. An omitted name or acceptSharedOrigin keeps the stored value, a null name removes the name, and path_clear turns the path off and removes both. Pass expected from path_read as { enabled, name, acceptSharedOrigin } to refuse a change made since that read. Requires publish.",
            json!({
                "projectId":{"type":"string","minLength":1},
                "enabled":{"type":"boolean"},
                "name":{"type":["string","null"],"minLength":1,"maxLength":260},
                "acceptSharedOrigin":{"type":"boolean"},
                "expected":{"type":"object","properties":{"enabled":{"type":"boolean"},"name":{"type":["string","null"]},"acceptSharedOrigin":{"type":"boolean"}},"required":["enabled","name","acceptSharedOrigin"],"additionalProperties":false}
            }),
            &["projectId", "enabled"],
            false,
        ),
        tool(
            "path_clear",
            "Turn off a deployment's path address and forget its name. Pass expected from path_read as { enabled, name, acceptSharedOrigin } to refuse a change made since that read. The generated hostname and any custom domain keep answering. Requires publish.",
            json!({"projectId":{"type":"string","minLength":1},"expected":{"type":"object","properties":{"enabled":{"type":"boolean"},"name":{"type":["string","null"]},"acceptSharedOrigin":{"type":"boolean"}},"required":["enabled","name","acceptSharedOrigin"],"additionalProperties":false}}),
            &["projectId"],
            false,
        ),
    ]
}


fn usage_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    let date = json!({"type": "string", "minLength": 10, "maxLength": 10});
    vec![
        tool("usage_summary", "Read daily runtime totals, guard refusals, signed-in account activity, retained scheduled runs, Signals, storage, published settings and coverage for this project. It includes the newest 500 scheduled runs in the inclusive date range. Dates use YYYY-MM-DD, both are required, the range can span at most 400 days, and the answer contains all range results without a cursor. Owner read permission required.", json!({"projectId": text, "from": date, "to": date}), &["projectId", "from", "to"], true),
        tool("usage_overview", "Read usage for this account projects only, including external and unpublished apps, 50 per page. Pass nextCursor back as cursor.", json!({"from": date, "to": date, "cursor": {"type": "string", "maxLength": 100}}), &["from", "to"], true),
        tool(
            "signals_read",
            "Read a deployment's Signals, newest first, 100 at a time, optionally of one name. Pass nextCursor back as cursor for the next page.",
            json!({"projectId": text, "name": {"type": "string", "minLength": 1, "maxLength": 64}, "cursor": {"type": "string", "minLength": 1, "maxLength": 20}}),
            &["projectId"],
            true,
        ),
    ]
}

async fn admin(State(state): State<Konae>, request: Request) -> Response {
    let headers = request.headers().clone();
    if super::security::from_app_origin(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Administration refuses requests from app origins.");
    }
    if !same_origin(&headers) || !valid_host(&state, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "Cross-origin or unrecognized-host MCP request rejected.",
        );
    }
    if !super::oauth::valid_owner_host(&state, &headers).await {
        return error(
            StatusCode::FORBIDDEN,
            "Administration is unavailable on app hosts.",
        );
    }
    let owner = match authenticate_owner(&state, &headers) {
        Ok(Some(owner)) => owner,
        Ok(None) => return owner_unauthorized(&state, &headers),
        Err(_) => return unauthorized(&state, &headers),
    };
    if headers.get("x-konae-owner-id").is_some_and(|expected| expected.as_bytes() != owner.id.as_bytes()) {
        return error(StatusCode::CONFLICT, "The signed-in owner changed. Reload before continuing.");
    }
    let principal = format!("owner:{}", owner.id);
    let tools = admin_tools(&owner);




    let bytes = match read_body_up_to(request, ADMIN_MCP_BODY_BYTES, &admin_body_refused()).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let call = match parse_call(&bytes) {
        Ok(call) if bytes.len() <= MCP_BODY_BYTES || (call["method"] == "tools/call" && call["params"]["name"] == "transfer_write") => call,
        Ok(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, MCP_BODY_REFUSED),
        Err(_) if bytes.len() > MCP_BODY_BYTES => return error(StatusCode::PAYLOAD_TOO_LARGE, MCP_BODY_REFUSED),
        Err(response) => return response,
    };
    let id = call.get("id").cloned().unwrap_or(Value::Null);
    let method = call["method"].as_str().unwrap();
    if call.get("id").is_none() {
        return without_id(method);
    }
    let result = match method {
        "initialize" => Ok(
            json!({"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"konae","version":env!("CARGO_PKG_VERSION")},"instructions":OWNER_INSTRUCTIONS}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":tools})),
        "tools/call" => {
            let Some(name) = call["params"]["name"].as_str() else {
                return rpc_error(id, -32602, "Tool name is required.");
            };
            let Some(definition) = tools.iter().find(|t| t["name"] == name) else {

                let hint = super::guide::missing_permission(name, &owner);
                return rpc_error(id, -32602, &hint.map_or("Unknown tool or insufficient permission.".to_owned(), |hint| format!("Unknown tool or insufficient permission. {hint}")));
            };
            let arguments = call["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            if let Err(e) = validate_arguments(&arguments, &definition["inputSchema"]) {
                return rpc_error(id, -32602, &e.to_string());
            }
            let previous_mode = if name == "signup_policy_set" {
                let deployments = state.deployments.read().await;
                deployments
                    .values()
                    .find(|d| Some(d.id.as_str()) == arguments["projectId"].as_str())
                    .map(|d| d.signup_policy.mode)
            } else {
                None
            };



            let package_transfer = (name.starts_with("transfer_") && name != "transfer_begin")
                .then(|| transfer(&state, &principal, &arguments).ok())
                .flatten();
            let export_transfer = package_transfer.clone().filter(|t| t["exportData"] == true);
            let component_upload = name == "transfer_write" && package_transfer.is_some_and(|t| t["component"].is_string());
            let mut result = if name.starts_with("transfer_") {
                let (project, live_transfer) = if name == "transfer_begin" {
                    (arguments["projectId"].as_str().map(str::to_owned), false)
                } else if name == "transfer_cancel" {
                    transfer_project_for_owner(&state, &owner, &principal, arguments["transferId"].as_str().unwrap_or_default()).unwrap_or((None, false))
                } else {
                    (transfer(&state, &principal, &arguments)
                        .ok()
                        .and_then(|t| t["projectId"].as_str().map(str::to_owned)), false)
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
                    Some(id) => owned_project(&state, &owner, id, scope).map(|_| ()),
                    None => owner.require(scope, None),
                };


                let held = |accepted: &[&'static str]| {
                    accepted
                        .iter()
                        .copied()
                        .find(|wanted| owner.scopes.iter().any(|s| s == wanted))
                        .unwrap_or(accepted[0])
                };
                let permission = if name == "transfer_cancel" {
                    match project.as_deref() {
                        Some(id) => owner.require(held(&["read", "publish", "write", "export_data"]), Some(id)),
                        None if live_transfer => owner.require(held(&["read", "publish", "write", "export_data"]), None),
                        // An unknown or already-consumed ID has no resource to
                        // access. Require one cancellation-capable scope, while
                        // using a retained plan mapping above when one exists.
                        None if ["read", "publish", "write", "export_data"].iter().any(|scope| owner.scopes.iter().any(|held| held == scope)) => Ok(()),
                        None => Err(anyhow::anyhow!("Permission denied for read.")),
                    }
                } else if scope == "publish" {
                    authorize(held(&["publish", "write"]))
                } else {
                    authorize(scope)
                };
                match permission {
                    Ok(()) => transfer_call(&state, &principal, name, &arguments, false).await,
                    Err(e) => Err(e),
                }
            } else {
                match admin_guard(&state, name, &arguments).await {
                    Ok(()) => admin_call(&state, &owner, &principal, name, arguments.clone()).await,
                    Err(e) => Err(e),
                }
            };
            if name == "release_apply" && result.is_ok() {
                if let Some(plan_id) = arguments["planId"].as_str() {
                    if let Err(error_value) = cleanup_release_uploads(&state, &owner, &principal, plan_id).await {
                        result = Err(error_value);
                    }
                }
            }
            if name == "transfer_cancel" && result.is_ok() {
                if let Some(transfer_id) = arguments["transferId"].as_str() {
                    if let Ok(db) = store(&state) {
                        let _ = db.execute("DELETE FROM source_read_requests WHERE transfer=?1", [transfer_id]);
                    }
                }
            }
            match export_transfer.as_ref().filter(|_| name == "transfer_read") {
                // One row per component download, written at its first
                // block or on a refusal, so a 256 MiB database read in
                // 64 blocks does not push the rest out of the log.
                Some(t) if arguments["offset"] == 0 || result.is_err() => {
                    let target = json!({"projectId":t["projectId"],"component":t["component"]});
                    audit(&state, &principal, "package_download", &target, None, &result);
                }
                Some(_) => {}
                // An upload of a part of an export is audited at its first block.
                None if component_upload && arguments["offset"] != 0 && result.is_ok() => {}
                None => audit(&state, &principal, name, &arguments, previous_mode, &result),
            }
            Ok(tool_result(result))
        }
        _ => return rpc_error(id, -32601, "Method not found."),
    };
    answer(id, result)
}

/// Records one tool call. The target is the project or Deployment the call
/// acted on; `detail` adds what the arguments alone do not show.
fn audit(
    state: &Konae,
    principal: &str,
    name: &str,
    arguments: &Value,
    previous_mode: Option<SignupMode>,
    result: &Result<Value>,
) {
    let mode = |value: &Value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    let (target, detail): (Option<String>, Option<String>) = match name {
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
        // Who entered as whom: the principal is the actor, the detail names the account.
        "app_user_enter" => (
            arguments["projectId"].as_str().map(str::to_owned),
            Some(match result {
                Ok(entered) => format!("as {} ({})", entered["email"].as_str().unwrap_or_default(), entered["userId"].as_str().unwrap_or_default()),
                Err(_) => format!("as {} (refused)", arguments["userId"].as_str().unwrap_or_default()),
            }),
        ),
        "project_import" => (
            arguments["projectId"].as_str().map(str::to_owned),
            Some(match result {
                Ok(done) if done["imported"] == true => format!("{} after backup {}", done["replace"].as_str().unwrap_or_default(), done["backup"]["id"].as_str().unwrap_or_default()),
                Ok(_) => format!("{} waits for a schema confirmation", arguments["replace"].as_str().unwrap_or_default()),
                Err(_) => format!("{} refused", arguments["replace"].as_str().unwrap_or_default()),
            }),
        ),
        "package_download" => (
            arguments["projectId"].as_str().map(str::to_owned),
            arguments["component"].as_str().map(str::to_owned),
        ),
        "signup_policy_set" => {
            let before = previous_mode.map(|m| mode(&json!(m))).unwrap_or_else(|| "unknown".into());
            let after = match result {
                Ok(value) => mode(&value["mode"]),
                Err(_) => format!("{} (refused)", mode(&arguments["mode"])),
            };
            (
                arguments["projectId"].as_str().map(str::to_owned),
                Some(format!("{before} to {after}")),
            )
        }
        _ => (
            arguments["projectId"].as_str().map(str::to_owned),
            None,
        ),
    };
    record_audit(&state.data_root, principal, name, target.as_deref(), detail.as_deref(), result.is_ok());
}

/// The sink for an app user's file uploads and deletes, which the runtime

pub(super) fn runtime_audit(root: PathBuf) -> super::runtime::AuditSink {
    Arc::new(move |principal: &str, name: &str, app: &str, ok: bool| record_audit(&root, principal, name, Some(app), None, ok))
}


fn record_audit(root: &Path, principal: &str, name: &str, target: Option<&str>, detail: Option<&str>, ok: bool) {
    let written = store_at(root).and_then(|db| {
        let text = |value: &str| value.chars().take(AUDIT_TEXT_CHARACTERS).collect::<String>();
        db.execute(
            "INSERT INTO audit(at,actor,tool,target,ok,detail) VALUES(?1,?2,?3,?4,?5,?6)",
            params![now_ms(), text(principal), text(name), target.map(text), ok, detail.map(text)],
        )?;
        let id = db.last_insert_rowid();
        if id % AUDIT_PRUNE_INTERVAL == 0 {
            db.execute("DELETE FROM audit WHERE at<?1", [now_ms().saturating_sub(AUDIT_RETENTION_MS)])?;



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



fn audit_partition(actor: &str) -> (String, String) {
    let prefix = actor
        .strip_prefix("runtime:")
        .and_then(|rest| rest.split_once(':'))
        .map(|(deployment, _)| format!("runtime:{deployment}:"))
        .unwrap_or_else(|| actor.to_owned());

    let high = if prefix.starts_with("runtime:") { format!("{};", &prefix[..prefix.len() - 1]) } else { format!("{prefix}\u{1}") };
    (prefix, high)
}



async fn admin_call(
    state: &Konae,
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
                "chunkBytes":ADMIN_CHUNK,
                "maxTransferBytes":MAX_TRANSFER,
                "files":{"quotaBytes":state.files.limits.quota_bytes,"maxFileBytes":state.files.limits.max_file_bytes},
                "yardstick":"A person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.",
                "firstRun":"Run konae init and konae dev, then konae login --browser and konae publish. Read the published configuration with project_inspect, open the Deployment URL, sign in if the app is private, and call a real Server Action through its runtime MCP endpoint with the appropriate app session.",
                "workflow":[
                    "konae init and konae dev",
                    "konae login --browser",
                    "konae publish",
                    "project_inspect",
                    "open the Deployment URL and sign in if the app is private",
                    "call a real Server Action through the runtime MCP endpoint with the appropriate app session"
                ],
                "credentialScope":"A project-scoped credential can act only on the existing project IDs it names, within its permissions. It cannot create projects.",
                "guideTopics":super::guide::TOPICS,
                "runtimeTools":runtime_tools().iter().filter_map(|tool| tool["name"].as_str()).collect::<Vec<_>>(),
                "anonymousRuntimeTools":anonymous_runtime_tools().iter().filter_map(|tool| tool["name"].as_str()).collect::<Vec<_>>(),
                "localDevelopment":"Use bun run konae -- init, dev, build, contract, login, publish, pull, sql, events, tool, conform or export; the development topic of platform_guide lists them. Development uses an isolated local Kernel. All platform operations use authenticated MCP. On a public app, actions declared anonymous run without a session, and visitors can list and read files uploaded as public. Uploads and private files require sign-in. Private apps require sign-in for everything."
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
            artifact::check_file_access(&a["manifest"]).map_err(anyhow::Error::msg)?;
            let initial_sources = if let Some(transfer_id) = a["sourcesTransferId"].as_str() {
                let upload = transfer(state, principal, &json!({"transferId":transfer_id}))?;
                anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload"
                    && (upload["projectId"].is_null() || upload["projectId"] == id), "Completed upload for this project required.");
                ensure_no_component(&upload)?;
                let bytes = tokio::fs::read(transfer_path(state, transfer_id)?).await?;
                let source = super::sources::normalize_document(&serde_json::from_slice::<Value>(&bytes)?)?;
                validate_sources(&source, &id)?;
                anyhow::ensure!(source["name"] == a["name"]
                    && a.get("slug").is_none_or(|slug| *slug == source["slug"])
                    && a.get("manifest").is_none_or(|manifest| super::sources::manifest_without_runtime(manifest) == source["manifest"]), "Initial files must match the project metadata.");
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
            let mut project = json!({"id":id,"name":a["name"],"slug":a["slug"].as_str().map(str::to_owned).unwrap_or_else(||safe_slug(a["name"].as_str().unwrap())),"manifest":a.get("manifest").map(super::sources::manifest_without_runtime).unwrap_or(json!({"version":1,"visibility":template["visibility"]})),"files":template["files"]});


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
            let request_id = a["requestId"].as_str();
            let cancel_request_id = a["cancelRequestId"].as_str();
            anyhow::ensure!(request_id.is_none() || cancel_request_id.is_none(), "Choose requestId or cancelRequestId.");
            for request_id in request_id.into_iter().chain(cancel_request_id) {
                anyhow::ensure!(
                    !request_id.is_empty() && request_id.len() <= 64 && request_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                    "Invalid source read request ID."
                );
            }
            let owner_principal = format!("owner:{}", owner.id);
            if let Some(request_id) = cancel_request_id {
                let _guard = database_lock(state, &format!("project:{id}")).await;
                let db = store(state)?;
                db.execute("DELETE FROM source_read_requests WHERE expires<=?1", [now_ms()])?;
                let existing: Option<(String, Option<String>)> = db
                    .query_row(
                        "SELECT project,transfer FROM source_read_requests WHERE owner=?1 AND request_id=?2",
                        params![owner.id, request_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((project, transfer_id)) = existing {
                    anyhow::ensure!(project == id, "Source read request ID belongs to another project.");
                    if let Some(transfer_id) = transfer_id {
                        let _ = transfer_call(state, &owner_principal, "transfer_cancel", &json!({"transferId":transfer_id}), false).await;
                    }
                }
                db.execute(
                    "INSERT INTO source_read_requests(owner,request_id,project,transfer,cancelled,expires) VALUES(?1,?2,?3,NULL,1,?4) ON CONFLICT(owner,request_id) DO UPDATE SET cancelled=1,expires=excluded.expires",
                    params![owner.id, request_id, id, now_ms().saturating_add(300_000)],
                )?;
                return Ok(json!({"cancelled":true}));
            }

            let _guard = database_lock(state, &format!("project:{id}")).await;
            let db = store(state)?;
            db.execute("DELETE FROM source_read_requests WHERE expires<=?1", [now_ms()])?;
            let revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            if a["metadataOnly"] == true {
                return match super::sources::read_metadata(&state.data_root, id).await? {
                    Some(project) => Ok(json!({"revision":revision,"project":project,"transfer":null})),
                    None => Ok(json!({"revision":0,"transfer":null})),
                };
            }
            if let Some(request_id) = request_id {
                let existing: Option<(String, bool)> = db
                    .query_row(
                        "SELECT project,cancelled FROM source_read_requests WHERE owner=?1 AND request_id=?2",
                        params![owner.id, request_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((project, cancelled)) = existing {
                    anyhow::ensure!(project == id, "Source read request ID belongs to another project.");
                    if cancelled { return Ok(json!({"revision":revision,"transfer":null,"cancelled":true})); }
                    anyhow::bail!("Source read request ID is already active.");
                }
            }
            match super::sources::read(&state.data_root, id).await? {
                Some(source) => {
                    let transfer = download(state, principal, &serde_json::to_vec(&source)?, Some(id)).await?;
                    if let Some(request_id) = request_id {
                        db.execute(
                            "INSERT INTO source_read_requests(owner,request_id,project,transfer,cancelled,expires) VALUES(?1,?2,?3,?4,0,?5)",
                            params![owner.id, request_id, id, transfer["transferId"].as_str(), now_ms().saturating_add(300_000)],
                        )?;
                    }
                    Ok(json!({"revision":revision,"project":{"id":source["id"],"name":source["name"],"slug":source["slug"],"manifest":source["manifest"]},"transfer":transfer}))
                }
                None => Ok(json!({"revision":0,"transfer":null})),
            }
        }
        "project_sources_save" => {
            let id = a["projectId"].as_str().unwrap();
            let transfer_id = a["transferId"].as_str().unwrap();
            let upload = transfer(state, principal, &json!({"transferId":transfer_id}))?;
            anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == id, "Completed upload for this project required.");
            ensure_no_component(&upload)?;
            let bytes = tokio::fs::read(transfer_path(state, transfer_id)?).await?;
            let update: Value = serde_json::from_slice(&bytes)?;
            let _guard = database_lock(state, &format!("project:{id}")).await;
            owned_project(state, owner, id, "write")?;
            let current: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            anyhow::ensure!(current == a["expectedRevision"].as_u64().unwrap(), "Source revision conflict. Reload the saved project before writing.");
            let previous = super::sources::read(&state.data_root, id).await?;
            let source = if update["format"] == "delta" {
                let saved = previous.clone().ok_or_else(|| anyhow::anyhow!("Saved project sources are unavailable."))?;
                merge_source_delta(&saved, &update, id)?
            } else {
                update
            };
            anyhow::ensure!(current == 0 || previous.is_some(), "Saved project sources are unavailable.");
            validate_sources(&source, id)?;
            let revision = current.checked_add(1).ok_or_else(|| anyhow::anyhow!("Source revision exhausted."))?;
            let pending = super::sources::stage_write(&state.data_root, id, &source).await?;
            let update_result = store(state).and_then(|db| db.execute("UPDATE projects SET sources=?2 WHERE id=?1", params![id, revision]).map_err(Into::into));
            if let Err(error) = update_result {
                if let Err(restore) = pending.rollback().await {
                    anyhow::bail!("Saving sources failed: {error}. Restoring the saved sources failed: {restore:#}");
                }
                return Err(error.into());
            }
            pending.commit().await;
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

            let _guard = database_lock(state, &format!("project:{id}")).await;
            let deployment = find_project_deployment(&*state.deployments.read().await, id);
            if name == "secret_set" && super::http::sets_route_secret(&a) {
                anyhow::ensure!(!route_secret_reused(state, &owner.id, id, a["value"].as_str().unwrap_or(""))?, super::http::SECRET_REUSED);
            }
            super::http::secrets_call(&super::http::secrets_store(&state.data_root, id)?, id, deployment.as_ref(), name, &a)
        }
        "api_keys_list" | "api_keys_create" | "api_keys_revoke" => {
            let id = a["projectId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, if name == "api_keys_list" { "read" } else { "publish" }).await?;
            super::http::api_keys_call(state, &deployment, name, &a).await
        }
        "notices_read" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap(), "read").await?;
            super::mail::notices_tool(state, &deployment, &a).await
        }
        "calls_read" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap(), "read").await?;
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
        "signals_read" | "usage_summary" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap(), "read").await?;
            super::usage::admin_call(&state.data_root, &deployment, name, &a).await
        }
        "sign_in_methods_read" => {
            let id = a["projectId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, "read").await?;
            Ok(sign_in_methods_value(state, &deployment))
        }
        "sign_in_methods_set" => {
            let id = a["projectId"].as_str().unwrap();
            let found = owned_deployment(state, owner, id, "manage_users").await?;


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
        "maintenance_read" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "read").await?;
            Ok(super::maintenance::read(&state.data_root, &deployment.id))
        }
        "contact_email_read" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "read").await?;
            Ok(super::notify::contact_view(&deployment))
        }
        "modules_read" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "read").await?;
            Ok(kernel::modules::read(&state, &deployment))
        }
        "module_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let module = a["module"].as_str().unwrap_or_default();
            let registry: Value = serde_json::from_str(include_str!("../../kernel/src/modules.json"))?;
            let scope = registry["modules"][module]["permission"].as_str().ok_or_else(|| anyhow::Error::new(kernel::refusal::refuse("module.unknown", format!("Unknown module {module}."))))?;
            let deployment = owned_deployment(state, owner, id, scope).await?;
            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            let result = kernel::modules::set(&state.data_root, &deployment.id, module, a["on"].as_bool(), a.get("options"), a.get("expected")).await?;
            let mut deployments = state.deployments.write().await;
            if let Some(current) = deployments.values_mut().find(|current| current.id == deployment.id) {
                current.modules.insert(module.to_owned(), result["state"].clone());
            }
            Ok(result)
        }
        "contact_email_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let deployment = owned_deployment(state, owner, id, "publish").await?;

            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            let current = owned_deployment(state, owner, id, "publish").await?;
            super::refusal::check_setting_expected(a.get("expected"), json!(current.contact_email))?;
            super::notify::set_contact_email(state, &current, a["email"].as_str()).await
        }
        "maintenance_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let found = owned_deployment(state, owner, id, "publish").await?;
            let _guard = database_lock(state, &format!("project:{}", found.project_id)).await;
            let deployment = owned_deployment(state, owner, id, "publish").await?;
            let on = a["on"].as_bool().ok_or_else(|| anyhow::anyhow!("on must be true or false."))?;
            let current = super::maintenance::read(&state.data_root, &deployment.id)["maintenance"].clone();
            super::refusal::check_setting_expected(a.get("expected"), current)?;
            super::maintenance::set(&state.data_root, &deployment.id, on)
        }
        "signup_policy_set" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap(), "manage_users").await?;
            let mode: SignupMode = serde_json::from_value(a["mode"].clone())?;
            let entries: Option<Vec<String>> = a.get("allowlist").map(|list| serde_json::from_value(list.clone())).transpose()?;
            set_signup_policy(state, &deployment.id, mode, entries).await
        }
        "database_execute" => {
            let id = a["projectId"].as_str().unwrap();
            let op = a["operation"].as_str().unwrap();
            let scope = if ["inspect", "browse", "list-backups"].contains(&op) {
                "read"
            } else {
                "write"
            };
            let deployment = owned_deployment(state, owner, id, scope).await?;
            let mut input = a.get("input").cloned().unwrap_or(json!({}));
            anyhow::ensure!(input.is_object(), "Input must be an object.");
            input["operation"] = json!(op);
            let answer = response_value(
                admin_database(state, id, input.to_string().as_bytes())
                .await,
            )
            .await?;


            if op == "restore-backup" && answer["holds"].as_array().is_some_and(|holds| holds.iter().any(|part| part == "sources")) {
                store(state)?.execute("UPDATE projects SET sources=sources+1 WHERE id=?1", [&deployment.project_id])?;
            }
            Ok(answer)
        }
        "project_export" => {
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap(), "export_data").await?;
            super::package::export(state, deployment, principal, |deployment| {
                let (_, published) = owned_project(state, owner, &deployment.project_id, "export_data")?;
                let sources: Option<u64> = store(state)?
                    .query_row("SELECT sources FROM projects WHERE id=?1", [&deployment.project_id], |r| r.get(0))
                    .optional()?
                    .filter(|saves: &u64| *saves > 0);
                Ok(super::package::Revisions { published, sources })
            })
            .await
        }
        "release_plan" => {
            let transfer_id = a["transferId"].as_str().unwrap();
            let sources_id = a["sourcesTransferId"].as_str();
            let input = release_input(state, principal, transfer_id, sources_id, None).await?;
            let artifact_sha256 = transfer(state, principal, &json!({"transferId": transfer_id}))?["sha256"].clone();
            let _guard = database_lock(state, &format!("project:{}", input.project.id)).await;
            let (_, revision) = owned_project(state, owner, &input.project.id, "publish")?;
            let source_revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [&input.project.id], |r| r.get(0))?;
            let has_deployment = find_project_deployment(&*state.deployments.read().await, &input.project.id).is_some();
            let observed = a["observedSourcesRevision"].as_u64();
            if (has_deployment || source_revision > 0) && observed != Some(source_revision) {
                return Err(source_revision_refusal(observed.unwrap_or(0)));
            }
            if let Some(package) = input.initial_package.as_ref() {

                if let Some(replay) = replay_import(state, owner, &input.project.id, &package.manifest.package_id)? {
                    return Ok(replay);
                }
                super::package::verify(state, principal, &input.project.id, package).await?;
            }
            let schema = validate_release(state, &input, None).await?;


            let existing = find_project_deployment(&*state.deployments.read().await, &input.project.id);
            let (hostname, first_release) = match existing {
                Some(deployment) => (Some(deployment.hostname), false),
                None => (super::hostname::planned(state, &input.project.slug, &input.project.id).await, true),
            };
            let url = hostname.as_deref().map(|hostname| deployment_url(&state.public_origin, hostname));
            let id = Uuid::new_v4().to_string();
            let expires = now_ms() + 900_000;


            let cleanup_uploads = release_upload_ids(state, principal, transfer_id, sources_id).await?;
            store(state)?.execute("INSERT INTO plans(id,owner,project,revision,transfer,expires,sources,source_revision,cleanup_uploads) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,owner.id,input.project.id,revision,transfer_id,expires,sources_id,source_revision,serde_json::to_string(&cleanup_uploads)?])?;
            if let Some(package) = input.initial_package.as_ref() {
                remember_import(state, &package.manifest.package_id, &input.project.id, &id)?;
            }
            Ok(
                json!({"planId":id,"projectId":input.project.id,"expectedRevision":revision,"expectedSourcesRevision":source_revision,"artifactSha256":artifact_sha256,"visibility":input.visibility,"expiresAt":expires,"schema":schema.to_json(),"hostname":hostname,"url":url,"firstRelease":first_release}),
            )
        }
        "release_apply" => {
            let id = a["planId"].as_str().unwrap();
            let row: (String,u64,String,u64,Option<String>,Option<String>,u64)=store(state)?.query_row("SELECT project,revision,transfer,expires,receipt,sources,source_revision FROM plans WHERE id=?1 AND owner=?2",params![id,owner.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
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
            let source_revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [&row.0], |r| r.get(0))?;
            if source_revision != row.6 {
                return Err(source_revision_refusal(row.6));
            }
            let mut input = release_input(state, principal, &row.2, row.5.as_deref(), None).await?;
            anyhow::ensure!(input.project.id == row.0, "Artifact belongs to another project.");

            input.contact_email = contact_email(state, owner, &row.0, input.contact_email.as_deref()).await?;


            input.schema_confirmation = a["confirmSchema"].as_str().map(str::to_owned);
            metadata["name"] = json!(input.project.name);
            metadata["slug"] = json!(input.project.slug);
            metadata["manifest"] = input.project.manifest.clone();
            let receipt = response_value(deploy_input(state, input).await).await?;
            let mut db = store(state)?;
            let tx = db.transaction()?;
            tx.execute("UPDATE projects SET revision=revision+1,sources=sources+1,body=?4 WHERE id=?1 AND owner=?2 AND revision=?3 AND sources=?5",params![row.0,owner.id,revision,metadata.to_string(),source_revision])?;
            tx.execute(
                "UPDATE plans SET receipt=?1 WHERE id=?2",
                params![receipt.to_string(), id],
            )?;
            tx.commit()?;
            drop(db);
            Ok(receipt)
        }
        "project_import" => project_import(state, owner, principal, &a).await,
        "app_user_enter" => {
            anyhow::ensure!(owner.session, "app_user_enter signs a browser in to the app, so it takes the owner's own browser session. A delegated token cannot call it.");
            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "manage_users").await?;
            super::enter::issue(state, &deployment, a["userId"].as_str().unwrap_or_default(), principal).await
        }
        name if name.starts_with("app_") => {

            let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "manage_users").await?;
            let inviter = super::invitations::Inviter { id: owner.id.clone(), email: inviter_email(state, &owner.id) };
            super::invitations::run(state, &deployment, &inviter, name, &a).await
        }
        name if name.starts_with("domain_") || name.starts_with("path_") || name.starts_with("hostname_") => {
            let scope = if name.ends_with("_read") { "read" } else { "publish" };
            let id = a["projectId"].as_str().unwrap_or_default();
            let found = owned_deployment(state, owner, id, scope).await?;
            let _guard = if scope == "publish" && !name.starts_with("hostname_") { Some(database_lock(state, &format!("project:{}", found.project_id)).await) } else { None };
            let deployment = owned_deployment(state, owner, id, scope).await?;
            if a.get("expected").is_some() && name != "hostname_set" {
                let current = if name == "domain_set" || name == "domain_clear" {
                    json!(super::domain::admin_call(state, &deployment, "domain_read", &a).await?["hostname"])
                } else if name == "path_set" || name == "path_clear" {
                    let view = super::path::admin_call(state, &deployment, "path_read", &a).await?;
                    json!({"enabled":view["enabled"],"name":view["name"],"acceptSharedOrigin":view["acceptSharedOrigin"]})
                } else {
                    Value::Null
                };
                super::refusal::check_setting_expected(a.get("expected"), current)?;
            }
            if name.starts_with("domain_") {
                super::domain::admin_call(state, &deployment, name, &a).await
            } else if name.starts_with("hostname_") {
                super::hostname::admin_call(state, &deployment, name, &a).await
            } else {
                super::path::admin_call(state, &deployment, name, &a).await
            }
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

const IMPORT_REPLACE: &str = "replace must be sources, data or both.";







async fn project_import(state: &Konae, owner: &Owner, principal: &str, a: &Value) -> Result<Value> {
    use super::import::{Outcome, Target};
    let deployment = owned_deployment(state, owner, a["projectId"].as_str().unwrap_or_default(), "publish").await?;
    let replace = a["replace"].as_str().unwrap_or_default();
    let (data, sources) = match replace {
        "sources" => (false, true),
        "data" => (true, false),
        "both" => (true, true),
        _ => anyhow::bail!(IMPORT_REPLACE),
    };
    anyhow::ensure!(data == a.get("package").is_some(), "Pass package exactly when replace is data or both.");
    anyhow::ensure!(sources == a.get("transferId").is_some(), "Pass transferId exactly when replace is sources or both.");
    anyhow::ensure!(!a.get("sourcesTransferId").is_some() || sources, "Pass sourcesTransferId only when replace is sources or both.");
    anyhow::ensure!(!a.get("artifactTransferId").is_some() || sources, "Pass artifactTransferId only when replace is sources or both.");
    let package: Option<super::package::InitialPackage> = a.get("package").map(|value| serde_json::from_value(value.clone())).transpose()?;
    let confirmation = a["confirmSchema"].as_str();
    let project = deployment.project_id.clone();
    let _guard = database_lock(state, &format!("project:{project}")).await;
    let (mut metadata, revision) = owned_project(state, owner, &project, "publish")?;
    if let Some(package) = &package {
        super::package::verify(state, principal, &project, package).await?;
    }
    let release = match a["transferId"].as_str() {
        Some(transfer_id) => {
            let upload = transfer(state, principal, &json!({"transferId": transfer_id}))?;
            anyhow::ensure!(upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == project.as_str(), "Completed upload for this project required.");
            let mut payload: Value = serde_json::from_slice(&release_bytes(state, &upload, transfer_id).await?)?;
            let input = release_input(state, principal, transfer_id, a["sourcesTransferId"].as_str(), a["artifactTransferId"].as_str()).await?;
            payload["project"]["files"] = json!(&input.project.files);
            payload["project"]["encodings"] = json!(&input.project.encodings);
            payload["artifact"] = json!({
                "version": input.artifact.version,
                "entrypoint": input.artifact.entrypoint,
                "files": input.artifact.files,
                "encodings": input.artifact.encodings,
            });
            anyhow::ensure!(input.project.id == project, "The release belongs to another project.");
            anyhow::ensure!(
                input.initial_package.is_none() && input.initial_signup_policy.is_none() && input.initial_sign_in_methods.is_none(),
                "An import into an existing app keeps its Signup Policy and sign-in methods, and takes its data as package."
            );
            let against = match &package {
                Some(package) => super::package::component_paths(state, &project, package)?.into_iter().find(|(name, _)| name == "database").map(|(_, path)| path),
                None => None,
            };
            let change = validate_release(state, &input, against.as_deref()).await?;
            if change.confirmation().is_some_and(|expected| confirmation != Some(expected.as_str())) {
                return Ok(json!({"imported": false, "replace": replace, "schema": change.to_json()}));
            }
            Some((payload, change))
        }
        None => None,
    };

    let resolved_contact_email = match &release {
        Some((payload, _)) => Some(contact_email(state, owner, &project, payload["contactEmail"].as_str()).await?),
        None => None,
    };
    let (backup, change) = match &package {
        Some(package) => {
            let target = match &release {
                Some((payload, _)) => Target::release(&serde_json::from_value::<DeployInput>(payload.clone())?)?,
                None => Target::live(state, &deployment)?,
            };
            match super::import::import_data(state, &deployment, package, &target, confirmation, sources).await? {
                Outcome::NeedsConfirmation(change) => return Ok(json!({"imported": false, "replace": replace, "schema": change.to_json()})),
                Outcome::Imported { backup, change } => (backup, change),
            }
        }
        None => (super::import::backup_sources(state, &deployment).await?, release.as_ref().map(|(_, change)| change.clone()).unwrap_or_default()),
    };
    let mut answer = json!({"imported": true, "replace": replace, "backup": backup, "schema": change.to_json()});
    if let Some((mut payload, _)) = release {
        payload["contactEmail"] = resolved_contact_email.flatten().map_or(Value::Null, Value::String);
        payload["schemaConfirmation"] = confirmation.map_or(Value::Null, |value| Value::String(value.to_owned()));
        for key in ["name", "slug", "manifest"] {
            if let Some(value) = payload["project"].get(key) {
                metadata[key] = value.clone();
            }
        }
        let receipt = match response_value(deploy(state, payload.to_string().as_bytes()).await).await {
            Ok(receipt) => receipt,
            Err(error) => return restore_import_after_release_failure(state, &deployment, package.is_some(), &backup, error).await,
        };
        store(state)?.execute(
            "UPDATE projects SET revision=revision+1, sources=sources+1, body=?3 WHERE id=?1 AND revision=?2",
            params![project, revision, metadata.to_string()],
        )?;
        answer["deployment"] = receipt;
    }
    Ok(answer)
}


async fn restore_import_after_release_failure(state: &Konae, deployment: &Deployment, has_package: bool, backup: &Value, error: anyhow::Error) -> Result<Value> {
    if has_package {
        let id = backup["id"].as_str().ok_or_else(|| anyhow::anyhow!("The import backup has no id."))?;
        let path = crate::layout::backups(&state.data_root, &deployment.id).join(format!("{id}.json"));
        let record: super::DatabaseBackup = serde_json::from_slice(&tokio::fs::read(path).await?)?;
        let _data = database_lock(state, &deployment.id).await;
        super::import::restore_held(&state.kernel, deployment, &record).await
            .map_err(|restore| anyhow::anyhow!("Import failed: {error:#}. Restoring its backup failed: {restore:#}"))?;
    }
    Err(error)
}





pub(super) fn inviter_email(state: &Konae, owner_id: &str) -> Option<String> {
    let db = Connection::open_with_flags(state.data_root.join("owners.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row("SELECT email FROM app_users WHERE id = ?1", [owner_id], |row| row.get(0)).optional().ok().flatten()
}

async fn contact_email(state: &Konae, owner: &Owner, project: &str, given: Option<&str>) -> Result<Option<String>> {
    let given = given.map(|given| super::auth::normalize_email(given).map_err(|_| anyhow::anyhow!(super::notify::CONTACT_EMAIL_INVALID))).transpose()?;
    let deployment = find_project_deployment(&*state.deployments.read().await, project);
    if deployment.is_none() && given.is_some() {
        return Ok(given);
    }
    let kept = deployment.and_then(|deployment| deployment.contact_email);
    Ok(kept.or_else(|| owner_email(state, &owner.id)))
}



fn owner_email(state: &Konae, owner: &str) -> Option<String> {
    let db = Connection::open_with_flags(state.data_root.join("owners.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row("SELECT email FROM app_users WHERE id = ?1 AND state = 'active'", [owner], |row| row.get(0)).optional().ok().flatten()
}

const METHODS_INVALID: &str = "Name the methods as password and google.";



fn requested_sign_in_methods(state: &Konae, deployment: &Deployment, requested: &Value) -> Result<Vec<SignInMethod>> {
    use super::refusal::refuse;
    let requested: Vec<SignInMethod> = serde_json::from_value(requested.clone())
        .map_err(|_| refuse("signin.methods-invalid", METHODS_INVALID))?;
    let methods = sign_in_methods_from_input(&requested).map_err(|message| refuse("signin.password-required", message))?;
    if methods.contains(&SignInMethod::Google) && !super::google::client_set(state, deployment).complete() {
        return Err(refuse("signin.google-client-unset", super::google::NEEDS_CLIENT).into());
    }
    Ok(methods)
}





fn sign_in_methods_value(state: &Konae, deployment: &Deployment) -> Value {
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




fn project_template(id: Option<&str>) -> Result<Value> {
    let all: Value = serde_json::from_str(include_str!("project-templates.json"))?;
    let id = id.or(all["default"].as_str()).unwrap_or_default();
    let template = &all["templates"][id];
    anyhow::ensure!(template.is_object(), "Unknown template {id}.");
    Ok(template.clone())
}

async fn owner_config(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
    match super::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => Json(json!({"mode":"clerk","publishableKey":config.publishable_key})).into_response(),
        Ok(None) => {

            let editor = std::env::var("KONAE_EDITOR_DIR").is_ok_and(|dir| !dir.trim().is_empty());

            let mail = std::env::var("KONAE_MAIL_TRANSPORT").unwrap_or_default();


            let local = url::Url::parse(&state.public_origin).ok().and_then(|u| u.host_str().map(crate::is_local_host)).unwrap_or(false);
            let default_key = local && state.setup_key == crate::DEFAULT_SETUP_KEY;
            Json(json!({"mode":"local","admission":super::admission::mode(&state).as_str(),"emailVerificationRequired":state.accounts.verification == auth::Verification::Required,"editor":editor,"mailToFile":mail == "development","defaultSetupKey":default_key.then_some(crate::DEFAULT_SETUP_KEY)})).into_response()
        }
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not configured."),
    }
}




pub(super) async fn owner_session(State(state): State<Konae>, headers: HeaderMap) -> Response {
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

async fn owner_clerk(State(state): State<Konae>, request: Request) -> Response {
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

pub(super) async fn owner_login(State(state): State<Konae>, request: Request) -> Response {
    owner_auth(state, request, false).await
}
pub(super) async fn owner_signup(State(state): State<Konae>, request: Request) -> Response {
    owner_auth(state, request, true).await
}

async fn owner_auth(state: Konae, request: Request, signup: bool) -> Response {
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

    if let Err(wait) = state
        .guards
        .auth_attempt("owner", client, a["email"].as_str().unwrap_or(""))
    {
        return too_many_attempts(wait);
    }


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
            &origin(&state, &headers),
            super::admission::return_to(&a).as_deref(),
        );
        return match super::owners::signup_creator(&state, &email, &page, admission, client).await {
            Ok(created) => {
                if let Some(user_id) = created {
                    claim_for_first_owner(&state, &user_id).await;
                }
                super::accounts::link_sent()
            }
            Err(e) => auth_error(e),
        };
    }


    let (result, _guard) = if let Some(admission) = admission {
        match blocking(move || auth::NewAccount::prepare(&email, &password)).await {
            Ok(account) => {
                let guard = database_lock(&state, "owner-auth").await;
                let auth = super::owners::creator_store(&state);
                let admit = super::admission::admit(admission);
                (blocking(move || auth?.create_account_admitted(account, admit)).await, Some(guard))
            }
            Err(error_value) => (Err(error_value), None),
        }
    } else {
        let failed_email = email.clone();
        let auth = super::owners::creator_store(&state);
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
        Err(AuthError::AccountPending) => error(StatusCode::FORBIDDEN, &super::accounts::pending_message(&state, false)),
        Err(e) => auth_error(e),
    }
}


async fn claim_for_first_owner(state: &Konae, user_id: &str) {
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


pub(super) fn owner_session_response(state: &Konae, session: auth::AuthSession) -> Response {
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

async fn owner_logout(State(state): State<Konae>, headers: HeaderMap) -> Response {
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
    if let Ok(auth) = super::owners::creator_store(&state) {
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

async fn owner_page(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        super::owner_page::LOGIN.as_str(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revision_conflicts_match_the_shared_case() {
        let fixture: Value = serde_json::from_str(include_str!("../../kernel/src/source-revision-cases.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let refusal = source_revision_refusal(case["observed"].as_u64().unwrap());
            let refusal = refusal.downcast_ref::<super::super::refusal::Refusal>().unwrap();
            assert_eq!(refusal.code, case["code"].as_str().unwrap());
            assert_eq!(refusal.message, case["error"].as_str().unwrap());
        }
    }

    #[tokio::test]
    async fn saved_sources_from_an_older_project_revision_remain_readable() {
        let root = std::env::temp_dir().join(format!("konae-old-sources-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        store(&state).unwrap().execute(
            "INSERT INTO projects(id,owner,body,revision,sources) VALUES('app_legacy','o','{}',0,0)", [],
        ).unwrap();
        let source = json!({
            "id":"app_legacy", "name":"Legacy", "slug":"legacy", "files":{"index.html":"<main>old source</main>"},
            "directories":[], "manifest":{"version":1,"visibility":"private","runtime":{"database":{"mode":"declared-actions","actions":[]},"auth":{"mode":"required"},"files":{"access":"deployment"}}}, "updatedAt":1
        });
        crate::Settings::new("app_legacy", "Legacy", "legacy", &source["manifest"]).write(&state.data_root).await.unwrap();
        super::super::sources::write(&state.data_root, "app_legacy", &source).await.unwrap();
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into()], projects:vec![], session:false };
        let answer = admin_call(&state, &owner, "owner:o", "project_sources_read", json!({"projectId":"app_legacy"})).await.unwrap();
        assert_eq!(answer["revision"], 0);
        assert!(answer["transfer"].is_object(), "a saved v0.8 source document remains available for the editor to migrate");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_failed_import_release_restores_the_old_sources_and_rows() {
        let root = std::env::temp_dir().join(format!("konae-import-release-failure-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let id = "app_import_failure";
        let deployment: Deployment = serde_json::from_value(json!({
            "id":id,"projectId":id,"projectName":"Import failure","slug":"import-failure","url":"http://import-failure.localhost",
            "createdAt":1,"releasedAt":1,"manifest":{"version":1,"visibility":"private"},"hostname":"import-failure.localhost",
            "actions":"{}","artifactVersion":"v1","signupPolicy":{"mode":"invitation","allowlist":[]},"signInMethods":["password"]
        })).unwrap();
        let old_source = json!({
            "id":id,"name":"Import failure","slug":"import-failure","files":{"index.html":"old source"},"directories":[],
            "manifest":{"version":1,"visibility":"private","runtime":{"database":{"mode":"declared-actions","actions":[]},"auth":{"mode":"required"},"files":{"access":"deployment"}}},"updatedAt":1
        });
        crate::Settings::new(id, "Import failure", "import-failure", &old_source["manifest"]).write(&state.data_root).await.unwrap();
        super::super::sources::write(&state.data_root, id, &old_source).await.unwrap();
        let database = kernel::database_path(&state.kernel, id);
        tokio::fs::create_dir_all(database.parent().unwrap()).await.unwrap();
        rusqlite::Connection::open(&database).unwrap().execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES('old row');").unwrap();

        let backup_id = "injected-release-failure";
        let key = "backup-data-and-sources";
        let backup_root = crate::layout::backups(&state.data_root, id);
        let backup_folder = backup_root.join(key);
        tokio::fs::create_dir_all(&backup_folder).await.unwrap();
        tokio::fs::copy(&database, backup_folder.join("database.sqlite")).await.unwrap();
        tokio::fs::write(backup_folder.join("sources.json"), serde_json::to_vec(&old_source).unwrap()).await.unwrap();
        let record = super::DatabaseBackup { id: backup_id.into(), label: "Before import".into(), created_at: 1, size_bytes: 0, key: key.into(), holds: vec!["data".into(), "sources".into()] };
        tokio::fs::write(backup_root.join(format!("{backup_id}.json")), serde_json::to_vec(&record).unwrap()).await.unwrap();

        rusqlite::Connection::open(&database).unwrap().execute_batch("DELETE FROM notes; INSERT INTO notes VALUES('imported row');").unwrap();
        let imported_source = json!({"id":id,"name":"Import failure","slug":"import-failure","files":{"index.html":"imported source"},"directories":[],"manifest":old_source["manifest"],"updatedAt":2});
        super::super::sources::write(&state.data_root, id, &imported_source).await.unwrap();
        let backup = record.view();
        let error = restore_import_after_release_failure(&state, &deployment, true, &backup, anyhow::anyhow!("Injected release failure")).await.unwrap_err();
        assert!(error.to_string().contains("Injected release failure"));
        let row: String = rusqlite::Connection::open(&database).unwrap().query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(row, "old row");
        let restored = super::super::sources::read(&state.data_root, id).await.unwrap().unwrap();
        assert_eq!(restored["files"]["index.html"], "old source");
        let _ = tokio::fs::remove_dir_all(root).await;
    }



    #[tokio::test]
    async fn the_path_host_refuses_administration() {
        use tower::ServiceExt;
        let root = std::env::temp_dir().join(format!("konae-path-admin-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState {
            public_origin: "https://app.konae.es".into(),
            host_suffix: "konae.es".into(),
            path_origin: Some("https://konae.es".into()),
            ..AppState::for_test(&root)
        };
        let konae = Konae::for_test(state.clone());
        for (path, body) in [("/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string()), ("/_konae/owner/login", "{}".to_owned())] {
            let request = Request::builder().method("POST").uri(path).header(header::HOST, "konae.es")
                .header(header::ORIGIN, "https://konae.es").header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body)).unwrap();
            let response = crate::stack(state.clone(), crate::owner_routes(konae.clone())).oneshot(request).await.unwrap();
            assert_eq!(response.status(), 403, "{path}");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }




    #[tokio::test]
    async fn an_expired_token_says_it_expired() {
        use tower::ServiceExt;
        let root = std::env::temp_dir().join(format!("konae-expired-token-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState {
            public_origin: "https://app.konae.es".into(),
            host_suffix: "konae.es".into(),
            ..AppState::for_test(&root)
        };
        let konae = Konae::for_test(state.clone());
        let expired = 1_700_000_000_000u64;
        let db = store(&konae).unwrap();
        for (token, audience, client) in [
            ("lb_cli", None, None),
            ("lb_oauth", Some("https://app.konae.es/mcp"), Some("client-1")),
            ("lb_elsewhere", Some("https://other.example/mcp"), Some("client-1")),
        ] {
            db.execute(
                "INSERT INTO credentials(hash,owner,scopes,projects,expires,audience,client) VALUES(?1,'o','[\"read\"]','[]',?2,?3,?4)",
                params![digest(token.as_bytes()), expired, audience, client],
            )
            .unwrap();
        }
        let call = |token: &'static str| {
            let request = Request::builder().method("POST").uri("/mcp").header(header::HOST, "app.konae.es")
                .header(header::CONTENT_TYPE, "application/json").header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string())).unwrap();
            crate::stack(state.clone(), crate::owner_routes(konae.clone())).oneshot(request)
        };
        let read = |response: Response| async move {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let challenge = response.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().to_owned();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            (challenge, serde_json::from_slice::<Value>(&body).unwrap()["error"].as_str().unwrap().to_owned())
        };
        let (challenge, unknown) = read(call("lb_unknown").await.unwrap()).await;
        assert_eq!(challenge, "Bearer resource_metadata=\"https://app.konae.es/.well-known/oauth-protected-resource\"");
        assert_eq!(unknown, "Sign in to use Konae MCP.");
        assert_eq!(read(call("lb_elsewhere").await.unwrap()).await, (challenge.clone(), unknown.clone()));
        let (cli_challenge, cli) = read(call("lb_cli").await.unwrap()).await;
        assert_eq!(cli_challenge, challenge);
        assert_eq!(cli, "This Konae token expired at 2023-11-14 22:13 UTC. Run konae login for a new CLI credential, reconnect the MCP client, or sign in as the owner at https://app.konae.es/_konae/owner/login.");
        assert!(!cli.contains("last 24 hours"));
        let (oauth_challenge, oauth) = read(call("lb_oauth").await.unwrap()).await;
        assert_eq!(oauth_challenge, challenge);
        assert_eq!(oauth, "This Konae sign-in expired at 2023-11-14 22:13 UTC. Run konae login --browser for the CLI, or reconnect your MCP client; either opens https://app.konae.es/_konae/owner/login.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A domain or path tool reads with read and changes with publish.
    #[tokio::test]
    async fn a_reader_reads_but_cannot_set_a_domain_or_a_path() {
        let root = std::env::temp_dir().join(format!("konae-tool-scope-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let body = json!({
            "project": {"id": "app_scope", "name": "Scope", "slug": "scope", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        store(&state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES('app_scope','o','{}')", []).unwrap();
        let id = state.deployments.read().await.values().next().unwrap().id.clone();
        let reader = Owner { id: "o".into(), display: None, scopes: vec!["read".into()], projects: vec![], session: false };
        for name in ["domain_read", "path_read"] {
            admin_call(&state, &reader, "owner:o", name, json!({"projectId": id})).await.unwrap();
        }
        for (name, arguments) in [("domain_set", json!({"projectId": id, "hostname": "scope.example.org"})), ("path_set", json!({"projectId": id, "enabled": true})), ("hostname_set", json!({"projectId": id, "name": "scope-two"}))] {
            let refused = admin_call(&state, &reader, "owner:o", name, arguments).await.unwrap_err().to_string();
            assert_eq!(refused, "Permission denied for publish.", "{name}");
        }
        // A publisher moves the generated hostname, and project_inspect reads the new url back.
        let publisher = Owner { id: "o".into(), display: None, scopes: vec!["read".into(), "publish".into()], projects: vec![], session: false };
        let moved = admin_call(&state, &publisher, "owner:o", "hostname_set", json!({"projectId": id, "name": "scope-two"})).await.unwrap();
        assert_eq!(moved["changed"], true);
        let inspected = admin_call(&state, &publisher, "owner:o", "project_inspect", json!({"projectId": "app_scope"})).await.unwrap();
        assert_eq!(inspected["deployment"]["hostname"], moved["hostname"]);
        assert_eq!(inspected["deployment"]["url"], moved["url"]);
        assert!(moved["hostname"].as_str().unwrap().starts_with("scope-two."), "{moved}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn module_tools_read_back_changes_and_refuse_stale_or_unauthorized_writes() {
        let root = std::env::temp_dir().join(format!("konae-module-tools-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let body = json!({
            "project":{"id":"app_modules","name":"Modules","slug":"modules","manifest":{},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},
            "visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<title>Modules</title>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        store(&state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES('app_modules','o','{}')", []).unwrap();
        let publisher = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"publish".into()], projects:vec![], session:false };
        let reader = Owner { scopes:vec!["read".into()], ..publisher.clone() };
        let before = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        let expected = before["modules"]["seo"]["state"].clone();
        admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":true,"options":{"canonical":"path"},"expected":expected})).await.unwrap();
        let after = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        assert_eq!(after["modules"]["seo"]["state"]["on"], true);
        assert_eq!(after["modules"]["seo"]["state"]["canonical"], "path");
        assert_eq!(state.deployments.read().await.values().find(|item| item.id == "app_modules").unwrap().modules["seo"]["canonical"], "path");
        assert_eq!(after["modules"]["actions"]["state"]["on"], true);
        assert_eq!(after["modules"]["actions"]["switch"], false);
        assert!(after["modules"]["actions"].get("canChange").is_none());
        let stale = admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":false,"expected":expected})).await.unwrap_err();
        let refusal = stale.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.changed");
        let denied = admin_call(&state, &reader, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":false})).await.unwrap_err().to_string();
        assert_eq!(denied, "Permission denied for publish.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

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


    /// The runtime schema leaves the size bound to the handler, which knows
    /// the installation's per-file limit and names it.
    #[tokio::test]
    async fn a_runtime_upload_is_bounded_by_the_configured_per_file_limit() {
        let root = std::env::temp_dir().join(format!("konae-runtime-limit-{}", Uuid::new_v4()));
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let mut state = Konae::for_test(state);
        state.kernel.files.limits = crate::file_store::FileLimits::parse(Some("10485760"), Some("5242880")).unwrap();
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
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(state);
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
        crate::Settings::new(draft, "Draft", "draft", &saved["manifest"]).write(&state.data_root).await.unwrap();
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
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(state);
        let owner = Owner { id: "ana".into(), display: None, scopes: vec!["read".into(), "write".into()], projects: vec![], session: false };
        let manifest = json!({"version": 1, "visibility": "private", "runtime": {"database": {"mode": "declared-actions", "actions": []}, "auth": {"mode": "required"}, "files": {"access": "deployment"}}});
        let create = |files: Value| {
            let (state, owner, manifest) = (state.clone(), owner.clone(), manifest.clone());
            async move {
                let source = json!({"id": "app_retry", "name": "Retry", "slug": "retry", "files": files, "directories": [], "manifest": manifest, "updatedAt": 1});
                let upload = super::super::transfers::new_transfer(&state, "owner:ana", json!({"direction": "upload", "size": 0, "sha256": "", "complete": true, "projectId": null})).await.unwrap();
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
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(state);
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
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(state);
        let ok: Result<Value> = Ok(json!({}));
        audit(&state, "owner:ana", "project_create", &json!({"projectId": "p1"}), None, &ok);
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
        runtime_audit(root.clone())("runtime:d1:user-y", "files_upload", "d1", true);
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
        assert!(!names(&reader).contains(&"signup_policy_read".to_owned()));
        assert!(names(&reader).contains(&"usage_summary".to_owned()));
        assert!(!names(&reader).contains(&"usage_read".to_owned()));
        for definition in admin_tools(&reader) {
            assert!(definition["inputSchema"]["properties"].get("deploymentId").is_none(), "owner tool {} uses deploymentId", definition["name"]);
        }
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
        let file: Value = serde_json::from_str(include_str!("../../kernel/src/google-client-cases.json")).unwrap();
        let (client_id, client_secret) = (file["context"]["clientId"].as_str().unwrap(), file["context"]["clientSecret"].as_str().unwrap());
        for case in file["cases"].as_array().unwrap() {
            let name = case["case"].as_str().unwrap();
            let root = std::env::temp_dir().join(format!("konae-google-cases-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let state = Konae::for_test(AppState {
                public_origin: "http://localhost:8790".into(),
                ..AppState::for_test(&root)
            });
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
        let state = Konae::for_test(AppState {
            public_origin: "http://localhost:8790".into(),
            ..AppState::for_test(&root)
        });
        let mut deployment: Deployment = serde_json::from_value(json!({
            "id": "p1", "projectId": "p1", "projectName": "Practice", "slug": "practice", "artifactVersion": "v1", "signupPolicy": {"mode": "closed"}, "signInMethods": ["password"], "url": "http://practice.localhost:8790", "createdAt": 0,
            "manifest": {"visibility": "private"}, "hostname": "practice.localhost", "actions": "{}"
        }))
        .unwrap();
        let refused = |state: &Konae, deployment: &Deployment, methods: Value| requested_sign_in_methods(state, deployment, &methods).unwrap_err().to_string();
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

        deployment.sign_in_methods = vec![SignInMethod::Password, SignInMethod::Google];
        let value = sign_in_methods_value(&state, &deployment);
        assert_eq!(value["googleClient"], json!({"clientIdSet": true, "clientSecretSet": true}));
        assert_eq!(value["offered"], json!(["password", "google"]));
        assert_eq!(value["redirectUris"], json!(["http://practice.localhost:8790/_konae/auth/google/callback"]));
        assert!(!value.to_string().contains("GOCSPX"));

        let deleted = super::super::http::secrets_call(&db, "p1", Some(&deployment), "secret_delete", &json!({"name": "google_client_secret"}));
        assert_eq!(deleted.unwrap_err().to_string(), "Google sign-in is on for this app and needs google_client_secret. Turn Google off before deleting it.");

        db.execute("DELETE FROM secrets", []).unwrap();
        let value = sign_in_methods_value(&state, &deployment);
        assert_eq!((value["methods"].clone(), value["offered"].clone()), (json!(["password", "google"]), json!(["password"])));
        assert_eq!(value["notice"], GOOGLE_CLIENT_MISSING);
        let _ = std::fs::remove_dir_all(root);
    }




    #[test]
    fn a_refused_source_save_names_the_expected_format() {
        let manifest = json!({"version":1,"visibility":"private","runtime":{"database":{"mode":"declared-actions","actions":[]},"auth":{"mode":"optional"},"files":{"access":"deployment"}}});
        let base = json!({"id":"app_a","name":"A","slug":"a","files":{"index.html":"<main></main>"},"directories":[],"manifest":manifest,"updatedAt":1});
        assert!(validate_sources(&base, "app_a").is_ok());
        let refusal = |change: &dyn Fn(&mut Value)| { let mut source = base.clone(); change(&mut source); validate_sources(&source, "app_a").unwrap_err().to_string() };
        assert_eq!(refusal(&|s| s["manifest"]["fileAccess"] = json!("public")), artifact::FILE_ACCESS_MESSAGE);
        assert!(refusal(&|s| { s.as_object_mut().unwrap().remove("directories"); }).starts_with("Invalid project directories."));
        assert!(refusal(&|s| s["manifest"] = json!({"version":1})).starts_with("Invalid project manifest. Send version 1 and visibility"));
        assert!(refusal(&|s| { s.as_object_mut().unwrap().remove("updatedAt"); }).starts_with("Invalid project metadata. Send { id, name, slug, files, directories, manifest, updatedAt }"));
        let big = |s: &mut Value| {
            s["files"]["public/big.bin"] = json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, vec![0xffu8; super::super::sources::MAX_BYTES + 1]));
            s["encodings"] = json!({"public/big.bin": "base64"});
        };
        assert_eq!(refusal(&big), "app/ holds 20.1 MiB. A project's sources hold at most 20 MiB. The largest files are public/big.bin (20.1 MiB) and index.html (13 bytes).", "a file that is not text counts its decoded bytes");
        let mut source = base.clone();
        big(&mut source);
        assert_eq!(super::super::refusal::of(&validate_sources(&source, "app_a").unwrap_err()).map(|refused| refused.code), Some("sources.size"));
        assert!(refusal(&|s| s["files"]["public/.DS_Store"] = json!("x")).starts_with("The sources cannot hold the path \"public/.DS_Store\"."));
        let with_png = |s: &mut Value| { s["files"]["public/logo.png"] = json!("iVBORw0KGgo="); s["encodings"] = json!({"public/logo.png": "base64"}); };
        let mut binary = base.clone();
        with_png(&mut binary);
        assert!(validate_sources(&binary, "app_a").is_ok());
        assert_eq!(refusal(&|s| { with_png(s); s["encodings"] = json!({"public/logo.png": 1}); }), "The sources' encodings must map each path to \"base64\".");
    }

    #[tokio::test]
    async fn a_failed_source_revision_write_restores_sources_and_keeps_revision() {
        use base64::Engine as _;
        use sha2::Digest as _;

        let root = std::env::temp_dir().join(format!("konae-refused-save-revision-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let id = "app_save_revision";
        store(&state).unwrap().execute(
            "INSERT INTO projects(id,owner,body,revision,sources) VALUES(?1,'o',?2,1,4)",
            params![id, json!({"id":id,"name":"Saved"}).to_string()],
        ).unwrap();
        store(&state).unwrap().execute_batch("CREATE TRIGGER fail_source_revision BEFORE UPDATE OF sources ON projects BEGIN SELECT RAISE(ABORT, 'injected source revision failure'); END;").unwrap();
        let applied_actions = r#"{"version":1,"actions":{"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT body FROM notes ORDER BY id LIMIT 50","bindings":[]}}}"#;
        let draft_actions = r#"{"version":1,"actions":{"draft.list":{"kind":"query","access":"anonymous","statement":"SELECT body FROM notes ORDER BY id LIMIT 50","bindings":[]}}}"#;
        let saved = json!({"id":id,"name":"Saved","slug":"saved","files":{"index.html":"before","actions.json":draft_actions,"schema.sql":"CREATE TABLE draft_only(id INTEGER PRIMARY KEY);"},"directories":["explicit"],"manifest":{"version":1,"visibility":"private"},"updatedAt":1});
        crate::Settings::new(id, "Saved", "saved", &saved["manifest"]).write(&state.data_root).await.unwrap();
        crate::sources::write(&state.data_root, id, &saved).await.unwrap();
        let database = kernel::database_path(&state.kernel, id);
        tokio::fs::create_dir_all(database.parent().unwrap()).await.unwrap();
        rusqlite::Connection::open(&database).unwrap().execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes(body) VALUES('applied row');").unwrap();
        let mut deployment = crate::Settings::read(&state.data_root, id).unwrap().deployment(crate::Installed {
            hostname:"saved.localhost".into(), created_at:1, released_at:Some(2), source_digest:Some("prior".into()),
            actions:applied_actions.into(), artifact_version:"vprior".into(), released:None,
        }, "http://localhost");
        deployment.released_schema = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT);".into();
        let installed = crate::Installed { hostname:deployment.hostname.clone(), created_at:deployment.created_at, released_at:Some(deployment.released_at), source_digest:deployment.source_digest.clone(), actions:deployment.actions.clone(), artifact_version:deployment.artifact_version.clone(), released:None };
        tokio::fs::write(crate::layout::record(&state.data_root, id), serde_json::to_vec(&installed).unwrap()).await.unwrap();
        let legacy_metadata = json!({"name":"Legacy saved draft","slug":"legacy-saved","manifest":{"version":1,"visibility":"public","fileAccess":"app-users"},"directories":["legacy-empty"],"updatedAt":9});
        let legacy_path = crate::layout::private(&state.data_root, id).join("sources.json");
        tokio::fs::write(&legacy_path, serde_json::to_vec(&legacy_metadata).unwrap()).await.unwrap();
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"write".into()], projects:vec![], session:false };
        let app_path = crate::layout::sources(&state.data_root, id).join("index.html");
        let settings_path = crate::layout::settings(&state.data_root, id);
        let record_path = crate::layout::record(&state.data_root, id);
        let before_record = tokio::fs::read(&record_path).await.unwrap();
        let before_legacy = tokio::fs::read(&legacy_path).await.unwrap();
        let before_app = tokio::fs::read(&app_path).await.unwrap();
        let before_settings = tokio::fs::read(&settings_path).await.unwrap();
        let mut next = crate::sources::read(&state.data_root, id).await.unwrap().unwrap();
        next["name"] = json!("Refused name");
        next["files"]["index.html"] = json!("must roll back");
        next["updatedAt"] = json!(2);
        let bytes = serde_json::to_vec(&next).unwrap();
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        let principal = "owner:o";
        let transfer = transfer_call(&state.kernel, principal, "transfer_begin", &json!({"projectId":id,"size":bytes.len(),"sha256":digest}), false).await.unwrap();
        let transfer_id = transfer["transferId"].as_str().unwrap();
        transfer_call(&state.kernel, principal, "transfer_write", &json!({"transferId":transfer_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
        transfer_call(&state.kernel, principal, "transfer_complete", &json!({"transferId":transfer_id}), false).await.unwrap();
        let error = admin_call(&state, &owner, principal, "project_sources_save", json!({"projectId":id,"transferId":transfer_id,"expectedRevision":4})).await.unwrap_err();
        assert!(error.to_string().contains("injected source revision failure"), "{error:#}");
        let revision: u64 = store(&state).unwrap().query_row("SELECT sources FROM projects WHERE id=?1", [id], |row| row.get(0)).unwrap();
        assert_eq!(revision, 4);
        assert_eq!(tokio::fs::read(app_path).await.unwrap(), before_app);
        assert_eq!(tokio::fs::read(settings_path).await.unwrap(), before_settings);
        assert_eq!(tokio::fs::read(record_path).await.unwrap(), before_record);
        assert_eq!(tokio::fs::read(legacy_path).await.unwrap(), before_legacy);
        let source = crate::sources::read(&state.data_root, id).await.unwrap().unwrap();
        assert_eq!(source["name"], "Legacy saved draft");
        assert_eq!(source["manifest"]["fileAccess"], "app-users");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn source_deltas_merge_changed_files_and_recheck_the_whole_document() {
        let manifest = json!({"version":1,"visibility":"private","runtime":{"database":{"mode":"declared-actions","actions":[]},"auth":{"mode":"optional"},"files":{"access":"deployment"}}});
        let current = json!({"id":"app_a","name":"A","slug":"a","files":{"index.html":"old","public/logo.png":"iVBORw0KGgo="},"encodings":{"public/logo.png":"base64"},"directories":["public"],"manifest":manifest,"updatedAt":1});
        let delta = json!({"format":"delta","metadata":{"name":"A","slug":"a","directories":["public"],"manifest":manifest,"updatedAt":2},"files":{"index.html":"new"},"removedPaths":[]});
        let merged = merge_source_delta(&current, &delta, "app_a").unwrap();
        assert_eq!(merged["files"]["index.html"], "new");
        assert_eq!(merged["files"]["public/logo.png"], "iVBORw0KGgo=");
        assert_eq!(merged["encodings"]["public/logo.png"], "base64");
        assert!(validate_sources(&merged, "app_a").is_ok());

        let remove = json!({"format":"delta","metadata":{"name":"A","slug":"a","directories":[],"manifest":manifest,"updatedAt":3},"files":{},"removedPaths":["public/logo.png"]});
        let merged = merge_source_delta(&merged, &remove, "app_a").unwrap();
        assert!(merged["files"].get("public/logo.png").is_none());
        assert!(merged.get("encodings").is_none());
        assert!(validate_sources(&merged, "app_a").is_ok());

        let invalid = json!({"format":"delta","metadata":{"name":"A","slug":"a","directories":[],"manifest":manifest,"updatedAt":4},"files":{"public/.secret":"x"},"removedPaths":[]});
        assert!(validate_sources(&merge_source_delta(&merged, &invalid, "app_a").unwrap(), "app_a").is_err());
    }




    #[tokio::test]
    async fn platform_guide_sdk_topic_is_the_rendered_contract() {
        let root = std::env::temp_dir().join(format!("konae-guide-{}", Uuid::new_v4()));
        let state = Konae::for_test(AppState::for_test(&root));
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
