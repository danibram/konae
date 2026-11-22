use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::{AppState, app_db};

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
