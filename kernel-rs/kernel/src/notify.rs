
use anyhow::Context;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use super::mail::{self, Notice, NoticeOutcome};
use super::refusal::{refuse, Refusal};
use super::{ActionDefinition, AppState, Deployment};

pub const CONTACT_EMAIL_INVALID: &str = "The contact email is not a valid email address.";
const INTENT_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS __konae_notify_intents (id TEXT PRIMARY KEY, payload_json TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('pending','handed_off','paused')), created_at INTEGER NOT NULL) WITHOUT ROWID";
const MAX_PENDING_INTENTS_PER_APP: i64 = 1000;

#[derive(Clone)]
pub struct MutationContext {
    pub app: String,
    pub release: String,
    pub action: String,
    pub input: Value,
    pub account: Option<String>,
    pub actor_id: Option<String>,
    pub at: u64,
    pub documents_rule: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct IntentPayload {
    app: String,
    release: String,
    action: String,
    declaration: Value,
    input: Value,
    account: Option<String>,
    actor_id: Option<String>,
    count: u64,
    rows: Vec<Map<String, Value>>,
    at: u64,
}

pub struct IntentEffect {
    pub id: String,
    payload: IntentPayload,
}

impl IntentEffect {
    pub fn new(id: String, action: &ActionDefinition, context: MutationContext, changed: &crate::Ran) -> anyhow::Result<Self> {
        let declaration = action.notify.clone().ok_or_else(|| anyhow::anyhow!("Notify declaration is missing."))?;
        Ok(Self { id, payload: IntentPayload {
            app: context.app, release: context.release, action: context.action, declaration,
            input: context.input, account: context.account, actor_id: context.actor_id,
            count: changed.answer["changes"].as_u64().unwrap_or(0), rows: changed.rows.clone(), at: context.at,
        } })
    }
}

impl crate::execution::TransactionEffect for IntentEffect {
    fn prepare(&self, connection: &Connection) -> rusqlite::Result<()> {
        ensure_intent_schema(connection)?;
        let pending: i64 = connection.query_row("SELECT COUNT(*) FROM __konae_notify_intents WHERE state='pending'", [], |row| row.get(0))?;
        if pending >= MAX_PENDING_INTENTS_PER_APP { return Err(rusqlite::Error::InvalidQuery); }
        let payload = serde_json::to_string(&self.payload).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        connection.execute("INSERT INTO __konae_notify_intents (id,payload_json,state,created_at) VALUES (?1,?2,'pending',?3)",
            params![self.id, payload, self.payload.at as i64])?;
        Ok(())
    }
}

fn ensure_intent_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(INTENT_SCHEMA)?;
    let has_transfer_state = connection.prepare("SELECT 1 FROM pragma_table_info('__konae_notify_intents') WHERE name='transfer_state'")?.exists([])?;
    if !has_transfer_state {
        connection.execute("ALTER TABLE __konae_notify_intents ADD COLUMN transfer_state TEXT", [])?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS __konae_notify_pending
           ON __konae_notify_intents(state,created_at,id);",
    )?;
    Ok(())
}

/// Pauses pending deliveries in an exported database until the receiving Kernel imports it.
pub(crate) fn pause_exported_intents(path: &std::path::Path) -> anyhow::Result<()> {
    let connection = rusqlite::Connection::open(path)?;
    if !connection.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents'")?.exists([])? { return Ok(()) }
    ensure_intent_schema(&connection)?;
    connection.execute("UPDATE __konae_notify_intents SET transfer_state='paused' WHERE state='pending'", [])?;
    Ok(())
}

/// Marks pending deliveries in the staged database ready for the receiving Kernel.
pub(crate) fn resume_imported_intents(path: &std::path::Path) -> anyhow::Result<()> {
    let connection = rusqlite::Connection::open(path)?;
    if !connection.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents'")?.exists([])? { return Ok(()) }
    ensure_intent_schema(&connection)?;
    connection.execute("UPDATE __konae_notify_intents SET transfer_state=NULL WHERE state='pending' AND transfer_state='paused'", [])?;
    Ok(())
}

/// Holds an app in maintenance while its portable state crosses an installation boundary.
#[must_use = "Keep the hold alive until the app transfer has finished."]
pub(crate) struct TransferHold {
    data_root: PathBuf,
    app: String,
    owned: bool,
}

impl TransferHold {
    pub(crate) fn begin(data_root: &Path, app: &str) -> anyhow::Result<Self> {
        let already_held = crate::maintenance::read(data_root, app)["maintenance"] == true;
        if !already_held {
            crate::maintenance::set(data_root, app, true)?;
        }
        Ok(Self { data_root: data_root.to_path_buf(), app: app.to_owned(), owned: !already_held })
    }
}

impl Drop for TransferHold {
    fn drop(&mut self) {
        if self.owned
            && let Err(error) = crate::maintenance::set(&self.data_root, &self.app, false)
        {
            eprintln!("Could not restore maintenance after app transfer: {error}");
        }
    }
}

/// Copies terminal mail outcomes into the app before export or replacement.
pub(crate) fn prepare_transfer(state: &AppState, app: &str) -> anyhow::Result<()> {
    let scope = super::auth::deployment_scope(app);
    if let Some(queue) = state.accounts.queue.as_ref() {
        queue.prepare_app_transfer(&scope)?;
    }
    let path = crate::layout::data(&state.data_root, app).join("database.sqlite");
    ensure_transfer_results(&path)
}

fn ensure_transfer_results(path: &Path) -> anyhow::Result<()> {
    if !path.is_file() { return Ok(()); }
    let connection = Connection::open(path)?;
    if !connection.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents'")?.exists([])? {
        return Ok(());
    }
    let mut statement = connection.prepare(
        "SELECT id,payload_json FROM __konae_notify_intents WHERE state='handed_off'",
    )?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (id, payload) = row?;
        let payload: Value = serde_json::from_str(&payload)?;
        if payload["results"].as_object().is_some_and(|results| results.values().any(|result| result == "queued")) {
            anyhow::bail!("App transfer is blocked because notice intent {id} still has an unconfirmed mail receipt. Keep the app on this installation until mail.sqlite records a terminal result.");
        }
    }
    Ok(())
}

