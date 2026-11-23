use crate::WorkerError;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum MemoryPolicy {
    DevelopmentPolling,
    LinuxCgroup { delegated_root: PathBuf },
}

pub(crate) struct Containment {
    path: Option<PathBuf>,
}
impl Containment {
    pub(crate) fn prepare(policy: MemoryPolicy, bytes: usize, generation: &str) -> Result<Self> {
        match policy {
            MemoryPolicy::DevelopmentPolling => Ok(Self { path: None }),
            MemoryPolicy::LinuxCgroup { delegated_root } => {
                Self::cgroup(delegated_root, bytes, generation)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    fn cgroup(_: PathBuf, _: usize, _: &str) -> Result<Self> {
        Err(WorkerError::new("containment_unavailable").into())
    }
    #[cfg(target_os = "linux")]
    fn cgroup(root: PathBuf, bytes: usize, generation: &str) -> Result<Self> {
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        let unavailable = || WorkerError::new("containment_unavailable");
        let root = root.canonicalize().map_err(|_| unavailable())?;
        let c_root =
            std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(|_| unavailable())?;
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        if unsafe { libc::statfs(c_root.as_ptr(), filesystem.as_mut_ptr()) } != 0
            || unsafe { filesystem.assume_init().f_type as u64 } != libc::CGROUP2_SUPER_MAGIC as u64
        {
            return Err(unavailable().into());
        }
        let enabled =
            fs::read_to_string(root.join("cgroup.subtree_control")).map_err(|_| unavailable())?;
        if !["memory", "pids"]
            .iter()
            .all(|name| enabled.split_whitespace().any(|word| word == *name))
        {
            return Err(unavailable().into());
        }
        reap_orphaned_workers(&root).map_err(|_| unavailable())?;
        // Solo se escribe el subgrupo nuevo; la delegación pertenece al operador.
        let supervisor = std::process::id();
        let started = process_start_time(supervisor)
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        let path = root.join(format!("konae-worker-{supervisor}-{started}-{generation}"));
        fs::create_dir(&path).map_err(|_| unavailable())?;
        let group = Self {
            path: Some(path.clone()),
        };
        for (name, value) in [
            ("memory.max", bytes.to_string()),
            ("memory.swap.max", "0".into()),
            ("pids.max", "64".into()),
            ("memory.oom.group", "1".into()),
        ] {
            fs::write(path.join(name), &value).map_err(|_| unavailable())?;
            let observed = fs::read_to_string(path.join(name)).map_err(|_| unavailable())?;
            if observed.trim() != value {
                return Err(unavailable().into());
            }
        }
        if !path.join("cgroup.kill").is_file() {
            return Err(unavailable().into());
        }
        Ok(group)
    }
    pub(crate) fn attach(&self, pid: u32) -> Result<()> {
        if let Some(path) = &self.path {
            let unavailable = || WorkerError::new("containment_unavailable");
            // The child sets oom_score_adj before dropping to its worker UID in pre_exec.
            // The non-root supervisor cannot reliably inspect the worker's proc entry after
            // that credential change, so containment setup does not probe it here.
            std::fs::write(path.join("cgroup.procs"), pid.to_string()).map_err(|error| {
                eprintln!("functions cgroup attach failed: {error}");
                unavailable()
            })?;
            let processes =
                std::fs::read_to_string(path.join("cgroup.procs")).map_err(|_| unavailable())?;
            if !processes.lines().any(|line| line == pid.to_string()) {
                return Err(unavailable().into());
            }
        }
        Ok(())
    }
    fn kill_path(path: Option<&Path>) -> bool {
        if let Some(path) = path {
            match std::fs::write(path.join("cgroup.kill"), "1") {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => {
                    eprintln!(
                        "functions cgroup kill failed for {}: {error}",
                        path.display()
                    );
                    true
                }
            }
        } else {
            true
        }
    }
    pub(crate) fn kill(&self) -> bool {
        Self::kill_path(self.path.as_deref())
    }
    pub(crate) fn kill_handle(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let path = self.path.clone();
        move || Self::kill_path(path.as_deref())
    }
    pub(crate) fn oom_kill_count(&self) -> Option<u64> {
        let path = self.path.as_ref()?.join("memory.events");
        Self::parse_oom_kill_count(&std::fs::read_to_string(path).ok()?)
    }
    fn parse_oom_kill_count(events: &str) -> Option<u64> {
        let mut found = false;
        let total = events
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let name = fields.next()?;
                let count = fields.next()?.parse::<u64>().ok()?;
                if name == "oom_kill" || name == "oom_group_kill" {
                    found = true;
                    Some(count)
                } else {
                    None
                }
            })
            .sum();
        found.then_some(total)
    }
    pub(crate) fn cleanup(&mut self) -> bool {
        let Some(path) = &self.path else {
            return true;
        };
        // Tras matar el grupo, la retirada de tareas del cgroup puede tardar un intervalo.
        for _ in 0..20 {
            match std::fs::remove_dir(path) {
                Ok(()) => {
                    self.path = None;
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.path = None;
                    return true;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        false
    }
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> std::io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc stat");
    let fields = stat
        .rsplit_once(") ")
        .ok_or_else(invalid)?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields
        .first()
        .is_some_and(|state| *state == "Z" || *state == "X")
    {
        return Ok(None);
    }
    let started = fields
        .get(19)
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    Ok(Some(started))
}

#[cfg(target_os = "linux")]
fn reap_orphaned_workers(root: &std::path::Path) -> std::io::Result<()> {
    let entries = std::fs::read_dir(root)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(parts) = name.strip_prefix("konae-worker-") else {
            continue;
        };
        let mut parts = parts.splitn(3, '-');
        let (Some(pid), Some(started), Some(_nonce)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(started)) = (pid.parse::<u32>(), started.parse::<u64>()) else {
            continue;
        };
        match process_start_time(pid)? {
            Some(current) if current == started => {
                continue;
            }
            Some(_) | None => {}
        }
        // Names under this exact prefix are created only by this executor.
        match std::fs::write(path.join("cgroup.kill"), "1") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
        let mut removed = false;
        for _ in 0..200 {
            match std::fs::remove_dir(&path) {
                Ok(()) => {
                    eprintln!("orphan_group=removed supervisor_pid={pid}");
                    removed = true;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    removed = true;
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        if !removed {
            return Err(std::io::Error::other(
                "orphan worker cgroup is still populated",
            ));
        }
    }
    Ok(())
}
impl Drop for Containment {
    fn drop(&mut self) {
        self.kill();
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_policy_refuses_missing_delegation_without_creating_it() {
        let root =
            std::env::temp_dir().join(format!("konae-missing-delegation-{}", std::process::id()));
        assert!(!root.exists());
        let result = Containment::prepare(
            MemoryPolicy::LinuxCgroup {
                delegated_root: root.clone(),
            },
            1024 * 1024,
            "test",
        );
        assert!(result.is_err());
        assert!(!root.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ordinary_files_cannot_impersonate_cgroup_controls() {
        let root =
            std::env::temp_dir().join(format!("konae-fake-delegation-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("cgroup.subtree_control"), "memory pids").unwrap();
        let result = Containment::prepare(
            MemoryPolicy::LinuxCgroup {
                delegated_root: root.clone(),
            },
            1024 * 1024,
            "test",
        );
        assert!(result.is_err());
        assert!(!root.join("konae-worker-test").exists());
        std::fs::remove_file(root.join("cgroup.subtree_control")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn oom_counter_uses_only_kernel_oom_events() {
        assert_eq!(
            Containment::parse_oom_kill_count("low 0\noom 3\noom_kill 2\n"),
            Some(2)
        );
        assert_eq!(
            Containment::parse_oom_kill_count("oom_group_kill 1\noom_kill 2\n"),
            Some(3)
        );
        assert_eq!(
            Containment::parse_oom_kill_count("low 8\nhigh 0\nmax 4\n"),
            None
        );
    }

    #[test]
    fn dropping_an_abort_handle_does_not_clean_up_the_worker_cgroup() {
        let path =
            std::env::temp_dir().join(format!("konae-cgroup-handle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("cgroup.kill"), "0").unwrap();
        let containment = Containment {
            path: Some(path.clone()),
        };

        {
            let kill = containment.kill_handle();
            assert!(kill());
        }

        assert!(path.exists());
        std::fs::remove_file(path.join("cgroup.kill")).unwrap();
        drop(containment);
        assert!(!path.exists());
    }
}
