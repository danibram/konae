pub mod integrations;
pub use integrations::{calls, http, provider};
pub mod app_folders;
pub use app_folders::{folder, import, layout, package, sources};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use aws_sdk_s3::Client as S3Client;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
#[cfg(test)]
use rusqlite::hooks::{AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

pub mod actions;
pub mod app_data;
pub use actions::{shape as action_shape, sql_rules};
pub use app_data::{connection as app_db, schema_change, schema_transform};

pub use actions::{
    ActionBinding, ActionDefinition, ActionLock, ActionProgram, Ran, ResultBudget, SqlTok,
    SqlTokKind, action_lock, aggregate_select_list, bounded_row, bounded_rows, is_sql_word_cont,
    is_sql_word_start, json_sql, limit_index, resolve_bindings, run_action, run_action_in_slot,
    run_mutation, run_mutation_on_connection, run_query, run_query_on_connection,
    sql_json, sql_tokens, unbounded_query_reason, validate_action, validate_declarations,
};
pub(crate) use actions::{
    QUERY_RESULT_TOO_LARGE, ResultTooLarge, browse_column_json_cost,
};
#[cfg(any(test, feature = "testing"))]
pub(crate) use actions::COLUMNS_CONVERTED;
pub use app_data::{
    DENIED_FUNCTIONS, MAX_APP_DATABASE_BYTES, MAX_LIKE_PATTERN_BYTES, arm_deadline,
    denied_function, initialize_database, limit_database_size, replace_database_file,
    restrict_database, use_wal,
};
pub(crate) use app_data::database::restrict_function_database;

pub mod copies;
pub mod execution;
pub mod file_store;
pub mod files;
pub mod functions;
pub use functions::{
    http as functions_http, invocation as function_data, runtime as functions_runtime,
};
pub mod guard;
pub mod heap;
pub mod installation;
pub use installation::{inventory, maintenance, mail, recovery};
pub mod modules;
pub mod notify;
pub mod own_page;
pub mod refusal;
pub mod schedule;
pub mod size;
#[cfg(test)]
pub mod sqlite_bench;
pub mod usage;
pub mod deployments;
pub mod app_accounts;
pub use app_accounts::{accounts, auth, enter, google, invitations};
pub mod server;
pub use server::{throttle as limits, operator, runtime, security, transfers};
pub mod routing;
pub use routing::{artifact, domain, hostname, hosts, page, path, path_name, seo};

pub use deployments::{Deployment, Settings, Installed, ReleasedSettings, KERNEL_VERSION, is_private};
pub use deployments::{DeployInput, ProjectInput, BuildArtifact};
pub use deployments::{deploy, deploy_input, deploy_import_held, stage_local_artifact, staged_dist, staged_server, switch_dist, remove_staged_dist, remove_stale_dist};
#[cfg(test)]
pub(crate) use deployments::{FAILING_RELEASE_STEPS, released_settings};
#[cfg(test)]
pub(crate) use deployments::swap_database_with;
pub use deployments::{find_project_deployment, deployment_by_id};
pub use deployments::{persist_deployment, persist_folder_snapshot, persist_folder_snapshot_with_transforms, persist_release, load_deployments};
#[cfg(any(test, feature = "testing"))]
pub use deployments::write_record_for_test;
pub use app_accounts::{SignInMethod, default_sign_in_methods, PASSWORD_REQUIRED, sign_in_methods_from_input, SignupMode, SignupPolicy, MAX_ALLOWLIST_ENTRIES, SIGNUP_REFUSED, signup_policy_value, set_signup_policy};
pub use app_accounts::{session, AuthInput, signup, login, auth_request, blocking, too_many_attempts, logout, cleared_app_cookie, cookie_secure, auth_error};
pub use app_accounts::{open_auth, authenticate_deployment, app_cookie_name, cookie_value};
pub use server::{Startup, Running, start, start_locked, state_from_env, bind};
pub use server::{listen, serve, stack, health, same_origin};
pub(crate) use server::Operation;
pub use routing::{serve_root, login_page, serve_artifact, serve_key, count_page_view, read_artifact, private_login_page, ArtifactCache, etag_matches, artifact_response};
pub(crate) use routing::serve_key_with_routes;
pub use routing::{deployment_for, deployment_for_host, deployment_for_request};
pub use routing::{deployment_url, safe_slug, content_type};

use auth::{AuthError, AuthService, AuthUser};
use limits::Guards;

#[derive(Clone)]
pub struct AppState {
    pub execution_mode: execution::ExecutionMode,
    pub deployments: Arc<RwLock<HashMap<String, Deployment>>>,
    pub db_locks: LockMap,
    pub s3: Option<S3Client>,
    pub data_bucket: String,
    pub data_root: PathBuf,
    /// Where file bytes live and how many a Deployment may hold.
    pub files: file_store::Files,
    pub copies: Arc<copies::CopyStatus>,
    pub public_origin: String,
    /// The origins the installation answers on itself besides the public
    /// origin, such as where Konae's MCP answers. The first is where a
    /// client signs in.
    pub own_origins: Vec<String>,
    pub host_suffix: String,
    /// `<scheme>://<domain>` where path addresses answer; `None` without `KONAE_DOMAIN`.
    pub path_origin: Option<String>,
    pub guards: Arc<Guards>,
    pub accounts: accounts::AccountSettings,
    /// The stores at the data root besides the app folders.
    pub installation: Arc<installation::Installation>,
    /// Where an app user's file uploads and deletes are recorded.
    pub audit: Option<runtime::AuditSink>,
    pub functions: Arc<functions::runtime::Runtime>,
}

#[cfg(any(test, feature = "testing"))]
impl AppState {
    /// A Kernel on `root` with no Deployments, local files, no mail and the
    /// `localhost` names. A test changes the fields it needs with `..`.
    pub fn for_test(root: &Path) -> Self {
        Self {
            execution_mode: execution::ExecutionMode::Live,
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            data_bucket: "data".into(),
            files: file_store::Files::local(root),
            data_root: root.into(),
            copies: Default::default(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            own_origins: Vec::new(),
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
            installation: Arc::new(installation::Installation::for_test()),
            audit: None,
            functions: Arc::new(functions::runtime::Runtime::disabled()),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBackup {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub size_bytes: u64,
    pub key: String,



    #[serde(default)]
    pub holds: Vec<String>,
}

impl DatabaseBackup {
    /// The backup as `list-backups` lists it.
    pub fn view(&self) -> Value {
        json!({"id": self.id, "label": self.label, "createdAt": self.created_at, "sizeBytes": self.size_bytes, "holds": self.holds})
    }
}

/// Runs the operator commands both binaries answer: `restore`, the file
/// store commands and `reconcile`. Returns whether one ran.
pub async fn operator_command(installation: &installation::Installation) -> Result<bool> {
    Ok(copies::history::operator_command().await?
        || copies::operator_command(installation).await?
        || file_store::operator_command().await?
        || recovery::operator_command(installation).await?
        || recovery::drill_check(installation).await?)
}

/// The client and bucket of copies and the revocation record, or `None`
/// when no S3 endpoint is set.
pub fn copy_bucket_from_env() -> Result<Option<(S3Client, String)>> {
    Ok(file_store::S3Settings::copies(&file_store::env_var)?.map(|settings| (file_store::s3_client(&settings), settings.bucket)))
}

pub fn backup_dir(state: &AppState, id: &str) -> PathBuf {
    layout::backups(&state.data_root, id)
}

pub async fn invoke_action(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Deployment not found."),
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "actions") {
        return refusal_error(&refused);
    }
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
    // An action whose route does not answer anyone answers only there.
    if let Some(route) = http::route_only(&name, action, is_private(&deployment)) {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} answers only on its HTTP route, {route}."));
    }
    // A call's then mutation records what the call's answer said, so no one
    // else may write it.
    if let Some(call) = calls::then_of(&program, &name) {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} runs only after the call {call} answers."));
    }
    // A scheduled mutation with no route belongs to the Kernel's timer

    if action.schedule.is_some() && action.http.is_none() {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} runs only on its schedule."));
    }
    let actor = match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if action.access == "granted" {
        return error(StatusCode::FORBIDDEN, "action-grant-required");
    }

    if (action.access != "anonymous" || is_private(&deployment)) && actor.is_none() {
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

    if action.kind == "call" {
        let response = calls::enqueue(&state, &deployment, &name, &input).await;
        if response.status().is_success() && !enter::entered(actor.as_ref()) {
            if let Some(user) = actor.as_ref() {
                usage::count_account(&state.data_root, &deployment.id, &user.user_id, usage::AccountKind::Action);
            }
        }
        return response;
    }
    let db_path = crate::layout::database(&state
        .data_root, &deployment.id);


    copies::operation::action(&state, &deployment.id, &name, action, async {
        let result = if action.kind == "signal" {
            Ok(Ran { answer: json!({"recorded": true}), rows: Vec::new() })
        } else {
            let _guard = action_lock(&state, &deployment.id, &action.kind).await;
            match state.guards.statement_permit(&deployment.id).await {
                Ok(permit) => run_action_in_slot(db_path, action, params, permit).await,
                Err(wait) => return http::rate_limited(wait),
            }
        };
        let actor_id = actor.as_ref().map(|user| user.user_id.as_str());

        let counted = !enter::entered(actor.as_ref());
        match result {
            Ok(ran) => match guard::outcome(&name, action, &ran.answer) {
                guard::Outcome::Refused(message) => {
                    if counted {
                        usage::execution(&state.data_root, &deployment.id, &name, "runtime", "refused");
                        usage::count(&state.data_root, &deployment.id, usage::Operation::Refused(name.clone()));
                    }
                    error(StatusCode::FORBIDDEN, &message)
                }
                changed => {
                    if counted {
                        usage::execution(&state.data_root, &deployment.id, &name, "runtime", "ok");
                        usage::count(&state.data_root, &deployment.id, usage::Operation::Action(name.clone()));
                        if let Some(account) = actor_id {
                            usage::count_account(&state.data_root, &deployment.id, account, usage::AccountKind::Action);
                        }
                    }
                    if matches!(changed, guard::Outcome::Changed) {
                        if counted {
                            usage::signal(&state.data_root, &deployment.id, &name, action, &input, actor_id);
                        }
                        let caller = notify::Caller { account: actor_id, actor_id };
                        notify::record(&state, &deployment, &name, action, &input, caller, ran.changed(now_ms())).await;
                    }
                    Json(json!({"result": ran.answer})).into_response()
                }
            },
            Err(error_value) => {
                if counted {
                    usage::execution(&state.data_root, &deployment.id, &name, "runtime", "failed");
                }
                error(StatusCode::BAD_REQUEST, &error_value.to_string())
            },
        }
    }).await
}

/// Runs one operation of the owner's database door on a Deployment: inspect,
/// browse, SQL, row edits and backups. `body` is its JSON input.
pub async fn admin_database(state: &AppState, id: &str, body: &[u8]) -> Response {
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
    if body.len() > 200_000 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "Database request is too large.");
    }
    let body: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Database request must be valid JSON.",
            );
        }
    };
    let operation = body.get("operation").and_then(Value::as_str).unwrap_or("");
    if !matches!(operation, "list-backups" | "restore-backup") {
        if let Err(refused) = modules::require_on(&deployment.modules, "actions") { return refusal_error(&refused); }
    }
    let _project_guard = database_lock(&state, &format!("project:{}", deployment.project_id)).await;
    let _release_guard = named_lock(state, &format!("publish:{}", deployment.id), operation == "restore-backup").await;
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
        _ => error(StatusCode::BAD_REQUEST, "Unknown database operation."),
    }
}

pub fn database_path(state: &AppState, id: &str) -> PathBuf {
    crate::layout::database(&state
        .data_root, &id)
}

/// Named locks on one database or transfer. Each is a read-write lock: a
/// Server Action shares its Database's lock, and work that reads or replaces
/// the whole file (a copy, a restore, an import, the owner's console) holds
/// it alone. An entry lives only while someone holds or waits for it, so
/// callers that pass many distinct names cannot grow the map.
pub type LockMap = Arc<std::sync::Mutex<HashMap<String, Arc<RwLock<()>>>>>;


#[allow(dead_code)]
pub enum Held {
    Shared(OwnedRwLockReadGuard<()>),
    Alone(OwnedRwLockWriteGuard<()>),
}

/// Holds one named lock. Dropping it releases the lock and removes the map
/// entry when no other task holds or waits for the same name.
pub struct NamedLockGuard {
    pub guard: Option<Held>,
    pub entry: Option<Arc<RwLock<()>>>,
    pub name: String,
    pub locks: LockMap,
}

impl Drop for NamedLockGuard {
    fn drop(&mut self) {
        let mut locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());


        self.guard.take();
        let entry = self.entry.take();


        if entry.is_some_and(|entry| Arc::strong_count(&entry) == 2) {
            locks.remove(&self.name);
        }
    }
}

pub async fn named_lock(state: &AppState, name: &str, alone: bool) -> NamedLockGuard {
    let entry = {
        let mut locks = state.db_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    };


    let mut held = NamedLockGuard {
        guard: None,
        entry: Some(entry.clone()),
        name: name.to_owned(),
        locks: state.db_locks.clone(),
    };
    held.guard = Some(if alone {
        Held::Alone(entry.write_owned().await)
    } else {
        Held::Shared(entry.read_owned().await)
    });
    held
}

/// Holds the named lock alone: no Server Action runs on that Database, and
/// no one else holds the name, until the guard drops.
pub async fn database_lock(state: &AppState, id: &str) -> NamedLockGuard {
    let held = named_lock(state, id, true).await;


    app_db::forget(&database_path(state, id));
    held
}

