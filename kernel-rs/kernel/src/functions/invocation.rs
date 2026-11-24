use super::Function;
use super::trace::{TraceBuffer, TraceRecord};
use crate::{ActionProgram, refusal::Refusal};
use konae_functions_executor::{Budget, BudgetError, MutationOutcomeState, Output};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const RECEIPTS: &str = "__konae_function_receipts";
const RECEIPT_SCHEMA: &str = "CREATE TABLE __konae_function_receipts (actor_scope TEXT NOT NULL, function_name TEXT NOT NULL, idempotency_key TEXT NOT NULL, fingerprint TEXT NOT NULL, response_json TEXT NOT NULL, PRIMARY KEY(actor_scope,function_name,idempotency_key)) WITHOUT ROWID";
const RECEIPT_RESPONSE_MARKER: &str = "null";
pub const IDEMPOTENCY_REPLAY_CODE: &str = "idempotency_replay";
pub const IDEMPOTENCY_REPLAY_MESSAGE: &str =
    "This request was already processed. Check its current state through an authorized read.";
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 128;
const MAX_OPERATIONS: usize = 64;

type Result<T> = std::result::Result<T, Failure>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
}
impl Failure {
    fn new(code: &'static str, status: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            status,
            message: message.into(),
        }
    }
    fn storage() -> Self {
        Self::new(
            "function.storage-error",
            500,
            "The function could not use its database.",
        )
    }
    fn unknown() -> Self {
        Self::new(
            "mutation_outcome_unknown",
            409,
            "The mutation outcome is unknown. Repeat this request with the same Idempotency-Key.",
        )
    }
    fn schema() -> Self {
        Self::new(
            "function.receipts-invalid",
            409,
            "The function receipt table has an invalid schema or attached objects.",
        )
    }
    pub fn replay() -> Self {
        Self::new(IDEMPOTENCY_REPLAY_CODE, 409, IDEMPOTENCY_REPLAY_MESSAGE)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
impl From<BudgetError> for Failure {
    fn from(error: BudgetError) -> Self {
        Self::new(error.code(), 504, error.code())
    }
}
impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        Self::new(refusal.code, 400, refusal.message)
    }
}

pub struct InvocationConfig {
    pub database: Option<PathBuf>,
    pub program: Arc<ActionProgram>,
    pub function_name: String,
    pub function: Function,
    pub actor: Option<String>,
    pub method: String,
    pub path: String,
    pub body_len: usize,
    pub fingerprint: String,
    pub headers: Vec<(String, String)>,
    pub idempotency_key: Option<String>,
}

pub enum Admission {
    Replay(Failure),
    Run(Invocation),
}

