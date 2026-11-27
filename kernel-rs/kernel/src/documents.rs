//! Durable document issues and the local Typst adapter.

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub const MAX_RENDER_INPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PDF_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_RENDER_LOG_BYTES: usize = 64 * 1024;
pub const RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEFAULT_TYPST_VERSION: &str = "0.14.2";
const MAX_PENDING_PER_APP: u64 = 1_000;
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS __konae_documents (issue_key TEXT PRIMARY KEY, release TEXT NOT NULL, read_rule_json TEXT NOT NULL, snapshot_json TEXT NOT NULL, template TEXT NOT NULL, resources_json TEXT NOT NULL, typst_version TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('pending','rendering','ready','failed','paused')), attempt INTEGER NOT NULL DEFAULT 0, attempt_token TEXT, pdf_blob TEXT, pdf_size INTEGER, error TEXT, created_at INTEGER NOT NULL) WITHOUT ROWID; CREATE INDEX IF NOT EXISTS __konae_documents_pending ON __konae_documents(state,created_at,issue_key);";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderInput {
    pub snapshot: Value,
    pub template: String,
    pub resources: std::collections::BTreeMap<String, String>,
    pub typst_version: String,
}

/// Inserts one immutable issue. Replaying a key succeeds only when every input matches.
pub struct IssueEffect {
    pub key: String,
    pub release: String,
    pub read_rule_json: String,
    pub input: RenderInput,
    pub created_at: u64,
}

impl crate::execution::TransactionEffect for IssueEffect {
    fn prepare(&self, db: &Connection) -> rusqlite::Result<()> {
        ensure_schema(db)?;
        let encode = |error| rusqlite::Error::ToSqlConversionFailure(Box::new(error));
        let snapshot = serde_json::to_string(&self.input.snapshot).map_err(encode)?;
        let resources = serde_json::to_string(&self.input.resources).map_err(encode)?;
        let existing: Option<(String,String,String,String,String,String)> = db.query_row(
            "SELECT release,read_rule_json,snapshot_json,template,resources_json,typst_version FROM __konae_documents WHERE issue_key=?1",
            [&self.key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
        if let Some(existing) = existing {
            let immutable = (
                self.release.clone(),
                self.read_rule_json.clone(),
                snapshot.clone(),
                self.input.template.clone(),
                resources.clone(),
                self.input.typst_version.clone(),
            );
            return if existing == immutable {
                Ok(())
            } else {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                    Some("Document issue key already has different inputs".into()),
                ))
            };
        }
        let pending: u64 = db.query_row(
            "SELECT count(*) FROM __konae_documents WHERE state IN ('pending','rendering','failed')",
            [],
            |row| row.get(0),
        )?;
        if pending >= MAX_PENDING_PER_APP {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "Documents already has {MAX_PENDING_PER_APP} pending issues for this app."
                )),
            ));
        }
        let input = serde_json::to_vec(&self.input).map_err(encode)?;
        if input.len() > MAX_RENDER_INPUT_BYTES {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some("Document render input exceeds 2 MiB.".into()),
            ));
        }
        db.execute("INSERT INTO __konae_documents(issue_key,release,read_rule_json,snapshot_json,template,resources_json,typst_version,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8)",
            params![self.key, self.release, self.read_rule_json, snapshot, self.input.template,
                resources, self.input.typst_version, self.created_at as i64])?;
        Ok(())
    }
}

pub fn ensure_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)
}

pub(crate) fn check_snapshot_columns(
    connection: &Connection,
    name: &str,
    action: &crate::ActionDefinition,
) -> Result<()> {
    let Some(issue) = &action.issue else {
        return Ok(());
    };
    let sql = crate::notify::with_returning(&action.statement, &issue.snapshot);
    if connection
        .prepare(&sql)
        .is_ok_and(|statement| statement.column_count() == issue.snapshot.len())
    {
        return Ok(());
    }
    anyhow::bail!(crate::refusal::refuse(
        "documents.snapshot-column-unknown",
        format!(
            "Action {name} names a Documents snapshot column its statement cannot return. Name columns returned by the mutation."
        ),
    ));
}

pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/_konae/documents/{key}", get(download))
}