pub fn inspect_database(path: &Path) -> Result<Vec<Value>> {
    let db = Connection::open(path)?;
    let mut statement = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND lower(name) NOT IN ('__konae_schema_state','__konae_schema_history','__konae_function_receipts') ORDER BY name")?;
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

pub fn table_columns(db: &Connection, table: &str) -> Result<Vec<Value>> {
    anyhow::ensure!(!schema_transform::is_internal_table(table), "Reserved Kernel table.");
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

pub fn table_exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE lower(type)='table' AND lower(name)=lower(?1)",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn browse_table(path: &Path, body: &Value) -> Result<Value> {
    let db = Connection::open(path)?;
    restrict_database(&db)?;
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
        "SELECT rowid AS __konae_rowid, * FROM {} ORDER BY {sort} {direction} LIMIT ?1 OFFSET ?2",
        quote_identifier(table)
    );
    let mut statement = db.prepare(&sql)?;
    let col_names = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut budget = ResultBudget::default();
    let metadata_bytes = columns.iter().fold(128usize, |bytes, column| {
        bytes.saturating_add(browse_column_json_cost(column))
    });
    anyhow::ensure!(budget.reserve(metadata_bytes), "{QUERY_RESULT_TOO_LARGE}");
    let rows = bounded_rows(statement
        .query_map([page_size, page.saturating_mul(page_size)], |row| {
            let mut values = serde_json::Map::new();
            let mut row_id = Value::Null;
            let mut first_value = true;
            if !budget.begin_row(22) {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
            }
            for (i, name) in col_names.iter().enumerate() {
                if name == "__konae_rowid" {
                    let value = row.get_ref(i)?;
                    if !budget.spend_value(value) {
                        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
                    }
                    row_id = sql_json(value);
                } else {
                    let value = row.get_ref(i)?;
                    if !budget.spend_column(name, value, first_value) {
                        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
                    }
                    first_value = false;
                    values.insert(name.clone(), sql_json(value));
                }
            }
            Ok(json!({"rowId": row_id, "values": values}))
        })?)?;
    let count_sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(table));
    let total: i64 = db.query_row(&count_sql, [], |row| row.get(0))?;
    Ok(
        json!({"columns": columns, "rows": rows, "totalRows": total, "page": page, "pageSize": page_size}),
    )
}