pub fn lookup_receipt(
    database: &PathBuf,
    actor: Option<&str>,
    function_name: &str,
    key: &str,
    budget: &Budget,
) -> Result<Option<String>> {
    let connection = RequestConnection::open(database, true, budget)?;
    let stored: Option<(String, Option<String>)> = connection.internal(|| {
        connection.sql(|| {
            connection
                .db
                .query_row(
                    "SELECT type,sql FROM sqlite_schema WHERE lower(name)=lower(?1) LIMIT 1",
                    [RECEIPTS],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
    })?;
    let Some((kind, Some(sql))) = stored else {
        return Ok(None);
    };
    if kind != "table" || normalize_sql(&sql) != normalize_sql(RECEIPT_SCHEMA) {
        return Err(Failure::schema());
    }
    let extras: i64 = connection.internal(|| connection.sql(|| connection.db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE lower(tbl_name)=lower(?1) AND lower(name)!=lower(?1) AND type IN ('trigger','view','index') LIMIT 1",
        [RECEIPTS],
        |row| row.get(0),
    )))?;
    if extras != 0 {
        return Err(Failure::schema());
    }
    let columns: Vec<(String, String, bool, i64, i64)> = connection.internal(|| {
        connection.sql(|| {
            let mut statement = connection
                .db
                .prepare("PRAGMA table_xinfo(__konae_function_receipts)")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })?
                .collect()
        })
    })?;
    let expected: Vec<_> = [
        "actor_scope",
        "function_name",
        "idempotency_key",
        "fingerprint",
        "response_json",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, name)| {
        (
            name.to_owned(),
            "TEXT".to_owned(),
            true,
            if index < 3 { index as i64 + 1 } else { 0 },
            0,
        )
    })
    .collect();
    if columns != expected {
        return Err(Failure::schema());
    }
    let actor_scope = serde_json::to_string(&actor).map_err(|_| Failure::storage())?;
    let receipt: Option<String> = connection.internal(|| connection.sql(|| connection.db.query_row(
        "SELECT fingerprint FROM __konae_function_receipts WHERE actor_scope=?1 AND function_name=?2 AND idempotency_key=?3 LIMIT 1",
        params![actor_scope, function_name, key],
        |row| row.get(0),
    ).optional()))?;
    Ok(receipt)
}
pub struct Invocation {
    database: Option<PathBuf>,
    program: Arc<ActionProgram>,
    function_name: String,
    allowed: HashSet<String>,
    actor: Option<String>,
    actor_scope: String,
    method: String,
    key: Option<String>,
    fingerprint: String,
    budget: Budget,
    connection: Option<RequestConnection>,
    poisoned: Option<Failure>,
    operations: usize,
    active: bool,
    trace: Option<TraceBuffer>,
    transaction_recorded: bool,
}

/// El caller ya comprobó identidad, módulos y permisos actuales y mantiene el bloqueo de publicación.
pub fn admit(config: InvocationConfig, budget: Budget) -> Result<Admission> {
    budget.check()?;
    if !matches!(config.method.as_str(), "GET" | "POST")
        || !config.function.methods.contains(&config.method)
    {
        return Err(Failure::new(
            "function.method-not-allowed",
            405,
            "The function does not declare this HTTP method.",
        ));
    }
    if config.function.access != "anonymous" && config.actor.is_none() {
        return Err(Failure::new(
            "authentication-required",
            401,
            "authentication-required",
        ));
    }
    if config.body_len > 100_000_000 || config.path.len() > 8192 {
        return Err(Failure::new(
            "body_too_large",
            413,
            "The function request is too large.",
        ));
    }
    for name in &config.function.actions {
        let action = config.program.actions.get(name).ok_or_else(|| {
            Failure::new(
                "function.action-missing",
                400,
                format!(
                    "Function {} names unknown action {name}.",
                    config.function_name
                ),
            )
        })?;
        crate::functions::check_function_action(
            &config.function_name,
            name,
            action,
            &config.program,
        )?;
        crate::validate_action(name, action)?;
    }
    let has_data = !config.function.actions.is_empty();
    if has_data && config.database.is_none() {
        return Err(Failure::storage());
    }
    let key = if config.method == "POST" && has_data {
        let key = config
            .idempotency_key
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                Failure::new(
                    "idempotency_key_required",
                    400,
                    "Idempotency-Key is required for this function.",
                )
            })?;
        if key.len() > 128 || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(Failure::new(
                "invalid_input",
                400,
                "Idempotency-Key must contain 1 to 128 visible ASCII bytes.",
            ));
        }
        Some(key)
    } else {
        None
    };
    let fingerprint = config.fingerprint;
    let actor_scope = serde_json::to_string(&config.actor).map_err(|_| Failure::storage())?;
    let mut invocation = Invocation {
        database: config.database,
        program: config.program,
        function_name: config.function_name,
        allowed: config.function.actions.into_iter().collect(),
        actor: config.actor,
        actor_scope,
        method: config.method,
        key,
        fingerprint,
        budget,
        connection: None,
        poisoned: None,
        operations: 0,
        active: true,
        trace: None,
        transaction_recorded: false,
    };
    if invocation.key.is_some() {
        invocation.open_connection()?;
        let conn = invocation.connection.as_ref().unwrap();
        conn.sql(|| conn.db.execute_batch("BEGIN IMMEDIATE"))?;
        conn.ensure_receipt_schema()?;
        let receipt: Option<String> = conn.internal(|| conn.sql(|| conn.db.query_row(
            "SELECT fingerprint FROM __konae_function_receipts WHERE actor_scope=?1 AND function_name=?2 AND idempotency_key=?3 LIMIT 1",
            params![invocation.actor_scope, invocation.function_name, invocation.key],
            |row| row.get(0)).optional()))?;
        if let Some(stored_fingerprint) = receipt {
            if stored_fingerprint != invocation.fingerprint {
                return Err(Failure::new(
                    "idempotency_conflict",
                    409,
                    "Idempotency-Key was already used with a different request.",
                ));
            }
            conn.rollback()?;
            invocation.budget.check()?;
            return Ok(Admission::Replay(Failure::replay()));
        }
    }
    Ok(Admission::Run(invocation))
}

