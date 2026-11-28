use std::{path::Path, sync::Arc};

use anyhow::Result;
use rusqlite::{Connection, hooks::{AuthAction, AuthContext, Authorization}};

use crate::app_data::schema_change;

/// The longest LIKE or GLOB pattern an action may compare.
pub const MAX_LIKE_PATTERN_BYTES: i32 = 500;

/// Each app database may grow to this size. Writes past it fail with
/// "database or disk is full".
pub const MAX_APP_DATABASE_BYTES: u64 = 256 * 1024 * 1024;

/// Caps the database file behind this connection. SQLite keeps the setting per
/// connection, so every connection that may write needs it.
pub fn limit_database_size(db: &Connection, max_bytes: u64) -> rusqlite::Result<()> {
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let pages = (max_bytes / page_size.max(1)).max(1);
    db.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))?;
    Ok(())
}

/// SQL functions no app statement or schema may call. They reach the file
/// system, load native code, or swap full-text tokenizers by pointer.
pub const DENIED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "edit",
    "fts3_tokenizer",
];

pub fn denied_function(name: &str) -> bool {
    DENIED_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str())
}

pub fn restrict_database(db: &Connection) -> Result<()> {
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    configure_database_limits(db)?;
    arm_deadline(db)?;
    install_database_authorizer(db, None, None)?;
    Ok(())
}

pub(crate) fn restrict_app_database(db: &Connection, kernel_effects: Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    configure_database_limits(db)?;
    arm_deadline(db)?;
    install_database_authorizer(db, None, Some(kernel_effects))?;
    Ok(())
}

pub(crate) fn restrict_function_database(db: &Connection, receipts: Arc<std::sync::atomic::AtomicBool>) -> rusqlite::Result<()> {
    db.busy_timeout(std::time::Duration::ZERO)?;
    db.pragma_update(None, "foreign_keys", true)?;
    configure_database_limits(db)?;
    install_database_authorizer(db, Some(receipts), None)
}

fn configure_database_limits(db: &Connection) -> rusqlite::Result<()> {
    db.pragma_update(None, "synchronous", "NORMAL")?;
    use_wal(db);

    limit_database_size(db, MAX_APP_DATABASE_BYTES)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, 100_000)?;
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)?;



    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, MAX_LIKE_PATTERN_BYTES)?;
    Ok(())
}

fn install_database_authorizer(db: &Connection, receipts: Option<Arc<std::sync::atomic::AtomicBool>>, kernel_effects: Option<Arc<std::sync::atomic::AtomicBool>>) -> rusqlite::Result<()> {
    let function_connection = receipts.is_some();
    let reserved = move |name: &str, accessor: Option<&str>| {
        schema_change::is_internal_table(name)
            && !(name.eq_ignore_ascii_case("__konae_function_receipts")
                && accessor.is_none()
                && receipts.as_ref().is_some_and(|allowed| allowed.load(std::sync::atomic::Ordering::SeqCst)))
            && !(accessor.is_none()
                && kernel_effects.as_ref().is_some_and(|allowed| allowed.load(std::sync::atomic::Ordering::SeqCst)))
    };
    db.authorizer(Some(move |context: AuthContext<'_>| {
        if function_connection && context.accessor.is_some() { return Authorization::Deny; }
        match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        AuthAction::CreateTable { table_name, .. } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::CreateIndex { index_name, table_name } if reserved(index_name, context.accessor) || reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::AlterTable { table_name, .. } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::DropTable { table_name } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::DropIndex { index_name, table_name } if reserved(index_name, context.accessor) || reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::Read { table_name, .. } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::Insert { table_name } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::Update { table_name, .. } if reserved(table_name, context.accessor) => Authorization::Deny,
        AuthAction::Delete { table_name } if reserved(table_name, context.accessor) => Authorization::Deny,
        // Konae runs no triggers or views, and Studio and the schema change
        // use this connection too, so neither can add one the Gate never saw.
        AuthAction::CreateTrigger { .. } | AuthAction::CreateTempTrigger { .. } => Authorization::Deny,
        AuthAction::CreateView { .. } | AuthAction::CreateTempView { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, .. } if !["table_info", "table_xinfo", "index_list", "index_info", "foreign_key_list", "foreign_key_check", "integrity_check", "quick_check"].contains(&pragma_name.to_ascii_lowercase().as_str()) => Authorization::Deny,
        AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
        _ => Authorization::Allow,
        }
    }))?;
    Ok(())
}

/// Interrupts any statement on `db` that runs past 2 seconds from now.
pub fn arm_deadline(db: &Connection) -> rusqlite::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    db.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline))
}

pub fn initialize_database(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(())
}

/// Moves a Database written before WAL mode, or arrived from a snapshot or a
/// backup, into it. The mode is stored in the file, so this runs once. A
/// switch that meets another connection is left for the next one: the
/// Database still answers in its old mode meanwhile.
pub fn use_wal(db: &Connection) {
    let mode = db.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
    if !mode.is_ok_and(|mode| mode.eq_ignore_ascii_case("wal")) {
        let _ = db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
    }
}

/// Puts `replacement` where the Database at `path` was. The old file's WAL
/// and shared-memory index go first: left beside the new file, SQLite would
/// read the old file's pages into it. The caller holds the Database's lock
/// alone, so no connection has them open.
pub fn replace_database_file(replacement: &Path, path: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match std::fs::remove_file(&sidecar) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    std::fs::rename(replacement, path)
}