/// Refuses an older or incomplete package whose handed-off work lacks a terminal receipt.
pub(crate) fn validate_imported_intents(path: &Path) -> anyhow::Result<()> {
    ensure_transfer_results(path)
}

/// Reads the committed intent and runs the existing mail handoff from its stored payload.
pub async fn record_intent(state: &AppState, deployment: &Deployment, _id: &str) {
    state.dispatcher.wake(crate::execution::WorkResource::Notices, deployment.id.clone()).await;
}

pub(crate) async fn handle_intent(state: &AppState, deployment: &Deployment, id: &str) {
    let _data = crate::database_lock(state, &deployment.id).await;
    if crate::maintenance::pause(&state.data_root, &deployment.id).is_some() { return; }
    let path = crate::layout::data(&state.data_root, &deployment.id).join("database.sqlite");
    let intent_id = id.to_owned();
    let query_id = intent_id.clone();
    let payload = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<IntentPayload>> {
        let connection = Connection::open(&path)?;
        ensure_intent_schema(&connection)?;
        let row: Option<String> = connection.query_row(
            "SELECT payload_json FROM __konae_notify_intents WHERE id=?1 AND state='pending' AND COALESCE(transfer_state,'')<>'paused'", [query_id], |row| row.get::<_, String>(0)
        ).optional()?;
        row.map(|json| serde_json::from_str(&json).map_err(anyhow::Error::from)).transpose()
    }).await;
    let payload = match payload {
        Ok(Ok(Some(payload))) => payload,
        Ok(Ok(None)) => return,
        Ok(Err(error_value)) => { eprintln!("Notify intent {id} could not be read: {error_value}"); return; },
        Err(error_value) => { eprintln!("Notify intent read task {id} failed: {error_value}"); return; },
    };
    if payload.app != deployment.id { return; }
    let bindings = payload.input.as_object().into_iter().flat_map(|input| input.keys()).map(|key| crate::ActionBinding {
        source: "input".into(), key: Some(key.clone()), value_type: Some("string".into()), max_length: None,
        nullable: true, field: None, value: None,
    }).collect();
    let action = ActionDefinition {
        kind: "mutation".into(), access: "anonymous".into(), statement: String::new(), bindings,
        http: None, page: None, signal: None, guard: None, notify: Some(payload.declaration),
        schedule: None, issue: None, subscribe: None, call: Default::default(), extra: Default::default(),
    };
    let Some(queue) = state.accounts.queue.as_ref() else { return };
    let Ok(Some(declared)) = declaration(&payload.action, &action) else { return };
    let creator = super::accounts::project_creator(state, &deployment.project_id).unwrap_or_else(|| "operator".into());
    let all: Vec<&Map<String, Value>> = payload.rows.iter().collect();
    let scope = super::auth::deployment_scope(&deployment.id);
    let failed = std::cell::Cell::new(false);
    let send = |account_id: Option<&str>, recipient: &str, rows: &[&Map<String, Value>], count: u64| {
        let values = Fill { input: &payload.input, actor: payload.actor_id.as_deref(), now: payload.at,
            columns: &declared.changes, rows, count };
        let (subject, text, html) = render(&declared, &deployment.project_name, &values);
        let notice = Notice { recipient, scope: &scope, creator: &creator, app_name: &deployment.project_name,
            action: &payload.action, subject: &subject, text: &text, html: &html,
            account_id, intent_id: Some(intent_id.as_str()) };
        match queue.record_notice_intent(&notice) {
            Ok(NoticeOutcome::Queued) => {},
            Ok(NoticeOutcome::Refused(kind)) => {
                if record_delivery_result(&state.data_root, &scope, &intent_id, recipient, &format!("refused:{kind}")).is_err() {
                    failed.set(true);
                }
            },
            Err(error_value) => { failed.set(true); eprintln!("Notify intent {id} remains pending: {error_value}"); },
        }
    };
    let refuse_recipient = |recipient: &str, reason: &str| {
        if record_delivery_result(&state.data_root, &scope, &intent_id, recipient, reason).is_err() { failed.set(true); }
    };
    match &declared.to {
        Recipient::Owner => if let Some(email) = deployment.contact_email.as_deref() {
            send(None, email, &all, payload.count);
        } else { refuse_recipient("", "refused:owner-has-no-email"); },
        Recipient::Actor => if let Some(account) = payload.account.as_deref() {
            match verified_email(state, &deployment.id, account) {
                Ok(Some(email)) => send(Some(account), &email, &all, payload.count),
                _ => refuse_recipient(account, "refused:unverified-email"),
            }
        } else { refuse_recipient("", "refused:no-account"); },
        Recipient::Column(column) => {
            for (index, (account, rows)) in recipients(column, &payload.rows).into_iter().enumerate() {
                if index >= MAX_RECIPIENTS_PER_RUN {
                    refuse_recipient(account, "refused:recipient-cap");
                    continue;
                }
                match verified_email(state, &deployment.id, account) {
                    Ok(Some(email)) => send(Some(account), &email, &rows, rows.len() as u64),
                    _ => refuse_recipient(account, "refused:not-a-verified-account"),
                }
            }
        }
    }
    if !failed.get() {
        let path = crate::layout::data(&state.data_root, &deployment.id).join("database.sqlite");
        if let Ok(connection) = Connection::open(&path) {
            let _ = connection.execute("UPDATE __konae_notify_intents SET state='handed_off' WHERE id=?1 AND state='pending'", [intent_id]);
        }
    }
}