impl Invocation {
    pub fn action(&mut self, name: &str, input: &Value) -> Result<Value> {
        self.action_inner_recorded(name, input, None, 0)
    }
    pub(super) fn action_traced(
        &mut self,
        name: &str,
        input: &Value,
        trace: &TraceBuffer,
        sequence: u16,
    ) -> Result<Value> {
        self.trace = Some(trace.clone());
        self.action_inner_recorded(name, input, Some(trace), sequence)
    }
    fn action_inner_recorded(
        &mut self,
        name: &str,
        input: &Value,
        trace: Option<&TraceBuffer>,
        sequence: u16,
    ) -> Result<Value> {
        if !self.active {
            return Err(Failure::new(
                "function.invocation-closed",
                403,
                "The function invocation is closed.",
            ));
        }
        if let Some(error) = &self.poisoned {
            return Err(error.clone());
        }
        let started = std::time::Instant::now();
        let trace_start_us = trace.map(TraceBuffer::elapsed_us);
        let resolved = self.allowed.contains(name) && self.program.actions.contains_key(name);
        let span_index = trace.map(|trace| {
            trace.push(TraceRecord {
                parent: Some(0),
                name: "konae.action",
                start_us: trace_start_us.unwrap_or_default(),
                duration_us: 0,
                action_name: Some(if resolved { name } else { "<unknown>" }),
                origin: Some("function"),
                sequence: Some(sequence),
                rows_read: None,
                rows_written: None,
                reply_bytes: None,
                sqlite_us: None,
                error_code: None,
            })
        });
        let result = self.action_inner(name, input, trace, sequence, span_index.flatten());
        if let Some(trace) = trace {
            let (rows_read, rows_written, reply_bytes) =
                result.as_ref().ok().map_or((None, None, None), |answer| {
                    let read = answer
                        .as_array()
                        .map(|rows| rows.len().min(u32::MAX as usize) as u32);
                    let written = answer
                        .get("changes")
                        .and_then(Value::as_u64)
                        .map(|n| n.min(u32::MAX as u64) as u32);
                    (read, written, None)
                });
            let code = result.as_ref().err().map(|error| match error.code {
                "function.invocation-closed" => "function.invocation-closed",
                "function.action-not-allowed" => "function.action-not-allowed",
                "function.operation-limit" => "function.operation-limit",
                "function.input-too-large" => "function.input-too-large",
                "function.action-refused" => "function.action-refused",
                "function.action-failed" => "function.action-failed",
                "authentication-required" => "authentication-required",
                "function.get-mutation" => "function.get-mutation",
                _ => "function.action-failed",
            });
            let duration_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            if let Some(index) = span_index.flatten() {
                trace.update(
                    index,
                    TraceRecord {
                        parent: Some(0),
                        name: "konae.action",
                        start_us: trace_start_us.unwrap_or_default(),
                        duration_us,
                        action_name: Some(if resolved { name } else { "<unknown>" }),
                        origin: Some("function"),
                        sequence: Some(sequence),
                        rows_read,
                        rows_written,
                        reply_bytes,
                        sqlite_us: None,
                        error_code: code,
                    },
                );
            }
        }
        if let Err(error) = &result {
            self.poisoned = Some(error.clone());
        }
        result
    }
    fn action_inner(
        &mut self,
        name: &str,
        input: &Value,
        trace: Option<&TraceBuffer>,
        sequence: u16,
        action_span: Option<u8>,
    ) -> Result<Value> {
        self.budget.check()?;
        if !self.allowed.contains(name) {
            return Err(Failure::new(
                "function.action-not-allowed",
                403,
                "The function cannot invoke this Server Action.",
            ));
        }
        self.operations += 1;
        if self.operations > MAX_OPERATIONS {
            return Err(Failure::new(
                "function.operation-limit",
                429,
                "The function exceeded its Server Action limit.",
            ));
        }
        bounded_json(
            input,
            konae_functions_executor::protocol::MAX_OPERATION_BYTES,
        )
        .map_err(|_| {
            Failure::new(
                "function.input-too-large",
                413,
                "The Server Action input is too large.",
            )
        })?;
        let action = self
            .program
            .actions
            .get(name)
            .ok_or_else(Failure::storage)?
            .clone();
        crate::functions::check_function_action(&self.function_name, name, &action, &self.program)?;
        crate::validate_action(name, &action)?;
        if self.method == "GET" && action.kind != "query" {
            return Err(Failure::new(
                "function.get-mutation",
                405,
                "GET functions cannot invoke mutations.",
            ));
        }
        if action.access != "anonymous" && self.actor.is_none() {
            return Err(Failure::new(
                "authentication-required",
                401,
                "authentication-required",
            ));
        }
        let params = crate::resolve_bindings(&action, input, self.actor.as_deref())
            .map_err(|message| Failure::new("function.input-invalid", 400, message))?;
        self.open_connection()?;
        let conn = self.connection.as_ref().unwrap();
        let sqlite_started = std::time::Instant::now();
        let result = if action.kind == "query" {
            crate::run_query_on_connection(&conn.db, &action, params).map(Value::from)
        } else {
            crate::run_mutation_on_connection(&conn.db, &action, params).map(|ran| ran.answer)
        };
        let sqlite_us = sqlite_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        if let Some(trace) = trace {
            let _ = trace.push(TraceRecord {
                parent: Some(action_span.unwrap_or(0)),
                name: "konae.sqlite",
                start_us: trace.elapsed_us().saturating_sub(sqlite_us),
                duration_us: sqlite_us,
                action_name: None,
                origin: None,
                sequence: Some(sequence),
                rows_read: None,
                rows_written: None,
                reply_bytes: None,
                sqlite_us: Some(sqlite_us),
                error_code: result.as_ref().err().map(|_| "function.action-failed"),
            });
        }
        self.budget.check()?;
        let answer = result
            .map_err(|error| Failure::new("function.action-failed", 400, error.to_string()))?;
        if let crate::guard::Outcome::Refused(message) =
            crate::guard::outcome(name, &action, &answer)
        {
            return Err(Failure::new("function.action-refused", 403, message));
        }
        Ok(answer)
    }
    fn open_connection(&mut self) -> Result<()> {
        if self.connection.is_none() {
            let path = self.database.as_ref().ok_or_else(Failure::storage)?;
            self.connection = Some(RequestConnection::open(
                path,
                self.method == "GET",
                &self.budget,
            )?);
        }
        Ok(())
    }
    pub fn finish(mut self, output: Output) -> Result<Output> {
        if !self.active {
            return Err(Failure::new(
                "function.invocation-closed",
                403,
                "The function invocation is closed.",
            ));
        }
        if let Some(error) = self.poisoned.take() {
            self.abort()?;
            return Err(error);
        }
        if let Err(error) = validate_output(&output) {
            self.abort()?;
            return Err(error);
        }
        if let Err(error) = self.budget.check() {
            self.abort()?;
            return Err(error.into());
        }
        if output.status >= 400 {
            self.abort()?;
            return Ok(output);
        }
        if let Some(key) = self.key.clone() {
            let conn = self.connection.as_ref().ok_or_else(Failure::storage)?;
            let commit_started = std::time::Instant::now();
            let result = (|| {
                conn.ensure_receipt_schema()?;
                conn.internal(|| conn.sql(|| conn.db.execute(
                    "INSERT INTO __konae_function_receipts (actor_scope,function_name,idempotency_key,fingerprint,response_json) VALUES (?1,?2,?3,?4,?5)",
                    params![self.actor_scope, self.function_name, key, self.fingerprint, RECEIPT_RESPONSE_MARKER])))?;
                self.budget.begin_commit()?;
                conn.commit()?;
                self.budget.committed();
                if self.budget.check().is_err() {
                    return Err(Failure::unknown());
                }
                Ok(())
            })();
            if result.is_ok() {
                self.record_transaction("committed", commit_started.elapsed(), None);
            }
            if let Err(error) = result {
                let code = if self.budget.mutation_outcome().may_have_committed() {
                    "mutation_outcome_unknown"
                } else {
                    error.code
                };
                let result_outcome = if code == "mutation_outcome_unknown" {
                    "unknown"
                } else {
                    "rolled_back"
                };
                self.record_transaction(result_outcome, commit_started.elapsed(), Some(code));
                self.abort()?;
                if self.budget.mutation_outcome().may_have_committed() {
                    return Err(Failure::unknown());
                }
                return Err(error);
            }
        }
        Ok(output)
    }
    pub fn abort(&mut self) -> Result<()> {
        self.active = false;
        if let Some(conn) = &self.connection {
            let started = std::time::Instant::now();
            let result = conn.rollback();
            if !self.transaction_recorded {
                let outcome = if result.is_ok() {
                    "rolled_back"
                } else {
                    "unknown"
                };
                self.record_transaction(
                    outcome,
                    started.elapsed(),
                    result.as_ref().err().map(|_| "mutation_outcome_unknown"),
                );
            }
            result?;
        }
        Ok(())
    }

