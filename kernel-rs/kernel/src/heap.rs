//! The ceiling on the memory SQLite may hold across every app.
//!
//! One Kernel process serves every app, and a row of `randomblob` values is
//! built inside SQLite before the Kernel reads a column of it. A hard heap
//! limit makes a statement that grows past the ceiling fail with SQLite's
//! out-of-memory error, where the operating system would kill the process.

use anyhow::{Result, bail};

/// The ceiling when the operating system reports no memory size.
const FALLBACK_BYTES: u64 = 256 * 1024 * 1024;

// The bundled SQLite compiles this function in, but the bindings `rusqlite`
// ships do not declare it.
unsafe extern "C" {
    fn sqlite3_hard_heap_limit64(n: i64) -> i64;
}

/// Half of the available memory, including a container's limit on Linux.
/// Use 256 MiB only when the operating system reports no size.
pub fn default_bytes(available_bytes: Option<u64>) -> u64 {
    available_bytes
        .map(|bytes| (bytes / 2).max(1))
        .unwrap_or(FALLBACK_BYTES)
}

#[cfg(unix)]
fn physical_memory() -> Option<u64> {
    // SAFETY: `sysconf` reads a system constant and touches no memory of ours.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    (pages > 0 && size > 0)
        .then(|| (pages as u64).checked_mul(size as u64))
        .flatten()
}

#[cfg(not(unix))]
fn physical_memory() -> Option<u64> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn cgroup_memory_limit(
    membership: &str,
    mounts: &str,
    read: impl Fn(&std::path::Path) -> Option<String>,
) -> Option<u64> {
    use std::path::{Path, PathBuf};

    let unescape = |value: &str| {
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    };
    let mut limit = None;
    for mount in mounts.lines() {
        let fields: Vec<_> = mount.split_whitespace().collect();
        let Some(separator) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        if separator < 6 || fields.len() <= separator + 3 {
            continue;
        }
        let unified = fields[separator + 1] == "cgroup2";
        let memory_v1 = fields[separator + 1] == "cgroup"
            && fields[separator + 3]
                .split(',')
                .any(|value| value == "memory");
        if !unified && !memory_v1 {
            continue;
        }
        let root = PathBuf::from(unescape(fields[3]));
        let mounted = PathBuf::from(unescape(fields[4]));
        let member = membership.lines().find_map(|line| {
            let mut parts = line.splitn(3, ':');
            parts.next()?;
            let controllers = parts.next()?;
            let path = parts.next()?;
            ((unified && controllers.is_empty())
                || (memory_v1 && controllers.split(',').any(|value| value == "memory")))
            .then_some(path)
        });
        let Some(member) = member else { continue };
        let relative = Path::new(member)
            .strip_prefix(&root)
            .unwrap_or(Path::new(""));
        // A cgroup namespace can show a membership path outside its visible
        // mount. The mount root still holds its effective limit.
        let mut directory = if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            mounted.clone()
        } else {
            mounted.join(relative)
        };
        let filename = if unified {
            "memory.max"
        } else {
            "memory.limit_in_bytes"
        };
        loop {
            if let Some(bytes) = read(&directory.join(filename))
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|bytes| *bytes > 0)
            {
                limit = Some(limit.map_or(bytes, |current: u64| current.min(bytes)));
            }
            if directory == mounted || !directory.pop() {
                break;
            }
        }
    }
    limit
}

fn available_memory() -> Option<u64> {
    let physical = physical_memory();
    #[cfg(target_os = "linux")]
    {
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .zip(std::fs::read_to_string("/proc/self/mountinfo").ok())
            .and_then(|(membership, mounts)| {
                cgroup_memory_limit(&membership, &mounts, |path| {
                    std::fs::read_to_string(path).ok()
                })
            });
        return match (physical, cgroup) {
            (Some(physical), Some(cgroup)) => Some(physical.min(cgroup)),
            (physical, cgroup) => physical.or(cgroup),
        };
    }
    #[cfg(not(target_os = "linux"))]
    physical
}

