//! Usage and Signals, ADR-0010.
//!
//! Each Deployment keeps `usage.sqlite` in its folder, next to `database.sqlite`
//! and `identity.sqlite`. It holds daily totals of the Runtime Interface
//! operations the Kernel authorized and the Signals that Server Actions
//! declared. Nothing else writes it and a Runtime Actor never reads it; the
//! owner reads it with `usage_read` and `signals_read`.
//!
//! A handler calls `count` or `signal` after the operation succeeded. Both put a
//! message on a bounded queue and return at once. One writer thread drains the
//! queue and writes each Deployment's file in one transaction per batch, so a
//! response never waits on this store. When the queue is full the message is
//! dropped and counted in `dropped`, rather than slowing the app down.
//!
//! The one other writer is the timer in `schedule.rs` (ADR-0017). It records
//! each scheduled run, its `scheduled` count and the slot it ran for with
//! `record_run`, directly and in one transaction, because a run the store lost
//! would run again after a restart. SQLite's WAL and busy timeout let the two
//! writers take turns.

use super::refusal::{refuse, Refusal};
use super::{ActionDefinition, Deployment};
use anyhow::Result;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) const USAGE_DATABASE: &str = "usage.sqlite";
/// The largest Usage store a package may carry. The row bounds below keep a
/// store well under it.
pub(crate) const MAX_USAGE_DATABASE_BYTES: u64 = 128 * 1024 * 1024;
/// Days of totals kept. Older days are dropped as new ones arrive.
pub(crate) const USAGE_DAYS: i64 = 400;
/// Rows of totals kept, oldest days dropped first. A Deployment has at most
/// 100 actions and 100 routes, so a day holds at most about 205 rows plus one
/// `page_view` row per HTML file the artifact serves.
pub(crate) const MAX_USAGE_ROWS: i64 = 100_000;
/// Signals kept, oldest dropped first.
pub(crate) const MAX_SIGNALS: i64 = 50_000;
/// Daily execution and Signal aggregates, bounded independently from operation totals.
const MAX_METRIC_ROWS: i64 = 100_000;
/// Scheduled runs kept, oldest dropped first. A job every 5 minutes makes 288 a day.
pub(crate) const MAX_SCHEDULED_RUNS: i64 = 10_000;
/// The scheduled runs `usage_read` returns, newest first.
const SCHEDULED_PAGE: i64 = 500;
pub(crate) const MAX_SIGNAL_PROPERTIES: usize = 8;
/// The property that records the caller's ID instead of an input.
const SIGNAL_ACTOR: &str = "actor";
/// A string property keeps at most this many bytes, cut at a character boundary.
pub(crate) const MAX_PROPERTY_BYTES: usize = 128;
const SIGNALS_PAGE: i64 = 100;
const QUEUE: usize = 16_384;
const BATCH: usize = 1_024;

/// An authorized Runtime Interface operation.
pub(crate) enum Operation {
    /// `auth_session`, which the SDK calls when an app opens.
    Session,
    Action(String),
    /// An HTTP route, as `POST /hooks/orders`.
    Route(String, String),
    Upload,
    FileRead,
    Signup,
    Login,
    /// A guarded mutation that changed no row, by action name. The one
    /// refusal Usage counts: the app's own rule said no (`guard.rs`).
    Refused(String),
    /// A mutation's notice, named `<action>: queued` or `<action>: <reason>`
    /// when nothing will be sent (`notify.rs`).
    Notify(String),
    /// A call queued by an action of `"kind": "call"`, by action name (`calls.rs`).
    Call(String),
    /// A browser navigation to one of the artifact's HTML pages, by the file
    /// served (`count_page_view` in `main.rs`).
    PageView(String),
}

impl Operation {
    fn parts(self) -> (&'static str, String) {
        match self {
            Operation::Session => ("session", String::new()),
            Operation::Action(name) => ("action", name),
            Operation::Route(method, path) => ("route", format!("{method} {path}")),
            Operation::Upload => ("upload", String::new()),
            Operation::FileRead => ("file_read", String::new()),
            Operation::Signup => ("signup", String::new()),
            Operation::Login => ("login", String::new()),
            Operation::Refused(name) => ("refused", name),
            Operation::Notify(name) => ("notify", name),
            Operation::Call(name) => ("call", name),
            Operation::PageView(file) => ("page_view", file),
        }
    }
}

enum Message {
    Count { folder: PathBuf, at: u64, kind: &'static str, name: String },
    Signal { folder: PathBuf, at: u64, name: String, properties: String },
    Metric { folder: PathBuf, at: u64, feature: String, name: String, origin: String, outcome: String },
    Flush(SyncSender<()>),
}

static QUEUE_SENDER: OnceLock<SyncSender<Message>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);
static WRITE_ERRORS: AtomicU64 = AtomicU64::new(0);

fn sender() -> &'static SyncSender<Message> {
    QUEUE_SENDER.get_or_init(|| {
        let (sender, receiver) = sync_channel(QUEUE);
        std::thread::Builder::new()
            .name("konae-usage".into())
            .spawn(move || writer(receiver))
            .expect("start the usage writer");
        sender
    })
}

fn enqueue(message: Message) {
    match sender().try_send(message) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            if DROPPED.fetch_add(1, Ordering::Relaxed) % 1_000 == 0 {
                eprintln!("Usage queue is full; dropping entries rather than delaying responses.");
            }
        }
    }
}

fn folder(data_root: &Path, deployment_id: &str) -> PathBuf {
    crate::layout::data(data_root, deployment_id)
}

