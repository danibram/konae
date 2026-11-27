
use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};

pub(crate) fn read_optional(path: &Path) -> anyhow::Result<Option<Connection>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(connection))
}

const MAX_KEPT: usize = 64;
const MAX_KEPT_PER_DATABASE: usize = 8;
const STATEMENT_CACHE: usize = 64;

struct Kept {
    connection: Connection,
    file: FileId,
    kernel_effects: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct Pool {
    by_path: HashMap<PathBuf, Vec<Kept>>,
    order: VecDeque<PathBuf>,
    total: usize,
}

static POOL: LazyLock<Mutex<Pool>> = LazyLock::new(Mutex::default);

fn pool() -> std::sync::MutexGuard<'static, Pool> {
    POOL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = ();

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|metadata| (metadata.dev(), metadata.ino()))
}

/// Without inode numbers there is no way to tell a replaced file, so
/// nothing is kept.
#[cfg(not(unix))]
fn file_id(_path: &Path) -> Option<FileId> {
    None
}

/// A connection to an app Database, configured by `restrict_database`.
/// Dropping it gives it back for the next Server Action.
pub struct AppConnection {
    connection: Option<Connection>,
    path: PathBuf,
    file: Option<FileId>,
    kernel_effects: Arc<std::sync::atomic::AtomicBool>,
}

impl AppConnection {
    pub(crate) fn kernel_effect_token(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.kernel_effects.clone()
    }
}

pub(crate) fn with_kernel_effect_access<T>(token: &Arc<std::sync::atomic::AtomicBool>, work: impl FnOnce() -> T) -> T {
    token.store(true, std::sync::atomic::Ordering::SeqCst);
    struct Reset(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Reset {
        fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::SeqCst); }
    }
    let _reset = Reset(token.clone());
    work()
}

impl std::ops::Deref for AppConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("held until drop")
    }
}

impl std::ops::DerefMut for AppConnection {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("held until drop")
    }
}

impl Drop for AppConnection {
    fn drop(&mut self) {
        let (Some(connection), Some(file)) = (self.connection.take(), self.file) else { return };
        // A transaction left open by a panic is not lent to the next caller.
        if !connection.is_autocommit() {
            return;
        }
        give_back(&self.path, Kept { connection, file, kernel_effects: self.kernel_effects.clone() });
    }
}

/// Borrows a kept connection to the Database at `path`, or opens one. Each
/// borrow starts a fresh 2-second deadline.
pub fn open(path: &Path) -> anyhow::Result<AppConnection> {
    let borrowed = kept_or_open(path, || {
        let connection = Connection::open(path)?;
        let kernel_effects = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::app_data::restrict_app_database(&connection, kernel_effects.clone())?;
        connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        Ok::<_, anyhow::Error>(Kept { connection, file: file_id(path).unwrap_or_default(), kernel_effects })
    })?;
    crate::app_data::arm_deadline(&borrowed.connection)?;
    // Each mutation starts with a fresh last inserted rowid.
    unsafe { rusqlite::ffi::sqlite3_set_last_insert_rowid(borrowed.connection.handle(), 0) };
    Ok(AppConnection { connection: Some(borrowed.connection), path: path.to_owned(), file: file_id(path), kernel_effects: borrowed.kernel_effects })
}

/// Borrows a read-only connection while the caller holds the app's identity lock.
pub(crate) fn identity_reader(path: &Path) -> rusqlite::Result<AppConnection> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(AppConnection { connection: Some(connection), path: path.to_owned(), file: None, kernel_effects: Arc::new(std::sync::atomic::AtomicBool::new(false)) })
}

fn kept_or_open<E>(
    path: &Path,
    open: impl FnOnce() -> Result<Kept, E>,
) -> Result<Kept, E> {
    let file = file_id(path);
    let mut kept = match file.and_then(|file| take(path, file)) {
        Some(kept) => kept,
        None => open()?,
    };
    if let Some(file) = file.filter(|file| file_id(path) == Some(*file)) { kept.file = file; }
    Ok(kept)
}

fn take(path: &Path, file: FileId) -> Option<Kept> {
    let mut pool = pool();
    let pool = &mut *pool;
    let kept = pool.by_path.get_mut(path)?;
    if kept.iter().any(|kept| kept.file != file) {
        pool.total -= kept.len();
        pool.by_path.remove(path);
        pool.order.retain(|other| other != path);
        return None;
    }
    let connection = kept.pop()?;
    pool.total -= 1;
    Some(connection)
}

fn give_back(path: &Path, connection: Kept) {
    let mut evicted = Vec::new();
    {
        let mut pool = pool();
        let pool = &mut *pool;
        let kept = pool.by_path.entry(path.to_owned()).or_default();
        if kept.len() >= MAX_KEPT_PER_DATABASE {
            evicted.push(connection);
        } else {
            kept.push(connection);
            pool.total += 1;
        }
        pool.order.retain(|other| other != path);
        pool.order.push_back(path.to_owned());
        while pool.total > MAX_KEPT {
            let Some(oldest) = pool.order.pop_front() else { break };
            if let Some(dropped) = pool.by_path.remove(&oldest) {
                pool.total -= dropped.len();
                evicted.extend(dropped);
            }
        }
    }
    drop(evicted);
}

/// Closes every kept connection of the Database at `path`. The caller holds
/// that Database's lock alone, so no connection of it is lent out.
pub fn forget(path: &Path) {
    let dropped = {
        let mut pool = pool();
        pool.order.retain(|other| other != path);
        let dropped = pool.by_path.remove(path);
        pool.total -= dropped.as_ref().map_or(0, Vec::len);
        dropped
    };
    drop(dropped);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("konae-app-db-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("database.sqlite")
    }

    fn count(path: &Path) -> i64 {
        open(path).unwrap().query_row("SELECT count(*) FROM t", [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_kept_connection_is_lent_again() {
        let path = scratch("lent");
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(count(&path), 0);
        assert_eq!(pool().by_path.get(&path).map(Vec::len), Some(1));
        assert_eq!(count(&path), 0);
        assert_eq!(pool().by_path.get(&path).map(Vec::len), Some(1), "the same connection came back");
        forget(&path);
        assert!(!pool().by_path.contains_key(&path));
    }

    #[test]
    fn a_file_renamed_over_the_path_is_read_and_not_the_old_one() {
        let path = scratch("renamed");
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(count(&path), 0);
        let replacement = path.with_extension("next.sqlite");
        Connection::open(&replacement).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1), (2)").unwrap();
        crate::replace_database_file(&replacement, &path).unwrap();
        assert_eq!(count(&path), 2, "the kept connection to the old file is not lent");
    }

    #[test]
    fn a_borrower_does_not_see_the_last_insert_of_an_earlier_one() {
        let path = scratch("rowid");
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        open(&path).unwrap().execute("INSERT INTO t VALUES (1)", []).unwrap();
        let connection = open(&path).unwrap();
        connection.execute("UPDATE t SET x = 2", []).unwrap();
        assert_eq!(connection.last_insert_rowid(), 0);
        drop(connection);
        forget(&path);
    }

    #[test]
    fn a_closed_wal_database_opens_read_only() {
        let path = scratch("read-only");
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1)").unwrap();
        assert_eq!(count(&path), 1);
        forget(&path);
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        assert!(!Path::new(&wal).exists(), "the last close removes the log");
        let reader = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(reader.query_row("SELECT count(*) FROM t", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn a_kept_connection_starts_a_new_deadline() {
        let path = scratch("deadline");
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(count(&path), 0);
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert_eq!(count(&path), 0, "a deadline set by the first borrow does not interrupt the second");
        forget(&path);
    }
}
