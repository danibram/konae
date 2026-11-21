use crate::budget::{Budget, BudgetError, MutationOutcomeState};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::Path,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Database = Arc<Mutex<Connection>>;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: i64,
    pub owner_id: String,
    pub quantity: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    pub order: Option<Order>,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataError {
    Budget(BudgetError),
    IdempotencyKeyRequired,
    InvalidInput,
    IdempotencyConflict,
    MutationOutcomeUnknown,
    StorageError,
}
impl DataError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Budget(error) => error.code(),
            Self::IdempotencyKeyRequired => "idempotency_key_required",
            Self::InvalidInput => "invalid_input",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::MutationOutcomeUnknown => "mutation_outcome_unknown",
            Self::StorageError => "storage_error",
        }
    }
}
impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for DataError {}
impl From<BudgetError> for DataError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}
type Result<T> = std::result::Result<T, DataError>;

pub fn open(dir: &Path, app: &str) -> anyhow::Result<Database> {
    anyhow::ensure!(["a", "b"].contains(&app), "invalid app");
    let conn = Connection::open(dir.join(format!("{app}.sqlite")))?;
    conn.busy_timeout(Duration::ZERO)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 10));
      CREATE TABLE IF NOT EXISTS sessions (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, actor_id TEXT NOT NULL, quantity INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS mutation_receipts (
        actor_id TEXT NOT NULL, operation TEXT NOT NULL, idempotency_key TEXT NOT NULL,
        order_id INTEGER NOT NULL, quantity INTEGER NOT NULL, response_json TEXT NOT NULL,
        PRIMARY KEY(actor_id, operation, idempotency_key));
      INSERT OR IGNORE INTO orders VALUES (1,'alice',2),(2,'bob',4);")?;
    for user in ["alice", "bob"] {
        conn.execute(
            "INSERT OR IGNORE INTO sessions VALUES (?1,?2,4102444800)",
            params![format!("{app}-{user}"), user],
        )?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO sessions VALUES (?1,'alice',1)",
        [format!("{app}-expired")],
    )?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn authenticate(db: &Database, token: &str, budget: &Budget) -> Result<Option<String>> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DataError::StorageError)?
        .as_secs();
    let conn = RequestConnection::acquire(db, budget)?;
    conn.sql(|| {
        conn.connection
            .prepare_cached(
                "SELECT user_id FROM sessions WHERE token=?1 AND expires_at>?2 LIMIT 1",
            )?
            .query_row(params![token, now], |row| row.get(0))
            .optional()
    })
}

pub fn read(db: &Database, actor: &str, id: i64, budget: &Budget) -> Result<Option<Order>> {
    validate_order(id)?;
    let conn = RequestConnection::acquire(db, budget)?;
    conn.sql(|| {
        conn.connection
            .prepare_cached(
                "SELECT id,owner_id,quantity FROM orders WHERE id=?1 AND owner_id=?2 LIMIT 1",
            )?
            .query_row(params![id, actor], row_order)
            .optional()
    })
}

pub fn update(
    db: &Database,
    actor: &str,
    id: i64,
    quantity: i64,
    idempotency_key: &str,
    budget: &Budget,
) -> Result<MutationOutcome> {
    update_inner(db, actor, id, quantity, idempotency_key, budget, |_| {})
}