/// The ceiling `KONAE_SQLITE_HEAP_BYTES` names, or the default when it is
/// unset or empty.
pub fn bytes_from(setting: Option<&str>, available_bytes: Option<u64>) -> Result<u64> {
    match setting.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(default_bytes(available_bytes)),
        Some(value) => match value.parse::<u64>() {
            Ok(bytes) if bytes > 0 && bytes <= i64::MAX as u64 => Ok(bytes),
            _ => bail!(
                "KONAE_SQLITE_HEAP_BYTES must be a whole number of bytes above 0, not {value:?}."
            ),
        },
    }
}

/// Sets the ceiling for this process. Call it before any connection opens.
pub fn limit_from_env() -> Result<u64> {
    let bytes = bytes_from(
        std::env::var("KONAE_SQLITE_HEAP_BYTES").ok().as_deref(),
        available_memory(),
    )?;
    // SAFETY: the function takes a number and keeps no pointer.
    unsafe { sqlite3_hard_heap_limit64(bytes as i64) };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_uses_half_the_available_memory() {
        assert_eq!(
            default_bytes(Some(64 * 1024 * 1024 * 1024)),
            32 * 1024 * 1024 * 1024
        );
        assert_eq!(default_bytes(Some(300 * 1024 * 1024)), 150 * 1024 * 1024);
        assert_eq!(default_bytes(None), FALLBACK_BYTES);
        assert_eq!(default_bytes(Some(128 * 1024 * 1024)), 64 * 1024 * 1024);
    }

    #[test]
    fn the_setting_overrides_the_default_and_refuses_nonsense() {
        assert_eq!(bytes_from(None, Some(8 << 30)).unwrap(), 4 << 30);
        assert_eq!(bytes_from(Some("  "), None).unwrap(), FALLBACK_BYTES);
        assert_eq!(bytes_from(Some("1048576"), Some(8 << 30)).unwrap(), 1 << 20);
        for bad in ["0", "-1", "lots", "1.5", "99999999999999999999"] {
            let message = bytes_from(Some(bad), None).unwrap_err().to_string();
            assert!(
                message.contains("KONAE_SQLITE_HEAP_BYTES"),
                "{bad}: {message}"
            );
        }
    }

    #[test]
    fn container_limits_include_parents_and_namespaced_mounts() {
        use std::collections::HashMap;
        let files = HashMap::from([
            ("/cg/memory.max", "1073741824"),
            ("/cg/team/memory.max", "268435456"),
            ("/cg/team/app/memory.max", "max"),
            ("/v1/memory.limit_in_bytes", "9223372036854771712"),
            ("/v1/app/memory.limit_in_bytes", "134217728"),
        ]);
        let read =
            |path: &std::path::Path| files.get(path.to_str()?).map(|value| (*value).to_owned());
        let unified = "29 23 0:26 / /cg rw - cgroup2 cgroup rw";
        assert_eq!(
            cgroup_memory_limit("0::/team/app", unified, read),
            Some(256 << 20)
        );
        assert_eq!(cgroup_memory_limit("0::/", unified, read), Some(1 << 30));
        let namespaced = "29 23 0:26 /docker/id /cg rw - cgroup2 cgroup rw";
        assert_eq!(cgroup_memory_limit("0::/", namespaced, read), Some(1 << 30));
        let v1 = "29 23 0:26 / /v1 rw - cgroup cgroup rw,memory";
        assert_eq!(
            cgroup_memory_limit("7:cpu,memory:/app", v1, read),
            Some(128 << 20)
        );
        assert_eq!(cgroup_memory_limit("7:cpu:/app", v1, read), None);
        assert_eq!(cgroup_memory_limit("0::/team/app", unified, |_| None), None);
        assert_eq!(cgroup_memory_limit("invalid", "invalid", read), None);
        assert_eq!(default_bytes(Some(128 << 20)), 64 << 20);
    }

    #[test]
    fn a_statement_past_the_ceiling_fails_with_out_of_memory() {
        // The ceiling is the whole process's, so this runs in a child process
        // and leaves the other tests' limit alone.
        if std::env::var_os("KONAE_HEAP_TEST_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "heap::tests::a_statement_past_the_ceiling_fails_with_out_of_memory",
                    "--nocapture",
                ])
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
        assert!(
            error.to_string().to_lowercase().contains("out of memory"),
            "{error}"
        );
        let small: i64 = db
            .query_row("SELECT length(randomblob(1000))", [], |row| row.get(0))
            .unwrap();
        assert_eq!(small, 1000);
    }
}