/// Starts the notice handler. It receives only ready apps and claims one
/// durable intent at a time. The mail queue has its own worker and capacity.
pub fn spawn_worker(state: AppState) {
    if state.accounts.queue.is_none() {
        return;
    }
    tokio::spawn(async move {
        wake_pending(&state).await;
        let mut timers = crate::execution::AppTimers::default();
        loop {
            let turn = match timers.next_deadline() {
                Some(deadline) => tokio::select! {
                    turn = state.dispatcher.next_turn(crate::execution::WorkResource::Notices, crate::execution::WorkLimits::SERIAL) => turn,
                    _ = tokio::time::sleep_until(deadline.into()) => {
                        for due in timers.take_due(std::time::Instant::now()) {
                            state.dispatcher.wake(crate::execution::WorkResource::Notices, due).await;
                        }
                        continue;
                    }
                },
                None => state.dispatcher.next_turn(crate::execution::WorkResource::Notices, crate::execution::WorkLimits::SERIAL).await,
            };
            let app = turn.app().to_owned();
            timers.cancel(&app);
            let Some(deployment) = state.deployments.read().await.values().find(|item| item.id == app).cloned() else {
                timers.remove(&app);
                continue;
            };
            if crate::maintenance::pause(&state.data_root, &app).is_some() {
                timers.reset_retries(&app);
                timers.schedule(app, std::time::Instant::now() + std::time::Duration::from_secs(60));
                continue;
            }
            let root = state.data_root.clone();
            let query_app = app.clone();
            let id = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
                let path = crate::layout::data(&root, &query_app).join("database.sqlite");
                if !path.is_file() { return Ok(None); }
                let connection = Connection::open(path)?;
                ensure_intent_schema(&connection)?;
                connection.query_row("SELECT id FROM __konae_notify_intents WHERE state='pending' AND COALESCE(transfer_state,'')<>'paused' ORDER BY created_at,id LIMIT 1", [], |row| row.get(0)).optional().map_err(Into::into)
            }).await;
            let mut claim_failed = false;
            let intent_id = match id {
                Ok(Ok(Some(id))) => Some(id),
                Ok(Ok(None)) => None,
                Ok(Err(error_value)) => {
                    eprintln!("Notify intent for {app} could not be claimed: {error_value}");
                    claim_failed = true;
                    None
                }
                Err(error_value) => {
                    eprintln!("Notify intent claim for {app} failed: {error_value}");
                    claim_failed = true;
                    None
                }
            };
            if let Some(intent_id) = intent_id.as_deref() {
                handle_intent(&state, &deployment, intent_id).await;
            }
            let root = state.data_root.clone();
            let query_app = app.clone();
            let query_intent = intent_id.clone();
            let remains = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
                let path = crate::layout::data(&root, &query_app).join("database.sqlite");
                if !path.is_file() { return Ok(false); }
                let connection = Connection::open(path)?;
                let schema = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents')", [], |row| row.get::<_, bool>(0))?;
                if !schema { return Ok(false); }
                let completed = match query_intent {
                    Some(id) => connection.query_row("SELECT EXISTS(SELECT 1 FROM __konae_notify_intents WHERE id=?1 AND state='handed_off')", [id], |row| row.get::<_, bool>(0))?,
                    None => false,
                };
                let pending = connection.query_row("SELECT EXISTS(SELECT 1 FROM __konae_notify_intents WHERE state='pending' AND COALESCE(transfer_state,'')<>'paused')", [], |row| row.get::<_, bool>(0))?;
                Ok(pending && !completed)
            }).await;
            if claim_failed {
                timers.schedule_retry(app, std::time::Instant::now());
                continue;
            }
            match remains {
                Ok(Ok(true)) => timers.schedule_retry(app, std::time::Instant::now()),
                Ok(Ok(false)) => {
                    timers.reset_retries(&app);
                    if intent_id.is_some() {
                        state.dispatcher.wake(crate::execution::WorkResource::Notices, app).await;
                    }
                }
                Ok(Err(error_value)) => {
                    eprintln!("Notify intent status for {app} could not be read: {error_value}");
                    timers.schedule_retry(app, std::time::Instant::now());
                }
                Err(error_value) => {
                    eprintln!("Notify intent status task for {app} failed: {error_value}");
                    timers.schedule_retry(app, std::time::Instant::now());
                }
            }
        }
    });
}

/// Queues apps with portable pending notices after startup. The scan runs
/// once; later commits and imports signal the same queue directly.
pub(crate) async fn wake_pending(state: &AppState) {
    let deployments: Vec<Deployment> = state.deployments.read().await.values().cloned().collect();
    for deployment in deployments {
        let root = state.data_root.clone();
        let app = deployment.id.clone();
        let pending = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let path = crate::layout::data(&root, &app).join("database.sqlite");
            if !path.is_file() { return Ok(false); }
            let connection = Connection::open(path)?;
            let exists = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents')", [], |row| row.get::<_, bool>(0))?;
            if !exists { return Ok(false); }
            ensure_intent_schema(&connection)?;
            Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM __konae_notify_intents WHERE state='pending' AND COALESCE(transfer_state,'')<>'paused')", [], |row| row.get(0))?)
        }).await;
        match pending {
            Ok(Ok(false)) => {},
            Ok(Ok(true)) => state.dispatcher.wake(crate::execution::WorkResource::Notices, deployment.id).await,
            Ok(Err(error_value)) => {
                eprintln!("Notify startup scan for {} failed: {error_value}", deployment.id);
                state.dispatcher.wake(crate::execution::WorkResource::Notices, deployment.id).await;
            }
            Err(error_value) => {
                eprintln!("Notify startup scan task for {} failed: {error_value}", deployment.id);
                state.dispatcher.wake(crate::execution::WorkResource::Notices, deployment.id).await;
            }
        }
    }
}

