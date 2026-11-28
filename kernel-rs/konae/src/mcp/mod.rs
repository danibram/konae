use super::*;
use super::runtime::*;
use super::transfers::{ADMIN_CHUNK, ADMIN_MCP_BODY_BYTES, MAX_TRANSFER, transfer, transfer_call, transfer_path};
use rusqlite::params;

#[path = "function_candidate.rs"]
mod function_candidate;
use crate::projects::sources::{ensure_no_component, validate_sources};
#[cfg(test)]
use crate::projects::release::source_revision_refusal;
#[cfg(test)]
use crate::projects::release::restore_import_after_release_failure;
#[cfg(test)]
use crate::projects::sources::merge_source_delta;


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

/// Tools that change nothing a paused app serves. Every other tool that
/// names a paused Deployment, its project or one of its plans is refused.
const OPEN_TOOLS: [&str; 18] = [
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
        RootDatabase {
            file: "owners.sqlite",
            sign_out: &["app_sessions", "creator_sessions", "account_challenges", "entry_links"],
        },
        RootDatabase {
            file: "mcp.sqlite",
            sign_out: &[
                "credentials", "oauth_codes", "plans", "source_read_requests", "package_imports",
                "conversation_live_authority", "conversation_execution_leases", "conversation_stop_requests",
            ],
        },
    ]);
    installation.accounts.push(AccountDatabase { scope: super::owners::CREATOR_SCOPE, kind: "creator", file: "owners.sqlite" });
    installation.accounts.push(AccountDatabase { scope: super::owners::CREATOR_SCOPE, kind: "clerk-owner", file: "owners.sqlite" });
    installation.mail_group = project_owner;
    installation.check_catalog = check_restored_catalog;
    installation.check_cut = super::owners::check_suspensions;
    installation
}


fn check_restored_catalog(root: &Path) -> Result<()> {
    let path = root.join("mcp.sqlite");
    if !path.try_exists()? {
        return Ok(());
    }
    let read = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let catalog = Connection::open_with_flags(path, read)?;
    if !catalog.query_row("SELECT EXISTS(SELECT 1 FROM projects)", [], |row| row.get::<_, bool>(0))? {
        return Ok(());
    }
    let owners = Connection::open_with_flags(root.join("owners.sqlite"), read)?;
    let mut owner_tables = Vec::new();
    for (table, column) in [("app_users", "id"), ("creator_identities", "owner")] {
        if crate::table_exists(&owners, table)? {
            owner_tables.push(format!("SELECT 1 FROM {table} WHERE {column}=?1"));
        }
    }
    anyhow::ensure!(!owner_tables.is_empty(), "The project catalog has no owner store.");
    let mut owner = owners.prepare(&format!("SELECT EXISTS({})", owner_tables.join(" UNION ALL ")))?;
    let mut projects = catalog.prepare("SELECT id, owner FROM projects")?;
    let mut rows = projects.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let account: String = row.get(1)?;
        anyhow::ensure!(crate::runtime::valid_app_id(&id) && crate::layout::settings(root, &id).is_file(),
            "Catalog project {id} has no matching app folder.");
        anyhow::ensure!(owner.query_row([account], |r| r.get::<_, bool>(0))?, "Catalog project {id} has no matching owner.");
    }
    Ok(())
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
    let has_project_sources: bool = db
        .prepare("PRAGMA table_info(projects)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "sources");
    if !has_project_sources {
        db.execute("ALTER TABLE projects ADD COLUMN sources INTEGER NOT NULL DEFAULT 0", [])?;
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
    super::owners::oauth::install(&db)?;
    super::projects::release::install_imports(&db)?;
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
    let clerk = super::owners::clerk::ClerkConfig::from_env()?;
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
                    anyhow::ensure!(super::owners::clerk::owner_allowed(&db, config, &r.0)?, "Owner is not admitted.");
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
        super::owners::clerk::authenticate(&db, config, cookie.as_deref(), (now_ms() / 1000) as i64)?
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
    let login = format!("{}{}", origin(state, headers), super::owners::oauth::owner_login_path());
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
    let _generation = state.copies.generations.read().await;
    let pending = crate::copies::Pending::begin(&state.data_root, id, "removing the project folder and catalogue entry")?;
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
    let owner: Option<String> = tx.query_row("SELECT owner FROM projects WHERE id=?1", [id], |r| r.get(0)).optional()?;
    if let Some(owner) = owner {
        super::conversations::delete_project_tx(&tx, &owner, id, now_ms() as i64)?;
    }
    for (table, column) in [("plans", "project"), ("package_imports", "project")] {
        let present: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |r| r.get(0))?;
        if present {
            tx.execute(&format!("DELETE FROM {table} WHERE {column}=?1"), [id])?;
        }
    }
    tx.execute("DELETE FROM projects WHERE id=?1", [id])?;
    tx.commit()?;
    pending.finish()?;
    Ok(ids)
}