pub fn run_admin_sql(path: &Path, sql: &str) -> Result<Value> {
    if sql.trim().is_empty() {
        anyhow::bail!("Write a SQL statement first.");
    }
    if sql.len() > 100_000 {
        anyhow::bail!("SQL script is too large.");
    }
    let tokens = sql_tokens(sql);
    if tokens.windows(3).any(|tokens| tokens[0].is_word("RENAME") && tokens[1].is_word("TO") && matches!(tokens[2].kind, SqlTokKind::Word | SqlTokKind::Ident) && schema_transform::is_internal_table(&tokens[2].text)) {
        anyhow::bail!("SQL cannot rename a table to a reserved Kernel table.");
    }
    let db = Connection::open(path)?;
    restrict_database(&db)?;
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
        let mut result_bytes = 0;
        let mut budget = ResultBudget::default();
        while let Some(row) = rows.next()? {
            let Some(record) = bounded_row(row, &names, &mut budget)? else {
                anyhow::bail!("SQL result exceeds 2 MiB.");
            };
            values.push(record);
            result_bytes += serde_json::to_vec(values.last().unwrap())?.len();
            if result_bytes > 2 * 1024 * 1024 {
                anyhow::bail!("SQL result exceeds 2 MiB.");
            }
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

pub fn admin_update_cell(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
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

pub fn admin_insert_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
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

pub fn admin_delete_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
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

pub fn value_to_sql(value: Option<&Value>) -> Result<SqlValue> {
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

pub async fn list_backups(state: &AppState, id: &str) -> Result<Vec<Value>> {
    let dir = backup_dir(state, id);
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
        backups.push(backup.view());
    }
    backups.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
    Ok(backups)
}

pub async fn create_backup(state: &AppState, id: &str, label: &str) -> Result<Value> {
    let db_path = database_path(state, id);
    let dir = backup_dir(state, id);
    tokio::fs::create_dir_all(&dir).await?;
    let backup_id = Uuid::new_v4().to_string();
    let snapshot_path = dir.join(format!("{backup_id}.sqlite"));
    copies::vacuum_into(&db_path, &snapshot_path)?;
    let size = tokio::fs::metadata(&snapshot_path).await?.len();
    let backup = DatabaseBackup {
        id: backup_id.clone(),
        label: label.chars().take(120).collect(),
        created_at: now_ms(),
        size_bytes: size,
        key: format!("{backup_id}.sqlite"),
        holds: Vec::new(),
    };
    tokio::fs::write(
        dir.join(format!("{backup_id}.json")),
        serde_json::to_vec(&backup)?,
    )
    .await?;
    Ok(backup.view())
}

pub async fn restore_backup(state: &AppState, id: &str, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }

    let directory = backup_dir(state, id);
    let record = copies::history::checked_record(&directory, backup_id)?;
    if !record.holds.is_empty() {
        let deployment = state.deployments.read().await.values().find(|d| d.id == id).cloned().context("Deployment not found.")?;
        return import::restore_held(state, &deployment, &record).await;
    }
    let backup_path = backup_dir(state, id).join(format!("{backup_id}.sqlite"));
    let bytes = tokio::fs::read(&backup_path).await?;
    create_backup(state, id, "Before restore").await?;
    let temp_path = database_path(state, id).with_extension("restore.sqlite");
    tokio::fs::write(&temp_path, &bytes).await?;
    let check = Connection::open(&temp_path)?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    drop(check);
    if integrity != "ok" {
        let _ = tokio::fs::remove_file(&temp_path).await;
        anyhow::bail!("The backup failed SQLite's integrity check.");
    }
    let _generation = state.copies.generations.read().await;
    replace_database_file(&temp_path, &database_path(state, id))?;
    Ok(json!({"id":backup_id,"sizeBytes":bytes.len()}))
}

/// Writes a sibling temp file and renames it over `path`, so readers never see
/// a truncated file.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let temp = path.with_file_name(format!(".{name}.tmp-{}", Uuid::new_v4()));
    if let Err(error_value) = tokio::fs::write(&temp, bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    if let Err(error_value) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    Ok(())
}

pub fn accepted_owner_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn encode_base64(bytes: &[u8]) -> String {
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

pub fn decode_base64(value: &str) -> Result<Vec<u8>> {
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

pub use konae_backup::safe_relative_path;

/// Persists changes made to the applied Deployment while keeping editable settings intact.
pub use konae_backup::read_s3_bytes;

pub use konae_backup::list_s3_keys;

pub const DEFAULT_SETUP_KEY: &str = "local-development-key";

/// True for localhost, *.localhost and loopback IP addresses.
pub fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}


pub fn base64url(value: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() % 4 != 1
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid base64url encoding."
    );
    let bytes = crate::decode_base64(value)?;
    let canonical = crate::encode_base64(&bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_");
    anyhow::ensure!(canonical == value, "Noncanonical base64url encoding.");
    Ok(bytes)
}


/// The name this process was started as, `kernel` or `konae`, for the
/// commands an answer tells the operator to run.
pub fn program() -> String {
    std::env::args()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "kernel".into())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

/// A refused Capsule: `error` stays the sentence clients read, and `code` and
/// `hint` sit beside it (ADR-0004).
pub fn refusal_error(refused: &refusal::Refusal) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": refused.message, "code": refused.code, "hint": refused.hint()}))).into_response()
}

#[cfg(any(test, feature = "testing"))]
#[allow(dead_code, unused_imports)]
pub mod tests {
    use super::*;
    use crate::files::{list_files, serve_file, upload_file};

    #[test]
    fn the_transformation_journal_is_hidden_and_owner_sql_cannot_rewrite_it() {
        let root = std::env::temp_dir().join(format!("konae-schema-journal-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY); CREATE TABLE __konae_schema_state(format_version INTEGER NOT NULL); CREATE TABLE __konae_schema_history(id TEXT);").unwrap();
        drop(db);
        let tables = inspect_database(&path).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0]["name"], "notes");
        assert!(run_admin_sql(&path, "SELECT * FROM __konae_schema_state").is_err());
        assert!(run_admin_sql(&path, "CREATE TABLE __konae_schema_state(id INTEGER)").is_err());
        assert!(run_admin_sql(&path, "DROP TABLE __konae_schema_history").is_err());
        assert!(run_admin_sql(&path, "ALTER TABLE notes RENAME TO __konae_schema_state").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deploy_input_reads_the_initial_signup_policy() {
        let input: DeployInput = serde_json::from_value(json!({
            "project": { "id": "app_1", "name": "App", "slug": "app", "files": {}, "manifest": {} },
            "visibility": "public",
            "artifact": { "version": 1, "entrypoint": "index.html", "files": { "index.html": "<main></main>" } },
            "initialSignupPolicy": { "mode": "closed", "allowlist": [] }
        })).unwrap();

        assert_eq!(input.initial_signup_policy.unwrap().mode, SignupMode::Closed);
    }

    #[tokio::test]
    async fn a_release_with_a_missing_page_template_uses_the_shared_case() {
        let cases: Value = serde_json::from_str(include_str!("page-release-cases.json")).unwrap();
        let case = &cases[0];
        let root = std::env::temp_dir().join(format!("konae-release-page-file-{}", Uuid::new_v4()));
        let state = test_state(&root);
        let body = release_body(
            "app_missing_page_file",
            "CREATE TABLE IF NOT EXISTS posts(id INTEGER PRIMARY KEY, title TEXT NOT NULL);",
            json!({
                "posts.head": {
                    "kind": "query", "access": "anonymous",
                    "statement": "SELECT title FROM posts LIMIT 10", "bindings": [],
                    "page": {"path": "/posts", "file": "post.html"}
                }
            }),
            "<main>posts</main>",
            None,
        );
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let text = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains(case["error"].as_str().unwrap()), "{text}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_stored_deployment_on_the_editor_host_does_not_capture_the_owner_door() {
        let root = std::env::temp_dir().join(format!("konae-editor-host-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
        let mut deployment = state.deployments.read().await.values().next().cloned().unwrap();
        deployment.hostname = "app.apps.example.com".into();
        let state = AppState {
            public_origin: "http://app.apps.example.com".into(),
            host_suffix: "apps.example.com".into(),
            deployments: Arc::new(RwLock::new(HashMap::from([(deployment.hostname.clone(), deployment)]))),
            ..state
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("app.apps.example.com"));
        assert!(deployment_for_host(&state, &headers).await.is_none());
        assert!(deployment_for(&state, &headers).await.is_none());
        headers.insert(header::HOST, HeaderValue::from_static("app.apps.example.com:8080"));
        assert!(deployment_for_host(&state, &headers).await.is_none());
        // A page on the editor host is not an app origin either.
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://app.apps.example.com"));
        assert!(!security::from_app_origin(&state, &headers).await);
        let _ = std::fs::remove_dir_all(root);
    }

    pub async fn login_test_state(root: &Path, visibility: &str) -> (AppState, HeaderMap) {
        let app_id = Uuid::new_v4().to_string();
        let deployment = Deployment {
            id: app_id.clone(),
            project_id: app_id,
            project_name: "Login test".into(),
            slug: "app".into(),
            url: "http://login-test.localhost".into(),
            created_at: now_ms(),
            released_at: 0,
            source_digest: None,
            manifest: json!({"visibility": visibility}),
            hostname: "login-test.localhost".into(),
            actions: "{}".into(),
            artifact_version: "v1".into(),
            signup_policy: SignupPolicy::default(),
            sign_in_methods: default_sign_in_methods(),
            contact_email: None,
            modules: {
                let mut modules = modules::auth_enabled_defaults();
                modules.get_mut("actions").unwrap()["on"] = json!(true);
                modules.get_mut("files").unwrap()["on"] = json!(true);
                modules
            },
            released_schema: String::new(),
            server: None,
        };
        tokio::fs::create_dir_all(crate::layout::data(&root, &deployment.id))
            .await
            .unwrap();
        let state = AppState {
            deployments: Arc::new(RwLock::new(HashMap::from([(
                deployment.hostname.clone(),
                deployment,
            )]))),
            ..AppState::for_test(&root)
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
            http: None,
            page: None,
            signal: None,
            guard: None,
            notify: None,
            schedule: None,
            subscribe: None,
            call: Default::default(),
            extra: Default::default(),
        }
    }

    fn apply_schema(path: &Path, schema: &str, program: &ActionProgram) -> Result<schema_change::Change> {
        schema_change::apply(path, schema, program, schema_change::Mode::Apply(None))
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
        assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks", vec![]))
            .unwrap_err()
            .message
            .contains("reads rows and has no LIMIT"));
        assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks WHERE title = 'LIMIT 5'", vec![])).is_err());
        assert!(validate_action("tasks.count", &action("query", "SELECT count(*) AS n FROM tasks", vec![])).is_ok());
        assert!(validate_action("votes.results", &action("query", "SELECT choice, count(*) AS n FROM votes GROUP BY choice", vec![])).is_err());
        assert!(validate_action("tasks.one", &action("query", "SELECT 1", vec![])).is_ok());
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
                nullable: false,
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
                .join(format!("konae-login-{visibility}-{}", Uuid::new_v4()));
            let (state, headers) = login_test_state(&root, visibility).await;
            let response = login_page(State(state), headers).await;
            assert_eq!(response.status(), StatusCode::OK, "visibility={visibility}");
            let bytes = axum::body::to_bytes(response.into_body(), 16_384)
                .await
                .unwrap();
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(html.contains("<h1>Sign in</h1>") && html.contains(r#"<p class="kicker">"#));
            assert!(html.contains("/_konae/auth/signup"));
            assert!(!html.contains("Account creation is open"));
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    async fn set_policy(state: &AppState, mode: SignupMode, allowlist: &[&str]) {
        let entries = allowlist.iter().map(|e| e.to_string()).collect::<Vec<_>>();
        for deployment in state.deployments.write().await.values_mut() {
            deployment.signup_policy = SignupPolicy::from_input(mode, &entries).unwrap();
        }
    }

    async fn signup_status(state: &AppState, headers: &HeaderMap, email: &str) -> StatusCode {
        let body = json!({"email": email, "password": "correct horse battery"});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        signup(State(state.clone()), headers.clone(), request).await.status()
    }

    #[test]
    fn signup_policy_matches_exact_emails_and_whole_domains() {
        let open = SignupPolicy::default();
        assert!(open.allows("anyone@example.test"));
        let closed = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        assert!(!closed.allows("anyone@example.test"));
        let invitation: SignupPolicy = serde_json::from_value(json!({"mode": "invitation"})).unwrap();
        assert!(!invitation.allows("anyone@example.test"), "Invitation only refuses every plain signup");
        let list = SignupPolicy::from_input(
            SignupMode::Allowlist,
            &[" Carol@Example.TEST ".into(), "@Team.Example".into(), "carol@example.test".into()],
        )
        .unwrap();
        assert_eq!(list.allowlist, vec!["carol@example.test", "@team.example"]);
        assert!(list.allows("CAROL@example.test"));
        assert!(list.allows("dana@TEAM.example"));
        assert!(!list.allows("dana@sub.team.example"));
        assert!(!list.allows("dana@evilteam.example"));
        assert!(!list.allows("team.example@attacker.test"));
        assert!(!list.allows("erin@example.test"));
        assert!(!list.allows("not-an-email"));
        let empty = SignupPolicy::from_input(SignupMode::Allowlist, &[]).unwrap();
        assert!(!empty.allows("carol@example.test"));
        for invalid in ["@", "@nodot", "two@@example.test", "@bad domain.test", "plain"] {
            assert!(
                SignupPolicy::from_input(SignupMode::Allowlist, &[invalid.into()]).is_err(),
                "entry={invalid}"
            );
        }
        let too_many = (0..=MAX_ALLOWLIST_ENTRIES)
            .map(|i| format!("user{i}@example.test"))
            .collect::<Vec<_>>();
        assert!(SignupPolicy::from_input(SignupMode::Allowlist, &too_many).is_err());
    }

    #[tokio::test]
    async fn signup_follows_the_deployment_policy() {
        let root = std::env::temp_dir().join(format!("konae-signup-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        assert_eq!(signup_status(&state, &headers, "open@example.test").await, StatusCode::OK);

        set_policy(&state, SignupMode::Closed, &[]).await;
        assert_eq!(signup_status(&state, &headers, "closed@example.test").await, StatusCode::FORBIDDEN);

        set_policy(&state, SignupMode::Allowlist, &["Listed@Example.test", "@team.example"]).await;
        assert_eq!(signup_status(&state, &headers, "listed@EXAMPLE.test").await, StatusCode::OK);
        assert_eq!(signup_status(&state, &headers, "any@team.example").await, StatusCode::OK);
        assert_eq!(signup_status(&state, &headers, "other@example.test").await, StatusCode::FORBIDDEN);
        assert_eq!(signup_status(&state, &headers, "any@sub.team.example").await, StatusCode::FORBIDDEN);

        // Existing accounts still sign in when signup is closed.
        set_policy(&state, SignupMode::Closed, &[]).await;
        let body = json!({"email": "open@example.test", "password": "correct horse battery"});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        assert_eq!(login(State(state.clone()), headers.clone(), request).await.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn operator_policy_updates_keep_saved_source_settings_across_restart() {
        let root = std::env::temp_dir().join(format!("konae-policy-preserves-draft-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
        let mut current = state.deployments.read().await.values().next().unwrap().clone();
        current.modules.get_mut("auth").unwrap()["on"] = json!(true);
        current.modules.get_mut("files").unwrap()["on"] = json!(true);
        current.actions = "{\"version\":1,\"actions\":{}}".into();
        state.deployments.write().await.insert(current.hostname.clone(), current.clone());
        Settings::of(&current).write(&root).await.unwrap();
        let draft = json!({"id":current.id,"name":"Unpublished draft name","slug":"app","files":{"index.html":"<main>app</main>","actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""},"directories":[],"manifest":{"version":1,"visibility":"private","fileAccess":"app-users"},"updatedAt":now_ms()});
        crate::sources::write(&root, &current.id, &draft).await.unwrap();
        tokio::fs::create_dir_all(crate::layout::dist(&root, &current.id)).await.unwrap();
        tokio::fs::write(crate::layout::dist(&root, &current.id).join("index.html"), "<main>deployed</main>").await.unwrap();

        set_signup_policy(&state, &current.id, SignupMode::Closed, None).await.unwrap();
        let saved = crate::sources::read(&root, &current.id).await.unwrap().unwrap();
        assert_eq!(saved["name"], "Unpublished draft name");
        assert_eq!(saved["manifest"]["visibility"], "private");
        assert!(saved["manifest"].get("fileAccess").is_none());

        let restarted = AppState::for_test(&root);
        let serving = folder::open(&restarted, &current.id, None).await.unwrap();
        assert_eq!(serving.project_name, current.project_name);
        assert_eq!(serving.manifest["visibility"], "public");
        assert_eq!(serving.modules["files"]["on"], true);
        assert!(serving.manifest.get("fileAccess").is_none());
        assert_eq!(serving.signup_policy.mode, SignupMode::Closed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn login_page_hides_account_creation_when_signup_is_closed() {
        let root = std::env::temp_dir().join(format!("konae-login-policy-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "private").await;
        let page = |state: AppState| {
            let headers = headers.clone();
            async move {
                let response = login_page(State(state), headers).await;
                let bytes = axum::body::to_bytes(response.into_body(), 16_384).await.unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };
        set_policy(&state, SignupMode::Closed, &[]).await;
        let html = page(state.clone()).await;
        assert!(!html.contains("/_konae/auth/signup"));
        assert!(html.contains("not accepting new accounts"));
        set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
        let html = page(state.clone()).await;
        assert!(html.contains("/_konae/auth/signup"));
        assert!(html.contains("Only invited email addresses"));
        set_policy(&state, SignupMode::Invitation, &[]).await;
        let html = page(state.clone()).await;
        assert!(!html.contains("/_konae/auth/signup"));
        assert!(html.contains("Only invited people can create an account"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn private_apps_refuse_anonymous_actions_without_a_session() {
        let root = std::env::temp_dir().join(format!("konae-private-action-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "private").await;
        let program = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n", "bindings": []}}});
        for deployment in state.deployments.write().await.values_mut() {
            deployment.actions = program.to_string();
            initialize_database(&database_path(&state, &deployment.id)).unwrap();
        }
        let call = |state: AppState| {
            let headers = headers.clone();
            async move {
                invoke_action(State(state), AxumPath("items.count".into()), headers, Json(json!({})))
                    .await
                    .status()
            }
        };
        assert_eq!(call(state.clone()).await, StatusCode::UNAUTHORIZED);
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest = json!({"visibility": "public"});
        }
        assert_eq!(call(state.clone()).await, StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_deployment_that_does_not_say_public_serves_no_asset_without_a_login() {
        let root = std::env::temp_dir().join(format!("konae-fail-closed-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let id = state.deployments.read().await.values().next().unwrap().id.clone();
        let artifact = layout::dist(&state.data_root, &id);
        tokio::fs::create_dir_all(&artifact).await.unwrap();
        tokio::fs::write(artifact.join("report.txt"), "secret").await.unwrap();
        let read = |manifest: Value| {
            let (state, headers) = (state.clone(), headers.clone());
            async move {
                for deployment in state.deployments.write().await.values_mut() {
                    deployment.manifest = manifest.clone();
                }
                let response = serve_key(state, headers, "report.txt".into()).await;
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
                String::from_utf8_lossy(&bytes).into_owned()
            }
        };
        assert_eq!(read(json!({"visibility": "public"})).await, "secret");
        for manifest in [json!({}), json!({"visibility": null}), json!({"visibility": "Public"}), json!({"visibility": "open"})] {
            let deployment = Deployment { manifest: manifest.clone(), ..state.deployments.read().await.values().next().unwrap().clone() };
            assert!(is_private(&deployment), "{manifest}");
            let body = read(manifest.clone()).await;
            assert!(!body.contains("secret") && body.contains("<form"), "{manifest}");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_scheduled_mutation_without_a_route_runs_only_on_its_schedule() {
        let root = std::env::temp_dir().join(format!("konae-scheduled-action-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let program = json!({"version": 1, "actions": {"holds.release": {
            "kind": "mutation", "access": "anonymous", "schedule": {"every": "15m"},
            "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL", "bindings": [{"source": "kernel", "field": "now"}]
        }}});
        for deployment in state.deployments.write().await.values_mut() {
            deployment.actions = program.to_string();
            deployment.manifest = json!({"visibility": "public"});
        }
        let response = invoke_action(State(state), AxumPath("holds.release".into()), headers, Json(json!({}))).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Action holds.release runs only on its schedule."));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn redeploy_adds_tables_without_losing_rows_and_rejects_destructive_schema() {
        let root = std::env::temp_dir().join(format!("konae-schema-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();

        let initial = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let original_action = action_program(&[("tasks.list", "SELECT id, title FROM tasks")]);
        apply_schema(&path, initial, &original_action).unwrap();
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
        apply_schema(&path, &extended, &extended_actions).unwrap();
        apply_schema(&path, &extended, &extended_actions).unwrap();

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

        assert!(apply_schema(&path, "DROP TABLE tasks;", &extended_actions).is_err());
        let connection = Connection::open(&path).unwrap();
        let title: String = connection
            .query_row("SELECT title FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Keep this row");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn schemas_may_call_safe_functions_but_not_dangerous_ones() {
        let root = std::env::temp_dir().join(format!("konae-schema-functions-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS items (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL CHECK (length(name) > 0),
              title TEXT NOT NULL DEFAULT '',
              slug TEXT GENERATED ALWAYS AS (lower(name)) VIRTUAL,
              created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS items_title ON items(lower(title));
            CREATE INDEX IF NOT EXISTS items_named ON items(name) WHERE length(name) > 0;";
        let actions = action_program(&[("items.list", "SELECT id, name, slug FROM items")]);
        apply_schema(&path, schema, &actions).unwrap();
        let connection = Connection::open(&path).unwrap();
        assert!(connection.execute("INSERT INTO items(name) VALUES ('')", []).is_err(), "the CHECK constraint runs");
        connection.execute("INSERT INTO items(name) VALUES ('Mixed')", []).unwrap();
        let slug: String = connection.query_row("SELECT slug FROM items", [], |row| row.get(0)).unwrap();
        assert_eq!(slug, "mixed");
        drop(connection);

        for function in DENIED_FUNCTIONS {
            let schema = format!(
                "CREATE TABLE IF NOT EXISTS evil_{function} (id INTEGER PRIMARY KEY, path TEXT CHECK ({function}(path) IS NULL));"
            );
            let error = apply_schema(&path, &schema, &actions).unwrap_err().to_string();
            assert_eq!(error, format!("schema.sql may only create tables and indexes, but it contains {function}()."), "{function}");
        }
        let evil = format!("{schema} CREATE INDEX IF NOT EXISTS evil_index ON items(load_extension(name));");
        let error = apply_schema(&path, &evil, &actions).unwrap_err().to_string();
        assert_eq!(error, "schema.sql may only create tables and indexes, but it contains load_extension().");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_schema_value_and_an_action_pattern_are_bounded_and_no_trigger_or_view_gets_in() {
        let root = std::env::temp_dir().join(format!("konae-sql-bounds-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let actions = action_program(&[("items.list", "SELECT id FROM items LIMIT 5")]);
        apply_schema(&path, "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT);", &actions).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection.execute("INSERT INTO items(name) VALUES ('a')", []).unwrap();
        drop(connection);
        // An index whose expression builds a 200 MB value per row hits the
        // value limit instead of the memory of the Kernel.
        let heavy = "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT); CREATE INDEX IF NOT EXISTS heavy ON items(length(hex(zeroblob(200000000))));";
        let error = apply_schema(&path, heavy, &actions).unwrap_err().to_string();
        assert!(error.contains("too big"), "{error}");

        let db = Connection::open(&path).unwrap();
        restrict_database(&db).unwrap();
        let pattern = |length: usize| format!("SELECT 'x' LIKE '{}'", "%".repeat(length));
        db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize), [], |row| row.get::<_, bool>(0)).unwrap();
        let long = db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize + 1), [], |row| row.get::<_, bool>(0)).unwrap_err().to_string();
        assert!(long.contains("pattern too complex"), "{long}");
        // Studio and the schema change open this connection too.
        for statement in ["CREATE TRIGGER t AFTER INSERT ON items BEGIN DELETE FROM items; END", "CREATE VIEW v AS SELECT 1",
            "CREATE TEMP TRIGGER t AFTER INSERT ON items BEGIN SELECT 1; END", "CREATE TEMP VIEW v AS SELECT 1"] {
            assert!(db.execute_batch(statement).unwrap_err().to_string().contains("not authorized"), "{statement}");
        }
        drop(db);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn named_locks_leave_no_entry_once_released() {
        let root = std::env::temp_dir().join(format!("konae-locks-{}", Uuid::new_v4()));
        let state = test_state(&root);
        let count = |state: &AppState| state.db_locks.lock().unwrap().len();
        for i in 0..100 {
            drop(database_lock(&state, &format!("transfer:{i}")).await);
        }
        assert_eq!(count(&state), 0);

        let first = database_lock(&state, "shared").await;
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move {
                let _second = database_lock(&state, "shared").await;
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(count(&state), 1);
        drop(first);
        waiter.await.unwrap();
        assert_eq!(count(&state), 0, "the last holder removes the entry");

        // A caller that gives up while waiting removes the entry too.
        let held = database_lock(&state, "contended").await;
        let gave_up = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            database_lock(&state, "contended"),
        )
        .await;
        assert!(gave_up.is_err());
        assert_eq!(count(&state), 1, "the holder keeps its entry");
        drop(held);
        assert_eq!(count(&state), 0);
    }

    #[test]
    fn redeploy_rolls_back_schema_when_an_action_does_not_prepare() {
        let root = std::env::temp_dir().join(format!("konae-schema-action-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let invalid_action = action_program(&[("tasks.list", "SELECT missing FROM tasks")]);
        assert!(
            apply_schema(
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

    fn test_state(root: &Path) -> AppState {
        AppState::for_test(&root)
    }

    fn input_binding(key: &str, value_type: &str, max_length: Option<usize>) -> ActionBinding {
        ActionBinding {
            source: "input".into(),
            key: Some(key.into()),
            value_type: Some(value_type.into()),
            max_length,
            nullable: false,
            field: None,
            value: None,
        }
    }

    #[test]
    fn nullable_inputs_preserve_types_and_required_inputs() {
        for value_type in ["string", "number", "boolean"] {
            let mut binding = input_binding("value", value_type, Some(4));
            binding.nullable = true;
            let definition = action("query", "SELECT ?1", vec![binding]);
            assert!(validate_action("inputs.echo", &definition).is_ok());
            for input in [json!({}), json!({"value": null})] {
                assert_eq!(resolve_bindings(&definition, &input, None).unwrap(), vec![SqlValue::Null]);
            }
            assert!(resolve_bindings(&definition, &json!({"other": null}), None).unwrap_err().contains("Unexpected"));
            let wrong = match value_type {
                "string" => json!({"value": 5}),
                "number" => json!({"value": "5"}),
                _ => json!({"value": 0}),
            };
            assert!(resolve_bindings(&definition, &wrong, None).is_err());
            let required = action("query", "SELECT ?1", vec![input_binding("value", value_type, Some(4))]);
            assert!(resolve_bindings(&required, &json!({}), None).is_err());
            assert!(resolve_bindings(&required, &json!({"value": null}), None).is_err());
        }
        let mut binding = input_binding("value", "string", Some(4));
        binding.nullable = true;
        let definition = action("query", "SELECT ?1", vec![binding]);
        assert!(resolve_bindings(&definition, &json!({"value": ""}), None).is_err());
        assert!(resolve_bindings(&definition, &json!({"value": "longer"}), None).is_err());
        assert_eq!(resolve_bindings(&definition, &json!({"value": "ship"}), None).unwrap(), vec![SqlValue::Text("ship".into())]);
        for source in ["actor", "kernel", "literal"] {
            let mut binding = input_binding("value", "string", None);
            binding.source = source.into();
            binding.nullable = true;
            assert!(validate_action("inputs.echo", &action("query", "SELECT ?1", vec![binding])).is_err());
        }
    }

    #[test]
    fn mutation_returning_fails_schema_validation_without_executing() {
        let root = std::env::temp_dir().join(format!("konae-returning-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT, \"RETURNING\" INTEGER);";
        for sql in [
            "INSERT INTO notes(body) VALUES ('note') RETURNING id",
            "UPDATE notes SET body='note' returning body",
            "DELETE FROM notes RETURNING *",
        ] {
            let program = ActionProgram { version: 1, actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]) };
            assert!(apply_schema(&path, schema, &program).unwrap_err().to_string().contains("cannot use RETURNING"));
        }
        for sql in [
            "INSERT INTO notes(body) VALUES ('RETURNING')",
            "UPDATE notes SET body='note' /* RETURNING id */",
            "UPDATE notes SET \"RETURNING\"=1",
        ] {
            let program = ActionProgram { version: 1, actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]) };
            apply_schema(&path, schema, &program).unwrap();
        }
        let count: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(root);
    }




    #[test]
    fn artifact_content_types_follow_the_extension() {
        for (path, expected) in [
            ("index.html", "text/html; charset=utf-8"),
            ("docs/page.HTM", "text/html; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            ("chunk.mjs", "text/javascript; charset=utf-8"),
            ("styles.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("app.js.map", "application/json"),
            ("site.webmanifest", "application/manifest+json"),
            ("robots.txt", "text/plain; charset=utf-8"),
            ("logo.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.JPEG", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("favicon.ico", "image/x-icon"),
            ("module.wasm", "application/wasm"),
            ("font.woff2", "font/woff2"),
            ("font.woff", "font/woff"),
            ("font.ttf", "font/ttf"),
            ("font.otf", "font/otf"),
            ("sitemap.xml", "application/xml; charset=utf-8"),
            ("feed.rss", "application/rss+xml; charset=utf-8"),
            ("feed.atom", "application/atom+xml; charset=utf-8"),
            ("notes.md", "text/markdown; charset=utf-8"),
            ("rows.csv", "text/csv; charset=utf-8"),
            ("photo.avif", "image/avif"),
            ("menu.pdf", "application/pdf"),
            ("archive.zip", "application/octet-stream"),
            ("LICENSE", "application/octet-stream"),
            ("dir.html/file", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), expected, "path={path}");
        }
        let response = artifact_response(b"x".to_vec(), "x.bin");
        assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/octet-stream");
    }

    #[test]
    fn number_inputs_bind_integers_as_integer_and_fractions_as_real() {
        let definition = action(
            "mutation",
            "INSERT INTO items(label) VALUES (?)",
            vec![input_binding("value", "number", None)],
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": 5}), None).unwrap(),
            vec![SqlValue::Integer(5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": -3}), None).unwrap(),
            vec![SqlValue::Integer(-3)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": 5.5}), None).unwrap(),
            vec![SqlValue::Real(5.5)]
        );
        assert_eq!(
            resolve_bindings(&definition, &json!({"value": u64::MAX}), None).unwrap(),
            vec![SqlValue::Real(u64::MAX as f64)]
        );

        let root = std::env::temp_dir().join(format!("konae-number-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE items(label TEXT)", [])
            .unwrap();
        let params = resolve_bindings(&definition, &json!({"value": 5}), None).unwrap();
        run_mutation(&path, &definition, params).unwrap();
        let stored: String = Connection::open(&path)
            .unwrap()
            .query_row("SELECT label FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, "5");
        let _ = std::fs::remove_dir_all(root);
    }

    /// ADR-0012: a notice that lists or addresses rows reads them back
    /// through the Kernel's RETURNING clause; the answer stays changes and

    #[test]
    fn a_mutation_reads_back_its_changed_rows_for_the_notice_only() {
        let program: ActionProgram = serde_json::from_value(json!({"version": 1, "actions": {
            "tasks.remind": {"kind": "mutation", "access": "authenticated", "schedule": {"every": "1h"},
                "statement": "UPDATE tasks SET reminded_at = ?1 WHERE reminded_at IS NULL; -- hourly",
                "bindings": [{"source": "kernel", "field": "now"}],
                "notify": {"to": {"column": "assignee"}, "changes": ["title"], "subject": "Due", "body": "{changes}"}}
        }})).unwrap();
        let definition = &program.actions["tasks.remind"];
        let root = std::env::temp_dir().join(format!("konae-returning-rows-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, assignee TEXT, reminded_at INTEGER);
             INSERT INTO tasks(title, assignee, reminded_at) VALUES ('a', 'ana', NULL), ('b', 'ben', NULL), ('c', 'ana', 5);",
        ).unwrap();
        let ran = run_mutation(&path, definition, vec![SqlValue::Integer(9)]).unwrap();
        assert_eq!(ran.answer, json!({"changes": 2, "lastInsertId": 0}));
        let mut rows: Vec<Value> = ran.rows.iter().cloned().map(Value::Object).collect();
        rows.sort_by_key(|row| row["title"].to_string());
        assert_eq!(rows, vec![json!({"title": "a", "assignee": "ana"}), json!({"title": "b", "assignee": "ben"})]);
        let reminded: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM tasks WHERE reminded_at = 9", [], |row| row.get(0)).unwrap();
        assert_eq!(reminded, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn string_max_length_counts_utf16_code_units() {
        let two = action("mutation", "SELECT ?", vec![input_binding("title", "string", Some(2))]);
        let one = action("mutation", "SELECT ?", vec![input_binding("title", "string", Some(1))]);

        let emoji = json!({"title": "\u{1F600}"});
        assert!(resolve_bindings(&two, &emoji, None).is_ok());
        assert!(resolve_bindings(&one, &emoji, None).unwrap_err().contains("too long"));
        assert!(resolve_bindings(&two, &json!({"title": "\u{e9}\u{e9}"}), None).is_ok());
    }

    async fn deploy_files(state: &AppState, files: Value) -> Value {
        deploy_with_visibility(state, files, "public").await
    }

    async fn deploy_with_visibility(state: &AppState, files: Value, visibility: &str) -> Value {
        let body = json!({
            "project": {
                "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
            },
            "visibility": visibility,
            "artifact": {"version": 1, "entrypoint": "index.html", "files": files}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
    }

    #[tokio::test]
    async fn a_first_release_takes_the_slug_hostname_when_free() {
        let root = std::env::temp_dir().join(format!("konae-slug-host-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        assert_eq!(first["hostname"], "republish.localhost");
        assert_eq!(first["url"], "http://republish.localhost");
        let again = deploy_files(&state, json!({"index.html": "<p>two</p>"})).await;
        assert_eq!(again["hostname"], "republish.localhost");
        let body = json!({
            "project": {
                "id": "app_other", "name": "Other", "slug": "republish", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
            },
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>other</p>"}}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let other = serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone();
        let host = other["hostname"].as_str().unwrap();
        assert!(host.starts_with("republish-"), "{host}");
        assert!(host.ends_with(".localhost"), "{host}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_browser_navigation_to_a_page_counts_one_page_view_by_the_file_served() {
        let root = std::env::temp_dir().join(format!("konae-page-views-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html": "<p>home</p>", "about.html": "<p>about</p>", "app.js": "1"})).await;
        let (host, deployment) = state.deployments.read().await.iter().next().map(|(h, d)| (h.clone(), d.clone())).unwrap();
        let get = |path: &str, extra: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
            for (name, value) in extra {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            serve_key(state.clone(), headers, path.into())
        };
        let navigation = [("sec-fetch-dest", "document")];
        assert_eq!(get("index.html", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);

        get("index.html", &[]).await;
        get("index.html", &[("sec-fetch-dest", "script")]).await;
        get("app.js", &navigation).await;
        get("index.html", &[("sec-fetch-dest", "document"), ("sec-purpose", "prefetch;prerender")]).await;
        assert_eq!(get("missing", &navigation).await.status(), StatusCode::NOT_FOUND);
        usage::flush();
        let today = usage::day(now_ms());
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        let views: Vec<_> = usage["totals"].as_array().unwrap().iter().filter(|row| row["kind"] == "page_view").cloned().collect();
        assert_eq!(views, vec![
            json!({"kind": "page_view", "name": "about.html", "count": 2}),
            json!({"kind": "page_view", "name": "index.html", "count": 1}),
        ]);


        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["routing"] = json!("spa");
        }
        assert_eq!(get("items/3", &navigation).await.status(), StatusCode::OK);
        assert_eq!(get("missing-chunk.js", &[("sec-fetch-dest", "script")]).await.status(), StatusCode::OK);
        usage::flush();
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        let index = usage["totals"].as_array().unwrap().iter().find(|row| row["kind"] == "page_view" && row["name"] == "index.html").cloned();
        assert_eq!(index, Some(json!({"kind": "page_view", "name": "index.html", "count": 2})));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn seo_lists_only_titled_pages_and_revalidates_the_chosen_address() {
        let root = std::env::temp_dir().join(format!("konae-seo-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let mut state = test_state(&root);
        state.public_origin = "https://konae.example.test".into();
        state.guards = Arc::new(Guards::new(Vec::new()).with_statement_wait(std::time::Duration::from_millis(5)));
        deploy_files(&state, json!({
            "index.html": "<!doctype html><html><head><title>Home</title><meta name=\"description\" content=\"Start here\"></head><body>Home</body></html>",
            "about.html": "<!doctype html><html><head><title>A \"small\" <app> &amp; more</title><meta property=\"og:title\" content=\"Owner title\"></head><body>About</body></html>",
            "post.html": "<!doctype html><html><head><title>Post template</title></head><body>Post</body></html>",
            "untitled.html": "<html><body>Missing title</body></html>",
            "robots.txt": "User-agent: special\nDisallow: /private\n",
            "card.png": "image"
        })).await;
        let (host, deployment) = {
            let mut deployments = state.deployments.write().await;
            let (host, deployment) = deployments.iter_mut().next().unwrap();
            deployment.actions = r#"{"version":1,"actions":{"posts.by":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts WHERE slug = ?1 LIMIT 1","bindings":[{"source":"input","key":"slug","type":"string"}],"page":{"path":"/posts/:slug","file":"post.html","list":"posts.list"}},"posts.list":{"kind":"query","access":"anonymous","statement":"SELECT 'listed' AS slug, ?1 AS updated LIMIT 1","bindings":[{"source":"kernel","field":"now"}]}}}"#.into();
            (host.clone(), deployment.clone())
        };
        if let Err(error_value) = action_shape::read(&deployment.actions) { panic!("{error_value}"); }
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
        let _off = serve_key(state.clone(), headers.clone(), "index.html".into()).await;
        {
            let mut deployments = state.deployments.write().await;
            let app = deployments.get_mut(&host).unwrap();
            app.modules.get_mut("seo").unwrap()["on"] = json!(true);
            app.modules.get_mut("seo").unwrap()["canonical"] = Value::Null;
            app.modules.get_mut("seo").unwrap()["image"] = json!("card.png");
        }
        let home = serve_key(state.clone(), headers.clone(), "index.html".into()).await;
        let home_body = axum::body::to_bytes(home.into_body(), 100_000).await.unwrap();
        let home_html = String::from_utf8(home_body.to_vec()).unwrap();
        for card in ["og:title", "og:description", "og:url", "og:image"] { assert!(home_html.contains(card), "missing {card}"); }
        let about = serve_key(state.clone(), headers.clone(), "about".into()).await;
        let old_etag = about.headers()[header::ETAG].to_str().unwrap().to_owned();
        let about_body = axum::body::to_bytes(about.into_body(), 100_000).await.unwrap();
        let about_html = String::from_utf8(about_body.to_vec()).unwrap();
        assert!(about_html.contains("<title>A \"small\" <app> &amp; more</title>"));
        assert!(about_html.contains("content=\"Owner title\""));
        assert!(about_html.contains("href=\"https://republish.localhost/about\""), "{about_html}");
        let mut navigation_headers = headers.clone();
        navigation_headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&old_etag).unwrap());
        navigation_headers.insert("sec-fetch-dest", HeaderValue::from_static("document"));
        let navigation_revalidation = serve_key(state.clone(), navigation_headers, "about".into()).await;
        assert_eq!(navigation_revalidation.status(), StatusCode::NOT_MODIFIED);
        let robots = serve_key(state.clone(), headers.clone(), "robots.txt".into()).await;
        assert!(String::from_utf8(axum::body::to_bytes(robots.into_body(), 100_000).await.unwrap().to_vec()).unwrap().contains("Disallow: /private"));
        let mut held = Vec::new();
        for _ in 0..4 { held.push(state.guards.statement_permit(&deployment.id).await.unwrap()); }
        let blocked_sitemap = serve_key(state.clone(), headers.clone(), "sitemap.xml".into()).await;
        let blocked_sitemap = String::from_utf8(axum::body::to_bytes(blocked_sitemap.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(!blocked_sitemap.contains("/posts/listed"));
        drop(held);
        let sitemap = serve_key(state.clone(), headers.clone(), "sitemap.xml".into()).await;
        let sitemap_etag = sitemap.headers()[header::ETAG].to_str().unwrap().to_owned();
        let sitemap = String::from_utf8(axum::body::to_bytes(sitemap.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(sitemap.contains("/about"));
        assert!(sitemap.contains("/posts/listed"));
        assert!(!sitemap.contains("/post</loc>"));
        assert!(!sitemap.contains("untitled"));
        let mut sitemap_headers = headers.clone();
        sitemap_headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&sitemap_etag).unwrap());
        let sitemap_revalidation = serve_key(state.clone(), sitemap_headers, "sitemap.xml".into()).await;
        assert_eq!(sitemap_revalidation.status(), StatusCode::NOT_MODIFIED);
        let template = serve_key(state.clone(), headers.clone(), "post.html".into()).await;
        assert_eq!(template.headers()["x-robots-tag"], "noindex");
        let listed = modules::read(&state, &deployment);
        assert_eq!(listed["modules"]["seo"]["current"]["canonical"]["reason"], "Generated hostname is the default.");
        assert!(listed["modules"]["seo"]["current"]["excluded"].to_string().contains("untitled.html"));
        let domain_file = crate::layout::private(&root, &deployment.id).join(crate::domain::RECORD_FILE);
        tokio::fs::create_dir_all(domain_file.parent().unwrap()).await.unwrap();
        tokio::fs::write(&domain_file, serde_json::to_vec(&json!({"hostname":"app.example.test","token":"test","verified":true,"createdAt":0})).unwrap()).await.unwrap();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&old_etag).unwrap());
        let changed = serve_key(state.clone(), headers, "about.html".into()).await;
        assert_eq!(changed.status(), StatusCode::OK);
        let changed = String::from_utf8(axum::body::to_bytes(changed.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(changed.contains("https://app.example.test/about"));
        state.deployments.write().await.get_mut(&host).unwrap().manifest["routing"] = json!("spa");
        let fallback = serve_key(state.clone(), HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]), "deep/page".into()).await;
        assert_eq!(fallback.headers()["x-robots-tag"], "noindex");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn page_queries_wait_through_database_swap_and_keep_the_new_wal() {
        let root = std::env::temp_dir().join(format!("konae-page-swap-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html":"<title>Home</title>","post.html":"<html><head><title>Template</title></head><body></body></html>"})).await;
        let (host, deployment) = {
            let mut deployments = state.deployments.write().await;
            let (host, deployment) = deployments.iter_mut().next().unwrap();
            deployment.actions = r#"{"version":1,"actions":{"posts.head":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts WHERE slug = ?1 LIMIT 1","bindings":[{"source":"input","key":"slug","type":"string"}],"page":{"path":"/posts/:slug","file":"post.html"}}}}"#.into();
            deployment.artifact_version = format!("{}-page-swap", deployment.artifact_version);
            (host.clone(), deployment.clone())
        };
        let routes = http::route_table(&deployment);
        assert!(routes.has_pages());
        let live = crate::layout::database(&root, &deployment.id);
        let staged = crate::layout::private(&root, &deployment.id).join("page-swap-staged.sqlite");
        let replaced = crate::layout::private(&root, &deployment.id).join("page-swap-replaced.sqlite");
        for path in [&live, &staged] {
            let connection = Connection::open(path).unwrap();
            connection.execute_batch("CREATE TABLE posts(slug TEXT PRIMARY KEY, title TEXT);").unwrap();
            connection.execute("INSERT INTO posts VALUES('hello', ?1)", [if path == &live { "Old" } else { "New" }]).unwrap();
        }
        let lock = database_lock(&state, &deployment.id).await;
        let (at_gap_tx, at_gap_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let swap = tokio::task::spawn_blocking(move || swap_database_with(&live, &staged, &replaced, || {
            at_gap_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
        }));
        tokio::task::spawn_blocking(move || at_gap_rx.recv().unwrap()).await.unwrap();
        let headers = HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]);
        let request = tokio::spawn(serve_key_with_routes(state.clone(), headers, "posts/hello".into(), Some(routes)));
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(20), async { while !request.is_finished() { tokio::task::yield_now().await; } }).await.is_err();
        continue_tx.send(()).unwrap();
        swap.await.unwrap().unwrap();
        assert!(blocked, "the page request completed inside the database swap");
        let writer = Connection::open(&crate::layout::database(&root, &deployment.id)).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        writer.execute("INSERT INTO posts VALUES('later', 'WAL row')", []).unwrap();
        assert!(crate::layout::database(&root, &deployment.id).with_extension("sqlite-wal").exists() || {
            let mut sidecar = crate::layout::database(&root, &deployment.id).as_os_str().to_owned();
            sidecar.push("-wal");
            std::path::PathBuf::from(sidecar).exists()
        });
        drop(lock);
        let response = request.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("New"), "{body}");
        writer.execute("UPDATE posts SET title=NULL WHERE slug='hello'", []).unwrap();
        let headers = HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]);
        let untitled = serve_key_with_routes(state.clone(), headers, "posts/hello".into(), Some(http::route_table(&deployment))).await;
        assert_eq!(untitled.headers()["x-robots-tag"], "noindex");
        let untitled = String::from_utf8(axum::body::to_bytes(untitled.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(untitled.contains("name=\"konae-page-params\""));
        assert!(untitled.contains("&quot;slug&quot;:&quot;hello&quot;"));
        let reopened = Connection::open(crate::layout::database(&root, &deployment.id)).unwrap();
        assert_eq!(reopened.query_row("SELECT title FROM posts WHERE slug='later'", [], |row| row.get::<_, String>(0)).unwrap(), "WAL row");
        drop(writer);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_file_the_browser_holds_answers_304_until_the_next_publish() {
        let root = std::env::temp_dir().join(format!("konae-etag-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "1"})).await;
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["visibility"] = json!("public");
        }
        let (host, deployment) = state.deployments.read().await.iter().next().map(|(h, d)| (h.clone(), d.clone())).unwrap();
        let get = |path: &str, extra: Vec<(&'static str, String)>| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
            for (name, value) in extra {
                headers.insert(name, HeaderValue::from_str(&value).unwrap());
            }
            serve_key(state.clone(), headers, path.into())
        };
        let first = get("app.js", vec![]).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CACHE_CONTROL], "public, no-cache");
        let tag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert_eq!(tag, format!("\"{}\"", deployment.artifact_version.clone()));

        let again = get("app.js", vec![("if-none-match", tag.clone())]).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG], tag.as_str());
        assert!(axum::body::to_bytes(again.into_body(), 1024).await.unwrap().is_empty());
        let weak = get("app.js", vec![("if-none-match", format!("\"other\", W/{tag}"))]).await;
        assert_eq!(weak.status(), StatusCode::NOT_MODIFIED);

        // A navigation still resolves its file, so the page view counts.
        let page = get("index.html", vec![("if-none-match", tag.clone()), ("sec-fetch-dest", "document".into())]).await;
        assert_eq!(page.status(), StatusCode::NOT_MODIFIED);
        usage::flush();
        let today = usage::day(now_ms());
        let usage = usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
        assert!(usage["totals"].as_array().unwrap().iter().any(|row| row["kind"] == "page_view" && row["name"] == "index.html"));

        // A publish changes the tag, so the old one gets the new bytes.
        deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "2"})).await;
        for deployment in state.deployments.write().await.values_mut() {
            deployment.manifest["visibility"] = json!("public");
        }
        let fresh = get("app.js", vec![("if-none-match", tag.clone())]).await;
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_ne!(fresh.headers()[header::ETAG], tag.as_str());
        assert_eq!(&axum::body::to_bytes(fresh.into_body(), 1024).await.unwrap()[..], b"2");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn a_private_apps_files_stay_out_of_shared_caches() {
        let deployment = Settings::new("app", "App", "app", &json!({"visibility": "private"}))
            .deployment(Installed { hostname: "app.localhost".into(), created_at: 1, released_at: None, source_digest: None, actions: "{}".into(), artifact_version: "v1".into(), released: None }, "http://localhost");
        assert_eq!(ArtifactCache::of(&deployment).control(), "private, no-cache");
    }

    #[test]
    fn a_record_without_a_release_time_reads_its_first_publish() {
        let installed: Installed =
            serde_json::from_value(json!({"hostname": "app.localhost", "createdAt": 7, "actions": "{}", "artifactVersion": "v1"})).unwrap();
        let deployment = Settings::new("app", "App", "app", &json!({"visibility": "public"})).deployment(installed, "http://localhost");
        assert_eq!(deployment.released_at, 7);
        assert_eq!(serde_json::to_value(&deployment).unwrap()["releasedAt"], json!(7));
    }

    #[tokio::test]
    async fn a_republish_moves_the_release_time_and_keeps_the_first_publish() {
        let root = std::env::temp_dir().join(format!("konae-released-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        assert!(first["releasedAt"].as_u64().unwrap() >= first["createdAt"].as_u64().unwrap());
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second = deploy_files(&state, json!({"index.html": "<p>two</p>"})).await;
        assert_eq!(second["createdAt"], first["createdAt"]);
        assert!(second["releasedAt"].as_u64().unwrap() > first["releasedAt"].as_u64().unwrap());
        let id = second["id"].as_str().unwrap();
        let record: Value = serde_json::from_slice(&std::fs::read(crate::layout::record(&root, id)).unwrap()).unwrap();
        assert_eq!(record["releasedAt"], second["releasedAt"]);
        let loaded = load_deployments(&root, "http://localhost").await.unwrap();
        assert_eq!(json!(loaded.values().next().unwrap().released_at), second["releasedAt"]);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn deployment_loading_recovers_legacy_files_after_an_operator_settings_write() {
        let root = std::env::temp_dir().join(format!("konae-load-legacy-files-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        deploy_files(&state, json!({"index.html":"<script>miniapp.files.list()</script>"})).await;
        let id = state.deployments.read().await.values().next().unwrap().id.clone();
        let path = layout::settings(&root, &id);
        let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw.as_object_mut().unwrap().remove("modules");
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        Settings::read(&root, &id).unwrap().write(&root).await.unwrap();
        let loaded = load_deployments(&root, "http://localhost").await.unwrap();
        assert!(loaded.values().next().unwrap().modules["files"]["on"] == true);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn konae_load_decides_legacy_auth_before_existing_accounts_sign_in() {
        let root = std::env::temp_dir().join(format!("konae-legacy-auth-load-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let released = deploy_files(&state, json!({"index.html": "<p>Members</p>"})).await;
        let id = released["id"].as_str().unwrap();
        let original = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
        accounts::app_store(&state, &original).unwrap().signup("member@example.test", "correct horse battery").unwrap();
        let settings_path = layout::settings(&root, id);
        let mut old: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        old["modules"].as_object_mut().unwrap().remove("auth");
        std::fs::write(&settings_path, serde_json::to_vec(&old).unwrap()).unwrap();

        let loaded = load_deployments(&root, "http://localhost").await.unwrap();
        let deployment = loaded.values().find(|deployment| deployment.id == id).unwrap();
        assert_eq!(deployment.modules["auth"]["on"], true);
        let auth = accounts::app_store(&state, deployment).unwrap();
        let session = auth.login("member@example.test", "correct horse battery").unwrap();
        assert!(auth.authenticate(Some(&session.token)).unwrap().is_some());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_release_names_the_digest_of_the_sources_it_carried() {
        let root = std::env::temp_dir().join(format!("konae-source-digest-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        let sources = [("actions.json", "{\"version\":1,\"actions\":{}}"), ("schema.sql", "")];
        let expected = sources::digest(sources.iter().map(|(path, text)| (*path, text.as_bytes())));
        assert_eq!(first["sourceDigest"], json!(expected));
        let id = first["id"].as_str().unwrap();
        let record: Value = serde_json::from_slice(&std::fs::read(crate::layout::record(&root, id)).unwrap()).unwrap();
        assert_eq!(record["sourceDigest"], json!(expected));
        // A write that is not a release keeps the digest.
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        persist_deployment(&state, &deployment).await.unwrap();
        let loaded = load_deployments(&root, "http://localhost").await.unwrap();
        assert_eq!(loaded.values().next().unwrap().source_digest.as_deref(), Some(expected.as_str()));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn a_record_without_a_source_digest_names_none() {
        let installed: Installed =
            serde_json::from_value(json!({"hostname": "app.localhost", "createdAt": 7, "actions": "{}", "artifactVersion": "v1"})).unwrap();
        let deployment = Settings::new("app", "App", "app", &json!({"visibility": "public"})).deployment(installed, "http://localhost");
        assert_eq!(deployment.source_digest, None);
        assert!(serde_json::to_value(&deployment).unwrap().get("sourceDigest").is_none());
    }

    #[tokio::test]
    async fn republish_stops_serving_files_missing_from_the_new_artifact() {
        let root = std::env::temp_dir().join(format!("konae-republish-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(
            &state,
            json!({"index.html": "<p>one</p>", "old.js": "1", "assets/old.css": "a{}"}),
        )
        .await;
        let id = first["id"].as_str().unwrap().to_owned();
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(read_artifact(&state, &deployment, "old.js").await.unwrap(), b"1");

        deploy_files(&state, json!({"index.html": "<p>two</p>", "new.js": "2"})).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(deployment.id, id);
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>two</p>");
        assert_eq!(read_artifact(&state, &deployment, "new.js").await.unwrap(), b"2");
        assert!(read_artifact(&state, &deployment, "old.js").await.is_err());
        assert!(read_artifact(&state, &deployment, "assets/old.css").await.is_err());
        assert!(staged_or_retired(&root, &id).is_empty(), "only dist/ holds files");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn new_deployments_pick_a_policy_from_visibility_and_redeploys_keep_it() {
        for (visibility, expected) in [("public", SignupMode::Open), ("private", SignupMode::Closed)] {
            let root = std::env::temp_dir().join(format!("konae-policy-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            let first = deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), visibility).await;
            assert_eq!(first["signupPolicy"]["mode"], json!(expected), "visibility={visibility}");

            set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
            let second = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), visibility).await;
            assert_eq!(second["signupPolicy"], json!({"mode": "allowlist", "allowlist": ["@example.test"]}));
            let id = second["id"].as_str().unwrap();
            let stored = Settings::read(&root, id).unwrap();
            assert_eq!(stored.signup_policy.mode, SignupMode::Allowlist);
            let mut legacy = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
            legacy.manifest["runtime"] = json!({"database":{"mode":"declared-actions"}});
            assert!(released_settings(&legacy).manifest.get("runtime").is_none());
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn an_operation_completes_a_legacy_applied_snapshot_from_live_files_and_database() {
        let root = std::env::temp_dir().join(format!("konae-legacy-operation-snapshot-{}", Uuid::new_v4()));
        let id = "app_legacy_operation";
        let settings = Settings::new(id, "Applied", "applied", &json!({"version":1,"visibility":"public"}));
        settings.write(&root).await.unwrap();
        tokio::fs::create_dir_all(crate::layout::dist(&root, id)).await.unwrap();
        tokio::fs::write(crate::layout::dist(&root, id).join("index.html"), "old applied artifact").await.unwrap();
        let database = crate::layout::database(&root, id);
        tokio::fs::create_dir_all(database.parent().unwrap()).await.unwrap();
        rusqlite::Connection::open(&database).unwrap().execute_batch("CREATE TABLE \"live notes\"(id INTEGER PRIMARY KEY, title TEXT); CREATE INDEX \"live notes title\" ON \"live notes\"(title); INSERT INTO \"live notes\" VALUES(1,'kept');").unwrap();
        let installed = Installed {
            hostname: "applied.localhost".into(), created_at: 1, released_at: Some(2), source_digest: Some("old-source".into()),
            actions: r#"{"version":1,"actions":{"applied.read":{"kind":"query","access":"anonymous","statement":"SELECT title FROM \"live notes\" LIMIT 5","bindings":[]}}}"#.into(),
            artifact_version: "v-old".into(), released: None,
        };
        tokio::fs::create_dir_all(crate::layout::private(&root, id)).await.unwrap();
        tokio::fs::write(crate::layout::record(&root, id), serde_json::to_vec(&installed).unwrap()).await.unwrap();
        let state = test_state(&root);
        let deployment = settings.deployment(installed, "http://localhost");
        state.deployments.write().await.insert(deployment.hostname.clone(), deployment.clone());

        let mut operated = deployment;
        operated.signup_policy = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        persist_deployment(&state, &operated).await.unwrap();

        let migrated = Settings::read(&root, id).unwrap().released.unwrap();
        assert_eq!(migrated.signup_policy.mode, SignupMode::Closed);
        assert!(migrated.schema_sql.contains("CREATE TABLE IF NOT EXISTS \"live notes\""));
        assert!(migrated.schema_sql.contains("CREATE INDEX IF NOT EXISTS \"live notes title\""));
        let expected_digest = crate::folder::version_of(&HashMap::from([("index.html".to_owned(), b"old applied artifact".to_vec())]));
        assert_eq!(migrated.artifact_digest.as_deref(), Some(expected_digest.as_str()));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn republishing_a_public_app_as_private_closes_an_open_signup() {
        // (first visibility, policy the owner set, second visibility, expected)
        let cases = [
            ("public", None, "private", json!({"mode": "closed", "allowlist": []})),
            (
                "public",
                Some((SignupMode::Allowlist, "@example.test")),
                "private",
                json!({"mode": "allowlist", "allowlist": ["@example.test"]}),
            ),
            // The owner opened a private app on purpose; a redeploy keeps that.
            ("private", Some((SignupMode::Open, "")), "private", json!({"mode": "open", "allowlist": []})),
            ("private", None, "public", json!({"mode": "closed", "allowlist": []})),
        ];
        for (first, chosen, second, expected) in cases {
            let root = std::env::temp_dir().join(format!("konae-flip-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), first).await;
            if second == "private" {
                let mut deployment = state.deployments.read().await.values().next().unwrap().clone();
                deployment.modules.get_mut("auth").unwrap()["on"] = json!(true);
                Settings::of(&deployment).write(&root).await.unwrap();
                state.deployments.write().await.insert(deployment.hostname.clone(), deployment);
            }
            if let Some((mode, entry)) = chosen {
                let entries: Vec<&str> = if entry.is_empty() { vec![] } else { vec![entry] };
                set_policy(&state, mode, &entries).await;
            }
            let republished = deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), second).await;
            assert_eq!(republished["signupPolicy"], expected, "{first} {chosen:?} -> {second}");
            let id = republished["id"].as_str().unwrap();
            let stored = Settings::read(&root, id).unwrap();
            assert_eq!(serde_json::to_value(&stored.signup_policy).unwrap(), expected);
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn signup_checks_input_first_and_gives_one_answer_for_closed_and_unlisted() {
        let root = std::env::temp_dir().join(format!("konae-signup-oracle-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let attempt = |email: &'static str, password: &'static str| {
            let state = state.clone();
            let headers = headers.clone();
            async move {
                let body = json!({"email": email, "password": password});
                let request = Request::builder().body(Body::from(body.to_string())).unwrap();
                let response = signup(State(state), headers, request).await;
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap()["error"].clone())
            }
        };
        set_policy(&state, SignupMode::Allowlist, &["listed@example.test"]).await;
        // A weak password gets the same answer whether or not the address is listed.
        let listed = attempt("listed@example.test", "x").await;
        let unlisted = attempt("other@example.test", "x").await;
        assert_eq!(listed.0, StatusCode::BAD_REQUEST);
        assert_eq!(listed, unlisted);
        assert_eq!(attempt("not-an-email", "correct horse battery").await.0, StatusCode::BAD_REQUEST);
        let refused = attempt("other@example.test", "correct horse battery").await;
        assert_eq!(refused.0, StatusCode::FORBIDDEN);
        set_policy(&state, SignupMode::Closed, &[]).await;
        assert_eq!(attempt("listed@example.test", "correct horse battery").await, refused);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn app_databases_are_capped_in_size() {
        let root = std::env::temp_dir().join(format!("konae-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        initialize_database(&path).unwrap();
        let db = Connection::open(&path).unwrap();
        restrict_database(&db).unwrap();
        db.authorizer::<fn(AuthContext<'_>) -> Authorization>(None).unwrap();
        let page_size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        let pages: u64 = db.query_row("PRAGMA max_page_count", [], |r| r.get(0)).unwrap();
        assert_eq!(pages * page_size, MAX_APP_DATABASE_BYTES);
        drop(db);

        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE notes(body TEXT)").unwrap();
        limit_database_size(&db, 64 * 1024).unwrap();
        let mut full = None;
        for _ in 0..100 {
            if let Err(error_value) = db.execute("INSERT INTO notes VALUES (?1)", ["x".repeat(4000)]) {
                full = Some(error_value.to_string());
                break;
            }
        }
        assert!(full.expect("the cap must stop writes").contains("full"));
        let _ = std::fs::remove_dir_all(root);
    }


    fn staged_or_retired(root: &Path, id: &str) -> Vec<String> {
        std::fs::read_dir(layout::private(root, id))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("dist-") || name.starts_with("server-"))
            .collect()
    }


    fn release_body(id: &str, schema: &str, actions: Value, page: &str, confirmation: Option<String>) -> Value {
        json!({
            "project": {
                "id": id, "name": "Steps", "slug": "steps", "manifest": {},
                "files": {"actions.json": json!({"version": 1, "actions": actions}).to_string(), "schema.sql": schema}
            },
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": page}},
            "schemaConfirmation": confirmation
        })
    }

    #[tokio::test]
    async fn a_static_release_with_only_seo_creates_no_action_database_and_serves_its_page() {
        let root = std::env::temp_dir().join(format!("konae-static-release-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let id = "app_static_home";
        let body = json!({
            "project":{"id":id,"name":"Home","slug":"home","manifest":{"version":1,"visibility":"public"},"files":{"index.html":"<!doctype html><title>Konae</title>"}},
            "visibility":"public",
            "initialModules":{"seo":{"on":true}},
            "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<!doctype html><title>Konae</title>"}}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let deployment = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
        assert_eq!(deployment.modules["seo"]["on"], true);
        assert_eq!(deployment.modules["actions"]["on"], false);
        assert!(!database_path(&state, id).exists());
        assert!(!crate::layout::identity(&root, id).exists());
        let html = read_artifact(&state, &deployment, "index.html").await.unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("<title>Konae</title>"));
        assert!(!html.contains("runtime.js"));
        let reloaded = load_deployments(&root, "http://localhost").await.unwrap();
        assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules["seo"]["on"], true);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_first_release_refuses_unknown_module_options_from_an_import_package() {
        let root = std::env::temp_dir().join(format!("konae-invalid-package-modules-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let mut body = release_body("app_bad_package_modules", "", json!({}), "<p>Home</p>", None);
        body["initialPackage"] = json!({
            "manifest": {
                "format": "konae-package", "version": 1, "packageId": "pkg_bad_modules", "createdAt": 1,
                "deploymentId": "dep_source", "projectId": "project_source", "visibility": "public",
                "modules": {
                    "actions": {"on": false},
                    "seo": {"on": true, "canonical": "path", "image": null, "futureOption": true}
                },
                "publishedRevision": 1, "contents": [], "excluded": []
            },
            "components": {}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let message = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(message.contains("Unknown option futureOption for module seo."), "{message}");
        assert!(!crate::layout::settings(&root, "app_bad_package_modules").exists());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn deployment_loading_skips_one_app_with_an_unknown_module_option() {
        let root = std::env::temp_dir().join(format!("konae-load-invalid-module-option-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        for (id, hostname, invalid) in [
            ("app_bad_option", "bad.localhost", true),
            ("app_good_option", "good.localhost", false),
        ] {
            tokio::fs::create_dir_all(crate::layout::private(&root, id)).await.unwrap();
            let mut settings = Settings::new(id, id, id, &json!({"visibility":"public"}));
            if invalid { settings.modules.get_mut("seo").unwrap()["futureOption"] = json!(true); }
            settings.write(&root).await.unwrap();
            let installed = Installed {
                hostname: hostname.into(), created_at: 1, released_at: Some(1), source_digest: None,
                actions: r#"{"version":1,"actions":{}}"#.into(), artifact_version: "v1".into(), released: None,
            };
            tokio::fs::write(crate::layout::record(&root, id), serde_json::to_vec(&installed).unwrap()).await.unwrap();
        }
        let loaded = load_deployments(&root, "http://localhost").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.values().any(|deployment| deployment.id == "app_good_option"));
        assert!(!loaded.values().any(|deployment| deployment.id == "app_bad_option"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_release_with_schema_and_actions_off_gets_the_module_refusal() {
        let root = std::env::temp_dir().join(format!("konae-schema-actions-off-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let mut body = release_body("app_schema_actions_off", "CREATE TABLE notes(id INTEGER PRIMARY KEY);", json!({}), "<main>Home</main>", None);
        body["initialModules"] = json!({"actions":{"on":false}});
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let message = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{message}");
        assert!(message.contains("Module actions is off"), "{message}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn an_actions_key_in_another_json_file_does_not_turn_on_actions() {
        let root = std::env::temp_dir().join(format!("konae-other-json-actions-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let mut body = release_body("app_other_json_actions", "", json!({}), "<main>Home</main>", None);
        body["initialModules"] = json!({"actions":{"on":false}});
        body["project"]["files"]["public/keys.json"] = json!("{\"actions\":{\"jump\":\"Space\"}}");
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let status = response.status();
        let message = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert_eq!(status, StatusCode::OK, "{message}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn release_checks_files_before_actions_when_both_modules_are_off() {
        let root = std::env::temp_dir().join(format!("konae-files-before-actions-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let mut body = release_body("app_files_before_actions", "", json!({"notes.list":{"kind":"query","statement":"SELECT 1"}}), "<main>Home</main>", None);
        body["initialModules"] = json!({"actions":{"on":false},"files":{"on":false}});
        body["artifact"]["files"]["app.js"] = json!("miniapp.files.list()");
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let refused = response_json(response).await;
        assert_eq!(refused["code"], "module.off");
        assert_eq!(refused["error"], "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_static_whole_document_can_release_without_a_browser_sdk_module() {
        let root = std::env::temp_dir().join(format!("konae-static-whole-document-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = AppState::for_test(&root);
        let mut body = release_body("app_static_whole_document", "", json!({}), "<!doctype html><title>Static</title>", None);
        body["initialModules"] = json!({"actions":{"on":false},"auth":{"on":false},"files":{"on":false}});
        body["project"]["files"].as_object_mut().unwrap().remove("actions.json");
        body["project"]["files"].as_object_mut().unwrap().remove("schema.sql");
        let response = deploy(&state, body.to_string().as_bytes()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn release_swaps_fresh_baseline_and_existing_data_transformations() {
        let root = std::env::temp_dir().join(format!("konae-release-transform-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let id = "app_transform_release";
        let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);";
        let actions = json!({
            "notes.list": {"kind":"query", "access":"anonymous", "statement":"SELECT body FROM notes ORDER BY id LIMIT 10", "bindings": []}
        });
        let v1 = json!({
            "id": "baseline", "predecessor": null,
            "before": format!("sha256:{}", "0".repeat(64)),
            "after": schema_transform::fingerprint_schema(schema).unwrap(),
            "maxRows": 10, "statements": ["CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)"]
        });
        let document = json!({"version": 1, "transitions": [v1.clone()]}).to_string();
        let mut first = release_body(id, schema, actions.clone(), "<p>first</p>", None);
        first["project"]["files"]["schema-transforms.json"] = json!(document);
        let response = deploy(&state, first.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));

        async fn read_notes(state: &AppState) -> (StatusCode, String) {
            let deployment = state.deployments.read().await.values().next().unwrap().clone();
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, deployment.hostname.parse().unwrap());
            let response = invoke_action(State(state.clone()), AxumPath("notes.list".into()), headers, Json(json!({}))).await;
            let status = response.status();
            let body = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
            (status, body)
        }

        assert_eq!(read_notes(&state).await.0, StatusCode::OK, "fresh baseline schema must reach the live database");
        let database = database_path(&state, id);
        Connection::open(&database).unwrap().execute("INSERT INTO notes(body) VALUES('before')", []).unwrap();
        let fingerprint = schema_transform::fingerprint(&Connection::open(&database).unwrap()).unwrap();
        let v2 = json!({
            "id": "rewrite", "predecessor": "baseline",
            "before": fingerprint, "after": schema_transform::fingerprint_schema(schema).unwrap(),
            "maxRows": 1, "statements": ["UPDATE notes SET body='after' WHERE id=1"]
        });
        let update_document = json!({"version": 1, "transitions": [v1, v2]}).to_string();
        let mut second = release_body(id, schema, actions, "<p>second</p>", None);
        second["project"]["files"]["schema-transforms.json"] = json!(update_document);
        let input: DeployInput = serde_json::from_value(second.clone()).unwrap();
        let confirmation = runtime::validate_release(&state, &input, None).await.unwrap().confirmation().unwrap();
        second["schemaConfirmation"] = json!(confirmation);
        let response = deploy(&state, second.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let (status, body) = read_notes(&state).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("after"), "the release must swap the staged transformed data: {body}");

        let restarted = test_state(&root);
        let deployment = folder::open(&restarted, id, None).await.unwrap();
        let (status, body) = read_notes(&restarted).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("after"), "the transformed rows must survive a restart: {body}");
        let db = Connection::open(database_path(&restarted, id)).unwrap();
        let history = schema_transform::load_history(&db).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert!(history.entries[0].baseline_skipped);
        assert_eq!(history.entries[1].id, "rewrite");
        assert_eq!(history.entries[1].affected_rows, 1);
        assert_eq!(deployment.id, id);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_release_that_fails_at_any_step_leaves_the_old_actions_on_the_old_rows() {
        let list = |table: &str, column: &str| json!({"kind": "query", "access": "anonymous", "statement": format!("SELECT {column} FROM {table} ORDER BY id LIMIT 50"), "bindings": []});
        let both = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\nCREATE TABLE IF NOT EXISTS tags(id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n";
        let notes_only = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\n";
        for step in ["stage_local_artifact", "stage_server", "schema_change", "stage_sources", "persist_deployment", "swap_sources", "switch_dist", "switch_server"] {
            let root = std::env::temp_dir().join(format!("konae-release-step-{}", Uuid::new_v4()));
            tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
            let state = test_state(&root);
            let id = format!("app_{step}");
            let mut first = release_body(&id, both, json!({"notes.list": list("notes", "body"), "tags.list": list("tags", "name")}), "<p>one</p>", None);
            functions::tests::add_function(&mut first, "export function handle() { return new Response('A'); }");
            let response = deploy(&state, first.to_string().as_bytes()).await;
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
            assert_eq!(status, StatusCode::OK, "{step}: {}", String::from_utf8_lossy(&bytes));
            Connection::open(database_path(&state, &id)).unwrap()
                .execute_batch("INSERT INTO notes(body) VALUES ('kept note'); INSERT INTO tags(name) VALUES ('old tag');").unwrap();
            let live = state.deployments.read().await.values().next().cloned().unwrap();
            let app = crate::layout::sources(&root, &id);
            let first_actions = std::fs::read(app.join("actions.json")).unwrap();
            assert!(!crate::layout::private(&root, &id).join("sources.json").exists());


            let mut second = release_body(&id, notes_only, json!({"notes.list": list("notes", "body")}), "<p>two</p>", None);
            functions::tests::add_function(&mut second, "export function handle() { return new Response('B'); }");
            second.as_object_mut().unwrap().remove("initialModules");
            let input: DeployInput = serde_json::from_value(second.clone()).unwrap();
            let confirmation = runtime::validate_release(&state, &input, None).await.unwrap().confirmation();
            assert!(confirmation.is_some(), "dropping tags loses a row");
            second["schemaConfirmation"] = json!(confirmation);
            FAILING_RELEASE_STEPS.lock().unwrap().push((id.clone(), step));
            assert_ne!(deploy(&state, second.to_string().as_bytes()).await.status(), StatusCode::OK, "{step}");
            FAILING_RELEASE_STEPS.lock().unwrap().retain(|(app, _)| app != &id);

            let call = |name: &'static str| {
                let state = state.clone();
                let mut headers = HeaderMap::new();
                headers.insert(header::HOST, live.hostname.parse().unwrap());
                async move {
                    let response = invoke_action(State(state), AxumPath(name.into()), headers, Json(json!({}))).await;
                    let status = response.status();
                    (status, String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap())
                }
            };
            let (status, body) = call("tags.list").await;
            assert_eq!(status, StatusCode::OK, "{step}: {body}");
            assert!(body.contains("old tag"), "{step}: {body}");
            assert!(call("notes.list").await.1.contains("kept note"), "{step}");
            let serving = state.deployments.read().await.values().next().cloned().unwrap();
            assert_eq!(serving.artifact_version, live.artifact_version, "{step}");
            let record: Value = serde_json::from_slice(&std::fs::read(crate::layout::record(&root, &id)).unwrap()).unwrap();
            assert_eq!(record["artifactVersion"], json!(live.artifact_version), "{step}: the record names the release memory serves");
            assert_eq!(read_artifact(&state, &serving, "index.html").await.unwrap(), b"<p>one</p>", "{step}");
            assert_eq!(std::fs::read(crate::layout::dist(&root, &id).join("index.html")).unwrap(), b"<p>one</p>", "{step}: dist/ is the first release's");
            assert_eq!(std::fs::read(app.join("actions.json")).unwrap(), first_actions, "{step}: app/ is the first release's");
            assert!(!crate::layout::private(&root, &id).join("sources.json").exists(), "{step}: the release does not write editor metadata outside app/");
            let restored = functions::read_artifact(&layout::server(&root, &id)).unwrap().unwrap();
            assert!(restored.bundles["orders"].contains("'A'"), "{step}");
            assert_eq!(serving.server, live.server, "{step}");
            let mut restarted = test_state(&root);
            restarted.execution_mode = execution::ExecutionMode::Live;
            let reopened = folder::open(&restarted, &id, None).await.unwrap();
            assert_eq!(reopened.server, live.server, "{step}");
            assert!(staged_or_retired(&root, &id).is_empty(), "{step}: no staged files stay");
            let staged_sources: Vec<_> = std::fs::read_dir(crate::layout::private(&root, &id)).unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.starts_with("app-")).collect();
            assert!(staged_sources.is_empty(), "{step}: {staged_sources:?}");
            let leftovers: Vec<_> = std::fs::read_dir(crate::layout::private(&root, &id)).unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.starts_with("release-")).collect();
            assert!(leftovers.is_empty(), "{step}: {leftovers:?}");

            // The same release goes through once nothing fails.
            assert_eq!(deploy(&state, second.to_string().as_bytes()).await.status(), StatusCode::OK, "{step}");
            assert_ne!(std::fs::read(app.join("actions.json")).unwrap(), first_actions, "{step}: app/ is the second release's");
            assert_eq!(call("tags.list").await.0, StatusCode::NOT_FOUND, "{step}");
            assert!(call("notes.list").await.1.contains("kept note"), "{step}");
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn a_failed_record_write_removes_the_new_artifact_version() {
        let root = std::env::temp_dir().join(format!("konae-orphan-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
        let id = first["id"].as_str().unwrap().to_owned();
        let record = crate::layout::record(&root, &id);
        tokio::fs::remove_file(&record).await.unwrap();
        tokio::fs::create_dir_all(record.join("blocker")).await.unwrap();
        let body = json!({
            "project": {
                "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
            },
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>two</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(staged_or_retired(&root, &id).is_empty(), "the staged files are gone");
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>one</p>");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restoring_a_backup_first_backs_up_the_live_database() {
        let root = std::env::temp_dir().join(format!("konae-restore-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
        let deployment_id = state.deployments.read().await.values().next().unwrap().id.clone();
        let id = deployment_id.as_str();
        let path = database_path(&state, id);
        initialize_database(&path).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('old');")
            .unwrap();
        let backup = create_backup(&state, id, "Manual backup").await.unwrap();
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO notes VALUES ('newer')", [])
            .unwrap();
        restore_backup(&state, id, backup["id"].as_str().unwrap()).await.unwrap();

        let backups = list_backups(&state, id).await.unwrap();
        let before = backups
            .iter()
            .find(|item| item["label"] == "Before restore")
            .expect("restore must keep a copy of the live database");
        let snapshot = crate::layout::backups(&root, &id)
            .join(format!("{}.sqlite", before["id"].as_str().unwrap()));
        let rows: i64 = Connection::open(snapshot)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        let rows: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    pub fn signed_in(state: &AppState, deployment_id: &str, headers: &HeaderMap, email: &str) -> (HeaderMap, String) {
        let auth = AuthService::open(
            crate::layout::identity(&state
                .data_root, &deployment_id),
        )
        .unwrap();
        let session = auth.signup(email, "correct horse battery").unwrap();
        let mut headers = headers.clone();
        headers.insert(
            header::COOKIE,
            format!("konae_app_session={}", session.token).parse().unwrap(),
        );
        (headers, session.user.user_id)
    }

    pub async fn response_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn upload_refuses_to_overwrite_an_unreadable_file_index() {
        let root = std::env::temp_dir().join(format!("konae-upload-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
        let files = crate::layout::files(&root, &deployment.id);
        tokio::fs::create_dir_all(&files).await.unwrap();
        tokio::fs::write(files.join("index.json"), "{not json").await.unwrap();
        let body = json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        let response = upload_file(State(state.clone()), headers.clone(), request).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(tokio::fs::read(files.join("index.json")).await.unwrap(), b"{not json");

        tokio::fs::remove_file(files.join("index.json")).await.unwrap();
        let request = Request::builder().body(Body::from(body.to_string())).unwrap();
        let response = upload_file(State(state), headers, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json(response).await["result"].get("sha256").is_none());
        let index: Vec<Value> =
            serde_json::from_slice(&tokio::fs::read(files.join("index.json")).await.unwrap()).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0]["visibility"], "private");
        assert_eq!(index[0]["sha256"], digest(b"hi"));
        let leftovers = std::fs::read_dir(&files)
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn files_off_refuses_upload_listing_and_read_with_the_module_sentence() {
        let root = std::env::temp_dir().join(format!("konae-files-off-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let mut deployments = state.deployments.write().await;
        deployments.values_mut().next().unwrap().modules.get_mut("files").unwrap()["on"] = json!(false);
        drop(deployments);
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
        let expected = "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.";
        let upload = Request::builder().body(Body::from(json!({"name":"a.txt","type":"text/plain","dataUrl":"data:text/plain;base64,aGk="}).to_string())).unwrap();
        for response in [
            upload_file(State(state.clone()), headers.clone(), upload).await,
            list_files(State(state.clone()), headers.clone()).await,
            serve_file(State(state.clone()), AxumPath("missing".into()), headers.clone()).await,
        ] {
            let body = response_json(response).await;
            assert_eq!(body["error"], expected);
            assert_eq!(body["code"], "module.off");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn release_refuses_a_capsule_that_uses_files_while_the_module_is_off() {
        let state = AppState::for_test(&std::env::temp_dir().join(format!("konae-files-release-off-{}", Uuid::new_v4())));
        let body = json!({
            "project": {"id":"files_off","name":"Files off","slug":"files-off","manifest":{},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},
            "visibility":"public",
            "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<main>app</main>","app.js":"miniapp.files.upload(file)"}},
            "initialModules":{"files":{"on":false}}
        });
        let response = deploy(&state, body.to_string().as_bytes()).await;
        let refused = response_json(response).await;
        assert_eq!(refused["error"], "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.");
        assert_eq!(refused["code"], "module.off");
    }

    #[tokio::test]
    async fn files_are_private_to_the_uploader_unless_marked_public() {
        let root = std::env::temp_dir().join(format!("konae-files-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, alice_id) = signed_in(&state, &deployment.id, &headers, "alice@example.test");
        let (bob, _) = signed_in(&state, &deployment.id, &headers, "bob@example.test");
        let body = |visibility: Option<&str>| {
            let mut value = json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
            if let Some(visibility) = visibility {
                value["visibility"] = json!(visibility);
            }
            Request::builder().body(Body::from(value.to_string())).unwrap()
        };

        let anonymous = upload_file(State(state.clone()), headers.clone(), body(None)).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let private_file = response_json(upload_file(State(state.clone()), alice.clone(), body(None)).await).await;
        assert_eq!(private_file["result"]["visibility"], "private");
        assert_eq!(private_file["result"]["ownerId"], alice_id);
        let private_id = private_file["result"]["id"].as_str().unwrap().to_owned();

        let public_file = response_json(upload_file(State(state.clone()), alice.clone(), body(Some("public"))).await).await;
        let public_id = public_file["result"]["id"].as_str().unwrap().to_owned();

        let alice_list = response_json(list_files(State(state.clone()), alice.clone()).await).await;
        assert_eq!(alice_list["result"].as_array().unwrap().len(), 2);
        assert!(alice_list["result"].as_array().unwrap().iter().any(|file| file["ownerId"] == alice_id));

        let bob_list = response_json(list_files(State(state.clone()), bob.clone()).await).await;
        assert_eq!(bob_list["result"].as_array().unwrap().len(), 1);
        assert_eq!(bob_list["result"][0]["id"], public_id);
        assert!(bob_list["result"][0].get("ownerId").is_none());

        let visitor_list = response_json(list_files(State(state.clone()), headers.clone()).await).await;
        assert_eq!(visitor_list["result"][0]["id"], public_id);
        assert!(visitor_list["result"][0].get("ownerId").is_none());

        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id.clone()), bob.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id.clone()), headers.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(private_id), alice.clone()).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(public_id.clone()), headers).await.status(),
            StatusCode::OK
        );

        let legacy_id = Uuid::new_v4().to_string();
        let directory = crate::layout::files(&root, &deployment.id);
        tokio::fs::write(directory.join(format!("{legacy_id}.blob")), b"hi").await.unwrap();
        let mut index: Vec<Value> = serde_json::from_slice(&tokio::fs::read(directory.join("index.json")).await.unwrap()).unwrap();
        index.push(json!({"id": legacy_id, "name": "old.txt", "type": "text/plain", "size": 2, "createdAt": 1}));
        tokio::fs::write(directory.join("index.json"), serde_json::to_vec(&index).unwrap()).await.unwrap();
        let after_legacy = response_json(list_files(State(state.clone()), alice).await).await;
        assert!(after_legacy["result"].as_array().unwrap().iter().all(|file| file["id"] != legacy_id));
        assert_eq!(
            serve_file(State(state.clone()), AxumPath(legacy_id), bob).await.status(),
            StatusCode::NOT_FOUND
        );

        let (private_state, private_headers) = login_test_state(&root.join("private"), "private").await;
        let private_deployment = private_state.deployments.read().await.values().next().cloned().unwrap();
        assert_eq!(
            list_files(State(private_state.clone()), private_headers.clone()).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let (member, _) = signed_in(&private_state, &private_deployment.id, &private_headers, "member@example.test");
        assert_eq!(list_files(State(private_state), member).await.status(), StatusCode::OK);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    fn query_definition(statement: &str) -> ActionDefinition {
        let program: ActionProgram = serde_json::from_value(json!({"version": 1, "actions": {
            "rows.read": {"kind": "query", "access": "anonymous", "statement": statement, "bindings": []}
        }})).unwrap();
        program.actions["rows.read"].clone()
    }

    fn converted_after<T>(work: impl FnOnce() -> T) -> (T, usize) {
        COLUMNS_CONVERTED.with(|count| count.set(0));
        let value = work();
        (value, COLUMNS_CONVERTED.with(|count| count.get()))
    }



    const THREE_BLOBS: &str = "SELECT zeroblob(700000) a, zeroblob(700000) b, zeroblob(700000) c";

    #[test]
    fn an_action_result_stops_being_read_once_it_passes_2_mib() {
        let root = std::env::temp_dir().join(format!("konae-row-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap();
        let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(THREE_BLOBS), Vec::new()));
        assert_eq!(outcome.unwrap_err().to_string(), "Query result exceeds 2 MiB or 1000 rows.");
        assert_eq!(converted, 1, "the column that passes the cap is the last one read");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn browse_refuses_an_oversized_row_before_converting_its_values() {
        let root = std::env::temp_dir().join(format!("konae-browse-row-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE items(payload BLOB); INSERT INTO items VALUES (zeroblob(1100000));",
        ).unwrap();
        let (outcome, converted) = converted_after(|| browse_table(&path, &json!({"tableName": "items"})));
        assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
        assert_eq!(converted, 1, "the row id is accounted before the oversized payload is read");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn browse_keeps_its_row_shape_for_ordinary_values() {
        let root = std::env::temp_dir().join(format!("konae-browse-row-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE items(name TEXT, done INTEGER); INSERT INTO items VALUES ('task', 1);",
        ).unwrap();
        let (outcome, _) = converted_after(|| browse_table(&path, &json!({"tableName": "items"})));
        let result = outcome.unwrap();
        assert_eq!(result["totalRows"], 1);
        assert_eq!(result["rows"], json!([{"rowId": 1, "values": {"name": "task", "done": 1}}]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn action_result_budget_counts_column_names_before_converting_values() {
        let root = std::env::temp_dir().join(format!("konae-column-name-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap();
        let alias = "a".repeat(16_000);
        let statement = format!("WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM rows WHERE n < 140) SELECT NULL AS \"{alias}\" FROM rows");
        let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(&statement), Vec::new()));
        assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
        assert_eq!(converted, 130, "the oversized repeated column name is refused before value conversion");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn action_result_budget_counts_controls_in_lossy_text_before_conversion() {
        let root = std::env::temp_dir().join(format!("konae-invalid-text-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap();
        let statement = "SELECT CAST(zeroblob(350000) || x'ff' AS TEXT) AS payload";
        let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(statement), Vec::new()));
        assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
        assert_eq!(converted, 0, "the NUL escapes pass the byte cap before lossy text conversion");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn notify_refuses_an_oversized_returning_row_and_rolls_back() {
        let root = std::env::temp_dir().join(format!("konae-notify-row-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE items(payload BLOB);").unwrap();
        let definition: ActionDefinition = serde_json::from_value(json!({
            "kind": "mutation", "access": "anonymous",
            "statement": "INSERT INTO items(payload) VALUES (zeroblob(1100000))",
            "bindings": [],
            "notify": {"to": "owner", "changes": ["payload"], "subject": "Changed", "body": "{changes}"}
        })).unwrap();
        let (outcome, converted) = converted_after(|| run_mutation(&path, &definition, Vec::new()));
        assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
        assert_eq!(converted, 0, "the oversized returned payload is rejected before conversion");
        let count: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM items", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "the mutation rolls back when its notice result exceeds the budget");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn owner_sql_stops_reading_a_result_once_it_passes_2_mib() {
        let root = std::env::temp_dir().join(format!("konae-admin-row-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap();
        let (outcome, converted) = converted_after(|| run_admin_sql(&path, THREE_BLOBS));
        assert_eq!(outcome.unwrap_err().to_string(), "SQL result exceeds 2 MiB.");
        assert_eq!(converted, 1, "the column that passes the cap is the last one read");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_result_under_the_cap_comes_back_unchanged() {
        let root = std::env::temp_dir().join(format!("konae-row-small-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        Connection::open(&path).unwrap();
        let sql = "SELECT x'00ff10' AS b, 'héllo \"q\"' AS t, 7 AS n, 1.5 AS r, NULL AS z";
        let expected = json!({"b": "00ff10", "t": "héllo \"q\"", "n": 7, "r": 1.5, "z": null});
        assert_eq!(run_query(&path, &query_definition(sql), Vec::new()).unwrap(), vec![expected.clone()]);
        assert_eq!(run_admin_sql(&path, sql).unwrap()["rows"], json!([expected]));
        let _ = std::fs::remove_dir_all(root);
    }



    async fn statement_cap_state(root: &Path) -> (AppState, HeaderMap, String) {
        let (mut state, headers) = login_test_state(root, "public").await;
        let program = json!({"version": 1, "actions": {
            "items.count": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n", "bindings": []},
            "items.broken": {"kind": "query", "access": "anonymous", "statement": "SELECT * FROM missing_table", "bindings": []},
            "items.route": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n LIMIT 1", "bindings": [], "http": {"method": "GET", "path": "/api/count"}}
        }});
        let mut id = String::new();
        for deployment in state.deployments.write().await.values_mut() {
            deployment.actions = program.to_string();
            deployment.manifest = json!({"visibility": "public"});
            deployment.modules.get_mut("http").unwrap()["on"] = json!(true);
            initialize_database(&database_path(&state, &deployment.id)).unwrap();
            id = deployment.id.clone();
        }
        state.guards = Arc::new(Guards::new(Vec::new()).with_statement_wait(std::time::Duration::from_millis(60)));
        (state, headers, id)
    }

    async fn hold_statements(state: &AppState, id: &str, count: usize) -> Vec<tokio::sync::OwnedSemaphorePermit> {
        let mut held = Vec::new();
        for _ in 0..count {
            held.push(state.guards.statement_permit(id).await.unwrap());
        }
        held
    }

    async fn call_action(state: &AppState, headers: &HeaderMap, name: &str) -> Response {
        invoke_action(State(state.clone()), AxumPath(name.into()), headers.clone(), Json(json!({}))).await
    }

    #[tokio::test]
    async fn a_fifth_action_statement_on_one_deployment_is_refused_while_another_is_served() {
        let root = std::env::temp_dir().join(format!("konae-statement-cap-{}", Uuid::new_v4()));
        let (state, headers, id) = statement_cap_state(&root).await;
        let _elsewhere = hold_statements(&state, "another-deployment", 4).await;
        assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK, "another Deployment's slots do not count");
        let mut held = hold_statements(&state, &id, 4).await;
        let refused = call_action(&state, &headers, "items.count").await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(refused.headers().contains_key(header::RETRY_AFTER));
        let body = axum::body::to_bytes(refused.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], "rate-limited");
        held.pop();
        assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK, "a freed slot lets the next call run");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn an_action_statement_that_errors_releases_its_permit() {
        let root = std::env::temp_dir().join(format!("konae-statement-error-{}", Uuid::new_v4()));
        let (state, headers, id) = statement_cap_state(&root).await;
        let _held = hold_statements(&state, &id, 3).await;
        for _ in 0..5 {
            assert_eq!(call_action(&state, &headers, "items.broken").await.status(), StatusCode::BAD_REQUEST);
        }
        assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn the_runtime_endpoint_answers_a_refused_statement_with_the_rate_limited_tool_error() {
        let root = std::env::temp_dir().join(format!("konae-statement-runtime-{}", Uuid::new_v4()));
        let (state, _, id) = statement_cap_state(&root).await;
        let _held = hold_statements(&state, &id, 4).await;
        let request = Request::builder()
            .method("POST")
            .uri("/_konae/mcp")
            .header(header::HOST, "login-test.localhost")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "actions_invoke", "arguments": {"name": "items.count"}}}).to_string()))
            .unwrap();
        let response = runtime::runtime(State(state.clone()), request).await;
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(body["result"]["isError"], true);
        assert_eq!(body["result"]["content"][0]["text"], "rate-limited");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn an_http_route_answers_a_refused_statement_with_the_rate_limited_response() {
        let root = std::env::temp_dir().join(format!("konae-statement-route-{}", Uuid::new_v4()));
        let (state, headers, id) = statement_cap_state(&root).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let client: std::net::IpAddr = "192.0.2.1".parse().unwrap();
        let _held = hold_statements(&state, &id, 4).await;
        let refused = http::call_route(&state, &deployment, "GET", "/api/count", "", b"", &headers, client).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(refused.headers().contains_key(header::RETRY_AFTER));
        let body = axum::body::to_bytes(refused.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], "rate-limited");
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