/// Mirrors the installation queue outcome into the app's portable intent record.
pub(crate) fn record_delivery_result(data_root: &std::path::Path, scope: &str, intent_id: &str, recipient: &str, result: &str) -> anyhow::Result<Option<bool>> {
    let Some(app) = scope.strip_prefix("deployment:") else { return Ok(None) };
    let path = crate::layout::data(data_root, app).join("database.sqlite");
    if !path.is_file() { return Ok(None) }
    let mut connection = rusqlite::Connection::open(&path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    anyhow::ensure!(transaction.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_notify_intents'")?.exists([])?,
        "Mail receipt for notice intent {intent_id} cannot be copied because the app database has no portable notice table.");
    let payload = transaction.query_row(
        "SELECT payload_json FROM __konae_notify_intents WHERE id=?1", [intent_id], |row| row.get::<_, String>(0),
    ).optional()?.with_context(|| format!("Mail receipt for notice intent {intent_id} cannot be copied because its app intent is missing."))?;
    let mut payload: Value = serde_json::from_str(&payload)?;
    if payload["results"][recipient] == result {
        transaction.commit()?;
        return Ok(Some(false));
    }
    if !payload["results"].is_object() { payload["results"] = Value::Object(Map::new()); }
    payload["results"][recipient] = Value::String(result.to_owned());
    let json = serde_json::to_string(&payload)?;
    let changed = transaction.execute("UPDATE __konae_notify_intents SET payload_json=?1 WHERE id=?2", rusqlite::params![json, intent_id])?;
    anyhow::ensure!(changed == 1, "Mail receipt for notice intent {intent_id} did not reach the app database.");
    transaction.commit()?;
    Ok(Some(true))
}

/// The actions of `actions` (an `actions.json`) whose notice goes to the
/// owner, by name.
pub fn owner_notices(actions: &str) -> Vec<String> {
    let program: Value = serde_json::from_str(actions).unwrap_or(Value::Null);
    let mut names: Vec<String> = program["actions"]
        .as_object()
        .map(|actions| actions.iter().filter(|(_, action)| action["notify"]["to"] == "owner").map(|(name, _)| name.clone()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Where the app's owner notices go, and which actions send them, as
/// `contact_email_read` answers.
pub fn contact_view(deployment: &Deployment) -> Value {
    serde_json::json!({
        "deploymentId": deployment.id,
        "contactEmail": deployment.contact_email,
        "ownerNotices": owner_notices(&deployment.actions),
    })
}

/// Sets the app's contact email in `konae.json`, or clears it with `None`,
/// and answers as `contact_view`. The caller holds the project lock.
pub async fn set_contact_email(state: &AppState, deployment: &Deployment, email: Option<&str>) -> anyhow::Result<Value> {
    let email = email
        .map(|email| super::auth::normalize_email(email).map_err(|_| anyhow::anyhow!(CONTACT_EMAIL_INVALID)))
        .transpose()?;
    let updated = Deployment { contact_email: email, ..deployment.clone() };
    super::persist_deployment(state, &updated).await?;
    state.deployments.write().await.insert(updated.hostname.clone(), updated.clone());
    Ok(contact_view(&updated))
}

pub const MAX_SUBJECT_CHARS: usize = 120;
pub const MAX_BODY_CHARS: usize = 2000;
const MAX_VALUE_CHARS: usize = 200;
const MAX_RENDERED_SUBJECT_CHARS: usize = 200;
const MAX_RENDERED_BODY_CHARS: usize = 5000;

const MAX_CHANGES_COLUMNS: usize = 8;
/// Changed rows the Kernel reads back from one run; the rest are counted
/// in `{changes.count}` but not listed or addressed.
pub const MAX_RETURNED_ROWS: usize = 1000;
const MAX_LISTED_ROWS: usize = 50;
/// Distinct accounts one run notifies through a column (ADR-0012).
pub const MAX_RECIPIENTS_PER_RUN: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    Owner,
    Actor,
    Column(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Input(String),
    ActorId,
    KernelNow,
    Changes,
    ChangesCount,
}

#[derive(Debug)]
pub struct Declaration {
    pub to: Recipient,
    changes: Vec<String>,
    subject: Vec<Part>,
    body: Vec<Part>,
}

impl Declaration {
    /// The columns the Kernel appends as RETURNING: the `changes` list, then
    /// the recipient column when the list does not hold it. Empty when the
    /// notice needs no row.
    pub fn returning(&self) -> Vec<String> {
        let mut columns = self.changes.clone();
        if let Recipient::Column(column) = &self.to {
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        }
        columns
    }
}

fn is_column_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value.len() <= 64
}

/// Parses and checks one action's `notify` key. `Ok(None)` when it has none.
pub fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Declaration>, Refusal> {
    let Some(value) = action.notify.as_ref() else { return Ok(None) };
    if action.kind == "query" {
        return Err(refuse("notify.not-mutation", format!("Action {name} is a query, so it cannot declare notify. Declare it on a mutation.")));
    }
    if action.kind != "mutation" {
        return Err(refuse("notify.not-mutation", format!("Action {name} is a signal action, so it cannot declare notify. Declare it on a mutation.")));
    }
    let record = value
        .as_object()
        .filter(|record| record.keys().all(|key| matches!(key.as_str(), "to" | "subject" | "body" | "changes")))
        .filter(|record| record.get("subject").is_some_and(Value::is_string) && record.get("body").is_some_and(Value::is_string))
        .ok_or_else(|| refuse("notify.invalid", format!(
            r#"Action {name} has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else."#
        )))?;
    let column = record
        .get("to")
        .and_then(Value::as_object)
        .filter(|to| to.len() == 1)
        .and_then(|to| to.get("column"))
        .and_then(Value::as_str)
        .filter(|column| is_column_name(column));
    let to = match (record.get("to").and_then(Value::as_str), column) {
        (Some("owner"), _) => Recipient::Owner,
        (Some("actor"), _) => Recipient::Actor,
        (None, Some(column)) => Recipient::Column(column.to_owned()),
        _ => return Err(refuse("notify.recipient-invalid", format!(
            r#"Action {name} has an invalid notify recipient. Use "owner", "actor" or {{"column": "<name>"}} for a column of account IDs."#
        ))),
    };
    let changes = match record.get("changes") {
        None => Vec::new(),
        Some(value) => {
            let columns: Option<Vec<String>> = value
                .as_array()
                .filter(|list| (1..=MAX_CHANGES_COLUMNS).contains(&list.len()))
                .and_then(|list| list.iter().map(|item| item.as_str().filter(|c| is_column_name(c)).map(str::to_owned)).collect());
            match columns {
                Some(columns) if columns.iter().enumerate().all(|(index, column)| !columns[..index].contains(column)) => columns,
                _ => return Err(refuse("notify.changes-invalid", format!(
                    "Action {name} has an invalid notify changes list. Name 1 to 8 columns of the changed table, each once, in letters, digits and underscores."
                ))),
            }
        }
    };
    let subject = record["subject"].as_str().unwrap_or_default();
    if subject.trim().is_empty() || subject.chars().count() > MAX_SUBJECT_CHARS || subject.chars().any(is_control) {
        return Err(refuse("notify.subject-invalid", format!("Action {name} has an invalid notify subject. Use 1 to 120 characters on one line.")));
    }
    let body = record["body"].as_str().unwrap_or_default();
    if body.trim().is_empty() || body.chars().count() > MAX_BODY_CHARS || body.chars().any(|c| c != '\n' && is_control(c)) {
        return Err(refuse("notify.body-invalid", format!(
            "Action {name} has an invalid notify body. Use 1 to 2000 characters, with line breaks and no other control characters."
        )));
    }
    let inputs: Vec<&str> = action
        .bindings
        .iter()
        .filter(|binding| binding.source == "input")
        .filter_map(|binding| binding.key.as_deref())
        .collect();
    let subject = parts(name, "subject", subject, &inputs)?;
    let body = parts(name, "body", body, &inputs)?;
    if subject.contains(&Part::Changes) {
        return Err(refuse("notify.changes-in-subject", format!(
            "Action {name} puts {{changes}} in its notify subject. Put {{changes}} in the body, and {{changes.count}} in the subject."
        )));
    }
    if body.contains(&Part::Changes) && changes.is_empty() {
        return Err(refuse("notify.changes-undeclared", format!(
            r#"Action {name} uses {{changes}} in its notify body without a changes list. Name the columns to show in notify "changes"."#
        )));
    }
    Ok(Some(Declaration { to, changes, subject, body }))
}

fn is_control(c: char) -> bool {
    (c as u32) < 0x20 || c as u32 == 0x7f
}

fn parts(name: &str, field: &str, template: &str, inputs: &[&str]) -> Result<Vec<Part>, Refusal> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(Part::Text(rest[..open].to_owned()));
        }
        let Some(close) = rest[open..].find('}') else {
            return Err(refuse("notify.placeholder-unclosed", format!(r#"Action {name} has an unclosed placeholder in its notify {field}. Close it with "}}"."#)));
        };
        let token = &rest[open + 1..open + close];
        let part = match token.strip_prefix("input.") {
            Some(key) if inputs.contains(&key) => Part::Input(key.to_owned()),
            _ if token == "actor.id" => Part::ActorId,
            _ if token == "kernel.now" => Part::KernelNow,
            _ if token == "changes" => Part::Changes,
            _ if token == "changes.count" => Part::ChangesCount,
            _ => {
                return Err(refuse("notify.placeholder-unknown", format!(
                    "Action {name} names an unknown placeholder {{{token}}} in its notify {field}. Use {{input.<key>}} for one of its input bindings, {{actor.id}}, {{kernel.now}}, {{changes}} or {{changes.count}}."
                )));
            }
        };
        parts.push(part);
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_owned()));
    }
    Ok(parts)
}

/// The statement with the Kernel's own RETURNING clause for these columns.
/// A trailing `;` and what follows it are cut; a trailing line comment ends
/// at the line break before the clause. The author still may not write
/// RETURNING (`mutation.returning`).
pub fn with_returning(statement: &str, columns: &[String]) -> String {
    let tokens = super::sql_tokens(statement);
    let end = tokens.iter().find(|token| token.is_symbol(";")).map_or(statement.chars().count(), |token| token.start);
    let kept: String = statement.chars().take(end).collect();
    let list = columns.iter().map(|column| format!("[{column}]")).collect::<Vec<_>>().join(", ");
    format!("{}\nRETURNING {list}", kept.trim_end())
}

/// The columns a mutation's notice reads back, or none when it lists no
/// rows and addresses no column.
pub fn returning(name: &str, action: &ActionDefinition) -> Vec<String> {
    match declaration(name, action) {
        Ok(Some(declared)) => declared.returning(),
        _ => Vec::new(),
    }
}

/// Refuses a `changes` or `to` column the statement cannot return, against
/// the schema `connection` holds. The Validation Gate makes the same check
/// in src/agent/gate.ts, and kernel-rs/kernel/src/notify-column-cases.json holds the
/// cases both run.
pub fn check_columns(connection: &Connection, name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    let columns = returning(name, action);
    if columns.is_empty() {
        return Ok(());
    }
    let returns = connection
        .prepare(&with_returning(&action.statement, &columns))
        .is_ok_and(|statement| statement.column_count() == columns.len());
    if returns {
        Ok(())
    } else {
        Err(refuse("notify.column-unknown", format!(
            "Action {name} names a notify column its statement cannot return. Name columns of the table the statement changes."
        )))
    }
}

/// `{kernel.now}`: the run's time as `YYYY-MM-DD HH:MM UTC`.
pub fn format_now(now_ms: u64) -> String {
    let minutes = now_ms / 60_000;
    let (year, month, day) = super::copies::civil_from_days((minutes / 1440) as i64);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", minutes % 1440 / 60, minutes % 60)
}

/// What the statement changed, as a notice reads it.
pub struct Changed<'a> {
    /// Rows the statement changed, as SQLite counts them.
    pub count: u64,
    /// The first rows the Kernel read back, with the RETURNING columns.
    pub rows: &'a [Map<String, Value>],
    /// The time `{kernel.now}` shows, in milliseconds.
    pub now: u64,
}

