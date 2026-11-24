#[cfg(target_os = "linux")]
mod linux {
    use crate::WorkerError;
    use anyhow::Result;
    use std::{io, mem, os::fd::RawFd};

    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_ACCESS_FS_ALL: u64 = 0x1_ffff;
    const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
    const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;
    const LANDLOCK_ACCESS_FS_IOCTL_DEV: u64 = 1 << 15;
    const LANDLOCK_ACCESS_FS_RESOLVE_UNIX: u64 = 1 << 16;
    // ABI 2 is required so the empty ruleset can deny REFER operations too.
    const LANDLOCK_ABI_REQUIRED: libc::c_long = 2;
    const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
    const PR_SET_SECCOMP: libc::c_int = 22;
    const CAP_VERSION_3: u32 = 0x2008_0522;
    const SECCOMP_MODE_FILTER: libc::c_ulong = 2;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const ENOSYS: u32 = libc::ENOSYS as u32;
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_JMP_JSET_K: u16 = 0x45;
    #[cfg(target_arch = "x86_64")]
    const BPF_JMP_JGE_K: u16 = 0x35;
    const BPF_RET_K: u16 = 0x06;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C)]
    struct CapHeader {
        version: u32,
        pid: i32,
    }

    #[repr(C)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    fn fail() -> anyhow::Error {
        WorkerError::new("sandbox_unavailable").into()
    }

    fn landlock_handled_access_fs(version: libc::c_long) -> u64 {
        let mut handled = LANDLOCK_ACCESS_FS_ALL;
        if version < 2 {
            handled &= !LANDLOCK_ACCESS_FS_REFER;
        }
        if version < 3 {
            handled &= !LANDLOCK_ACCESS_FS_TRUNCATE;
        }
        if version < 5 {
            handled &= !LANDLOCK_ACCESS_FS_IOCTL_DEV;
        }
        if version < 9 {
            handled &= !LANDLOCK_ACCESS_FS_RESOLVE_UNIX;
        }
        handled
    }

    fn close_inherited_descriptors() -> Result<()> {
        let result = unsafe { libc::syscall(436, 3u32, u32::MAX, 0u32) };
        if result == 0 {
            return Ok(());
        }
        let mut limit = mem::MaybeUninit::<libc::rlimit>::zeroed();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return Err(fail());
        }
        let limit = unsafe { limit.assume_init().rlim_cur.min(1_048_576) };
        for fd in 3..limit {
            unsafe { libc::close(fd as RawFd) };
        }
        Ok(())
    }

    fn restrict_filesystem() -> Result<()> {
        let version = unsafe {
            libc::syscall(
                444,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if version < LANDLOCK_ABI_REQUIRED {
            return Err(fail());
        }
        // ABI 2 adds REFER, and ABI 9 adds RESOLVE_UNIX.
        let attr = RulesetAttr {
            handled_access_fs: landlock_handled_access_fs(version),
        };
        let fd = unsafe { libc::syscall(444, &attr, mem::size_of::<RulesetAttr>(), 0u32) };
        if fd < 0 {
            return Err(fail());
        }
        if unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
            || unsafe { libc::syscall(446, fd as RawFd, 0u32) } != 0
        {
            unsafe { libc::close(fd as RawFd) };
            return Err(fail());
        }
        unsafe { libc::close(fd as RawFd) };
        Ok(())
    }

    fn clear_capabilities() -> io::Result<()> {
        let header = CapHeader {
            version: CAP_VERSION_3,
            pid: 0,
        };
        let data = [
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        if unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn drop_privileges() -> io::Result<()> {
        clear_capabilities()?;
        if unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn stmt(code: u16, k: u32) -> SockFilter {
        SockFilter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }

    fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
        SockFilter { code, jt, jf, k }
    }

    fn deny_syscall(filter: &mut Vec<SockFilter>, syscall: libc::c_long) {
        filter.push(jump(BPF_JMP_JEQ_K, syscall as u32, 0, 1));
        filter.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32));
    }

    fn seccomp_arch_filter() -> Vec<SockFilter> {
        #[cfg(target_arch = "aarch64")]
        let audit_arch = 0xc000_00b7;
        #[cfg(target_arch = "x86_64")]
        let audit_arch = 0xc000_003e;

        vec![
            stmt(BPF_LD_W_ABS, 4),
            jump(BPF_JMP_JEQ_K, audit_arch, 1, 0),
            stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
            stmt(BPF_LD_W_ABS, 0),
            #[cfg(target_arch = "x86_64")]
            jump(BPF_JMP_JGE_K, 0x4000_0000, 0, 1),
            #[cfg(target_arch = "x86_64")]
            stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        ]
    }

    fn deny_network_syscalls(filter: &mut Vec<SockFilter>) {
        for syscall in [
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
            libc::SYS_sendto,
            libc::SYS_recvfrom,
            libc::SYS_sendmsg,
            libc::SYS_recvmsg,
            libc::SYS_shutdown,
            libc::SYS_getsockname,
            libc::SYS_getpeername,
            libc::SYS_setsockopt,
            libc::SYS_getsockopt,
        ] {
            deny_syscall(filter, syscall);
        }
        filter.extend([
            jump(BPF_JMP_JEQ_K, libc::SYS_socketpair as u32, 0, 3),
            stmt(BPF_LD_W_ABS, 16),
            jump(BPF_JMP_JEQ_K, libc::AF_UNIX as u32, 1, 0),
            stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            stmt(BPF_LD_W_ABS, 0),
        ]);
        for syscall in [
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            deny_syscall(filter, syscall);
        }
    }

    fn seccomp_filter() -> Vec<SockFilter> {
        let mut filter = seccomp_arch_filter();
        filter.extend([
            jump(BPF_JMP_JEQ_K, libc::SYS_prlimit64 as u32, 0, 4),
            stmt(BPF_LD_W_ABS, 16),
            jump(BPF_JMP_JEQ_K, 0, 1, 0),
            stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            stmt(BPF_LD_W_ABS, 0),
        ]);
        deny_network_syscalls(&mut filter);
        for syscall in [
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_chroot,
            libc::SYS_setns,
            libc::SYS_unshare,
            libc::SYS_execve,
            libc::SYS_execveat,
            libc::SYS_kill,
            libc::SYS_tkill,
            libc::SYS_tgkill,
            libc::SYS_rt_sigqueueinfo,
            libc::SYS_rt_tgsigqueueinfo,
            libc::SYS_pidfd_open,
            libc::SYS_pidfd_send_signal,
            libc::SYS_pidfd_getfd,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_open_by_handle_at,
            libc::SYS_name_to_handle_at,
            libc::SYS_userfaultfd,
            libc::SYS_kexec_load,
            libc::SYS_reboot,
            libc::SYS_init_module,
            libc::SYS_finit_module,
            libc::SYS_delete_module,
            libc::SYS_setuid,
            libc::SYS_setgid,
            libc::SYS_setreuid,
            libc::SYS_setregid,
            libc::SYS_setresuid,
            libc::SYS_setresgid,
            libc::SYS_setgroups,
            libc::SYS_capset,
            libc::SYS_personality,
        ] {
            deny_syscall(&mut filter, syscall);
        }
        filter.push(jump(BPF_JMP_JEQ_K, libc::SYS_clone as u32, 0, 4));
        filter.push(stmt(BPF_LD_W_ABS, 16));
        filter.push(jump(BPF_JMP_JSET_K, 0x7e02_0000, 0, 1));
        filter.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32));
        filter.push(stmt(BPF_LD_W_ABS, 0));
        filter.push(jump(BPF_JMP_JEQ_K, libc::SYS_clone3 as u32, 0, 1));
        filter.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS));
        filter.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
        filter
    }

    fn install_filter(filter: &[SockFilter]) -> io::Result<()> {
        let program = SockFprog {
            len: filter.len() as u16,
            filter: filter.as_ptr(),
        };
        if unsafe { libc::prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn network_only_filter() -> Vec<SockFilter> {
        let mut filter = seccomp_arch_filter();
        deny_network_syscalls(&mut filter);
        filter.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
        filter
    }

    pub fn network_filter() -> impl Fn() -> io::Result<()> + Send + Sync {
        let filter = network_only_filter();
        move || install_filter(&filter)
    }

    pub fn enter() -> Result<()> {
        close_inherited_descriptors()?;
        clear_capabilities().map_err(|_| fail())?;
        restrict_filesystem()?;
        install_filter(&seccomp_filter()).map_err(|_| fail())?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn denied_pair_position(filter: &[SockFilter], syscall: libc::c_long) -> usize {
            filter
                .windows(2)
                .position(|pair| {
                    pair[0].code == BPF_JMP_JEQ_K
                        && pair[0].k == syscall as u32
                        && pair[0].jt == 0
                        && pair[0].jf == 1
                        && pair[1].code == BPF_RET_K
                        && pair[1].k == (SECCOMP_RET_ERRNO | libc::EPERM as u32)
                })
                .expect("syscall denial")
        }

        #[test]
        fn seccomp_filter_guards_x32_and_denies_sibling_signals_and_remote_limits() {
            let filter = seccomp_filter();
            #[cfg(target_arch = "x86_64")]
            let audit_arch = 0xc000_003e;
            #[cfg(target_arch = "aarch64")]
            let audit_arch = 0xc000_00b7;
            assert_eq!(filter[0].k, 4);
            assert_eq!(filter[1].k, audit_arch);
            assert_eq!(filter[2].k, SECCOMP_RET_KILL_PROCESS);
            assert_eq!(filter[3].k, 0);
            #[cfg(target_arch = "x86_64")]
            {
                assert_eq!(filter[4].code, BPF_JMP_JGE_K);
                assert_eq!(filter[4].k, 0x4000_0000);
                assert_eq!((filter[4].jt, filter[4].jf), (0, 1));
                assert_eq!(filter[5].k, SECCOMP_RET_KILL_PROCESS);
                assert_eq!(filter[6].k, libc::SYS_prlimit64 as u32);
                assert_eq!((filter[6].jt, filter[6].jf), (0, 4));
                assert_eq!(filter[7].k, 16);
                assert_eq!(filter[8].k, 0);
                assert_eq!((filter[8].jt, filter[8].jf), (1, 0));
                assert_eq!(filter[9].k, SECCOMP_RET_ERRNO | libc::EPERM as u32);
                assert_eq!(filter[10].k, 0);
            }
            #[cfg(target_arch = "aarch64")]
            {
                assert_eq!(filter[4].k, libc::SYS_prlimit64 as u32);
                assert_eq!((filter[4].jt, filter[4].jf), (0, 4));
                assert_eq!(filter[5].k, 16);
                assert_eq!(filter[6].k, 0);
                assert_eq!((filter[6].jt, filter[6].jf), (1, 0));
                assert_eq!(filter[7].k, SECCOMP_RET_ERRNO | libc::EPERM as u32);
                assert_eq!(filter[8].k, 0);
            }
            denied_pair_position(&filter, libc::SYS_rt_sigqueueinfo);
            denied_pair_position(&filter, libc::SYS_rt_tgsigqueueinfo);
        }

        #[test]
        fn both_filters_deny_network_syscalls() {
            for filter in [network_only_filter(), seccomp_filter()] {
                for syscall in [
                    libc::SYS_socket,
                    libc::SYS_connect,
                    libc::SYS_bind,
                    libc::SYS_listen,
                    libc::SYS_accept4,
                    libc::SYS_sendmsg,
                    libc::SYS_recvmsg,
                    libc::SYS_io_uring_setup,
                    libc::SYS_io_uring_enter,
                    libc::SYS_io_uring_register,
                ] {
                    denied_pair_position(&filter, syscall);
                }
            }
        }

        #[test]
        fn wasm_network_filter_refuses_socket_creation_after_exec() {
            use std::{os::unix::process::CommandExt, process::Command};

            const PROBE: &str = "KONAE_TEST_WASM_NETWORK_FILTER";
            if std::env::var_os(PROBE).is_some() {
                let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
                assert_eq!(fd, -1);
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
                return;
            }
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.env(PROBE, "1").args([
                "--exact",
                "sandbox::linux::tests::wasm_network_filter_refuses_socket_creation_after_exec",
            ]);
            let filter = network_filter();
            unsafe {
                command.pre_exec(move || {
                    drop_privileges()?;
                    filter()
                });
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }

        #[test]
        fn landlock_access_mask_follows_reported_abi() {
            assert_eq!(landlock_handled_access_fs(1), 0x1fff);
            assert_eq!(landlock_handled_access_fs(2), 0x3fff);
            assert_eq!(landlock_handled_access_fs(3), 0x7fff);
            assert_eq!(landlock_handled_access_fs(4), 0x7fff);
            assert_eq!(landlock_handled_access_fs(5), 0xffff);
            assert_eq!(landlock_handled_access_fs(8), 0xffff);
            assert_eq!(landlock_handled_access_fs(9), 0x1_ffff);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::drop_privileges;
#[cfg(target_os = "linux")]
pub(crate) use linux::network_filter;

#[cfg(target_os = "linux")]
pub fn enter_worker_sandbox() -> anyhow::Result<()> {
    linux::enter()
}

#[cfg(not(target_os = "linux"))]
pub fn enter_worker_sandbox() -> anyhow::Result<()> {
    Err(crate::WorkerError::new("sandbox_unavailable").into())
}
