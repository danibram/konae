//! Open connections to app Databases, kept between Server Actions.
//!
//! Opening a connection, installing its limits and authorizer, and closing
//! it again cost more than a small indexed query. In WAL mode the close of a
//! Database's last connection also checkpoints the log and removes it. So a
//! Server Action borrows a configured connection and gives it back.
//!
//! A kept connection reads the file it opened, even after another file is
//! renamed over its path. So a connection is lent only while its path still
//! names the file it opened, and `database_lock` (which a restore, a copy or
//! a migration holds) drops every kept connection of that Database.

use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Kept connections across every Database of this Kernel. Each holds three
/// file descriptors (the file, its WAL and its index) and up to SQLite's
/// default page cache of 2 MiB.
const MAX_KEPT: usize = 64;
/// Kept connections of one Database: as many as readers commonly overlap.
const MAX_KEPT_PER_DATABASE: usize = 8;
/// Prepared statements a connection keeps, one per distinct action text.
const STATEMENT_CACHE: usize = 64;

struct Kept {
    connection: Connection,
    file: FileId,
}

#[derive(Default)]
struct Pool {
    by_path: HashMap<PathBuf, Vec<Kept>>,
    /// Paths from least to most recently returned, for eviction.
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
pub(super) struct AppConnection {
    connection: Option<Connection>,
    path: PathBuf,
    file: Option<FileId>,
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
        give_back(&self.path, Kept { connection, file });
    }
}

/// Borrows a kept connection to the Database at `path`, or opens one. Each
/// borrow starts a fresh 2-second deadline.
pub(super) fn open(path: &Path) -> anyhow::Result<AppConnection> {
    let file = file_id(path);
    let kept = file.and_then(|file| take(path, file));
    let connection = match kept {
        Some(connection) => connection,
        None => {
            let connection = Connection::open(path)?;
            super::restrict_database(&connection)?;
            connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
            connection
        }
    };
    super::arm_deadline(&connection);
    // A mutation answers SQLite's last inserted rowid, which belongs to the
    // connection. A borrower starts at 0, as on a connection just opened, so
    // an UPDATE never answers the id an earlier INSERT made.
    // SAFETY: the handle is this live connection's, used on this thread only.
    unsafe { rusqlite::ffi::sqlite3_set_last_insert_rowid(connection.handle(), 0) };
    // The file is named again after the open, so a connection opened on a
    // path that was replaced meanwhile is used once and not kept.
    let file = file.filter(|file| file_id(path) == Some(*file));
    Ok(AppConnection { connection: Some(connection), path: path.to_owned(), file })
}

fn take(path: &Path, file: FileId) -> Option<Connection> {
    let mut pool = pool();
    let pool = &mut *pool;
    let kept = pool.by_path.get_mut(path)?;
    // A file renamed over this path since these were opened: none of them
    // reads it.
    if kept.iter().any(|kept| kept.file != file) {
        pool.total -= kept.len();
        pool.by_path.remove(path);
        pool.order.retain(|other| other != path);
        return None;
    }
    let connection = kept.pop()?.connection;
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
    // Closed outside the pool's lock: the last close of a Database
    // checkpoints its log.
    drop(evicted);
}

/// Closes every kept connection of the Database at `path`. The caller holds
/// that Database's lock alone, so no connection of it is lent out.
pub(super) fn forget(path: &Path) {
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
        let directory = std::env::temp_dir().join(format!("littlebox-app-db-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("database.sqlite")
    }

    fn count(path: &Path) -> i64 {
        open(path).unwrap().query_row("SELECT count(*) FROM t", [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_kept_connection_is_lent_again() {
        let path = scratch("lent");
        super::super::initialize_database(&path).unwrap();
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
        super::super::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(count(&path), 0);
        let replacement = path.with_extension("next.sqlite");
        Connection::open(&replacement).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1), (2)").unwrap();
        super::super::replace_database_file(&replacement, &path).unwrap();
        assert_eq!(count(&path), 2, "the kept connection to the old file is not lent");
    }

    #[test]
    fn a_borrower_does_not_see_the_last_insert_of_an_earlier_one() {
        let path = scratch("rowid");
        super::super::initialize_database(&path).unwrap();
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
        super::super::initialize_database(&path).unwrap();
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
        super::super::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(count(&path), 0);
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert_eq!(count(&path), 0, "a deadline set by the first borrow does not interrupt the second");
        forget(&path);
    }
}