fn update_inner(
    db: &Database,
    actor: &str,
    id: i64,
    quantity: i64,
    key: &str,
    budget: &Budget,
    observe: impl Fn(MutationStep),
) -> Result<MutationOutcome> {
    validate_order(id)?;
    if !(1..=10).contains(&quantity) {
        return Err(DataError::InvalidInput);
    }
    if key.is_empty() {
        return Err(DataError::IdempotencyKeyRequired);
    }
    if key.len() > 128 || !key.bytes().all(|c| (0x21..=0x7e).contains(&c)) {
        return Err(DataError::InvalidInput);
    }
    let conn = RequestConnection::acquire(db, budget)?;
    let result = (|| {
        conn.sql(|| conn.connection.execute_batch("BEGIN IMMEDIATE"))?;
        let receipt: Option<(i64, i64, String)> = conn.sql(|| conn.connection.prepare_cached(
            "SELECT order_id,quantity,response_json FROM mutation_receipts WHERE actor_id=?1 AND operation='orders.update' AND idempotency_key=?2 LIMIT 1")?
            .query_row(params![actor, key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional())?;
        if let Some((previous_id, previous_quantity, response_json)) = receipt {
            if previous_id != id || previous_quantity != quantity {
                return Err(DataError::IdempotencyConflict);
            }
            let order: Option<Order> =
                serde_json::from_str(&response_json).map_err(|_| DataError::StorageError)?;
            conn.rollback()?;
            budget.check()?;
            if order.is_some() {
                budget.committed();
            }
            return Ok(MutationOutcome {
                order,
                replayed: true,
            });
        }
        let order = conn.sql(|| conn.connection.prepare_cached(
            "UPDATE orders SET quantity=?1 WHERE id=?2 AND owner_id=?3 RETURNING id,owner_id,quantity")?
            .query_row(params![quantity, id, actor], row_order).optional())?;
        let Some(order) = order else {
            conn.rollback()?;
            budget.check()?;
            return Ok(MutationOutcome {
                order: None,
                replayed: false,
            });
        };
        conn.sql(|| {
            conn.connection
                .prepare_cached("INSERT INTO audit (order_id,actor_id,quantity) VALUES (?1,?2,?3)")?
                .execute(params![id, actor, quantity])
        })?;
        let response_json = serde_json::to_string(&order).map_err(|_| DataError::StorageError)?;
        conn.sql(|| conn.connection.prepare_cached(
            "INSERT INTO mutation_receipts (actor_id,operation,idempotency_key,order_id,quantity,response_json) VALUES (?1,'orders.update',?2,?3,?4,?5)")?
            .execute(params![actor, key, id, quantity, response_json]))?;
        observe(MutationStep::BeforeCommit);
        budget.begin_commit()?;
        conn.commit()?;
        budget.committed();
        observe(MutationStep::AfterCommit);
        if budget.check().is_err() {
            return Err(DataError::MutationOutcomeUnknown);
        }
        Ok(MutationOutcome {
            order: Some(order),
            replayed: false,
        })
    })();
    if result.is_err() {
        if conn.rollback().is_err() {
            budget.uncertain();
            return Err(DataError::MutationOutcomeUnknown);
        }
        if budget.mutation_outcome().may_have_committed() {
            return Err(DataError::MutationOutcomeUnknown);
        }
    }
    result
}

#[derive(Clone, Copy)]
enum MutationStep {
    BeforeCommit,
    AfterCommit,
}
fn validate_order(id: i64) -> Result<()> {
    if (1..=999_999_999).contains(&id) {
        Ok(())
    } else {
        Err(DataError::InvalidInput)
    }
}
fn row_order(row: &rusqlite::Row<'_>) -> rusqlite::Result<Order> {
    Ok(Order {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        quantity: row.get(2)?,
    })
}

struct RequestConnection<'a> {
    connection: MutexGuard<'a, Connection>,
    budget: Budget,
}
impl<'a> RequestConnection<'a> {
    fn acquire(db: &'a Database, budget: &Budget) -> Result<Self> {
        let connection = loop {
            budget.check()?;
            match db.try_lock() {
                Ok(connection) => break connection,
                Err(TryLockError::WouldBlock) => budget.wait_retry()?,
                Err(TryLockError::Poisoned(_)) => return Err(DataError::StorageError),
            }
        };
        budget.check()?;
        if !connection.is_autocommit() {
            return Err(DataError::StorageError);
        }
        connection
            .busy_timeout(Duration::ZERO)
            .map_err(|_| DataError::StorageError)?;
        let progress_budget = budget.clone();
        connection.progress_handler(100, Some(move || progress_budget.check().is_err()));
        let commit_budget = budget.clone();
        connection.commit_hook(Some(move || commit_budget.check().is_err()));
        let rollback_budget = budget.clone();
        connection.rollback_hook(Some(move || rollback_budget.rolled_back()));
        Ok(Self {
            connection,
            budget: budget.clone(),
        })
    }
    fn sql<T>(&self, mut operation: impl FnMut() -> rusqlite::Result<T>) -> Result<T> {
        loop {
            self.budget.check()?;
            match operation() {
                Ok(value) => {
                    self.budget.check()?;
                    return Ok(value);
                }
                Err(error) if is_busy(&error) => self.budget.wait_retry()?,
                Err(_) => {
                    self.budget.check()?;
                    return Err(DataError::StorageError);
                }
            }
        }
    }
    fn commit(&self) -> Result<()> {
        loop {
            self.budget.check()?;
            match self.connection.execute_batch("COMMIT") {
                Ok(()) => return Ok(()),
                Err(error) if is_busy(&error) => self.budget.wait_retry()?,
                Err(_) => {
                    if self.budget.mutation_outcome() == MutationOutcomeState::Committing
                        && self.connection.is_autocommit()
                    {
                        self.budget.uncertain();
                        return Err(DataError::MutationOutcomeUnknown);
                    }
                    self.budget.check()?;
                    return Err(DataError::StorageError);
                }
            }
        }
    }
    fn rollback(&self) -> Result<()> {
        if !self.connection.is_autocommit() {
            // La limpieza debe poder revertir incluso cuando el presupuesto ya venció.
            self.connection.progress_handler(0, None::<fn() -> bool>);
            self.connection
                .execute_batch("ROLLBACK")
                .map_err(|_| DataError::StorageError)?;
        }
        Ok(())
    }
}
impl Drop for RequestConnection<'_> {
    fn drop(&mut self) {
        if self.rollback().is_err() {
            self.budget.uncertain();
        }
        self.connection.progress_handler(0, None::<fn() -> bool>);
        self.connection.commit_hook(None::<fn() -> bool>);
        self.connection.rollback_hook(None::<fn()>);
    }
}
fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