fn route_secret_reused(state: &Konae, owner: &str, project: &str, value: &str) -> Result<bool> {
    let db = store(state)?;
    let mut query = db.prepare("SELECT id FROM projects WHERE owner = ?1 AND id <> ?2")?;
    let others = query.query_map(params![owner, project], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for other in others {
        if crate::layout::secrets(&state.data_root, &other).is_file() && super::http::holds_route_secret(&super::http::read_secrets(&state.data_root, &other)?, value)? {
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
            "Read a guide topic. Start with rules (yardstick, first run, read-back, divergences), then pieces (app declarations and setup for Documents, App MCP, File intake and FTPS). Also credentials, development, actions, sdk, publishing, users, portability and refusals.",
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
            tool("function_candidate_check", "Check a completed candidate Server Artifact before Apply. The upload must belong to projectId, fit its write grant and match buildFingerprint, the SHA-256 of the exact uploaded bytes. This changes no project or Deployment.", json!({"projectId":text,"transferId":text,"scratchRevision":text,"buildFingerprint":{"type":"string","pattern":"^[0-9a-f]{64}$"}}), &["projectId","transferId","scratchRevision","buildFingerprint"], false),
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
    let module_registry: Value = serde_json::from_str(include_str!("../../../kernel/src/modules.json")).expect("module registry is valid JSON");
    if owner.scopes.iter().any(|scope| module_registry["modules"].as_object().into_iter().flatten().any(|(_, module)| module["switch"] == true && module["permission"] == scope.as_str())) {
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
    if owner.scopes.iter().any(|s| s == "manage_users") {
        tools.extend(invitations_tools());
        tools.extend([
            tool("ftps_credentials_list", "List this deployment's FTPS destinations and credentials without passwords.", json!({"projectId":text}), &["projectId"], true),
            tool("ftps_device_create", "Issue a permanent FTPS credential for one device destination. The password appears once in this answer. Read back with ftps_credentials_list.", json!({"projectId":text,"destination":{"type":"string","minLength":1,"maxLength":100},"label":{"type":"string","minLength":1,"maxLength":80}}), &["projectId","destination","label"], false),
            tool("ftps_device_rotate", "Replace a device FTPS password. The new password appears once in this answer. Read back with ftps_credentials_list.", json!({"projectId":text,"credentialId":{"type":"string","minLength":1,"maxLength":100}}), &["projectId","credentialId"], false),
            tool("ftps_credential_revoke", "Revoke an FTPS credential. Read back with ftps_credentials_list.", json!({"projectId":text,"credentialId":{"type":"string","minLength":1,"maxLength":100}}), &["projectId","credentialId"], false),
        ]);
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
            tool("secret_set", "Set a signing secret an action's http signature names, or, with use call, one a call action signs with or an API key it sends, or, with use google, the app's Google OAuth client as google_client_id and google_client_secret. The value is never returned. Read back with secrets_list, and read Google client state with modules_read.", json!({"projectId":text,"name":{"type":"string","minLength":1,"maxLength":64},"value":{"type":"string","minLength":16,"maxLength":512},"use":{"type":"string","enum":["route","call","google"]}}), &["projectId","name","value"], false),
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
    if !super::owners::oauth::valid_owner_host(&state, &headers).await {
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
            let execution_headers = [
                "x-konae-execution-project",
                "x-konae-execution-conversation",
                "x-konae-execution-id",
                "x-konae-execution-generation",
                "x-konae-execution-call",
            ];
            let any_execution = execution_headers
                .iter()
                .any(|name| headers.contains_key(*name));
            if any_execution && matches!(name, "ftps_device_create" | "ftps_device_rotate") {
                return rpc_error(id, -32000, "Issue FTPS device passwords from Integrations or a direct owner MCP client.");
            }
            let mut execution: Option<(String, String, String, i64, String, Option<String>, bool)> = None;
            if any_execution {
                let get = |name: &str| {
                    headers.get(name).and_then(|value| value.to_str().ok())
                };
                let (Some(project), Some(conversation), Some(executor), Some(generation), Some(call_id)) = (
                    get(execution_headers[0]), get(execution_headers[1]), get(execution_headers[2]),
                    get(execution_headers[3]), get(execution_headers[4]),
                ) else { return rpc_error(id, -32602, "Incomplete agent execution identity."); };
                let generation = match generation.parse::<i64>() {
                    Ok(value) => value,
                    Err(_) => return rpc_error(id, -32602, "Invalid agent execution generation."),
                };
                let observes = get("x-konae-execution-observes").map(str::to_owned);
                let admission = {
                    let _project_effects = database_lock(&state, &format!("conversation-effects:{project}")).await;
                    super::conversations::authorize_agent_mcp_call(
                        &state, &owner.id, project, conversation, executor, generation, call_id,
                        name, &arguments, observes.as_deref(),
                    )
                };
                let write = match admission {
                    Ok(value) => value,
                    Err(message) => return rpc_error(id, -32000, &message),
                };
                execution = Some((project.to_owned(), conversation.to_owned(), executor.to_owned(), generation, call_id.to_owned(), observes, write));
            }
            let package_transfer = (name.starts_with("transfer_") && name != "transfer_begin")
                .then(|| transfer(&state, &principal, &arguments).ok())
                .flatten();
            let export_transfer = package_transfer.clone().filter(|t| t["exportData"] == true);
            let component_upload = name == "transfer_write" && package_transfer.is_some_and(|t| t["component"].is_string());
            let mut result = if name.starts_with("transfer_") {
                let (project, live_transfer) = if name == "transfer_begin" {
                    (arguments["projectId"].as_str().map(str::to_owned), false)
                } else if name == "transfer_cancel" {
                    super::projects::release::transfer_project_for_owner(&state, &owner, &principal, arguments["transferId"].as_str().unwrap_or_default()).unwrap_or((None, false))
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
                    if let Err(error_value) = super::projects::release::cleanup_release_uploads(&state, &owner, &principal, plan_id).await {
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
            if let Some((project, conversation, _executor, _generation, call_id, observes, write)) = &execution {
                let message = result.as_ref().err().map(|error| format!("{error:#}"));
                let definitive_refusal = message.as_deref().is_some_and(|value| {
                    value.contains("Permission denied") || value.contains("Sources changed since revision")
                });
                if let Err(message) = super::conversations::finish_agent_mcp_call(
                    &state, &owner.id, project, conversation, call_id, *write, result.is_ok(),
                    definitive_refusal, observes.as_deref(), name,
                ) {
                    result = Err(anyhow::anyhow!(message));
                }
            }
            match export_transfer.as_ref().filter(|_| name == "transfer_read") {
                // One row per component download, written at its first
                // block or on a refusal, so a 256 MiB database read in
                // 64 blocks does not push the rest out of the log.
                Some(t) if arguments["offset"] == 0 || result.is_err() => {
                    let target = json!({"projectId":t["projectId"],"component":t["component"]});
                    audit(&state, &principal, "package_download", &target, &result);
                }
                Some(_) => {}
                // An upload of a part of an export is audited at its first block.
                None if component_upload && arguments["offset"] != 0 && result.is_ok() => {}
                None => audit(&state, &principal, name, &arguments, &result),
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
    result: &Result<Value>,
) {
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
                        let pending = super::sources::stage_write(&state.data_root, &id, &source).await?;
                        db.execute("UPDATE projects SET sources=1 WHERE id=?1", [&id])?;
                        pending.commit().await?;
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


            let pending = crate::copies::Pending::begin(&state.data_root, &id, "creating the project folder and catalogue entry")?;
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
                match tokio::fs::remove_dir_all(crate::layout::folder(&state.data_root, &id)).await {
                    Ok(()) => pending.finish()?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => pending.finish()?,
                    Err(e) => return Err(error_value.context(format!("Could not remove the incomplete project: {e}"))),
                }
                return Err(error_value);
            }
            pending.finish()?;
            Ok(project)
        }
        "project_inspect" => {
            let id = a["projectId"].as_str().unwrap();
            let (project, revision) = owned_project(state, owner, id, "read")?;
            let deployment = find_project_deployment(&*state.deployments.read().await, id);
            let sources_revision: u64 = store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| r.get(0))?;
            Ok(json!({"project":project,"revision":revision,"sourcesRevision":sources_revision,"deployment":deployment}))
        }
        "project_sources_read" => crate::projects::sources::read(state, owner, principal, &a).await,
        "project_sources_save" => crate::projects::sources::save(state, owner, principal, &a).await,
        "project_delete" => {
            let id = a["projectId"].as_str().unwrap();
            owner.require("write", Some(id))?;
            let _project_effects = database_lock(state, &format!("conversation-effects:{id}")).await;
            super::conversations::ensure_no_running_project_effect(&owner.id, id)
                .map_err(|message| anyhow::anyhow!(message))?;
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
            let ttl = if super::owners::clerk::ClerkConfig::from_env()?.is_some() { ttl.min(3600) } else { ttl };
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
            let db = if name == "secrets_list" || (name == "secret_delete" && !crate::layout::secrets(&state.data_root, id).try_exists()?) {
                super::http::read_secrets(&state.data_root, id)?
            } else {
                super::http::secrets_store(&state.data_root, id)?
            };
            super::http::secrets_call(&db, id, deployment.as_ref(), name, &a)
        }
        "api_keys_list" | "api_keys_create" | "api_keys_revoke" => {
            let id = a["projectId"].as_str().unwrap();
            let deployment = owned_deployment(state, owner, id, if name == "api_keys_list" { "read" } else { "publish" }).await?;
            super::http::api_keys_call(state, &deployment, name, &a).await
        }
        "ftps_credentials_list" | "ftps_device_create" | "ftps_device_rotate" | "ftps_credential_revoke" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let deployment = owned_deployment(state, owner, id, "manage_users").await?;
            kernel::ftps::owner_credentials_call(state, &deployment, name, &a).await
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
            let mut value = kernel::modules::read(&state, &deployment);
            value["modules"]["auth"]["current"] = sign_in_methods_value(state, &deployment);
            Ok(value)
        }
        "module_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let module = a["module"].as_str().unwrap_or_default();
            let registry: Value = serde_json::from_str(include_str!("../../../kernel/src/modules.json"))?;
            let scope = registry["modules"][module]["permission"].as_str().ok_or_else(|| anyhow::Error::new(kernel::refusal::refuse("module.unknown", format!("Unknown module {module}."))))?;
            let deployment = owned_deployment(state, owner, id, scope).await?;
            let _release_guard = kernel::named_lock(state, &format!("publish:{}", deployment.id), true).await;
            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            if module == "auth" {
                if let Some(methods) = a.get("options").and_then(|options| options.get("signInMethods")) {
                    requested_sign_in_methods(state, &deployment, methods)?;
                }
            }
            let result = kernel::modules::set(&state.data_root, &deployment.id, module, a["on"].as_bool(), a.get("options"), a.get("expected")).await?;
            let mut deployments = state.deployments.write().await;
            if let Some(current) = deployments.values_mut().find(|current| current.id == deployment.id) {
                current.modules.insert(module.to_owned(), result["state"].clone());
                if module == "auth" {
                    if let Some(policy) = result["state"].get("signupPolicy").filter(|value| !value.is_null()) { current.signup_policy = serde_json::from_value(policy.clone())?; }
                    if let Some(methods) = result["state"].get("signInMethods") { current.sign_in_methods = serde_json::from_value(methods.clone())?; }
                    persist_deployment(state, current).await?;
                }
            }
            Ok(result)
        }
        "contact_email_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let deployment = owned_deployment(state, owner, id, "publish").await?;

            let _guard = database_lock(state, &format!("project:{}", deployment.project_id)).await;
            let current = owned_deployment(state, owner, id, "publish").await?;
            super::refusal::check_setting_expected_read(a.get("expected"), json!(current.contact_email), owner.scopes.iter().any(|scope| scope == "read"))?;
            super::notify::set_contact_email(state, &current, a["email"].as_str()).await
        }
        "maintenance_set" => {
            let id = a["projectId"].as_str().unwrap_or_default();
            let found = owned_deployment(state, owner, id, "publish").await?;
            let _guard = database_lock(state, &format!("project:{}", found.project_id)).await;
            let deployment = owned_deployment(state, owner, id, "publish").await?;
            let _release = kernel::named_lock(state, &format!("publish:{}", deployment.id), true).await;
            let on = a["on"].as_bool().ok_or_else(|| anyhow::anyhow!("on must be true or false."))?;
            let current = super::maintenance::read(&state.data_root, &deployment.id)["maintenance"].clone();
            super::refusal::check_setting_expected(a.get("expected"), current)?;
            super::maintenance::set(&state.data_root, &deployment.id, on)
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
        "project_export" => crate::projects::release::export(state, owner, principal, &a).await,
        "function_candidate_check" => function_candidate::check(state, owner, principal, &a).await,
        "release_plan" => crate::projects::release::plan(state, owner, principal, &a).await,
        "release_apply" => crate::projects::release::apply(state, owner, principal, &a).await,
        "project_import" => crate::copies::Pending::through_catalogue(crate::projects::release::import(state, owner, principal, &a)).await?,
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
                super::refusal::check_setting_expected_read(a.get("expected"), current, owner.scopes.iter().any(|scope| scope == "read"))?;
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

pub(super) fn inviter_email(state: &Konae, owner_id: &str) -> Option<String> {
    let db = Connection::open_with_flags(state.data_root.join("owners.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row("SELECT email FROM app_users WHERE id = ?1", [owner_id], |row| row.get(0)).optional().ok().flatten()
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
    let all: Value = serde_json::from_str(include_str!("../project-templates.json"))?;
    let id = id.or(all["default"].as_str()).unwrap_or_default();
    let template = &all["templates"][id];
    anyhow::ensure!(template.is_object(), "Unknown template {id}.");
    Ok(template.clone())
}

async fn owner_config(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !super::owners::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
    match super::owners::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => Json(json!({"mode":"clerk","publishableKey":config.publishable_key})).into_response(),
        Ok(None) => {

            let editor = std::env::var("KONAE_EDITOR_DIR").is_ok_and(|dir| !dir.trim().is_empty());

            let mail = std::env::var("KONAE_MAIL_TRANSPORT").unwrap_or_default();


            let local = url::Url::parse(&state.public_origin).ok().and_then(|u| u.host_str().map(crate::is_local_host)).unwrap_or(false);
            let default_key = local && state.setup_key == crate::DEFAULT_SETUP_KEY;
            Json(json!({"mode":"local","admission":super::owners::admission::mode(&state).as_str(),"emailVerificationRequired":state.accounts.verification == auth::Verification::Required,"editor":editor,"mailToFile":mail == "development","defaultSetupKey":default_key.then_some(crate::DEFAULT_SETUP_KEY)})).into_response()
        }
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Owner identity is not configured."),
    }
}




pub(super) async fn owner_session(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !super::owners::oauth::valid_owner_host(&state, &headers).await { return StatusCode::FORBIDDEN.into_response(); }
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
    if !same_origin(&headers) || !super::owners::oauth::valid_owner_host(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Cross-origin sign-in rejected.");
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return error(StatusCode::FORBIDDEN, "Sign-in origin is required.");
    };
    let config = match super::owners::clerk::ClerkConfig::from_env() {
        Ok(Some(config)) => config,
        _ => return error(StatusCode::SERVICE_UNAVAILABLE, "Clerk sign-in is not configured."),
    };
    let client = state.guards.client_ip(request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|info| info.0), &headers);
    if let Err(wait) = state.guards.auth_attempt("owner", client, "clerk") { return too_many_attempts(wait); }
    let bytes = match axum::body::to_bytes(request.into_body(), 20_000).await { Ok(bytes) => bytes, Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response() };
    let body: Value = match serde_json::from_slice(&bytes) { Ok(body) => body, Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid sign-in request.") };
    let Some(token) = body["token"].as_str() else { return error(StatusCode::BAD_REQUEST, "Sign-in token required."); };
    let mut db = match Connection::open(state.data_root.join("owners.sqlite")) { Ok(db) => db, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    let session = match super::owners::clerk::exchange(&mut db, &config, token, origin, (now_ms() / 1000) as i64) {
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
    match super::owners::clerk::ClerkConfig::from_env() {
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
        || !super::owners::oauth::valid_owner_host(&state, &headers).await
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
        match super::owners::admission::read(&state, &a) {
            Ok(admission) => Some(admission),
            Err(e) => return auth_error(e),
        }
    } else {
        None
    };
    let email = a["email"].as_str().unwrap_or("").to_owned();
    let password = a["password"].as_str().unwrap_or("").to_owned();
    if let Some(admission) = admission.clone().filter(|_| state.accounts.verification == auth::Verification::Required) {
        let page = super::owners::admission::login_page(
            &origin(&state, &headers),
            super::owners::admission::return_to(&a).as_deref(),
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
                let admit = super::owners::admission::admit(admission);
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
        || !super::owners::oauth::valid_owner_host(&state, &headers).await
    {
        return error(StatusCode::FORBIDDEN, "Cross-origin logout rejected.");
    }
    let cookie_name = owner_cookie_name(&state);
    if super::owners::clerk::ClerkConfig::from_env().ok().flatten().is_some() {
        if let Ok(db) = Connection::open(state.data_root.join("owners.sqlite")) {
            let _ = super::owners::clerk::revoke(&db, cookie_value(&headers, cookie_name).as_deref());
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
    if !super::owners::oauth::valid_owner_host(&state, &headers).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        super::owners::owner_page::LOGIN.as_str(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revision_conflicts_match_the_shared_case() {
        let fixture: Value = serde_json::from_str(include_str!("../../../kernel/src/source-revision-cases.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let refusal = source_revision_refusal(case["observed"].as_u64().unwrap());
            let refusal = refusal.downcast_ref::<super::super::refusal::Refusal>().unwrap();
            assert_eq!(refusal.code, case["code"].as_str().unwrap());
            assert_eq!(refusal.message, case["error"].as_str().unwrap());
        }
    }

    #[tokio::test]
    async fn restored_catalog_requires_the_app_and_its_local_or_hosted_owner() {
        let root = std::env::temp_dir().join(format!("konae-catalog-{}", Uuid::new_v4()));
        crate::Settings::new("notes", "Notes", "notes", &json!({})).write(&root).await.unwrap();
        let db = store_at(&root).unwrap();
        check_restored_catalog(&root).unwrap();
        assert!(!root.join("owners.sqlite").exists());
        db.execute("INSERT INTO projects(id,owner,body) VALUES('notes','fixture','{}')", []).unwrap();
        let owners = Connection::open(root.join("owners.sqlite")).unwrap();
        super::super::owners::clerk::install(&owners).unwrap();
        assert!(check_restored_catalog(&root).unwrap_err().to_string().contains("no matching owner"));
        owners.execute("INSERT INTO creator_identities(owner,provider,subject) VALUES('fixture','lab','person')", []).unwrap();
        check_restored_catalog(&root).unwrap();
        let suspended = crate::recovery::Entry::new("clerk-owner", None, "fixture", "suspended");
        assert_eq!(crate::recovery::reconcile(&root, &installation(), &[suspended]).unwrap(), 1);
        assert!(owners.query_row("SELECT suspended FROM creator_identities", [], |row| row.get::<_, bool>(0)).unwrap());
        db.execute("UPDATE projects SET id='missing'", []).unwrap();
        assert!(check_restored_catalog(&root).unwrap_err().to_string().contains("no matching app folder"));
        drop((db, owners));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn saved_sources_from_an_older_project_revision_remain_readable() {
        let root = std::env::temp_dir().join(format!("konae-old-sources-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        Connection::open(root.join("mcp.sqlite")).unwrap().execute_batch(
            "CREATE TABLE projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0);
             INSERT INTO projects(id,owner,body) VALUES('app_existing','o','{}');",
        ).unwrap();
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
        assert_eq!(store(&state).unwrap().query_row("SELECT sources FROM projects WHERE id='app_existing'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
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
        kernel::write_record_for_test(&root, &deployment);
        let dist = crate::layout::dist(&root, id);
        tokio::fs::create_dir_all(&dist).await.unwrap();
        tokio::fs::write(dist.join("index.html"), "published artifact").await.unwrap();
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
        let result = crate::copies::Pending::through_catalogue(restore_import_after_release_failure(
            &state, &deployment, true, &backup, anyhow::anyhow!("Injected release failure"),
        )).await.unwrap();
        let error = result.unwrap_err();
        assert_eq!(std::fs::read_dir(root.join(".copy-operations")).unwrap().count(), 0);
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
    async fn setting_changed_redacts_current_value_without_read_permission() {
        let root = std::env::temp_dir().join(format!("konae-setting-redaction-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let body = json!({"project":{"id":"app_redact","name":"Redact","slug":"redact","manifest":{},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},"visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<title>Redact</title>"}}});
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        store(&state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES('app_redact','o','{}')", []).unwrap();
        let publish = Owner { id:"o".into(), display:None, scopes:vec!["publish".into()], projects:vec![], session:false };
        let publish_and_read = Owner { scopes:vec!["publish".into(),"read".into()], ..publish.clone() };
        for (owner, contains_value) in [(&publish, false), (&publish_and_read, true)] {
            let error = admin_call(&state, owner, "owner:o", "contact_email_set", json!({"projectId":"app_redact","email":"new@example.test","expected":"stale@example.test"})).await.unwrap_err();
            let refusal = error.downcast_ref::<crate::refusal::Refusal>().unwrap();
            assert_eq!(refusal.code, "setting.changed");
            assert_eq!(refusal.message.contains("current value is null"), contains_value);
        }
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
        let publisher = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"publish".into(),"manage_users".into()], projects:vec![], session:false };
        let reader = Owner { scopes:vec!["read".into()], ..publisher.clone() };
        let before = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"auth","on":true})).await.unwrap();
        let auth_on = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        let file_state = auth_on["modules"]["files"]["state"].clone();
        admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"files","on":true,"options":{"fileAccess":"app-users"},"expected":file_state})).await.unwrap();
        let files_after = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        assert_eq!(files_after["modules"]["files"]["state"]["on"], true);
        assert_eq!(files_after["modules"]["files"]["state"]["fileAccess"], "app-users");
        assert_eq!(state.deployments.read().await.values().find(|item| item.id == "app_modules").unwrap().modules["files"]["fileAccess"], "app-users");
        let expected = before["modules"]["seo"]["state"].clone();
        admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":true,"options":{"canonical":"path"},"expected":expected})).await.unwrap();
        let after = admin_call(&state, &publisher, "owner:o", "modules_read", json!({"projectId":"app_modules"})).await.unwrap();
        assert_eq!(after["modules"]["seo"]["state"]["on"], true);
        assert_eq!(after["modules"]["seo"]["state"]["canonical"], "path");
        assert_eq!(state.deployments.read().await.values().find(|item| item.id == "app_modules").unwrap().modules["seo"]["canonical"], "path");
        assert_eq!(after["modules"]["actions"]["state"]["on"], false);
        assert_eq!(after["modules"]["actions"]["switch"], true);
        let stale = admin_call(&state, &publisher, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":false,"expected":expected})).await.unwrap_err();
        let refusal = stale.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.changed");
        let denied = admin_call(&state, &reader, "owner:o", "module_set", json!({"projectId":"app_modules","module":"seo","on":false})).await.unwrap_err().to_string();
        assert_eq!(denied, "Permission denied for publish.");

        let project = json!({"projectId":"app_modules"});
        let guide = admin_call(&state, &publisher, "owner:o", "platform_guide", json!({"topic":"pieces"})).await.unwrap();
        let setup = &guide["moduleSetup"];
        assert!(setup["ftps"]["operator"].as_str().unwrap().contains("ftps.json"));
        assert!(setup["ftps"]["declare"].as_str().unwrap().contains("kind device"));
        assert!(setup["ftps"]["credentials"].as_str().unwrap().contains("ftps_device_rotate"));
        assert!(setup["ftps"]["portable"].as_str().unwrap().contains("konae pull and export"));
        for module in ["documents", "mcp", "file-intake"] {
            let expected = after["modules"][module]["state"].clone();
            let args = json!({"projectId":"app_modules", "module":module, "on":true, "expected":expected});
            let error = admin_call(&state, &publisher, "owner:o", "module_set", args).await.unwrap_err();
            assert_eq!(error.downcast_ref::<crate::refusal::Refusal>().unwrap().code, "module.requires");
        }
        let mut current = after;
        for module in ["actions", "documents", "mcp", "file-intake"] {
            let expected = current["modules"][module]["state"].clone();
            let args = json!({"projectId":"app_modules", "module":module, "on":true, "expected":expected});
            admin_call(&state, &publisher, "owner:o", "module_set", args).await.unwrap();
            current = admin_call(&state, &publisher, "owner:o", "modules_read", project.clone()).await.unwrap();
            assert_eq!(current["modules"][module]["state"]["on"], true, "{module}");
            if module != "actions" {
                assert!(setup[module]["declare"].is_string(), "{module}");
                assert!(setup[module]["verify"].is_string(), "{module}");
            }
        }
        assert!(current["modules"].get("ftps").is_none());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn file_api_key_creation_requires_files_but_not_http() {
        let root = std::env::temp_dir().join(format!("konae-api-key-http-module-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"publish".into(),"manage_users".into()], projects:vec![], session:false };
        let body = json!({"project":{"id":"api_key_http","name":"API key HTTP","slug":"api-key-http","manifest":{"version":1,"visibility":"public"},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},"visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<title>Keys</title>"}}});
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        store(&state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES('api_key_http','o','{}')", []).unwrap();
        let before = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"api_key_http"})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":"api_key_http","module":"auth","on":true,"expected":before["modules"]["auth"]["state"]})).await.unwrap();
        let before = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"api_key_http"})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":"api_key_http","module":"files","on":true,"expected":before["modules"]["files"]["state"]})).await.unwrap();
        let current = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"api_key_http"})).await.unwrap();
        assert_eq!(current["modules"]["http"]["state"]["on"], false);
        let created = admin_call(&state, &owner, "owner:o", "api_keys_create", json!({"projectId":"api_key_http","name":"Files key","actions":[],"files":["public"]})).await.unwrap();
        let current = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"api_key_http"})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":"api_key_http","module":"files","on":false,"expected":current["modules"]["files"]["state"]})).await.unwrap();
        let refused = admin_call(&state, &owner, "owner:o", "api_keys_create", json!({"projectId":"api_key_http","name":"Blocked","files":["public"]})).await.unwrap_err();
        assert!(refused.to_string().contains("Module files is off."));
        assert_eq!(admin_call(&state, &owner, "owner:o", "api_keys_list", json!({"projectId":"api_key_http"})).await.unwrap()["keys"].as_array().unwrap().len(), 1);
        admin_call(&state, &owner, "owner:o", "api_keys_revoke", json!({"projectId":"api_key_http","keyId":created["keyId"]})).await.unwrap();
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn actions_cannot_be_turned_off_while_an_action_feature_is_declared() {
        let root = std::env::temp_dir().join(format!("konae-actions-in-use-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"publish".into()], projects:vec![], session:false };
        let body = json!({"project":{"id":"app_actions_in_use","name":"Actions in use","slug":"actions-in-use","manifest":{},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},"visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<title>Actions</title>"}}});
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        store(&state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES('app_actions_in_use','o','{}')", []).unwrap();
        let cases = [
            ("route", json!({"kind":"query","access":"anonymous","http":{"method":"GET","path":"/route"}})),
            ("call", json!({"kind":"call","method":"POST","url":"https://example.test/hook"})),
            ("schedule", json!({"kind":"mutation","access":"anonymous","schedule":{"every":"5m"}})),
            ("notice", json!({"kind":"mutation","access":"anonymous","notify":{"to":"owner","subject":"Changed","body":"Changed"}})),
            ("page path", json!({"kind":"query","access":"anonymous","page":{"path":"/notes/:slug","file":"note.html"}})),
            ("authenticated access", json!({"kind":"query","access":"authenticated"})),
        ];
        for (index, (label, action)) in cases.into_iter().enumerate() {
            let name = format!("feature_{index}");
            let actions = json!({"version":1,"actions":{name:action}});
            tokio::fs::write(crate::layout::sources(&state.data_root, "app_actions_in_use").join("actions.json"), serde_json::to_vec(&actions).unwrap()).await.unwrap();
            let record = kernel::layout::record(&state.data_root, "app_actions_in_use");
            let mut installed: Value = serde_json::from_slice(&tokio::fs::read(&record).await.unwrap()).unwrap();
            installed["actions"] = json!(serde_json::to_string(&actions).unwrap());
            tokio::fs::write(&record, serde_json::to_vec(&installed).unwrap()).await.unwrap();
            let before = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"app_actions_in_use"})).await.unwrap();
            if before["modules"]["actions"]["state"]["on"] != true {
                admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":"app_actions_in_use","module":"actions","on":true,"expected":before["modules"]["actions"]["state"]})).await.unwrap();
            }
            let current = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":"app_actions_in_use"})).await.unwrap();
            let error = admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":"app_actions_in_use","module":"actions","on":false,"expected":current["modules"]["actions"]["state"]})).await.unwrap_err();
            let refusal = error.downcast_ref::<kernel::refusal::Refusal>().unwrap();
            assert_eq!(refusal.code, "module.in-use", "{label}");
            assert!(refusal.message.contains("Module actions is in use by actions.json"), "{label}: {}", refusal.message);
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn first_release_persists_inferred_files_state_for_missing_and_partial_initial_modules() {
        use base64::Engine;
        use sha2::Digest;
        let root = std::env::temp_dir().join(format!("konae-first-files-release-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"write".into(),"publish".into()], projects:vec![], session:false };
        for (id, initial_modules) in [("app_first_files_none", None), ("app_first_files_partial", Some(json!({"seo":{"on":false}})))] {
            admin_call(&state, &owner, "owner:o", "project_create", json!({"projectId":id,"name":"Files first release","slug":id,"manifest":{"version":1,"visibility":"public"}})).await.unwrap();
            let mut payload = json!({
                "project":{"id":id,"name":"Files first release","slug":id,"manifest":{"version":1,"visibility":"public"},"files":{"actions.json":"{\"version\":1,\"actions\":{\"notes.list\":{\"kind\":\"query\",\"access\":\"anonymous\",\"statement\":\"SELECT 1 AS n LIMIT 1\",\"bindings\":[]}}}","schema.sql":""}},
                "visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<script>miniapp.files.upload()</script>"}}
            });
            if let Some(modules) = initial_modules { payload["initialModules"] = modules; }
            let bytes = serde_json::to_vec(&payload).unwrap();
            let digest = hex::encode(sha2::Sha256::digest(&bytes));
            let transfer = transfer_call(&state.kernel, "owner:o", "transfer_begin", &json!({"projectId":id,"size":bytes.len(),"sha256":digest}), false).await.unwrap();
            let transfer_id = transfer["transferId"].as_str().unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_write", &json!({"transferId":transfer_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_complete", &json!({"transferId":transfer_id}), false).await.unwrap();
            let plan = admin_call(&state, &owner, "owner:o", "release_plan", json!({"transferId":transfer_id,"observedSourcesRevision":0})).await.unwrap();
            admin_call(&state, &owner, "owner:o", "release_apply", json!({"planId":plan["planId"]})).await.unwrap();
            let deployment = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
            assert_eq!(deployment.modules["files"]["on"], true, "{id}");
            assert_eq!(deployment.modules["actions"]["on"], true, "{id}");
            let settings = crate::Settings::read(&state.data_root, id).unwrap();
            assert_eq!(settings.modules["files"]["on"], true, "{id}");
            assert_eq!(settings.modules["actions"]["on"], true, "{id}");
            let reloaded = kernel::load_deployments(&state.data_root, "http://localhost").await.unwrap();
            assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules["files"]["on"], true, "{id}");
            assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules["actions"]["on"], true, "{id}");
            let read = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":id})).await.unwrap();
            let expected = read["modules"]["seo"]["state"].clone();
            admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":id,"module":"seo","on":true,"expected":expected})).await.unwrap();
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn first_release_persists_inferred_module_state_in_memory_and_on_disk() {
        use base64::Engine;
        use sha2::Digest;
        let root = std::env::temp_dir().join(format!("konae-first-actions-release-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"write".into(),"publish".into()], projects:vec![], session:false };
        for (id, actions, schema, expected_actions, expected_http, expected_calls) in [
            ("app_first_actions_on", r#"{"version":1,"actions":{"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT 1 AS n LIMIT 1","bindings":[]}}}"#, "", true, false, false),
            ("app_first_actions_off", r#"{"version":1,"actions":{}}"#, "", false, false, false),
            ("app_first_http_on", r#"{"version":1,"actions":{"hooks.receive":{"kind":"mutation","access":"anonymous","statement":"INSERT INTO events (name) VALUES (?1)","bindings":[{"source":"input","key":"name","type":"string"}],"http":{"method":"POST","path":"/hooks/events"}}}}"#, "CREATE TABLE IF NOT EXISTS events(name TEXT);", true, true, false),
            ("app_first_calls_on", r#"{"version":1,"actions":{"notice.send":{"kind":"call","access":"anonymous","bindings":[],"method":"POST","url":"https://example.test/hooks/events"}}}"#, "", true, false, true),
        ] {
            admin_call(&state, &owner, "owner:o", "project_create", json!({"projectId":id,"name":"First actions release","slug":id,"manifest":{"version":1,"visibility":"public"}})).await.unwrap();
            let payload = json!({
                "project":{"id":id,"name":"First actions release","slug":id,"manifest":{"version":1,"visibility":"public"},"files":{"actions.json":actions,"schema.sql":schema}},
                "visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<!doctype html><title>Static</title>"}}
            });
            let bytes = serde_json::to_vec(&payload).unwrap();
            let digest = hex::encode(sha2::Sha256::digest(&bytes));
            let transfer = transfer_call(&state.kernel, "owner:o", "transfer_begin", &json!({"projectId":id,"size":bytes.len(),"sha256":digest}), false).await.unwrap();
            let transfer_id = transfer["transferId"].as_str().unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_write", &json!({"transferId":transfer_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_complete", &json!({"transferId":transfer_id}), false).await.unwrap();
            let plan = admin_call(&state, &owner, "owner:o", "release_plan", json!({"transferId":transfer_id,"observedSourcesRevision":0})).await.unwrap();
            admin_call(&state, &owner, "owner:o", "release_apply", json!({"planId":plan["planId"]})).await.unwrap();
            assert_eq!(state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().modules["actions"]["on"], expected_actions, "memory actions {id}");
            assert_eq!(crate::Settings::read(&state.data_root, id).unwrap().modules["actions"]["on"], expected_actions, "disk actions {id}");
            let reloaded = kernel::load_deployments(&state.data_root, "http://localhost").await.unwrap();
            assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules["actions"]["on"], expected_actions, "reload actions {id}");
            for (module, expected) in [("http", expected_http), ("calls", expected_calls)] {
                assert_eq!(state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().modules[module]["on"], expected, "memory {module} {id}");
                assert_eq!(crate::Settings::read(&state.data_root, id).unwrap().modules[module]["on"], expected, "disk {module} {id}");
                assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules[module]["on"], expected, "reload {module} {id}");
            }
            let read = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":id})).await.unwrap();
            let expected = read["modules"]["actions"]["state"].clone();
            admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":id,"module":"actions","on":expected_actions,"expected":expected})).await.unwrap();
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn project_create_normalizes_legacy_file_access_on_both_metadata_inputs() {
        use base64::Engine;
        use sha2::Digest;
        let root = std::env::temp_dir().join(format!("konae-create-file-access-{}", Uuid::new_v4()));
        let (kernel_state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(kernel_state);
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"write".into()], projects:vec![], session:false };
        let source = json!({"id":"app_legacy_access","name":"Legacy access","slug":"legacy-access","files":{"index.html":"<main>Legacy</main>"},"directories":[],"manifest":{"version":1,"visibility":"public","fileAccess":"app-users"},"updatedAt":1});
        let bytes = serde_json::to_vec(&source).unwrap();
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        let transfer = transfer_call(&state.kernel, "owner:o", "transfer_begin", &json!({"projectId":null,"size":bytes.len(),"sha256":digest}), false).await.unwrap();
        let transfer_id = transfer["transferId"].as_str().unwrap();
        transfer_call(&state.kernel, "owner:o", "transfer_write", &json!({"transferId":transfer_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
        transfer_call(&state.kernel, "owner:o", "transfer_complete", &json!({"transferId":transfer_id}), false).await.unwrap();
        admin_call(&state, &owner, "owner:o", "project_create", json!({"projectId":"app_legacy_access","name":"Legacy access","slug":"legacy-access","manifest":{"version":1,"visibility":"public","fileAccess":"app-users"},"sourcesTransferId":transfer_id})).await.unwrap();
        let saved = super::super::sources::read(&state.data_root, "app_legacy_access").await.unwrap().unwrap();
        assert!(saved["manifest"].get("fileAccess").is_none());
        assert_eq!(saved["files"]["index.html"], "<main>Legacy</main>");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn functions_project_import_keeps_draft_and_restores_compiled_source_backups() {
        use base64::Engine;
        use sha2::Digest;
        async fn upload(state: &Konae, payload: &Value, id: &str) -> String {
            let bytes = serde_json::to_vec(payload).unwrap();
            let created = transfer_call(&state.kernel, "owner:o", "transfer_begin", &json!({"projectId":id,"size":bytes.len(),"sha256":hex::encode(sha2::Sha256::digest(&bytes))}), false).await.unwrap();
            let transfer = created["transferId"].as_str().unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_write", &json!({"transferId":transfer,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}), false).await.unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_complete", &json!({"transferId":transfer}), false).await.unwrap(); transfer.into()
        }
        let root = std::env::temp_dir().join(format!("konae-functions-import-door-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(),display:None,scopes:vec!["read".into(),"write".into(),"publish".into()],projects:vec![],session:false };
        let id = "function_import";
        admin_call(&state, &owner, "owner:o", "project_create", json!({"projectId":id,"name":"Functions","slug":id,"manifest":{"version":1,"visibility":"public"}})).await.unwrap();
        let manifest = json!({"version":1,"functions":{"hello":{"entry":"functions/hello.ts","mount":"/api/hello","methods":["GET"],"access":"anonymous","actions":[]}}});
        let mut payload = json!({"project":{"id":id,"name":"Functions","slug":id,"manifest":{"version":1,"visibility":"public"},"files":{"functions.json":manifest.to_string(),"functions/hello.ts":"export function handle() { return new Response('A'); }"}},"visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<p>A</p>"}},"serverArtifact":{"version":1,"abi":"konae-functions/1","manifest":manifest,"bundles":{"hello":"export function handle() { return new Response('A'); }"}},"initialModules":{"functions":{"on":true},"auth":{"on":true}}});
        let transfer = upload(&state, &payload, id).await;
        let plan = admin_call(&state, &owner, "owner:o", "release_plan", json!({"transferId":transfer,"observedSourcesRevision":0})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "release_apply", json!({"planId":plan["planId"]})).await.unwrap();
        let a = Settings::read(&root, id).unwrap().released.unwrap();
        let auth = kernel::auth::AuthService::open(kernel::layout::identity(&root, id)).unwrap();
        let session = auth.signup("keep@example.test", "correct horse battery").unwrap();
        let mut imported_payload = payload.clone();
        imported_payload.as_object_mut().unwrap().remove("initialModules");
        imported_payload["initialReleased"] = serde_json::to_value(&a).unwrap();
        imported_payload["project"]["files"] = json!({"functions.json":"invalid saved draft"});
        let normal = kernel::deployments::validate_release(&state.kernel, &kernel::functions::parse_deploy_input(imported_payload.clone()).unwrap(), None).await;
        assert!(normal.err().unwrap().to_string().contains("project_import"));
        payload.as_object_mut().unwrap().remove("initialModules");
        payload["artifact"]["files"]["index.html"] = json!("<p>B</p>");
        payload["serverArtifact"]["bundles"]["hello"] = json!("export function handle() { return new Response('B'); }");
        assert_eq!(kernel::deploy(&state.kernel, payload.to_string().as_bytes()).await.status(), StatusCode::OK);
        let b = Settings::read(&root, id).unwrap().released.unwrap();
        let mut mismatched = imported_payload.clone(); mismatched["artifact"]["files"]["index.html"] = json!("wrong artifact");
        let transfer = upload(&state, &mismatched, id).await;
        assert!(admin_call(&state, &owner, "owner:o", "project_import", json!({"projectId":id,"replace":"sources","transferId":transfer})).await.is_err());
        let transfer = upload(&state, &imported_payload, id).await;
        let result = admin_call(&state, &owner, "owner:o", "project_import", json!({"projectId":id,"replace":"sources","transferId":transfer})).await.unwrap();
        assert_eq!(Settings::read(&root,id).unwrap().released.unwrap().server, a.server);
        assert_eq!(std::fs::read_to_string(kernel::layout::sources(&root,id).join("functions.json")).unwrap(), "invalid saved draft");
        assert!(!kernel::layout::sources(&root,id).join("functions/hello.ts").exists());
        assert!(auth.authenticate(Some(&session.token)).unwrap().is_some());
        let restored = admin_call(&state,&owner,"owner:o","database_execute",json!({"projectId":id,"operation":"restore-backup","input":{"backupId":result["backup"]["id"]}})).await.unwrap();
        assert!(restored["holds"].as_array().unwrap().iter().any(|value| value == "sources"));
        assert_eq!(Settings::read(&root,id).unwrap().released.unwrap().server, b.server);
        assert_eq!(std::fs::read_to_string(kernel::layout::dist(&root,id).join("index.html")).unwrap(), "<p>B</p>");
        assert!(kernel::functions::read_artifact(&kernel::layout::server(&root,id)).unwrap().unwrap().bundles["hello"].contains("'B'"));
        assert!(auth.authenticate(Some(&session.token)).unwrap().is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_file_access_survives_release_apply_and_project_import_for_other_users() {
        async fn upload_release(state: &Konae, payload: &Value, project: &str) -> String {
            let bytes = serde_json::to_vec(payload).unwrap();
            let digest = hex::encode(sha2::Sha256::digest(&bytes));
            let transfer = transfer_call(&state.kernel, "owner:o", "transfer_begin", &json!({"projectId":project,"size":bytes.len(),"sha256":digest}), false).await.unwrap();
            let transfer_id = transfer["transferId"].as_str().unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_write", &json!({"transferId":transfer_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
            transfer_call(&state.kernel, "owner:o", "transfer_complete", &json!({"transferId":transfer_id}), false).await.unwrap();
            transfer_id.to_owned()
        }
        use axum::{http::{Request, header}, Router};
        use base64::Engine;
        use sha2::Digest;
        use tower::ServiceExt;
        let root = std::env::temp_dir().join(format!("konae-file-access-doors-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = Konae::for_test(AppState::for_test(&root));
        let owner = Owner { id:"o".into(), display:None, scopes:vec!["read".into(),"write".into(),"publish".into()], projects:vec![], session:false };
        let id = "app_file_access_doors";
        admin_call(&state, &owner, "owner:o", "project_create", json!({"projectId":id,"name":"File access","slug":"file-access","manifest":{"version":1,"visibility":"public"}})).await.unwrap();
        let payload = json!({"project":{"id":id,"name":"File access","slug":"file-access","manifest":{"version":1,"visibility":"public","fileAccess":"app-users"},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},"visibility":"public","artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<title>File access</title>"}}});
        let transfer_id = upload_release(&state, &payload, id).await;
        let plan = admin_call(&state, &owner, "owner:o", "release_plan", json!({"transferId":transfer_id,"observedSourcesRevision":0})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "release_apply", json!({"planId":plan["planId"]})).await.unwrap();
        let deployment = state.kernel.deployments.read().await.values().find(|item| item.id == id).unwrap().clone();
        assert_eq!(deployment.modules["files"]["fileAccess"], "app-users");

        let auth = kernel::auth::AuthService::open(kernel::layout::identity(&state.data_root, id)).unwrap();
        let uploader = auth.signup("uploader@example.test", "correct horse battery").unwrap();
        let other = auth.signup("other@example.test", "correct horse battery").unwrap();
        let file_id = "018f2b2c-4a10-7a01-8000-000000000001";
        let file_dir = kernel::layout::files(&state.data_root, id);
        tokio::fs::create_dir_all(&file_dir).await.unwrap();
        tokio::fs::write(file_dir.join(format!("{file_id}.blob")), b"door proof\n").await.unwrap();
        let file = json!([{"id":file_id,"name":"proof.txt","type":"text/plain","size":11,"visibility":"private","ownerId":uploader.user.user_id,"createdAt":1}]);
        tokio::fs::write(file_dir.join("index.json"), serde_json::to_vec(&file).unwrap()).await.unwrap();
        let read_as = |token: &str| {
            let request = Request::builder().uri(format!("/_konae/files/{file_id}")).header(header::HOST, deployment.hostname.as_str()).header(header::COOKIE, format!("konae_app_session={token}")).body(axum::body::Body::empty()).unwrap();
            kernel::stack(state.kernel.clone(), Router::new()).oneshot(request)
        };
        let first_read = read_as(&other.token).await.unwrap();
        assert_eq!(first_read.status(), StatusCode::OK);
        assert_eq!(axum::body::to_bytes(first_read.into_body(), 1024).await.unwrap().as_ref(), b"door proof\n");

        let current = admin_call(&state, &owner, "owner:o", "modules_read", json!({"projectId":id})).await.unwrap();
        admin_call(&state, &owner, "owner:o", "module_set", json!({"projectId":id,"module":"files","options":{"fileAccess":null},"expected":current["modules"]["files"]["state"]})).await.unwrap();
        assert_ne!(read_as(&other.token).await.unwrap().status(), StatusCode::OK);
        let import_transfer = upload_release(&state, &payload, id).await;
        admin_call(&state, &owner, "owner:o", "project_import", json!({"projectId":id,"replace":"sources","transferId":import_transfer})).await.unwrap();
        let final_deployment = state.kernel.deployments.read().await.values().find(|item| item.id == id).unwrap().clone();
        assert_eq!(final_deployment.modules["files"]["fileAccess"], "app-users");
        let final_read = read_as(&other.token).await.unwrap();
        assert_eq!(final_read.status(), StatusCode::OK);
        assert_eq!(axum::body::to_bytes(final_read.into_body(), 1024).await.unwrap().as_ref(), b"door proof\n");
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
        audit(&state, "owner:ana", "project_create", &json!({"projectId": "p1"}), &ok);
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

    #[tokio::test]
    async fn ftps_credential_audit_rows_do_not_contain_passwords() {
        let root = std::env::temp_dir().join(format!("konae-ftps-audit-{}", Uuid::new_v4()));
        let (state, _) = kernel::tests::login_test_state(&root, "public").await;
        let state = Konae::for_test(state);
        let issued = Ok(json!({"credentialId":"credential-1","password":"one-time-secret"}));
        audit(
            &state, "owner:ana", "ftps_device_create",
            &json!({"projectId":"p1","destination":"meters","label":"Meter"}), &issued,
        );
        let detail: String = store(&state).unwrap().query_row(
            "SELECT COALESCE(target,'') || COALESCE(detail,'') FROM audit WHERE tool='ftps_device_create'",
            [], |row| row.get(0),
        ).unwrap();
        assert!(!detail.contains("one-time-secret"));
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
        for ftps_tool in ["ftps_credentials_list", "ftps_device_create", "ftps_device_rotate", "ftps_credential_revoke"] {
            assert!(!names(&reader).contains(&ftps_tool.to_owned()));
        }
        assert!(names(&reader).contains(&"usage_summary".to_owned()));
        assert!(!names(&reader).contains(&"usage_read".to_owned()));
        for definition in admin_tools(&reader) {
            assert!(definition["inputSchema"]["properties"].get("deploymentId").is_none(), "owner tool {} uses deploymentId", definition["name"]);
        }
        for removed in ["signup_policy_set", "sign_in_methods_set", "sign_in_methods_read"] { assert!(!names(&reader).contains(&removed.to_owned())); }
        let publisher = Owner { scopes: vec!["read".into(), "publish".into()], ..reader.clone() };
        assert!(names(&publisher).contains(&"module_set".to_owned()));
        assert!(!names(&publisher).iter().any(|name| name.starts_with("app_")));
        let manager = Owner { scopes: vec!["read".into(), "manage_users".into()], ..reader };
        assert!(names(&manager).contains(&"module_set".to_owned()));
        for ftps_tool in ["ftps_credentials_list", "ftps_device_create", "ftps_device_rotate", "ftps_credential_revoke"] {
            assert!(names(&manager).contains(&ftps_tool.to_owned()), "{ftps_tool}");
        }
        let definitions = admin_tools(&manager);
        let list = definitions.iter().find(|tool| tool["name"] == "ftps_credentials_list").unwrap();
        assert_eq!(list["annotations"]["readOnlyHint"], true);
        for tool_name in ["ftps_device_create", "ftps_device_rotate"] {
            let definition = definitions.iter().find(|tool| tool["name"] == tool_name).unwrap();
            assert_eq!(definition["annotations"]["readOnlyHint"], false);
            assert!(definition["description"].as_str().unwrap().contains("appears once"));
        }
        let registry: Value = serde_json::from_str(include_str!("../../../kernel/src/modules.json")).unwrap();
        assert_eq!(registry["modules"]["auth"]["permission"], "manage_users");
        for removed in ["signup_policy_set", "sign_in_methods_set", "sign_in_methods_read"] { assert!(!names(&manager).contains(&removed.to_owned())); }
        for tool in ["app_users_list", "app_user_suspend", "app_user_resume", "app_invitations_create", "app_invitations_list", "app_invitation_resend", "app_invitation_revoke"] {
            assert!(names(&manager).contains(&tool.to_owned()), "{tool}");
        }
    }

    #[test]
    fn the_google_client_cases_refuse_with_their_sentence_and_code() {
        let file: Value = serde_json::from_str(include_str!("../../../kernel/src/google-client-cases.json")).unwrap();
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
                "module_set" => requested_sign_in_methods(&state, &deployment, &case["input"]["methods"]).map(|_| json!(null)),
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
    fn auth_sign_in_methods_keep_password_and_refuse_google_without_the_apps_client() {
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
        let mut old_file_option = base.clone();
        old_file_option["manifest"]["fileAccess"] = json!("public");
        assert!(validate_sources(&old_file_option, "app_a").is_ok());
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
        assert!(source["manifest"].get("fileAccess").is_none());
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
        let rendered: Value = serde_json::from_str(include_str!("../sdk-guide.json")).unwrap();
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