async fn download(
    State(state): State<crate::AppState>,
    AxumPath(key): AxumPath<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let Some(deployment) = crate::deployment_for(&state, &headers).await else {
        return (StatusCode::NOT_FOUND, "Deployment not found.").into_response();
    };
    let _app_lock = crate::action_lock(&state, &deployment.id, "query").await;
    let deployment = match state
        .deployments
        .read()
        .await
        .values()
        .find(|current| current.id == deployment.id)
        .cloned()
    {
        Some(current) => current,
        None => return (StatusCode::NOT_FOUND, "Deployment not found.").into_response(),
    };
    if let Err(refused) = crate::modules::require_on(&deployment.modules, "documents") {
        return (StatusCode::FORBIDDEN, refused.message).into_response();
    }
    let actor = match crate::authenticate_deployment(&state, &deployment, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "Sign in to read this document.").into_response();
        }
        Err(response) => return response,
    };
    let database = crate::layout::database(&state.data_root, &deployment.id);
    let record = match tokio::task::spawn_blocking({ let database=database.clone(); let key=key.clone(); move || -> Result<Option<(String,String,String,i64)>> {
        let db=Connection::open(database)?;
        if !table_exists(&db)? { return Ok(None); }
        Ok(db.query_row("SELECT release,read_rule_json,pdf_blob,pdf_size FROM __konae_documents WHERE issue_key=?1 AND state='ready'", [key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?)
    }}).await {
        Ok(Ok(Some(record))) => record,
        _ => return (StatusCode::NOT_FOUND, "Document not found.").into_response(),
    };
    if record.0.is_empty() {
        return (StatusCode::NOT_FOUND, "Document not found.").into_response();
    }
    let rule: crate::ActionDefinition =
        match serde_json::from_str::<crate::ActionDefinition>(&record.1) {
            Ok(rule) if rule.kind == "query" && rule.access == "authenticated" => rule,
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Document access rule is unavailable.",
                )
                    .into_response();
            }
        };
    let params = match crate::resolve_bindings(
        &rule,
        &serde_json::json!({"issueKey": key}),
        Some(&actor.user_id),
    ) {
        Ok(params) => params,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Document access rule is unavailable.",
            )
                .into_response();
        }
    };
    let permit = match state.guards.statement_permit(&deployment.id).await {
        Ok(permit) => permit,
        Err(wait) => return crate::integrations::http::rate_limited(wait),
    };
    let allowed = match crate::run_action_in_slot(database, &rule, params, permit).await {
        Ok(result) => result
            .answer
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        Err(_) => false,
    };
    if !allowed {
        return (StatusCode::NOT_FOUND, "Document not found.").into_response();
    }
    let Some(blob) = state
        .files
        .store
        .get(&deployment.id, &record.2, None)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, "Document not found.").into_response();
    };
    if blob.length != record.3 as u64 || blob.length > MAX_PDF_BYTES {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Stored document size does not match its record.",
        )
            .into_response();
    }
    let mut bytes = Vec::with_capacity(blob.length as usize);
    if tokio::io::AsyncReadExt::take(blob.reader, MAX_PDF_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() as u64 != blob.length
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not read the stored document.",
        )
            .into_response();
    }
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pdf"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"document.pdf\""),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

pub fn pending(database: &Path) -> Result<bool> {
    let db = Connection::open(database)?;
    if !table_exists(&db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM __konae_documents WHERE state IN ('pending','rendering'))",
        [],
        |row| row.get(0),
    )?)
}

fn table_exists(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_documents')", [], |row| row.get(0))
}