struct Fill<'a> {
    input: &'a Value,
    actor: Option<&'a str>,
    now: u64,
    columns: &'a [String],
    rows: &'a [&'a Map<String, Value>],
    count: u64,
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => mail::one_line(value, MAX_VALUE_CHARS),
        Some(Value::Null) | None => String::new(),
        Some(value) => mail::one_line(&value.to_string(), MAX_VALUE_CHARS),
    }
}

fn listed(fill: &Fill<'_>) -> String {
    let mut lines: Vec<String> = fill
        .rows
        .iter()
        .take(MAX_LISTED_ROWS)
        .map(|row| {
            let values: Vec<String> = fill.columns.iter().map(|column| value_text(row.get(column))).collect();
            format!("- {}", values.join(" · "))
        })
        .collect();
    let more = fill.count.saturating_sub(lines.len() as u64);
    if more > 0 {
        lines.push(format!("- and {more} more"));
    }
    lines.join("\n")
}

/// Fills a template. A value loses its line breaks and keeps at most 200
/// characters; a missing or null input and a caller without an ID fill
/// nothing.
fn fill(parts: &[Part], values: &Fill<'_>) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            Part::Text(value) => text.push_str(value),
            Part::Input(key) => text.push_str(&value_text(values.input.get(key))),
            Part::ActorId => text.push_str(values.actor.unwrap_or_default()),
            Part::KernelNow => text.push_str(&format_now(values.now)),
            Part::Changes => text.push_str(&listed(values)),
            Part::ChangesCount => text.push_str(&values.count.to_string()),
        }
    }
    text
}

