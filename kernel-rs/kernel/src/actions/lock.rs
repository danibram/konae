use crate::{AppState, NamedLockGuard, named_lock};

/// What one Server Action holds on its Database while it runs. Queries share
/// the Database's lock, so they run side by side and beside a write: the
/// Database is in WAL mode, where a reader sees the last commit and never
/// waits for the writer. Mutations also take the Database's write lock, so
/// they reach SQLite one at a time instead of meeting `SQLITE_BUSY`.
pub struct ActionLock {
    pub _write: Option<NamedLockGuard>,
    pub _database: NamedLockGuard,
}

pub async fn action_lock(state: &AppState, id: &str, kind: &str) -> ActionLock {
    let database = named_lock(state, id, false).await;
    let write = if kind == "query" { None } else { Some(named_lock(state, &format!("write:{id}"), true).await) };
    ActionLock { _write: write, _database: database }
}