    fn record_transaction(
        &mut self,
        outcome: &'static str,
        duration: std::time::Duration,
        error_code: Option<&'static str>,
    ) {
        if self.transaction_recorded {
            return;
        }
        if let Some(trace) = &self.trace {
            let duration_us = duration.as_micros().min(u64::MAX as u128) as u64;
            let _ = trace.push(TraceRecord {
                parent: Some(0),
                name: "konae.transaction",
                start_us: trace.elapsed_us().saturating_sub(duration_us),
                duration_us,
                action_name: Some(outcome),
                origin: None,
                sequence: None,
                rows_read: None,
                rows_written: None,
                reply_bytes: None,
                sqlite_us: None,
                error_code,
            });
        }
        self.transaction_recorded = true;
    }
}
impl Drop for Invocation {
    fn drop(&mut self) {
        let _ = self.abort();
    }
}

struct RequestConnection {
    db: Connection,
    internal: Arc<AtomicBool>,
    budget: Budget,
}
impl RequestConnection {
    fn open(path: &PathBuf, read_only: bool, budget: &Budget) -> Result<Self> {
        budget.check()?;
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let db = Connection::open_with_flags(path, flags).map_err(|_| Failure::storage())?;
        let progress = budget.clone();
        db.progress_handler(100, Some(move || progress.check().is_err()))
            .map_err(|_| Failure::storage())?;
        let connection = Self {
            db,
            internal: Arc::new(AtomicBool::new(false)),
            budget: budget.clone(),
        };
        connection.sql(|| {
            crate::restrict_function_database(&connection.db, connection.internal.clone())
        })?;
        let commit = budget.clone();
        connection
            .db
            .commit_hook(Some(move || commit.check().is_err()))
            .map_err(|_| Failure::storage())?;
        let rollback = budget.clone();
        connection
            .db
            .rollback_hook(Some(move || rollback.rolled_back()))
            .map_err(|_| Failure::storage())?;
        budget.check()?;
        Ok(connection)
    }
    fn sql<T>(&self, mut operation: impl FnMut() -> rusqlite::Result<T>) -> Result<T> {
        loop {
            self.budget.check()?;
            match operation() {
                Ok(value) => {
                    self.budget.check()?;
                    return Ok(value);
                }
                Err(error)
                    if matches!(
                        error.sqlite_error_code(),
                        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    ) =>
                {
                    self.budget.wait_retry()?
                }
                Err(_) => {
                    self.budget.check()?;
                    return Err(Failure::storage());
                }
            }
        }
    }
    fn internal<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        self.internal.store(true, Ordering::SeqCst);
        let _reset = Reset(&self.internal);
        operation()
    }
    fn ensure_receipt_schema(&self) -> Result<()> {
        self.internal(|| {
            let stored: Option<(String, Option<String>)> = self.sql(|| self.db.query_row(
                "SELECT type,sql FROM sqlite_schema WHERE lower(name)=lower(?1) LIMIT 1", [RECEIPTS],
                |row| Ok((row.get(0)?, row.get(1)?))).optional())?;
            match stored {
                None => { self.sql(|| self.db.execute_batch(RECEIPT_SCHEMA))?; }
                Some((kind, Some(sql))) if kind == "table" && normalize_sql(&sql) == normalize_sql(RECEIPT_SCHEMA) => {}
                _ => return Err(Failure::schema()),
            }
            let extras: i64 = self.sql(|| self.db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE lower(tbl_name)=lower(?1) AND lower(name)!=lower(?1) AND type IN ('trigger','view','index') LIMIT 1", [RECEIPTS], |row| row.get(0)))?;
            if extras != 0 { return Err(Failure::schema()); }
            let columns: Vec<(String, String, bool, i64, i64)> = self.sql(|| {
                let mut statement = self.db.prepare("PRAGMA table_xinfo(__konae_function_receipts)")?;
                statement.query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(5)?, row.get(6)?)))?.collect()
            })?;
            let expected: Vec<_> = ["actor_scope","function_name","idempotency_key","fingerprint","response_json"].into_iter()
                .enumerate().map(|(index,name)| (name.to_owned(), "TEXT".to_owned(), true, if index < 3 { index as i64 + 1 } else { 0 }, 0)).collect();
            if columns != expected { return Err(Failure::schema()); }
            Ok(())
        })
    }
    fn commit(&self) -> Result<()> {
        loop {
            self.budget.check()?;
            match self.db.execute_batch("COMMIT") {
                Ok(()) => return Ok(()),
                Err(error)
                    if matches!(
                        error.sqlite_error_code(),
                        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    ) =>
                {
                    self.budget.wait_retry()?
                }
                Err(_) => {
                    if self.db.is_autocommit()
                        && self.budget.mutation_outcome() == MutationOutcomeState::Committing
                    {
                        self.budget.uncertain();
                        return Err(Failure::unknown());
                    }
                    self.budget.check()?;
                    return Err(Failure::storage());
                }
            }
        }
    }
    fn rollback(&self) -> Result<()> {
        if !self.db.is_autocommit() {
            self.db
                .progress_handler(0, None::<fn() -> bool>)
                .map_err(|_| Failure::storage())?;
            if self.db.execute_batch("ROLLBACK").is_err() {
                self.budget.uncertain();
                return Err(Failure::unknown());
            }
        }
        Ok(())
    }
}
impl Drop for RequestConnection {
    fn drop(&mut self) {
        let _ = self.rollback();
    }
}
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn request_fingerprint_v3(
    method: &str,
    path: &str,
    body_len: usize,
    body_digest: &[u8; 32],
    headers: &[(String, String)],
) -> Result<String> {
    let mut canonical: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut bytes = 0usize;
    if headers.len() > MAX_HEADERS {
        return Err(Failure::new(
            "invalid_input",
            400,
            "The function request has too many headers.",
        ));
    }
    for (name, value) in headers {
        bytes = bytes.saturating_add(name.len()).saturating_add(value.len());
        if bytes > MAX_RESPONSE_HEADER_BYTES
            || axum::http::HeaderName::from_bytes(name.as_bytes()).is_err()
            || axum::http::HeaderValue::from_str(value).is_err()
        {
            return Err(Failure::new(
                "invalid_input",
                400,
                "The function request has invalid headers.",
            ));
        }
        let name = name.to_ascii_lowercase();
        if [
            "cookie",
            "authorization",
            "proxy-authorization",
            "idempotency-key",
            "host",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        canonical
            .entry(name)
            .or_default()
            .push(value.trim_matches([' ', '\t']).to_owned());
    }
    let mut digest = Sha256::new();
    digest.update(b"konae-function-request-v3");
    for part in [method.as_bytes(), path.as_bytes()] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.update((body_len as u64).to_be_bytes());
    digest.update(body_digest);
    digest.update((canonical.len() as u64).to_be_bytes());
    for (name, values) in canonical {
        for part in [name.as_bytes(), values.join(", ").as_bytes()] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
    }
    Ok(format!("v3:{}", hex::encode(digest.finalize())))
}

fn validate_output(output: &Output) -> Result<String> {
    if !(200..=599).contains(&output.status)
        || (matches!(output.status, 204 | 304) && !output.body.is_empty())
    {
        return Err(Failure::new(
            "function.output-invalid",
            500,
            "The function returned an invalid HTTP response.",
        ));
    }
    let mut header_bytes = 0usize;
    if output.headers.len() > MAX_HEADERS || output.body.len() > MAX_RESPONSE_BYTES {
        return Err(output_too_large());
    }
    for (name, value) in &output.headers {
        header_bytes = header_bytes
            .saturating_add(name.len())
            .saturating_add(value.len());
        if header_bytes > MAX_RESPONSE_HEADER_BYTES {
            return Err(output_too_large());
        }
        if axum::http::HeaderName::from_bytes(name.as_bytes()).is_err()
            || axum::http::HeaderValue::from_str(value).is_err()
            || [
                "connection",
                "transfer-encoding",
                "content-length",
                "upgrade",
                "keep-alive",
                "proxy-authenticate",
                "proxy-authorization",
                "te",
                "trailer",
            ]
            .contains(&name.to_ascii_lowercase().as_str())
        {
            return Err(Failure::new(
                "function.output-invalid",
                500,
                "The function returned an invalid HTTP response.",
            ));
        }
    }
    bounded_json(output, MAX_RESPONSE_BYTES).map_err(|_| output_too_large())
}
fn output_too_large() -> Failure {
    Failure::new(
        "function.output-too-large",
        500,
        "The function response exceeds its size limit.",
    )
}
fn bounded_json(value: &impl serde::Serialize, maximum: usize) -> std::result::Result<String, ()> {
    struct Bounded {
        bytes: Vec<u8>,
        maximum: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len().saturating_add(bytes.len()) > self.maximum {
                return Err(std::io::Error::other("response too large"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer(&mut output, value).map_err(|_| ())?;
    String::from_utf8(output.bytes).map_err(|_| ())
}

#[cfg(test)]
#[path = "invocation_tests.rs"]
mod tests;