fn render(declared: &Declaration, app_name: &str, values: &Fill<'_>) -> (String, String, String) {
    let app = mail::one_line(app_name, 80);
    let app = if app.is_empty() { "Konae".to_owned() } else { app };
    let subject = mail::one_line(&fill(&declared.subject, values), MAX_RENDERED_SUBJECT_CHARS);
    let subject = if subject.is_empty() { format!("Notice from {app}") } else { subject };
    let body: String = fill(&declared.body, values).trim().chars().take(MAX_RENDERED_BODY_CHARS).collect();
    let why = match declared.to {
        Recipient::Owner => format!("{app} sent this notice through Konae because you own the app."),
        Recipient::Actor => format!("{app} sent this notice through Konae after something you did in the app."),
        Recipient::Column(_) => format!("{app} sent this notice through Konae because a change in the app names your account."),
    };
    let text = format!("{body}\n\n-- \n{why}\n");
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;line-height:1.5;color:#20232a\"><p>{}</p><p style=\"color:#586174\">{}</p></body></html>",
        mail::escape_html(&body).replace('\n', "<br>"),
        mail::escape_html(&why),
    );
    (subject, text, html)
}

/// Who called the mutation, as the notice needs it.
pub struct Caller<'a> {
    /// The app account behind a session. A visitor, a signed request and an
    /// API key have none.
    pub account: Option<&'a str>,
    /// The ID `{actor.id}` fills: the account's, or an API key's.
    pub actor_id: Option<&'a str>,
}

/// Records the notices of a mutation that changed a row, and counts each
/// outcome in Usage under `notify`: `<action>: queued`, or `<action>:
/// <reason>` when nothing will be sent. Call it only after the statement
/// changed a row. It never fails the mutation.
pub async fn record(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    action: &ActionDefinition,
    input: &Value,
    caller: Caller<'_>,
    changed: Changed<'_>,
) {
    let Ok(Some(declared)) = declaration(name, action) else { return };
    let state = state.clone();
    let deployment = deployment.clone();
    let name = name.to_owned();
    let input = input.clone();
    let account = caller.account.map(str::to_owned);
    let actor_id = caller.actor_id.map(str::to_owned);
    let (count, rows, now) = (changed.count, changed.rows.to_vec(), changed.now);
    let outcome = tokio::task::spawn_blocking(move || {
        let run = Changed { count, rows: &rows, now };
        let outcomes = queue_notices(&state, &deployment, &name, &declared, &input, account.as_deref(), actor_id.as_deref(), &run)
            .unwrap_or_else(|error_value| {
                eprintln!("Notice of {name} on {} could not be recorded: {error_value}", deployment.id);
                vec![NoticeOutcome::Refused("storage-error")]
            });
        for outcome in outcomes {
            let reason = match outcome {
                NoticeOutcome::Queued => "queued",
                NoticeOutcome::Refused(kind) => kind,
            };
            super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Notify(format!("{name}: {reason}")));
        }
    })
    .await;
    if let Err(error_value) = outcome {
        eprintln!("Notice task failed: {error_value}");
    }
}

fn recipients<'a>(column: &str, rows: &'a [Map<String, Value>]) -> std::collections::BTreeMap<&'a str, Vec<&'a Map<String, Value>>> {
    let mut named: std::collections::BTreeMap<&str, Vec<&Map<String, Value>>> = std::collections::BTreeMap::new();
    for row in rows {
        if let Some(id) = row.get(column).and_then(Value::as_str).filter(|id| !id.is_empty()) {
            named.entry(id).or_default().push(row);
        }
    }
    named
}