pub fn blob_references_in(directory: &Path) -> Result<Vec<(String, u64)>> {
    let database = directory.join("database.sqlite");
    if !database.is_file() {
        return Ok(Vec::new());
    }
    let db = Connection::open(database)?;
    if !table_exists(&db)? {
        return Ok(Vec::new());
    }
    let mut stmt = db.prepare("SELECT pdf_blob,pdf_size FROM __konae_documents WHERE state='ready' AND pdf_blob IS NOT NULL ORDER BY pdf_blob")?;
    Ok(stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub async fn render(input: RenderInput, output: &Path, scratch: &Path) -> Result<()> {
    let serialized = serde_json::to_vec(&input)?;
    ensure!(
        serialized.len() <= MAX_RENDER_INPUT_BYTES,
        "Document render input exceeds 2 MiB."
    );
    ensure!(
        input.typst_version == DEFAULT_TYPST_VERSION,
        "This Kernel supports Typst {} for Documents.",
        DEFAULT_TYPST_VERSION
    );
    tokio::fs::create_dir_all(scratch).await?;
    let source = scratch.join("document.typ");
    let wrapped = format!(
        "#let issue = json.decode({})\n{}",
        typst_string(&serde_json::to_string(&input.snapshot)?),
        input.template
    );
    tokio::fs::write(&source, wrapped).await?;
    for (name, text) in &input.resources {
        let relative = Path::new(name);
        ensure!(
            !relative.is_absolute()
                && relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Document resource path is invalid."
        );
        let bytes = if let Some(encoded) = text.strip_prefix("base64:") {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .context("Document resource has invalid base64.")?
        } else {
            text.as_bytes().to_vec()
        };
        ensure!(
            bytes.len() <= 256 * 1024,
            "Document resource exceeds 256 KiB."
        );
        let path = scratch.join(relative);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, bytes).await?;
    }
    let binary = std::env::var_os("KONAE_DOCUMENTS_TYPST_BIN").unwrap_or_else(|| "typst".into());
    let log_path = scratch.join("typst.log");
    let log = std::fs::File::create(&log_path)?;
    let mut command = Command::new(binary);
    command
        .args(["compile", "--root"])
        .arg(scratch)
        .arg("--font-path")
        .arg(scratch)
        .arg(&source)
        .arg(output)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().pre_exec(|| {
            #[cfg(not(target_os = "macos"))]
            if libc::setrlimit(
                libc::RLIMIT_AS,
                &libc::rlimit {
                    rlim_cur: 1024 * 1024 * 1024,
                    rlim_max: 1024 * 1024 * 1024,
                },
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let cpu = libc::setrlimit(
                libc::RLIMIT_CPU,
                &libc::rlimit {
                    rlim_cur: 30,
                    rlim_max: 31,
                },
            );
            if cpu != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("Could not start the local Typst renderer.")?;
    let deadline = tokio::time::Instant::now() + RENDER_TIMEOUT;
    let status = loop {
        if tokio::fs::metadata(output)
            .await
            .map(|meta| meta.len() > MAX_PDF_BYTES)
            .unwrap_or(false)
            || tokio::fs::metadata(&log_path)
                .await
                .map(|meta| meta.len() > MAX_RENDER_LOG_BYTES as u64)
                .unwrap_or(false)
        {
            let _ = child.kill().await;
            bail!("Typst exceeded the 10 MiB PDF or 64 KiB log limit.");
        }
        #[cfg(target_os = "macos")]
        if let Some(pid) = child.id() {
            let rss = Command::new("/bin/ps")
                .args(["-o", "rss=", "-p", &pid.to_string()])
                .output()
                .await?;
            let kib = String::from_utf8_lossy(&rss.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or_default();
            if kib > 1024 * 1024 {
                let _ = child.kill().await;
                bail!("Typst exceeded the 1 GiB memory limit.");
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            bail!("Document rendering exceeded its 30 second deadline.");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let stderr = tokio::fs::read(&log_path).await.unwrap_or_default();
    ensure!(
        status.success(),
        "Typst failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let size = tokio::fs::metadata(output).await?.len();
    ensure!(
        size > 0 && size <= MAX_PDF_BYTES,
        "Rendered PDF must be between 1 byte and 10 MiB."
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IssueDeclaration {
    pub key: String,
    pub download: String,
    pub snapshot: Vec<String>,
    pub template: String,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_typst_version")]
    pub typst_version: String,
}

fn default_typst_version() -> String {
    DEFAULT_TYPST_VERSION.to_owned()
}

#[derive(Clone)]
pub struct MutationContext {
    pub app: String,
    pub release: String,
    pub action: String,
    pub input: Value,
    pub at: u64,
    pub read_rule: String,
}

impl IssueEffect {
    pub fn from_mutation(
        action: &crate::ActionDefinition,
        context: MutationContext,
        ran: &crate::Ran,
    ) -> Result<Option<Self>> {
        let Some(declaration) = &action.issue else {
            return Ok(None);
        };
        if ran.answer["changes"].as_u64().unwrap_or_default() == 0 {
            return Ok(None);
        }
        let key = context
            .input
            .get(&declaration.key)
            .and_then(Value::as_str)
            .context("The document issue key input must be a string.")?
            .to_owned();
        ensure!(
            !key.is_empty()
                && key.len() <= 200
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte)),
            "Document issue key must contain 1 to 200 URL-safe characters."
        );
        ensure!(
            ran.rows.len() == 1,
            "A document issue mutation must change exactly one row."
        );
        let row = &ran.rows[0];
        let mut snapshot = serde_json::Map::new();
        for column in &declaration.snapshot {
            snapshot.insert(
                column.clone(),
                row.get(column)
                    .cloned()
                    .context("The document issue snapshot column was not returned.")?,
            );
        }
        Ok(Some(Self {
            key,
            release: context.release,
            read_rule_json: context.read_rule,
            input: RenderInput {
                snapshot: Value::Object(snapshot),
                template: declaration.template.clone(),
                resources: declaration.resources.clone(),
                typst_version: declaration.typst_version.clone(),
            },
            created_at: context.at,
        }))
    }
}

fn typst_string(value: &str) -> String {
    format!(
        "{}",
        serde_json::to_string(value).unwrap_or_else(|_| "\"{}\"".into())
    )
}

pub async fn require_renderer() -> Result<()> {
    let binary = std::env::var_os("KONAE_DOCUMENTS_TYPST_BIN").unwrap_or_else(|| "typst".into());
    let output = tokio::time::timeout(RENDER_TIMEOUT, Command::new(binary).arg("--version").output()).await
        .context("Could not check the Documents Typst renderer within 30 seconds.")?
        .context("Documents is enabled, but Typst is unavailable. Install Typst 0.14.2 or set KONAE_DOCUMENTS_TYPST_BIN to its executable path.")?;
    let version = String::from_utf8_lossy(&output.stdout);
    ensure!(
        output.status.success()
            && version.split_whitespace().take(2).collect::<Vec<_>>()
                == ["typst", DEFAULT_TYPST_VERSION],
        "Documents needs Typst {}. Set KONAE_DOCUMENTS_TYPST_BIN to its executable path.",
        DEFAULT_TYPST_VERSION
    );
    Ok(())
}

/// Claims one job with a monotonically increasing attempt number.
pub fn claim(db: &Connection) -> rusqlite::Result<Option<(String, i64, String, RenderInput)>> {
    ensure_schema(db)?;
    let tx = db.unchecked_transaction()?;
    let item: Option<(String, String, String, String, String, i64)> = tx.query_row(
        "SELECT issue_key,snapshot_json,template,resources_json,typst_version,attempt FROM __konae_documents WHERE state IN ('pending','failed','rendering') ORDER BY CASE state WHEN 'failed' THEN 1 ELSE 0 END,created_at,issue_key LIMIT 1",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
    let Some((key, snapshot, template, resources, version, attempt)) = item else {
        tx.commit()?;
        return Ok(None);
    };
    let next = attempt + 1;
    let token = uuid::Uuid::new_v4().to_string();
    tx.execute("UPDATE __konae_documents SET state='rendering',attempt=?2,attempt_token=?3,error=NULL WHERE issue_key=?1", params![key,next,token])?;
    tx.commit()?;
    let input = RenderInput {
        snapshot: serde_json::from_str(&snapshot).unwrap_or(Value::Null),
        template,
        resources: serde_json::from_str(&resources).unwrap_or_default(),
        typst_version: version,
    };
    Ok(Some((key, next, token, input)))
}

/// A renderer may publish only while its attempt still owns the issue.
pub fn complete(
    db: &Connection,
    key: &str,
    attempt: i64,
    token: &str,
    blob: &str,
    size: u64,
) -> rusqlite::Result<bool> {
    let changed = db.execute("UPDATE __konae_documents SET state='ready',pdf_blob=?4,pdf_size=?5,error=NULL WHERE issue_key=?1 AND attempt=?2 AND attempt_token=?3 AND state='rendering'",
        params![key,attempt,token,blob,size as i64])?;
    Ok(changed == 1)
}

pub(crate) fn check_pending_on_connection(db: &Connection) -> Result<()> {
    if table_exists(db)? {
        let mut query = db.prepare("SELECT snapshot_json,template,resources_json,typst_version FROM __konae_documents WHERE state IN ('pending','rendering','failed')")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (snapshot, template, resources, version) = row?;
            let input = RenderInput {
                snapshot: serde_json::from_str(&snapshot)?,
                template,
                resources: serde_json::from_str(&resources)?,
                typst_version: version,
            };
            ensure!(
                serde_json::to_vec(&input)?.len() <= MAX_RENDER_INPUT_BYTES,
                "A pending Documents issue exceeds the render input limit."
            );
        }
    }
    Ok(())
}

pub(crate) fn prepare_transfer(_state: &crate::AppState, _app: &str) -> Result<()> {
    Ok(())
}

pub(crate) fn pause_export(database: &Path) -> Result<()> {
    let db = Connection::open(database)?;
    if table_exists(&db)? {
        db.execute("UPDATE __konae_documents SET state='paused' WHERE state IN ('pending','rendering','failed')", [])?;
    }
    Ok(())
}

pub(crate) fn resume_import(database: &Path) -> Result<()> {
    let db = Connection::open(database)?;
    if table_exists(&db)? {
        db.execute(
            "UPDATE __konae_documents SET state='pending' WHERE state='paused'",
            [],
        )?;
    }
    Ok(())
}

pub(crate) fn spawn_worker(state: crate::AppState) {
    use crate::execution::{AppTimers, WorkLimits, WorkResource};
    use std::num::NonZeroUsize;
    use std::time::Instant;
    use tokio::task::JoinSet;

    const RESOURCE: WorkResource = WorkResource::Documents;
    const LIMITS: WorkLimits = WorkLimits::new(NonZeroUsize::new(2).unwrap(), NonZeroUsize::MIN);

    tokio::spawn(async move {
        let mut timers = AppTimers::default();
        let mut renders = JoinSet::new();
        let apps = state
            .deployments
            .read()
            .await
            .values()
            .map(|deployment| deployment.id.clone())
            .collect::<Vec<_>>();
        for app in apps {
            let database = crate::layout::database(&state.data_root, &app);
            if database.is_file()
                && (pending(&database).unwrap_or(false) || has_failed(&database).unwrap_or(false))
            {
                state.dispatcher.wake(RESOURCE, app).await;
            }
        }
        loop {
            let deadline = timers.next_deadline();
            tokio::select! {
                turn = state.dispatcher.next_turn(RESOURCE, LIMITS) => {
                    let state = state.clone();
                    renders.spawn(async move {
                        let result = run_one(&state, turn.app()).await;
                        (turn, result)
                    });
                }
                Some(finished) = renders.join_next(), if !renders.is_empty() => {
                    match finished {
                        Ok((turn, result)) => {
                            let app = turn.app();
                            match result {
                                Ok(true) => state.dispatcher.wake(RESOURCE, app).await,
                                Ok(false) => timers.remove(app),
                                Err(error) => {
                                    eprintln!("Documents worker failed for {app}: {error:#}");
                                    timers.schedule_retry(app.to_owned(), Instant::now());
                                }
                            }
                        }
                        Err(error) => eprintln!("Documents worker task failed: {error}"),
                    }
                }
                _ = async {
                    if let Some(deadline) = deadline {
                        tokio::time::sleep_until(deadline.into()).await;
                    }
                }, if deadline.is_some() => {
                    for app in timers.take_due(Instant::now()) {
                        state.dispatcher.wake(RESOURCE, app).await;
                    }
                }
            }
        }
    });
}

async fn run_one(state: &crate::AppState, app: &str) -> Result<bool> {
    let database = crate::layout::database(&state.data_root, app);
    let job = {
        let _lock = crate::database_lock(state, app).await;
        if !database.is_file()
            || crate::maintenance::pause(&state.data_root, app).is_some()
            || !state
                .deployments
                .read()
                .await
                .values()
                .any(|deployment| deployment.id == app)
        {
            return Ok(false);
        }
        let database = database.clone();
        tokio::task::spawn_blocking(move || {
            let db = Connection::open(database)?;
            if table_exists(&db)? {
                claim(&db)
            } else {
                Ok(None)
            }
        })
        .await??
    };
    let Some((key, attempt, token, input)) = job else {
        return Ok(false);
    };
    let directory = state
        .data_root
        .join("transfers")
        .join("documents")
        .join(&token);
    let pdf = directory.join("document.pdf");
    let rendered = render(input, &pdf, &directory).await;
    let result = async {
        let _lock = crate::database_lock(state, app).await;
        if !database.is_file() || crate::maintenance::pause(&state.data_root, app).is_some() {
            return Ok(false);
        }
        let db = Connection::open(&database)?;
        if !table_exists(&db)? || !current_attempt(&db, &key, attempt, &token)? {
            return Ok(true);
        }
        drop(db);

        let result = match rendered {
            Ok(()) => store_rendered(state, app, &database, &key, attempt, &token, &pdf).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            Connection::open(&database)?.execute(
                "UPDATE __konae_documents SET state='failed',error=?4 WHERE issue_key=?1 AND attempt=?2 AND attempt_token=?3 AND state='rendering'",
                params![key, attempt, token, format!("{error:#}")],
            )?;
            return Err(error);
        }
        Ok(true)
    }.await;
    match tokio::fs::remove_dir_all(&directory).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            eprintln!(
                "Could not remove Documents scratch {}: {error}",
                directory.display()
            );
        }
        _ => {}
    }
    result
}

fn current_attempt(
    db: &Connection,
    key: &str,
    attempt: i64,
    token: &str,
) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM __konae_documents WHERE issue_key=?1 AND attempt=?2 AND attempt_token=?3 AND state='rendering')",
        params![key, attempt, token],
        |row| row.get(0),
    )
}

async fn store_rendered(
    state: &crate::AppState,
    app: &str,
    database: &Path,
    key: &str,
    attempt: i64,
    token: &str,
    pdf: &Path,
) -> Result<()> {
    let size = tokio::fs::metadata(pdf).await?.len();
    let file_limit = MAX_PDF_BYTES.min(state.files.limits.max_file_bytes);
    ensure!(
        size <= file_limit,
        "Rendered PDF exceeds the app's {file_limit} per-file limit."
    );
    let used = crate::modules::blob_references(&state.data_root, app)?
        .iter()
        .fold(0u64, |total, reference| {
            total.saturating_add(reference.size)
        });
    ensure!(
        used.saturating_add(size) <= state.files.limits.quota_bytes,
        "The app's shared file quota is full."
    );

    let blob = uuid::Uuid::new_v4().to_string();
    state.files.store.put(app, &blob, pdf).await?;
    let committed =
        Connection::open(database).and_then(|db| complete(&db, key, attempt, token, &blob, size));
    if !matches!(committed, Ok(true)) {
        if let Err(error) = state.files.store.delete(app, &blob).await {
            eprintln!("Could not remove uncommitted document blob {blob} for {app}: {error:#}");
        }
        ensure!(
            committed?,
            "The document render attempt no longer owns this issue."
        );
    }
    Ok(())
}

fn has_failed(database: &Path) -> Result<bool> {
    let db = Connection::open(database)?;
    if !table_exists(&db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM __konae_documents WHERE state='failed')",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) async fn record_issue(state: &crate::AppState, deployment: &crate::Deployment) {
    state
        .dispatcher
        .wake(
            crate::execution::WorkResource::Documents,
            deployment.id.clone(),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn effect(total: i64, release: &str) -> IssueEffect {
        IssueEffect {
            key: "invoice-17".into(), release: release.into(),
            read_rule_json: r#"{"kind":"query","access":"authenticated","statement":"SELECT id FROM invoices WHERE id=? AND owner_id=? LIMIT 1","bindings":[{"source":"input","key":"issueKey","type":"string"},{"source":"actor","field":"id"}]}"#.into(),
            input: RenderInput { snapshot: json!({"id":"invoice-17","total":total}),
                template: "#text(issue.id)\n#text(str(issue.total))".into(), resources: Default::default(), typst_version: DEFAULT_TYPST_VERSION.into() },
            created_at: 1,
        }
    }

    #[test]
    fn the_issue_effect_rolls_back_and_keeps_one_immutable_key() {
        let root =
            std::env::temp_dir().join(format!("konae-documents-tx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        let db = Connection::open(&path).unwrap();
        {
            let tx = db.unchecked_transaction().unwrap();
            crate::execution::TransactionEffect::prepare(&effect(1700, "r1"), &tx).unwrap();
        }
        assert!(!table_exists(&db).unwrap());
        let tx = db.unchecked_transaction().unwrap();
        crate::execution::TransactionEffect::prepare(&effect(1700, "r1"), &tx).unwrap();
        tx.commit().unwrap();
        let replay = db.unchecked_transaction().unwrap();
        crate::execution::TransactionEffect::prepare(&effect(1700, "r1"), &replay).unwrap();
        replay.commit().unwrap();
        let conflict = db.unchecked_transaction().unwrap();
        assert!(
            crate::execution::TransactionEffect::prepare(&effect(1800, "r2"), &conflict).is_err()
        );
        drop(conflict);
        let (count, snapshot): (i64, String) = db.query_row("SELECT COUNT(*),MAX(snapshot_json) FROM __konae_documents WHERE issue_key='invoice-17'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            serde_json::from_str::<Value>(&snapshot).unwrap(),
            json!({"id":"invoice-17","total":1700})
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_app_at_capacity_can_replay_existing_issues() {
        use crate::execution::TransactionEffect;
        let db = Connection::open_in_memory().unwrap();
        let first = effect(1700, "r1");
        first.prepare(&db).unwrap();
        db.execute(
            "WITH RECURSIVE queue(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM queue WHERE n+1 < ?1)
             INSERT INTO __konae_documents(issue_key,release,read_rule_json,snapshot_json,template,resources_json,typst_version,state,created_at)
             SELECT 'queued-' || n, release,read_rule_json,snapshot_json,template,resources_json,typst_version,'pending',created_at
             FROM queue, __konae_documents WHERE issue_key = ?2",
            params![MAX_PENDING_PER_APP, first.key],
        ).unwrap();
        first.prepare(&db).unwrap();
        let mut next = effect(1700, "r1");
        next.key = "new-issue".into();
        assert!(
            next.prepare(&db)
                .unwrap_err()
                .to_string()
                .contains("1000 pending issues")
        );
    }

    #[test]
    fn a_server_action_commits_its_snapshot_with_the_row_change() {
        let root =
            std::env::temp_dir().join(format!("konae-documents-action-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE invoices(id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, total INTEGER NOT NULL, issued INTEGER NOT NULL DEFAULT 0); INSERT INTO invoices VALUES('invoice-17','account-a',1700,0);").unwrap();
        let cases: Vec<Value> = serde_json::from_str(include_str!("documents-cases.json")).unwrap();
        let source = json!({"version":1,"actions":cases[0]["actions"]});
        let program = crate::action_shape::read(&source.to_string()).unwrap();
        let action = &program.actions["documents.issue"];
        let context = || crate::notify::MutationContext {
            app: "app".into(),
            release: "r1".into(),
            action: "documents.issue".into(),
            input: json!({"id":"invoice-17"}),
            account: Some("account-a".into()),
            actor_id: Some("account-a".into()),
            at: 1,
            documents_rule: Some(cases[0]["actions"]["documents.read"].to_string()),
        };
        let first = crate::run_mutation(
            &path,
            action,
            vec![rusqlite::types::Value::Text("invoice-17".into())],
            Some(context()),
        )
        .unwrap();
        assert_eq!(first.answer["changes"], 1);
        let db = Connection::open(&path).unwrap();
        let stored: String = db
            .query_row(
                "SELECT snapshot_json FROM __konae_documents WHERE issue_key='invoice-17'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored).unwrap(),
            json!({"id":"invoice-17","total":1700})
        );
        db.execute(
            "UPDATE invoices SET total=1800,issued=0 WHERE id='invoice-17'",
            [],
        )
        .unwrap();
        drop(db);
        assert!(
            crate::run_mutation(
                &path,
                action,
                vec![rusqlite::types::Value::Text("invoice-17".into())],
                Some(context())
            )
            .is_err()
        );
        let db = Connection::open(&path).unwrap();
        let (issued, total, count): (i64,i64,i64) = db.query_row("SELECT issued,total,(SELECT COUNT(*) FROM __konae_documents) FROM invoices WHERE id='invoice-17'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(
            (issued, total, count),
            (0, 1800, 1),
            "the conflicting action change rolls back and the old issue stays intact"
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restored_attempt_counters_cannot_authorize_an_old_render() {
        let root =
            std::env::temp_dir().join(format!("konae-documents-attempt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let live = root.join("live.sqlite");
        let backup = root.join("backup.sqlite");
        let db = Connection::open(&live).unwrap();
        ensure_schema(&db).unwrap();
        crate::execution::TransactionEffect::prepare(&effect(1700, "r1"), &db).unwrap();
        drop(db);
        std::fs::copy(&live, &backup).unwrap();
        let old = claim(&Connection::open(&live).unwrap()).unwrap().unwrap();
        std::fs::copy(&backup, &live).unwrap();
        let imported = Connection::open(&live).unwrap();
        resume_import(&live).unwrap();
        let current = claim(&imported).unwrap().unwrap();
        assert_eq!(
            old.1, current.1,
            "the restored counter repeats the same number"
        );
        assert_ne!(old.2, current.2, "each claim receives a random token");
        assert!(!complete(&imported, &old.0, old.1, &old.2, "old-pdf", 10).unwrap());
        assert!(complete(&imported, &current.0, current.1, &current.2, "new-pdf", 10).unwrap());
        drop(imported);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_named_query_separates_two_app_accounts() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("documents-cases.json")).unwrap();
        let program = crate::action_shape::read(
            &json!({"version":1,"actions":cases[0]["actions"]}).to_string(),
        )
        .unwrap();
        let rule = &program.actions["documents.read"];
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE invoices(id TEXT, owner_id TEXT); INSERT INTO invoices VALUES('invoice-17','account-a');").unwrap();
        for (actor, expected) in [("account-a", 1), ("account-b", 0)] {
            let params =
                crate::resolve_bindings(rule, &json!({"issueKey":"invoice-17"}), Some(actor))
                    .unwrap();
            assert_eq!(
                crate::run_query_on_connection(&db, rule, params)
                    .unwrap()
                    .len(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn typst_renders_a_portable_issue_to_a_pdf() {
        if require_renderer().await.is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("konae-documents-render-{}", uuid::Uuid::new_v4()));
        let output = root.join("issue.pdf");
        let input = RenderInput {
            snapshot: json!({"id":"invoice-17","total":1700}),
            template: "#set page(paper: \"a4\")\n#text(issue.id)\n#text(str(issue.total))".into(),
            resources: Default::default(),
            typst_version: DEFAULT_TYPST_VERSION.into(),
        };
        render(input, &output, &root).await.unwrap();
        let bytes = tokio::fs::read(&output).await.unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() as u64 <= MAX_PDF_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn package_snapshot_and_import_keep_pdf_bytes_and_pending_render_inputs() {
        let root =
            std::env::temp_dir().join(format!("konae-documents-package-{}", uuid::Uuid::new_v4()));
        let data = root.join("apps/app/data");
        let snapshot = root.join("snapshot");
        let imported = root.join("imported/apps/app/data");
        std::fs::create_dir_all(data.join("files")).unwrap();
        std::fs::write(data.join("files/index.json"), b"[]").unwrap();
        let pdf = b"%PDF-1.7\nportable-document\n%%EOF\n";
        let source = root.join("source.pdf");
        std::fs::write(&source, pdf).unwrap();
        let blob = uuid::Uuid::new_v4().to_string();
        let store: std::sync::Arc<dyn crate::file_store::FileStore> =
            std::sync::Arc::new(crate::file_store::LocalFolder::new(&root));
        store.put("app", &blob, &source).await.unwrap();
        let db = Connection::open(data.join("database.sqlite")).unwrap();
        crate::execution::TransactionEffect::prepare(&effect(1700, "r1"), &db).unwrap();
        db.execute("UPDATE __konae_documents SET state='ready',pdf_blob=?1,pdf_size=?2 WHERE issue_key='invoice-17'", params![blob,pdf.len() as i64]).unwrap();
        db.execute("INSERT INTO __konae_documents(issue_key,release,read_rule_json,snapshot_json,template,resources_json,typst_version,state,created_at) VALUES('pending-issue','r1',?1,'{\"id\":\"pending-issue\"}','#text(issue.id)','{\"fonts/note.txt\":\"portable\"}','0.14.2','pending',2)", [effect(0,"r1").read_rule_json]).unwrap();
        drop(db);
        let mut modules = crate::modules::defaults();
        modules.get_mut("actions").unwrap()["on"] = json!(true);
        modules.get_mut("documents").unwrap()["on"] = json!(true);
        let snapshot_store = store.clone();
        let snapshot_data = data.clone();
        let snapshot_target = snapshot.clone();
        let parts = tokio::task::spawn_blocking(move || {
            let blobs = crate::file_store::BlockingFiles::new(snapshot_store, "app");
            crate::package::snapshot(&snapshot_data, &snapshot_target, &blobs, &modules)
        })
        .await
        .unwrap()
        .unwrap();
        assert!(parts.iter().any(|(name, _)| *name == "database"));
        assert!(parts.iter().any(|(name, _)| *name == "files"));
        let refs = crate::modules::blob_references_in_data_dir(&snapshot).unwrap();
        assert_eq!(
            refs,
            vec![crate::modules::BlobReference {
                module: "documents".into(),
                id: blob.clone(),
                size: pdf.len() as u64
            }]
        );
        std::fs::create_dir_all(&imported).unwrap();
        std::fs::copy(
            snapshot.join("database.sqlite"),
            imported.join("database.sqlite"),
        )
        .unwrap();
        crate::modules::resume_imported_work(&imported.join("database.sqlite")).unwrap();
        let imported_store: std::sync::Arc<dyn crate::file_store::FileStore> =
            std::sync::Arc::new(crate::file_store::LocalFolder::new(&root.join("imported")));
        let import_store = imported_store.clone();
        let archive = snapshot.join("files.tar");
        let destination = imported.join("files");
        tokio::task::spawn_blocking(move || {
            let imported_blobs = crate::file_store::BlockingFiles::new(import_store, "app");
            crate::package::unpack_files_with_references(
                &archive,
                None,
                Some((&destination, &imported_blobs)),
                &Default::default(),
                &refs,
            )
        })
        .await
        .unwrap()
        .unwrap();
        let mut original = store.get("app", &blob, None).await.unwrap().unwrap();
        let mut imported_pdf = imported_store
            .get("app", &blob, None)
            .await
            .unwrap()
            .unwrap();
        let mut original_bytes = Vec::new();
        original
            .reader
            .read_to_end(&mut original_bytes)
            .await
            .unwrap();
        let mut imported_bytes = Vec::new();
        imported_pdf
            .reader
            .read_to_end(&mut imported_bytes)
            .await
            .unwrap();
        assert_eq!(original_bytes, pdf);
        assert_eq!(imported_bytes, pdf);
        let imported_db = Connection::open(imported.join("database.sqlite")).unwrap();
        let recovered = claim(&imported_db).unwrap().unwrap();
        assert_eq!(recovered.0, "pending-issue");
        assert_eq!(recovered.3.template, "#text(issue.id)");
        assert_eq!(recovered.3.resources["fonts/note.txt"], "portable");
        drop(imported_db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