/// Counts one authorized operation. Call it only after the operation succeeded.
pub(crate) fn count(data_root: &Path, deployment_id: &str, operation: Operation) {
    let (kind, name) = operation.parts();
    enqueue(Message::Count { folder: folder(data_root, deployment_id), at: super::now_ms(), kind, name });
}

/// Records the action's Signal, if it declares one, from the input it ran
/// with and the caller's ID for an `actor` property. Call it only after the
/// action succeeded and, for a mutation, changed a row.
pub(crate) fn signal(data_root: &Path, deployment_id: &str, action_name: &str, action: &ActionDefinition, input: &Value, actor: Option<&str>) {
    let Ok(Some(declared)) = declaration(action_name, action) else { return };
    let mut properties = Map::new();
    for key in &declared.properties {
        if key == SIGNAL_ACTOR {
            properties.insert(key.clone(), actor.map_or(Value::Null, |id| Value::String(id.to_owned())));
            continue;
        }
        let value = match input.get(key) {
            Some(Value::String(text)) => Value::String(truncate(text, MAX_PROPERTY_BYTES).to_owned()),
            Some(value) => value.clone(),
            None => Value::Null,
        };
        properties.insert(key.clone(), value);
    }
    enqueue(Message::Signal {
        folder: folder(data_root, deployment_id),
        at: super::now_ms(),
        name: declared.name,
        properties: Value::Object(properties).to_string(),
    });
}

/// Records one completed execution without retaining its inputs or error text.
pub(crate) fn execution(data_root: &Path, deployment_id: &str, name: &str, origin: &str, outcome: &str) {
    enqueue(Message::Metric { folder: folder(data_root, deployment_id), at: super::now_ms(),
        feature: "execution".into(), name: name.into(), origin: origin.into(), outcome: outcome.into() });
}

fn truncate(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Waits until every message queued before this call is written, or five
/// seconds pass. `usage_read` and `signals_read` call it so the owner sees what
/// happened up to the moment they asked.
pub(crate) fn flush() {
    let _ = flush_status();
}

pub(crate) fn flush_status() -> bool {
    let (done, wait) = sync_channel(1);
    send_waiting(Message::Flush(done)).is_ok() && wait.recv_timeout(Duration::from_secs(5)).is_ok()
}

/// A flush is the one sender that waits for room, and only for five seconds.
fn send_waiting(message: Message) -> std::result::Result<(), ()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut message = message;
    loop {
        match sender().try_send(message) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(()),
            Err(TrySendError::Full(back)) => {
                if std::time::Instant::now() >= deadline {
                    return Err(());
                }
                message = back;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

#[derive(Default)]
struct Batch {
    counts: HashMap<(String, &'static str, String), i64>,
    signals: Vec<(u64, String, String)>,
    metrics: Vec<(u64, String, String, String, String)>,
}

fn writer(receiver: Receiver<Message>) {
    while let Ok(first) = receiver.recv() {
        let mut batches: HashMap<PathBuf, Batch> = HashMap::new();
        let mut flushes = Vec::new();
        let mut next = Some(first);
        let mut taken = 0;
        while let Some(message) = next.take() {
            match message {
                Message::Count { folder, at, kind, name } => {
                    *batches.entry(folder).or_default().counts.entry((day(at), kind, name)).or_insert(0) += 1;
                }
                Message::Signal { folder, at, name, properties } => {
                    batches.entry(folder).or_default().signals.push((at, name, properties));
                }
                Message::Metric { folder, at, feature, name, origin, outcome } => {
                    batches.entry(folder).or_default().metrics.push((at, feature, name, origin, outcome));
                }
                Message::Flush(done) => flushes.push(done),
            }
            taken += 1;
            if taken < BATCH {
                next = receiver.try_recv().ok();
            }
        }
        for (folder, batch) in batches {
            if let Err(error) = write(&folder, &batch, super::now_ms()) {
                WRITE_ERRORS.fetch_add(1, Ordering::Relaxed);
                eprintln!("Usage write failed for {}: {error}", folder.display());
            }
        }
        for done in flushes {
            let _ = done.try_send(());
        }
    }
}

fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS usage_daily(
           day TEXT NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL, count INTEGER NOT NULL,
           PRIMARY KEY(day, kind, name));
         CREATE TABLE IF NOT EXISTS signals(
           id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, at INTEGER NOT NULL, properties TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS signals_by_name ON signals(name, id);
         CREATE TABLE IF NOT EXISTS scheduled_runs(
           id INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL, at INTEGER NOT NULL,
           outcome TEXT NOT NULL, changes INTEGER NOT NULL, error TEXT);
         CREATE INDEX IF NOT EXISTS scheduled_runs_by_time ON scheduled_runs(at);
         CREATE TABLE IF NOT EXISTS schedule_state(
           action TEXT PRIMARY KEY, schedule TEXT NOT NULL, slot INTEGER NOT NULL);",
    )?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS metric_daily(
           day TEXT NOT NULL, feature TEXT NOT NULL, name TEXT NOT NULL, origin TEXT NOT NULL,
           outcome TEXT NOT NULL, count INTEGER NOT NULL, last_at INTEGER NOT NULL,
           PRIMARY KEY(day, feature, name, origin, outcome));
         CREATE TABLE IF NOT EXISTS metric_meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
    )?;
    connection.execute("INSERT OR IGNORE INTO metric_meta VALUES('started_at', ?1)", [super::now_ms() as i64])?;
    Ok(connection)
}

// ---------------------------------------------------------------------------
// Scheduled runs, written by `schedule.rs`.

/// One finished scheduled run.
pub(crate) struct Run<'a> {
    pub(crate) action: &'a str,
    /// The schedule as `schedule::Schedule::key` writes it.
    pub(crate) schedule: &'a str,
    /// The slot the run was for. A run that succeeded marks it done.
    pub(crate) slot: i64,
    pub(crate) at: u64,
    /// The rows the statement changed, or None when it failed.
    pub(crate) changes: Option<u64>,
    pub(crate) error: Option<String>,
}

/// The slot each scheduled action last ran for, with the schedule it ran
/// under, by action name. An action the store has not seen is absent.
pub(crate) fn schedule_slots(folder: &Path) -> Result<HashMap<String, (String, i64)>> {
    let db = open(&folder.join(USAGE_DATABASE))?;
    let mut query = db.prepare("SELECT action, schedule, slot FROM schedule_state")?;
    let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get::<_, String>(1)?, row.get::<_, i64>(2)?))))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// Marks a slot as passed without a run: the first time the timer sees an