#[allow(clippy::too_many_arguments)]
fn queue_notices(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    declared: &Declaration,
    input: &Value,
    account: Option<&str>,
    actor_id: Option<&str>,
    changed: &Changed<'_>,
) -> anyhow::Result<Vec<NoticeOutcome>> {
    let Some(queue) = state.accounts.queue.as_ref() else {
        return Ok(vec![NoticeOutcome::Refused("no-transport")]);
    };
    let creator = super::accounts::project_creator(state, &deployment.project_id).unwrap_or_else(|| "operator".into());
    let send = |account_id: Option<&str>, recipient: &str, rows: &[&Map<String, Value>], count: u64| -> anyhow::Result<NoticeOutcome> {
        let values = Fill { input, actor: actor_id, now: changed.now, columns: &declared.changes, rows, count };
        let (subject, text, html) = render(declared, &deployment.project_name, &values);
        let scope = super::auth::deployment_scope(&deployment.id);
        queue.record_notice(&Notice {
            recipient,
            scope: &scope,
            creator: &creator,
            app_name: &deployment.project_name,
            action: name,
            subject: &subject,
            text: &text,
            html: &html,
            account_id,
            intent_id: None,
        })
    };
    let all: Vec<&Map<String, Value>> = changed.rows.iter().collect();
    match &declared.to {
        Recipient::Owner => match deployment.contact_email.as_deref() {
            Some(email) => Ok(vec![send(None, email, &all, changed.count)?]),
            None => Ok(vec![NoticeOutcome::Refused("owner-has-no-email")]),
        },
        Recipient::Actor => {
            let Some(account) = account else {
                return Ok(vec![NoticeOutcome::Refused("no-account")]);
            };
            match verified_email(state, &deployment.id, account)? {
                Some(email) => Ok(vec![send(Some(account), &email, &all, changed.count)?]),
                None => Ok(vec![NoticeOutcome::Refused("unverified-email")]),
            }
        }
        // Each account gets only the rows that name it, so one member
        // never reads another's rows. Past the cap, nobody else is written to.
        Recipient::Column(column) => {
            let mut outcomes = Vec::new();
            for (index, (id, rows)) in recipients(column, changed.rows).into_iter().enumerate() {
                if index >= MAX_RECIPIENTS_PER_RUN {
                    outcomes.push(NoticeOutcome::Refused("recipient-cap"));
                    continue;
                }
                outcomes.push(match verified_email(state, &deployment.id, id)? {
                    Some(email) => send(Some(id), &email, &rows, rows.len() as u64)?,
                    None => NoticeOutcome::Refused("not-a-verified-account"),
                });
            }
            Ok(outcomes)
        }
    }
}

fn read_only(path: &std::path::Path) -> anyhow::Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(connection))
}

