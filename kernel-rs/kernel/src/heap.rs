//! The ceiling on the memory SQLite may hold across every app.
//!
//! One Kernel process serves every app, and a row of `randomblob` values is
//! built inside SQLite before the Kernel reads a column of it. A hard heap
//! limit makes a statement that grows past the ceiling fail with SQLite's
//! out-of-memory error, where the operating system would kill the process.

use anyhow::{Result, bail};

/// The lowest ceiling the Kernel picks for itself.
pub const FLOOR_BYTES: u64 = 256 * 1024 * 1024;

// The bundled SQLite compiles this function in, but the bindings `rusqlite`
// ships do not declare it.
unsafe extern "C" {
    fn sqlite3_hard_heap_limit64(n: i64) -> i64;
}

/// Half of `physical_bytes`, and never below the floor. `None` means the
/// machine would not say how much memory it has.
pub fn default_bytes(physical_bytes: Option<u64>) -> u64 {
    (physical_bytes.unwrap_or(0) / 2).max(FLOOR_BYTES)
}

#[cfg(unix)]
fn physical_memory() -> Option<u64> {
    // SAFETY: `sysconf` reads a system constant and touches no memory of ours.
    let (pages, size) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
    (pages > 0 && size > 0).then(|| pages as u64 * size as u64)
}

#[cfg(not(unix))]
fn physical_memory() -> Option<u64> {
    None
}

/// The ceiling `KONAE_SQLITE_HEAP_BYTES` names, or the default when it is
/// unset or empty.
pub fn bytes_from(setting: Option<&str>, physical_bytes: Option<u64>) -> Result<u64> {
    match setting.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(default_bytes(physical_bytes)),
        Some(value) => match value.parse::<u64>() {
            Ok(bytes) if bytes > 0 && bytes <= i64::MAX as u64 => Ok(bytes),
            _ => bail!("KONAE_SQLITE_HEAP_BYTES must be a whole number of bytes above 0, not {value:?}."),
        },
    }
}

/// Sets the ceiling for this process. Call it before any connection opens.
pub fn limit_from_env() -> Result<u64> {
    let bytes = bytes_from(std::env::var("KONAE_SQLITE_HEAP_BYTES").ok().as_deref(), physical_memory())?;
    // SAFETY: the function takes a number and keeps no pointer.
    unsafe { sqlite3_hard_heap_limit64(bytes as i64) };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_half_the_memory_and_never_below_the_floor() {
        assert_eq!(default_bytes(Some(64 * 1024 * 1024 * 1024)), 32 * 1024 * 1024 * 1024);
        assert_eq!(default_bytes(Some(300 * 1024 * 1024)), FLOOR_BYTES);
        assert_eq!(default_bytes(None), FLOOR_BYTES);
        assert!(default_bytes(physical_memory()) >= FLOOR_BYTES);
    }

    #[test]
    fn the_setting_overrides_the_default_and_refuses_nonsense() {
        assert_eq!(bytes_from(None, Some(8 << 30)).unwrap(), 4 << 30);
        assert_eq!(bytes_from(Some("  "), None).unwrap(), FLOOR_BYTES);
        assert_eq!(bytes_from(Some("1048576"), Some(8 << 30)).unwrap(), 1 << 20);
        for bad in ["0", "-1", "lots", "1.5", "99999999999999999999"] {
            let message = bytes_from(Some(bad), None).unwrap_err().to_string();
            assert!(message.contains("KONAE_SQLITE_HEAP_BYTES"), "{bad}: {message}");
        }
    }

    #[test]
    fn a_statement_past_the_ceiling_fails_with_out_of_memory() {
        // The ceiling is the whole process's, so this runs in a child process
        // and leaves the other tests' limit alone.
        if std::env::var_os("KONAE_HEAP_TEST_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "heap::tests::a_statement_past_the_ceiling_fails_with_out_of_memory", "--nocapture"])
                .env("KONAE_HEAP_TEST_CHILD", "1")
                .env("KONAE_SQLITE_HEAP_BYTES", (64 * 1024 * 1024).to_string())
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        assert_eq!(limit_from_env().unwrap(), 64 * 1024 * 1024);
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let error = db
            .query_row("SELECT length(randomblob(33554432) || randomblob(33554432) || randomblob(33554432))", [], |row| row.get::<_, i64>(0))
            .unwrap_err();
        assert!(error.to_string().to_lowercase().contains("out of memory"), "{error}");
        let small: i64 = db.query_row("SELECT length(randomblob(1000))", [], |row| row.get(0)).unwrap();
        assert_eq!(small, 1000);
    }
}