/// action, or after its schedule changed, so the first run is the next
/// occurrence and never the moment of publishing.
pub(crate) fn schedule_baseline(folder: &Path, action: &str, schedule: &str, slot: i64) -> Result<()> {
    let db = open(&folder.join(USAGE_DATABASE))?;
    db.execute(
        "INSERT INTO schedule_state(action, schedule, slot) VALUES(?1, ?2, ?3)
         ON CONFLICT(action) DO UPDATE SET schedule = excluded.schedule, slot = excluded.slot",
        params![action, schedule, slot],
    )?;
    Ok(())
}

/// Records a run, counts it under `scheduled` and, when it succeeded, marks
/// its slot done, in one transaction. A failed run leaves the slot open, so
/// the next tick tries again.
pub(crate) fn record_run(folder: &Path, run: &Run) -> Result<()> {
    let mut db = open(&folder.join(USAGE_DATABASE))?;
    let transaction = db.transaction()?;
    let outcome = if run.changes.is_some() { "ok" } else { "failed" };
    transaction.execute(
        "INSERT INTO scheduled_runs(action, at, outcome, changes, error) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![run.action, run.at as i64, outcome, run.changes.unwrap_or(0) as i64, run.error.as_deref().map(|text| truncate(text, 500))],
    )?;
    transaction.execute(
        "INSERT INTO usage_daily(day, kind, name, count) VALUES(?1, 'scheduled', ?2, 1)
         ON CONFLICT(day, kind, name) DO UPDATE SET count = count + 1",
        params![day(run.at), run.action],
    )?;
    metric(&transaction, run.at, "execution", run.action, "schedule",
        if run.changes == Some(0) { "unchanged" } else { outcome }, 1)?;
    if run.changes.is_some() {
        transaction.execute(
            "INSERT INTO schedule_state(action, schedule, slot) VALUES(?1, ?2, ?3)
             ON CONFLICT(action) DO UPDATE SET schedule = excluded.schedule, slot = excluded.slot",
            params![run.action, run.schedule, run.slot],
        )?;
    }
    prune(&transaction, run.at)?;
    transaction.commit()?;
    Ok(())
}

fn write(folder: &Path, batch: &Batch, now: u64) -> Result<()> {
    // A Deployment removed or moved since the message was queued gets no new folder.
    if !folder.is_dir() {
        return Ok(());
    }
    let mut connection = open(&folder.join(USAGE_DATABASE))?;
    let transaction = connection.transaction()?;
    for ((day, kind, name), count) in &batch.counts {
        transaction.execute(
            "INSERT INTO usage_daily(day, kind, name, count) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(day, kind, name) DO UPDATE SET count = count + excluded.count",
            params![day, kind, name, count],
        )?;
    }
    for (at, feature, name, origin, outcome) in &batch.metrics {
        metric(&transaction, *at, feature, name, origin, outcome, 1)?;
    }
    for (at, name, properties) in &batch.signals {
        metric(&transaction, *at, "signal", name, "kernel", "recorded", 1)?;
        transaction.execute(
            "INSERT INTO signals(name, at, properties) VALUES(?1, ?2, ?3)",
            params![name, *at as i64, properties],
        )?;
    }
    prune(&transaction, now)?;
    transaction.commit()?;
    Ok(())
}

fn metric(db: &Connection, at: u64, feature: &str, name: &str, origin: &str, outcome: &str, count: i64) -> Result<()> {
    db.execute(
        "INSERT INTO metric_daily VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(day, feature, name, origin, outcome) DO UPDATE SET
           count = count + excluded.count, last_at = max(last_at, excluded.last_at)",
        params![day(at), feature, name, origin, outcome, count, at as i64],
    )?;
    Ok(())
}

/// Keeps the store within its bounds: `USAGE_DAYS` days and `MAX_USAGE_ROWS`
/// rows of totals, and the newest `MAX_SIGNALS` Signals.
fn prune(db: &Connection, now: u64) -> Result<()> {
    let cutoff = day(now.saturating_sub((USAGE_DAYS as u64 - 1) * 86_400_000));
    prune_daily(db, "usage_daily", MAX_USAGE_ROWS, &cutoff)?;
    prune_daily(db, "metric_daily", MAX_METRIC_ROWS, &cutoff)?;
    db.execute(
        "DELETE FROM signals WHERE id <= (SELECT MAX(id) FROM signals) - ?1",
        [MAX_SIGNALS],
    )?;
    db.execute(
        "DELETE FROM scheduled_runs WHERE id <= (SELECT MAX(id) FROM scheduled_runs) - ?1",
        [MAX_SCHEDULED_RUNS],
    )?;
    Ok(())
}