fn verified_email(state: &AppState, deployment_id: &str, account: &str) -> anyhow::Result<Option<String>> {
    let path = crate::layout::identity(&state.data_root, &deployment_id);
    let Some(db) = read_only(&path)? else { return Ok(None) };
    Ok(db
        .query_row(
            "SELECT email FROM app_users WHERE id = ?1 AND state = 'active' AND verified_at IS NOT NULL",
            [account],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn startup_wakes_a_committed_notice_when_its_post_commit_signal_was_lost() {
        let root = std::env::temp_dir().join(format!("konae-notify-recovery-{}", uuid::Uuid::new_v4()));
        let state = AppState::for_test(&root);
        let deployment: Deployment = serde_json::from_value(json!({
            "id": "notice_app", "projectId": "notice_project", "projectName": "Notice app",
            "slug": "notice-app", "url": "http://notice-app.localhost", "createdAt": 1,
            "manifest": {}, "hostname": "notice-app.localhost", "actions": "{}",
            "artifactVersion": "v1", "signupPolicy": {"mode": "open", "allowlist": []},
            "signInMethods": ["password"]
        })).unwrap();
        state.deployments.write().await.insert(deployment.hostname.clone(), deployment.clone());
        let database = crate::layout::data(&root, &deployment.id).join("database.sqlite");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(&database).unwrap();
        ensure_intent_schema(&connection).unwrap();
        connection.execute(
            "INSERT INTO __konae_notify_intents (id,payload_json,state,created_at) VALUES ('committed-intent','{}','pending',1)",
            [],
        ).unwrap();
        drop(connection);

        wake_pending(&state).await;
        assert_eq!(state.dispatcher.next(crate::execution::WorkResource::Notices).await, deployment.id);
        let _ = std::fs::remove_dir_all(root);
    }

    fn program(actions: &Value) -> super::super::ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    #[test]
    fn notify_refuses_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("notify-cases.json")).unwrap();
        assert!(cases.len() >= 15);
        for case in cases {
            let program = program(&case["actions"]);
            let outcome = super::super::action_shape::check_program(&program);
            crate::refusal::assert_case(&case, outcome);
        }
    }

    fn plain<'a>(input: &'a Value, actor: Option<&'a str>) -> Fill<'a> {
        Fill { input, actor, now: 0, columns: &[], rows: &[], count: 1 }
    }

    #[test]
    fn notify_columns_are_checked_against_the_schema_as_the_editor_checks_them() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("notify-column-cases.json")).unwrap();
        assert!(cases.len() >= 8);
        for case in cases {
            let program = program(&case["actions"]);
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(case["schema"].as_str().unwrap()).unwrap();
            let outcome = program.actions.iter().try_for_each(|(name, action)| {
                connection.prepare(&action.statement).expect("the statement prepares on its own");
                check_columns(&connection, name, action)
            });
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn the_kernel_appends_its_returning_clause_after_cutting_a_trailing_semicolon() {
        let columns = vec!["title".to_owned(), "assignee_id".to_owned()];
        assert_eq!(with_returning("UPDATE t SET a = 1 ; -- done", &columns), "UPDATE t SET a = 1\nRETURNING [title], [assignee_id]");
        assert_eq!(with_returning("UPDATE t SET a = ';' -- x", &columns), "UPDATE t SET a = ';' -- x\nRETURNING [title], [assignee_id]");
    }

    #[test]
    fn kernel_now_reads_as_a_utc_minute() {
        assert_eq!(format_now(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_now(1_793_948_400_000 + 59_999), "2026-11-06 07:00 UTC");
    }

    fn reminders() -> super::super::ActionProgram {
        program(&json!({
            "tasks.remind": {"kind": "mutation", "access": "authenticated", "schedule": {"daily": "07:00"},
                "statement": "UPDATE tasks SET reminded_at = ?1 WHERE reminded_at IS NULL",
                "bindings": [{"source": "kernel", "field": "now"}],
                "notify": {"to": {"column": "assignee_id"}, "changes": ["title", "due_day"],
                    "subject": "{changes.count} due", "body": "At {kernel.now}:\n{changes}"}}
        }))
    }

    fn row(title: &str, assignee: Value) -> Map<String, Value> {
        let value = json!({"title": title, "due_day": "2026-11-08", "assignee_id": assignee});
        value.as_object().unwrap().clone()
    }

    #[test]
    fn each_named_account_reads_only_the_rows_that_name_it() {
        let program = reminders();
        let declared = declaration("tasks.remind", &program.actions["tasks.remind"]).unwrap().unwrap();
        assert_eq!(declared.returning(), vec!["title", "due_day", "assignee_id"]);
        let rows = vec![row("Water\nplants", json!("ana")), row("File taxes", json!("ben")), row("Call mum", json!("ana")), row("Nobody", Value::Null), row("Number", json!(7))];
        let named = recipients("assignee_id", &rows);
        assert_eq!(named.keys().copied().collect::<Vec<_>>(), vec!["ana", "ben"]);
        let input = json!({});
        let ana = &named["ana"];
        let values = Fill { input: &input, actor: None, now: 1_793_948_400_000, columns: &declared.changes, rows: ana, count: ana.len() as u64 };
        let (subject, text, _) = render(&declared, "Chores", &values);
        assert_eq!(subject, "2 due");
        assert!(text.starts_with("At 2026-11-06 07:00 UTC:\n- Water plants · 2026-11-08\n- Call mum · 2026-11-08\n\n-- \n"), "{text}");
        assert!(!text.contains("File taxes"));
        assert!(text.contains("because a change in the app names your account."));
    }

    #[test]
    fn changes_lists_fifty_rows_and_counts_the_rest() {
        let program = reminders();
        let declared = declaration("tasks.remind", &program.actions["tasks.remind"]).unwrap().unwrap();
        let rows: Vec<Map<String, Value>> = (0..60).map(|index| row(&format!("t{index}"), json!("ana"))).collect();
        let listed: Vec<&Map<String, Value>> = rows.iter().collect();
        let input = json!({});
        let values = Fill { input: &input, actor: None, now: 0, columns: &declared.changes, rows: &listed, count: 1200 };
        let (_, text, _) = render(&declared, "Chores", &values);
        assert!(text.contains("- t49 · 2026-11-08\n- and 1150 more\n"), "{text}");
        assert!(!text.contains("t50 "));
    }

    fn booking() -> super::super::ActionProgram {
        program(&json!({
            "bookings.create": {"kind": "mutation", "access": "authenticated",
                "statement": "INSERT INTO bookings (user_id, room, title) VALUES (?1, ?2, ?3)",
                "bindings": [{"source": "actor", "field": "id"}, {"source": "input", "key": "room", "type": "string"},
                    {"source": "input", "key": "title", "type": "string", "nullable": true},
                    {"source": "input", "key": "hours", "type": "number", "nullable": true}],
                "notify": {"to": "actor", "subject": "Booked {input.room}{input.title}",
                    "body": "Room {input.room} for {input.hours} h.\nBooked by {actor.id}. Braces } stay."}}
        }))
    }

    #[test]
    fn a_notice_escapes_values_strips_their_line_breaks_and_caps_them() {
        let program = booking();
        let action = &program.actions["bookings.create"];
        let declared = declaration("bookings.create", action).unwrap().unwrap();
        assert_eq!(declared.to, Recipient::Actor);
        let input = json!({"room": "Oak\r\nBcc: x@example.test <b>", "title": "x".repeat(500), "hours": 2});
        let (subject, text, html) = render(&declared, "Room\nboard", &plain(&input, Some("user-1")));
        assert!(!subject.contains(['\r', '\n']));
        assert!(subject.starts_with("Booked Oak Bcc: x@example.test <b>"));
        assert_eq!(subject.chars().count(), MAX_RENDERED_SUBJECT_CHARS);
        assert!(text.starts_with("Room Oak Bcc: x@example.test <b> for 2 h.\nBooked by user-1. Braces } stay.\n\n-- \n"));
        assert!(text.contains("Room board sent this notice through Konae after something you did in the app."));
        assert!(html.contains("Room Oak Bcc: x@example.test &lt;b&gt; for 2 h.<br>Booked by user-1.") && !html.contains("<b>"));
        let (subject, text, _) = render(&declared, "Rooms", &plain(&json!({"room": "Oak", "title": null}), None));
        assert_eq!(subject, "Booked Oak");
        assert!(text.starts_with("Room Oak for  h.\nBooked by . Braces } stay."));
    }

    #[test]
    fn a_subject_that_renders_empty_names_the_app() {
        let program = program(&json!({
            "notes.add": {"kind": "mutation", "access": "authenticated", "statement": "INSERT INTO notes (body) VALUES (?1)",
                "bindings": [{"source": "input", "key": "body", "type": "string", "nullable": true}],
                "notify": {"to": "owner", "subject": "{input.body}", "body": "{input.body}"}}
        }));
        let declared = declaration("notes.add", &program.actions["notes.add"]).unwrap().unwrap();
        let (subject, text, _) = render(&declared, "Notes", &plain(&json!({}), None));
        assert_eq!(subject, "Notice from Notes");
        assert!(text.ends_with("Notes sent this notice through Konae because you own the app.\n"));
    }
}
