mod locks;
mod admin;
mod backups;
pub mod connection;
pub mod database;
pub mod schema_change;
pub mod schema_transform;

pub use connection::{AppConnection, forget, open};
pub(crate) use connection::with_kernel_effect_access;
pub use database::{
    DENIED_FUNCTIONS, MAX_APP_DATABASE_BYTES, MAX_LIKE_PATTERN_BYTES, arm_deadline,
    denied_function, initialize_database, limit_database_size, replace_database_file,
    restrict_database, use_wal,
};
pub(crate) use database::restrict_app_database;
pub use backups::{DatabaseBackup, backup_dir, list_backups, create_backup, prepare_backup, restore_backup};
pub use admin::{admin_database, inspect_database, table_columns, table_exists, quote_identifier, browse_table, run_admin_sql, admin_update_cell, admin_insert_row, admin_delete_row, value_to_sql};
pub use locks::{database_path, LockMap, Held, NamedLockGuard, named_lock, database_lock};