fn prune_daily(db: &Connection, table: &str, limit: i64, cutoff: &str) -> Result<()> {
    // Table names come only from prune, never from a request.
    let predicate = format!("day < ?1 OR rowid IN (SELECT rowid FROM {table} ORDER BY day LIMIT max(0, (SELECT COUNT(*) FROM {table}) - ?2))");
    let latest: Option<String> = db.query_row(&format!("SELECT MAX(day) FROM {table} WHERE {predicate}"), params![cutoff, limit], |r| r.get(0))?;
    if let Some(latest) = latest {
        db.execute("INSERT INTO metric_meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = max(value, excluded.value)",
            params![format!("{table}_pruned_through"), parse_day(&latest).unwrap_or(0) * 86_400_000])?;
        db.execute(&format!("DELETE FROM {table} WHERE {predicate}"), params![cutoff, limit])?;
    }
    Ok(())
}

/// The UTC calendar day of a time in milliseconds, written `YYYY-MM-DD`.
pub(crate) fn day(ms: u64) -> String {
    let (year, month, date) = civil((ms / 86_400_000) as i64);
    format!("{year:04}-{month:02}-{date:02}")
}

/// Days since 1970-01-01 to a proleptic Gregorian date.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, date)
}

/// Parses `YYYY-MM-DD` into days since 1970-01-01, refusing dates that do not exist.
fn parse_day(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, date) = (number(0..4)?, number(5..7)?, number(8..10)?);
    if !(1..=12).contains(&month) || date < 1 {
        return None;
    }
    // Days from civil, the inverse of `civil`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + date - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (civil(days) == (year, month as u32, date as u32)).then_some(days)
}

// ---------------------------------------------------------------------------
// Declarations. The same checks, order and messages as `parseSignal` in
// src/kernel/actions.ts; kernel-rs/src/signal-cases.json holds the cases.

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Signal {
    pub(crate) name: String,
    pub(crate) properties: Vec<String>,
}

/// Checks the parts of an action that a `"kind": "signal"` action changes, and
/// its `signal` key. `validate_action` calls it after kind and access and
/// before the SQL checks, which a signal action skips.
pub(crate) fn check_signal_action(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if action.kind == "signal" && !action.statement.trim().is_empty() {
        return Err(refuse("signal.statement", format!("Action {name} is a signal action, so it runs no SQL and cannot declare a statement.")));
    }
    Ok(())
}

/// Parses and checks one action's `signal` key. `Ok(None)` when it has none.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Signal>, Refusal> {
    let Some(value) = action.signal.as_ref() else {
        if action.kind == "signal" {
            return Err(refuse("signal.missing", format!("Action {name} is a signal action, so it must declare a signal.")));
        }
        return Ok(None);
    };
    if action.kind == "query" {
        return Err(refuse("signal.on-query", format!(
            r#"Action {name} is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action."#
        )));
    }
    let invalid = || refuse("signal.invalid", format!("Action {name} has an invalid signal declaration."));
    let record = value.as_object().ok_or_else(invalid)?;
    if record.keys().any(|key| key != "name" && key != "properties") {
        return Err(invalid());
    }
    let signal_name = record.get("name").and_then(Value::as_str).ok_or_else(invalid)?;
    if !valid_name(signal_name) {
        return Err(refuse("signal.name-invalid", format!(
            r#"Action {name} names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters."#
        )));
    }
    let properties = match record.get("properties") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(invalid()),
    };
    if properties.len() > MAX_SIGNAL_PROPERTIES {
        // The number is written out so src/sdk/contract.test.ts finds the message.
        return Err(refuse("signal.properties-too-many", format!("Action {name} names more than 8 signal properties.")));
    }
    let inputs: Vec<&str> = action
        .bindings
        .iter()
        .filter(|binding| binding.source == "input")
        .filter_map(|binding| binding.key.as_deref())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for key in &properties {
        if !seen.insert(key.as_str()) {
            return Err(refuse("signal.property-twice", format!("Action {name} names signal property {key} twice.")));
        }
        if key == SIGNAL_ACTOR && inputs.contains(&SIGNAL_ACTOR) {
            return Err(refuse("signal.property-actor-input", format!(
                "Action {name} names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input."
            )));
        }
        if key != SIGNAL_ACTOR && !inputs.contains(&key.as_str()) {
            return Err(refuse("signal.property-unbound", format!("Action {name} names signal property {key}, which is not one of its input bindings.")));
        }
    }
    Ok(Some(Signal { name: signal_name.to_owned(), properties }))
}

/// Dot-separated lowercase words, at least two, as action names are, up to 64 characters.
fn valid_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    name.len() <= 64
        && parts.len() >= 2
        && parts.iter().all(|part| {
            let mut bytes = part.bytes();
            bytes.next().is_some_and(|c| c.is_ascii_lowercase())
                && bytes.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

// ---------------------------------------------------------------------------
// Owner tools. `mcp.rs` lists them for every owner credential, since each
// needs only `read`, and checks the Deployment's project grant on every call.
// The runtime endpoint never lists them.

pub(crate) fn admin_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    let date = json!({"type": "string", "minLength": 10, "maxLength": 10});
    vec![
        super::mcp::tool("usage_summary", "Read a deployment usage monitor with daily execution outcomes, Signal totals, published configuration and coverage. Owner read permission required.", json!({"deploymentId": text, "from": date, "to": date}), &["deploymentId", "from", "to"], true),
        super::mcp::tool("usage_overview", "Read usage for this account projects only, including external and unpublished apps, 50 per page. Pass nextCursor back as cursor.", json!({"from": date, "to": date, "cursor": {"type": "string", "maxLength": 100}}), &["from", "to"], true),
        super::mcp::tool(
            "usage_read",
            "Read a deployment's Usage: daily totals of the runtime operations the Kernel allowed (session, action, route, upload, file_read, signup, login, call), of guard refusals (refused, by action) and of scheduled runs (scheduled, by action) between two UTC dates written YYYY-MM-DD, both included, at most 400 days apart, and the newest 500 scheduled runs in that range with their outcome and changes.",
            json!({"deploymentId": text, "from": date, "to": date}),
            &["deploymentId", "from", "to"],
            true,
        ),
        super::mcp::tool(
            "signals_read",
            "Read a deployment's Signals, newest first, 100 at a time, optionally of one name. Pass nextCursor back as cursor for the next page.",
            json!({"deploymentId": text, "name": {"type": "string", "minLength": 1, "maxLength": 64}, "cursor": {"type": "string", "minLength": 1, "maxLength": 20}}),
            &["deploymentId"],
            true,
        ),
    ]
}

/// Answers `usage_read` and `signals_read` for a Deployment the caller may read.
pub(crate) async fn admin_call(data_root: &Path, deployment: &Deployment, name: &str, a: &Value) -> Result<Value> {
    let path = folder(data_root, &deployment.id).join(USAGE_DATABASE);
    let arguments = a.clone();
    let deployment = deployment.clone();
    let data_root = data_root.to_path_buf();
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || {
        let flushed = flush_status();
        match name.as_str() {
            "usage_summary" => read_summary(&data_root, &deployment, arguments["from"].as_str().unwrap_or(""), arguments["to"].as_str().unwrap_or(""), flushed),
            "usage_read" => read_usage(&path, arguments["from"].as_str().unwrap_or(""), arguments["to"].as_str().unwrap_or("")),
            _ => read_signals(&path, arguments["name"].as_str(), arguments["cursor"].as_str()),
        }
    })
    .await?
}

fn read_only(path: &Path) -> Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(Some(connection))
}

pub(crate) fn validate_range(from: &str, to: &str) -> Result<(i64, i64)> {
    let (Some(start), Some(end)) = (parse_day(from), parse_day(to)) else {
        anyhow::bail!("from and to must be dates written YYYY-MM-DD.");
    };
    anyhow::ensure!(start <= end, "from must not be after to.");
    anyhow::ensure!(end - start < USAGE_DAYS, "Usage covers at most 400 days at a time.");
    Ok((start, end))
}

pub(crate) fn read_usage(path: &Path, from: &str, to: &str) -> Result<Value> {
    let (start, end) = validate_range(from, to)?;
    let mut days = Vec::new();
    let mut totals: Vec<Value> = Vec::new();
    let mut scheduled: Vec<Value> = Vec::new();
    if let Some(db) = read_only(path)? {
        // A store written before scheduled work existed has no runs table.
        let has_runs: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'scheduled_runs')", [], |row| row.get(0))?;
        if has_runs {
            let mut query = db.prepare(
                "SELECT action, at, outcome, changes, error FROM scheduled_runs WHERE at >= ?1 AND at < ?2 ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = query.query_map(params![start * 86_400_000, (end + 1) * 86_400_000, SCHEDULED_PAGE], |row| {
                let error: Option<String> = row.get(4)?;
                let mut run = json!({"action": row.get::<_, String>(0)?, "at": row.get::<_, i64>(1)?,
                    "outcome": row.get::<_, String>(2)?, "changes": row.get::<_, i64>(3)?});
                if let Some(error) = error {
                    run["error"] = Value::String(error);
                }
                Ok(run)
            })?;
            scheduled = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        }
        let mut query = db.prepare(
            "SELECT day, kind, name, count FROM usage_daily WHERE day BETWEEN ?1 AND ?2 ORDER BY day, kind, name",
        )?;
        let rows = query.query_map(params![from, to], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
        })?;
        let mut sums: Vec<((String, String), i64)> = Vec::new();
        for row in rows {
            let (day, kind, name, count) = row?;
            match sums.iter_mut().find(|((k, n), _)| *k == kind && *n == name) {
                Some((_, total)) => *total += count,
                None => sums.push(((kind.clone(), name.clone()), count)),
            }
            days.push(json!({"day": day, "kind": kind, "name": name, "count": count}));
        }
        sums.sort();
        totals = sums.into_iter().map(|((kind, name), count)| json!({"kind": kind, "name": name, "count": count})).collect();
    }
    Ok(json!({"from": from, "to": to, "days": days, "totals": totals, "scheduled": scheduled}))
}

/// Reads only Kernel aggregates and published declarations, never application rows.
pub(crate) fn read_summary(data_root: &Path, deployment: &Deployment, from: &str, to: &str, flushed: bool) -> Result<Value> {
    let path = folder(data_root, &deployment.id).join(USAGE_DATABASE);
    let usage = read_usage(&path, from, to)?;
    let mut metrics = Vec::new();
    let mut started_at: Option<i64> = None;
    let mut earliest_day: Option<String> = None;
    let mut pruned_through: Option<i64> = None;
    if let Some(db) = read_only(&path)? {
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'metric_daily')", [], |r| r.get(0))?;
        if exists {
            started_at = db.query_row("SELECT value FROM metric_meta WHERE key = 'started_at'", [], |r| r.get(0)).ok();
            pruned_through = db.query_row("SELECT MAX(value) FROM metric_meta WHERE key IN ('usage_daily_pruned_through', 'metric_daily_pruned_through')", [], |r| r.get(0))?;
            earliest_day = db.query_row("SELECT MIN(day) FROM metric_daily", [], |r| r.get(0))?;
            let mut q = db.prepare("SELECT day, feature, name, origin, outcome, count, last_at FROM metric_daily WHERE day BETWEEN ?1 AND ?2 ORDER BY day, feature, name, origin, outcome")?;
            metrics = q.query_map(params![from, to], |r| Ok(json!({"day": r.get::<_, String>(0)?, "feature": r.get::<_, String>(1)?,
                "name": r.get::<_, String>(2)?, "origin": r.get::<_, String>(3)?, "outcome": r.get::<_, String>(4)?,
                "count": r.get::<_, i64>(5)?, "lastAt": r.get::<_, i64>(6)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        }
    }
    let program: super::ActionProgram = serde_json::from_str(&deployment.actions)?;
    let mut actions: Vec<Value> = program.actions.iter().map(|(name, a)| json!({"name": name, "kind": a.kind,
        "http": a.http, "signal": a.signal, "schedule": a.schedule, "notify": a.notify})).collect();
    actions.sort_by(|a,b| a["name"].as_str().cmp(&b["name"].as_str()));
    let start = parse_day(from).unwrap() * 86_400_000;
    let end = (parse_day(to).unwrap() + 1) * 86_400_000;
    Ok(json!({"deploymentId": deployment.id, "projectId": deployment.project_id, "projectName": deployment.project_name, "url": deployment.url,
        "usage": usage, "metrics": metrics, "actions": actions,
        "calls": retained_states(&data_root.join("calls.sqlite"), "calls", "deployment", &deployment.id, start, end)?,
        "mail": retained_states(&data_root.join("mail.sqlite"), "jobs", "scope", &format!("deployment:{}", deployment.id), start, end)?,
        "coverage": {"startedAt": started_at, "earliestMetricDay": earliest_day, "prunedThrough": pruned_through, "retentionDays": USAGE_DAYS,
            "approximate": true, "previewIncluded": false, "queueFlushed": flushed,
            "installationDroppedSinceRestart": DROPPED.load(Ordering::Relaxed),
            "installationWriteErrorsSinceRestart": WRITE_ERRORS.load(Ordering::Relaxed)}, "updatedAt": super::now_ms()}))
}

/// Keeps account responses bounded by operation and outcome categories, not event volume.
pub(crate) fn compact_overview(mut summary: Value) -> Value {
    let latest = summary["usage"]["days"].as_array().and_then(|days| days.last()).cloned();
    summary["usage"]["days"] = json!(latest.into_iter().collect::<Vec<_>>());
    summary["usage"]["scheduled"] = json!([]);
    // A notice keeps its outcome as "*: <reason>", so the overview can tell a queued notice from one never sent.
    let mut totals: std::collections::BTreeMap<(String, String), i64> = std::collections::BTreeMap::new();
    for row in summary["usage"]["totals"].as_array().into_iter().flatten() {
        let kind = row["kind"].as_str().unwrap_or("").to_owned();
        let name = match (kind.as_str(), row["name"].as_str().and_then(|name| name.rsplit_once(": "))) {
            ("notify", Some((_, reason))) => format!("*: {reason}"),
            _ => String::new(),
        };
        *totals.entry((kind, name)).or_default() += row["count"].as_i64().unwrap_or(0);
    }
    summary["usage"]["totals"] = json!(totals.into_iter().map(|((kind, name), count)| json!({"kind": kind, "name": name, "count": count})).collect::<Vec<_>>());
    let mut outcomes: std::collections::BTreeMap<(String, String, String), (i64, i64)> = std::collections::BTreeMap::new();
    for row in summary["metrics"].as_array().into_iter().flatten() {
        let key = (row["feature"].as_str().unwrap_or("").to_owned(), row["origin"].as_str().unwrap_or("").to_owned(), row["outcome"].as_str().unwrap_or("").to_owned());
        let entry = outcomes.entry(key).or_default();
        entry.0 += row["count"].as_i64().unwrap_or(0);
        entry.1 = entry.1.max(row["lastAt"].as_i64().unwrap_or(0));
    }
    summary["metrics"] = json!(outcomes.into_iter().map(|((feature, origin, outcome), (count, at))|
        json!({"feature": feature, "name": "", "origin": origin, "outcome": outcome, "count": count, "lastAt": at, "day": day(at as u64)})).collect::<Vec<_>>());
    summary
}

/// Current states of operations created in the period, limited to retained job records.
fn retained_states(path: &Path, table: &str, column: &str, scope: &str, from: i64, to: i64) -> Result<Value> {
    let Some(db) = read_only(path)? else { return Ok(json!({"states": [], "retainedOnly": true})); };
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)", [table], |r| r.get(0))?;
    if !exists { return Ok(json!({"states": [], "retainedOnly": true})); }
    // All identifiers are constants supplied above; only scope and dates come from a caller.
    let sql = format!("SELECT state, COUNT(*) FROM {table} WHERE {column} = ?1 AND created_at >= ?2 AND created_at < ?3{} GROUP BY state", if table == "jobs" { " AND purpose = 'notify'" } else { "" });
    let mut q = db.prepare(&sql)?;
    let states = q.query_map(params![scope, from, to], |r| Ok(json!({"state": r.get::<_, String>(0)?, "count": r.get::<_, i64>(1)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"states": states, "retainedOnly": true}))
}

pub(crate) fn read_signals(path: &Path, name: Option<&str>, cursor: Option<&str>) -> Result<Value> {
    let before = match cursor {
        None => i64::MAX,
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid cursor. Pass back the nextCursor of an earlier page."))?,
    };
    let Some(db) = read_only(path)? else {
        return Ok(json!({"signals": [], "nextCursor": null}));
    };
    let mut query = db.prepare(
        "SELECT id, name, at, properties FROM signals WHERE id < ?1 AND (?2 IS NULL OR name = ?2) ORDER BY id DESC LIMIT ?3",
    )?;
    let rows = query
        .query_map(params![before, name, SIGNALS_PAGE + 1], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() as i64 > SIGNALS_PAGE;
    let page: Vec<_> = rows.into_iter().take(SIGNALS_PAGE as usize).collect();
    let next = if more { page.last().map(|(id, ..)| id.to_string()) } else { None };
    let signals: Vec<Value> = page
        .into_iter()
        .map(|(_, name, at, properties)| {
            json!({"name": name, "at": at, "properties": serde_json::from_str::<Value>(&properties).unwrap_or(json!({}))})
        })
        .collect();
    Ok(json!({"signals": signals, "nextCursor": next}))
}

/// The number of Signals in a store.
#[cfg(test)]
fn signal_count(path: &Path) -> i64 {
    read_only(path)
        .unwrap()
        .map(|db| db.query_row("SELECT COUNT(*) FROM signals", [], |row| row.get(0)).unwrap())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActionProgram;

    fn program(actions: &Value) -> ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("konae-usage-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_overview_keeps_why_a_notice_was_not_sent_and_drops_other_names() {
        let summary = json!({"usage": {"days": [], "totals": [
            {"kind": "action", "name": "messages.send", "count": 3},
            {"kind": "notify", "name": "messages.send: no-transport", "count": 2},
            {"kind": "notify", "name": "replies.send: no-transport", "count": 1},
            {"kind": "notify", "name": "messages.send: queued", "count": 4},
        ]}, "metrics": []});
        assert_eq!(compact_overview(summary)["usage"]["totals"], json!([
            {"kind": "action", "name": "", "count": 3},
            {"kind": "notify", "name": "*: no-transport", "count": 3},
            {"kind": "notify", "name": "*: queued", "count": 4},
        ]));
    }

    /// The editor runs the same file in src/kernel/actions.test.ts.
    #[test]
    fn signals_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("signal-cases.json")).unwrap();
        assert!(cases.len() > 15);
        for case in cases {
            let program = program(&case["actions"]);
            let mut names = program.actions.keys().collect::<Vec<_>>();
            names.sort();
            let outcome = names.into_iter().try_for_each(|name| crate::validate_action(name, &program.actions[name]));
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn days_are_utc_calendar_dates() {
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(1_762_430_000_000), "2025-11-06");
        assert_eq!(day(951_782_400_000), "2000-02-29");
        for days in [-1, 0, 59, 11_016, 20_398, 40_000] {
            let (y, m, d) = civil(days);
            assert_eq!(parse_day(&format!("{y:04}-{m:02}-{d:02}")), Some(days));
        }
        assert_eq!(parse_day("2026-02-29"), None);
        assert_eq!(parse_day("2026-13-01"), None);
        assert_eq!(parse_day("2026-1-01"), None);
    }

    #[test]
    fn the_writer_counts_and_records_off_the_request_path() {
        let root = scratch("writer");
        let id = "deployment-a";
        std::fs::create_dir_all(crate::layout::data(&root, &id)).unwrap();
        let program = program(&json!({
            "orders.place": {"kind": "mutation", "access": "anonymous", "statement": "INSERT INTO orders (id, note) VALUES (?1, ?2)",
              "bindings": [{"source": "input", "key": "id", "type": "string"}, {"source": "input", "key": "note", "type": "string", "nullable": true}],
              "signal": {"name": "order.placed", "properties": ["id", "note"]}}
        }));
        let action = &program.actions["orders.place"];
        for _ in 0..3 {
            count(&root, id, Operation::Action("orders.place".into()));
        }
        count(&root, id, Operation::Route("POST".into(), "/hooks/orders".into()));
        count(&root, id, Operation::Login);
        let long = "é".repeat(100);
        signal(&root, id, "orders.place", action, &json!({"id": long}), None);
        // A Deployment folder that is gone gets no new file.
        count(&root, "deployment-gone", Operation::Session);
        flush();
        let path = crate::layout::usage(&root, &id);
        let today = day(super::super::now_ms());
        let usage = read_usage(&path, &today, &today).unwrap();
        let totals = usage["totals"].as_array().unwrap();
        assert!(totals.contains(&json!({"kind": "action", "name": "orders.place", "count": 3})), "{usage}");
        assert!(totals.contains(&json!({"kind": "route", "name": "POST /hooks/orders", "count": 1})));
        assert!(totals.contains(&json!({"kind": "login", "name": "", "count": 1})));
        let signals = read_signals(&path, Some("order.placed"), None).unwrap();
        let recorded = &signals["signals"][0]["properties"];
        assert_eq!(recorded["note"], Value::Null, "an omitted nullable input records null");
        let kept = recorded["id"].as_str().unwrap();
        assert!(kept.len() <= MAX_PROPERTY_BYTES && kept.chars().all(|c| c == 'é'), "cut at a character boundary");
        assert!(!crate::layout::folder(&root, "deployment-gone").exists());
        assert_eq!(read_signals(&path, Some("other.name"), None).unwrap()["signals"], json!([]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_store_drops_the_oldest_rows_past_its_bounds() {
        let root = scratch("bounds");
        let path = root.join(USAGE_DATABASE);
        let now = 1_800_000_000_000u64;
        {
            let mut db = open(&path).unwrap();
            let transaction = db.transaction().unwrap();
            // Days older than the window go, whatever the row count.
            let old = day(now - USAGE_DAYS as u64 * 86_400_000);
            transaction.execute("INSERT INTO usage_daily VALUES(?1, 'login', '', 7)", [&old]).unwrap();
            // More rows than the bound inside the window: the oldest days go first.
            let mut insert = transaction.prepare("INSERT INTO usage_daily VALUES(?1, 'action', ?2, 1)").unwrap();
            for index in 0..(MAX_USAGE_ROWS + 10) {
                let at = now - (index as u64 % 300) * 86_400_000;
                insert.execute(params![day(at), format!("a.n{index}")]).unwrap();
            }
            drop(insert);
            let mut insert = transaction.prepare("INSERT INTO signals(name, at, properties) VALUES('page.liked', ?1, '{}')").unwrap();
            for index in 0..(MAX_SIGNALS + 25) {
                insert.execute([index]).unwrap();
            }
            drop(insert);
            prune(&transaction, now).unwrap();
            transaction.commit().unwrap();
        }
        let db = Connection::open(&path).unwrap();
        let rows: i64 = db.query_row("SELECT COUNT(*) FROM usage_daily", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, MAX_USAGE_ROWS);
        let oldest_kept: String = db.query_row("SELECT MIN(day) FROM usage_daily", [], |r| r.get(0)).unwrap();
        assert_eq!(oldest_kept, day(now - 299 * 86_400_000), "the oldest day lost rows first");
        let today: i64 = db.query_row("SELECT COUNT(*) FROM usage_daily WHERE day=?1", [day(now)], |r| r.get(0)).unwrap();
        assert!(today > 300, "the newest day is whole");
        assert_eq!(signal_count(&path), MAX_SIGNALS);
        let oldest_signal: i64 = db.query_row("SELECT MIN(at) FROM signals", [], |r| r.get(0)).unwrap();
        assert_eq!(oldest_signal, 25, "the first 25 Signals were dropped");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn daily_signal_totals_survive_detail_pruning_and_old_stores_stay_readable() {
        let root = scratch("metric-history");
        let path = root.join(USAGE_DATABASE);
        let at = super::super::now_ms();
        let mut db = open(&path).unwrap();
        let tx = db.transaction().unwrap();
        metric(&tx, at, "signal", "order.placed", "kernel", "recorded", MAX_SIGNALS + 10).unwrap();
        tx.execute("INSERT INTO signals(name, at, properties) VALUES('order.placed', ?1, '{}')", [at as i64]).unwrap();
        tx.execute("DELETE FROM signals", []).unwrap();
        prune(&tx, at).unwrap();
        tx.commit().unwrap();
        assert_eq!(signal_count(&path), 0);
        assert_eq!(db.query_row("SELECT count FROM metric_daily WHERE name = 'order.placed'", [], |r| r.get::<_, i64>(0)).unwrap(), MAX_SIGNALS + 10);
        // A pre-monitor store remains unchanged until its next write.
        let old = root.join("old.sqlite");
        Connection::open(&old).unwrap().execute_batch("CREATE TABLE usage_daily(day TEXT, kind TEXT, name TEXT, count INTEGER);").unwrap();
        assert_eq!(read_usage(&old, &day(at), &day(at)).unwrap()["totals"], json!([]));
        assert!(!Connection::open(&old).unwrap().query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='metric_daily')", [], |r| r.get::<_, bool>(0)).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reads_refuse_bad_ranges_and_cursors_and_page_signals() {
        let root = scratch("reads");
        let path = root.join(USAGE_DATABASE);
        assert_eq!(read_usage(&path, "2026-11-01", "2026-11-06").unwrap()["days"], json!([]), "no store yet reads as empty");
        assert!(!path.exists(), "a read never creates the store");
        for (from, to, message) in [
            ("2026-11-6", "2026-11-06", "from and to must be dates written YYYY-MM-DD."),
            ("2026-11-07", "2026-11-06", "from must not be after to."),
            ("2025-01-01", "2026-11-06", "Usage covers at most 400 days at a time."),
        ] {
            assert_eq!(read_usage(&path, from, to).unwrap_err().to_string(), message);
        }
        {
            let db = open(&path).unwrap();
            for index in 0..150 {
                db.execute("INSERT INTO signals(name, at, properties) VALUES('page.liked', ?1, '{}')", [index]).unwrap();
            }
        }
        let first = read_signals(&path, None, None).unwrap();
        assert_eq!(first["signals"].as_array().unwrap().len(), 100);
        assert_eq!(first["signals"][0]["at"], 149, "newest first");
        let second = read_signals(&path, None, first["nextCursor"].as_str()).unwrap();
        assert_eq!(second["signals"].as_array().unwrap().len(), 50);
        assert_eq!(second["nextCursor"], Value::Null);
        assert_eq!(
            read_signals(&path, None, Some("abc")).unwrap_err().to_string(),
            "Invalid cursor. Pass back the nextCursor of an earlier page."
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
