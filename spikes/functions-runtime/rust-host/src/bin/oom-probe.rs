use konae_functions_executor::protocol::{ChildMessage, ParentMessage};
use konae_functions_executor::{
    Budget, Input, MemoryPolicy, Worker, WorkerLimits, error_code, protocol,
};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    os::unix::process::CommandExt,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

const MIB: usize = 1024 * 1024;

fn read(path: &Path, file: &str) -> String {
    fs::read_to_string(path.join(file))
        .unwrap()
        .trim()
        .to_owned()
}

fn counter(events: &str, name: &str) -> u64 {
    events
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == name).then(|| value.parse().unwrap())
        })
        .unwrap_or(0)
}

fn oom_count(path: &Path) -> u64 {
    let events = read(path, "memory.events");
    counter(&events, "oom_kill") + counter(&events, "oom_group_kill")
}

fn invoke(worker: &mut Worker, route: &str) -> Result<(), String> {
    worker
        .invoke_with(
            Input {
                url: format!("http://probe.test/{route}"),
                method: "GET".into(),
                headers: vec![],
                body: vec![],
                body_len: 0,
                actor: String::new(),
            },
            &Budget::new(Duration::from_secs(3)),
            &mut |_, _| panic!("unexpected native operation"),
        )
        .map(|_| ())
        .map_err(|error| error_code(&error).to_owned())
}

fn spawn(exe: &Path, bundle: &Path, root: &Path) -> Worker {
    Worker::spawn_with_policy(
        exe,
        "pressure",
        bundle,
        &Budget::new(Duration::from_secs(5)),
        WorkerLimits {
            rss_bytes: 256 * MIB,
            ..WorkerLimits::default()
        },
        MemoryPolicy::LinuxCgroup {
            delegated_root: root.to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("worker spawn failed: {error:#}"))
}

fn groups(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("konae-worker-")
        })
        .collect()
}

fn charge(group: &Path) {
    fs::write(group.join("cgroup.procs"), std::process::id().to_string()).unwrap();
    let memory = vec![7u8; 96 * MIB];
    println!("ready");
    let _ = std::io::stdin().read(&mut [0]);
    std::hint::black_box(memory);
}

fn exhaust_pids(group: &Path, result: &Path) {
    fs::write(group.join("cgroup.procs"), std::process::id().to_string()).unwrap();
    let mut children = Vec::new();
    loop {
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            loop {
                unsafe { libc::pause() };
            }
        }
        if pid < 0 {
            break;
        }
        children.push(pid);
    }
    let current = read(group, "pids.current");
    fs::write(
        result,
        format!(
            "fork_denied child_count={} pids_current={current}\n",
            children.len()
        ),
    )
    .unwrap();
    for pid in children {
        unsafe { libc::kill(pid, libc::SIGKILL) };
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
    }
}

fn start_charge(group: &Path) -> Child {
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--charge")
        .arg(group)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    child
}

fn sandbox_attack(group: &Path, inherited_fd: i32) -> String {
    assert!(env::var_os("KONAE_TEST_SECRET").is_none());
    assert!(unsafe { libc::fcntl(inherited_fd, libc::F_GETFD) } >= 0);
    konae_functions_executor::sandbox::enter_worker_sandbox().unwrap();
    assert_eq!(unsafe { libc::fcntl(inherited_fd, libc::F_GETFD) }, -1);
    assert!(unsafe { libc::open(c"/etc/passwd".as_ptr(), libc::O_RDONLY) } < 0);
    assert!(unsafe { libc::open(c"/proc/self/environ".as_ptr(), libc::O_RDONLY) } < 0);
    let control =
        std::ffi::CString::new(group.join("memory.max").as_os_str().as_encoded_bytes()).unwrap();
    assert!(unsafe { libc::open(control.as_ptr(), libc::O_WRONLY) } < 0);
    assert_eq!(
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) },
        -1
    );
    assert_eq!(
        unsafe { libc::socketpair(libc::AF_INET, libc::SOCK_STREAM, 0, [0; 2].as_mut_ptr()) },
        -1
    );
    let mut unix_sockets = [0; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM,
                0,
                unix_sockets.as_mut_ptr(),
            )
        },
        0
    );
    unsafe {
        libc::close(unix_sockets[0]);
        libc::close(unix_sockets[1]);
    }
    assert_eq!(unsafe { libc::kill(libc::getppid(), libc::SIGKILL) }, -1);
    "filesystem=denied proc_credentials=denied cgroup=denied network=denied signal=denied inherited_fd=closed env=cleared".into()
}

fn start_sandbox_attack(group: &Path, inherited_fd: i32) -> String {
    let executable = env::current_exe().unwrap();
    let group = group.to_owned();
    let child = unsafe {
        Command::new(executable)
            .arg("--sandbox-attack")
            .arg(&group)
            .arg(inherited_fd.to_string())
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .pre_exec(move || {
                if libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setregid(65534, 65534) != 0
                    || libc::setreuid(65534, 65534) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()
            .unwrap()
    };
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "sandbox probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn sandbox_launch(executable: &Path) {
    let mut child_command = Command::new(executable);
    child_command
        .arg("--function-worker")
        .env_clear()
        .env("KONAE_INTERNAL_FUNCTIONS_SANDBOX", "linux-v1")
        .env("KONAE_INTERNAL_FUNCTIONS_UID", "65534")
        .env("KONAE_INTERNAL_FUNCTIONS_GID", "65534")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = child_command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    stdin.write_all(&protocol::STRICT_WORKER_BOOTSTRAP).unwrap();
    protocol::write(
        &mut stdin,
        &ParentMessage::Init {
            abi: protocol::ABI.into(),
            function: "smoke".into(),
            source: "export function handle() { return new Response('ok'); }".into(),
            generation: "sandbox-startup".into(),
            limits: WorkerLimits::default(),
        },
    )
    .unwrap();
    let mut readiness = libc::pollfd {
        fd: stdout.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut readiness, 1, 5000) } > 0;
    let response = if ready {
        protocol::read::<_, ChildMessage>(&mut stdout)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "worker_ready_timeout",
        ))
    };
    match response {
        Ok(ChildMessage::Ready { .. }) => {
            protocol::write(&mut stdin, &ParentMessage::Shutdown { grace_ms: 100 }).unwrap();
            let mut shutdown = libc::pollfd {
                fd: stdout.as_raw_fd(),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            };
            assert!(unsafe { libc::poll(&mut shutdown, 1, 5000) } > 0);
            let complete: ChildMessage = protocol::read(&mut stdout).unwrap();
            assert!(matches!(complete, ChildMessage::ShutdownComplete { .. }));
            assert!(child.wait().unwrap().success());
            println!("sandboxed_worker_start=ready shutdown=clean");
        }
        other => {
            let _ = child.kill();
            let status = child.wait().unwrap();
            let mut diagnostic = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut diagnostic);
            panic!("sandboxed worker failed: response={other:?} exit={status} stderr={diagnostic}");
        }
    }
}

fn main() {
    let args: Vec<_> = env::args_os().skip(1).map(PathBuf::from).collect();
    if args
        .first()
        .is_some_and(|arg| arg == &PathBuf::from("--sandbox-attack"))
    {
        println!(
            "{}",
            sandbox_attack(&args[1], args[2].to_str().unwrap().parse().unwrap())
        );
        return;
    }
    if args
        .first()
        .is_some_and(|arg| arg == &PathBuf::from("--sandbox-launch"))
    {
        sandbox_launch(&args[1]);
        return;
    }
    if args
        .first()
        .is_some_and(|arg| arg == &PathBuf::from("--charge"))
    {
        charge(&args[1]);
        return;
    }
    if args
        .first()
        .is_some_and(|arg| arg == &PathBuf::from("--pids"))
    {
        exhaust_pids(&args[1], &args[2]);
        return;
    }
    if args
        .first()
        .is_some_and(|arg| arg == &PathBuf::from("--die-with-worker"))
    {
        let mut worker = spawn(&args[1], &args[2], &args[3]);
        let pid = worker.pid().unwrap();
        let group = groups(&args[3]).pop().unwrap();
        fs::write(&args[4], format!("{pid} {}\n", group.display())).unwrap();
        let _ = invoke(&mut worker, "spin");
        panic!("blocked worker unexpectedly returned from synchronous spin");
    }
    let [exe, bundle, root]: [PathBuf; 3] = args.try_into().unwrap();
    assert_eq!(
        read(Path::new("/proc/self"), "cgroup"),
        "0::/delegated/supervisor"
    );
    sandbox_launch(&exe);
    let orphan_present = !groups(&root).is_empty();

    assert_eq!(
        env::var("KONAE_TEST_SECRET").as_deref(),
        Ok("parent-secret")
    );
    let inherited = fs::File::open("/dev/null").unwrap();
    assert_eq!(
        unsafe { libc::fcntl(inherited.as_raw_fd(), libc::F_SETFD, 0) },
        0
    );
    let mut sandboxed = spawn(&exe, &bundle, &root);
    let sandboxed_pid = sandboxed.pid().unwrap();
    let subgroup = groups(&root).pop().unwrap();
    assert_eq!(groups(&root).len(), 1);
    if orphan_present {
        println!("supervisor_kill=child_reaped=true orphan_subgroup_reaped=true");
    }
    let status = read(&PathBuf::from(format!("/proc/{sandboxed_pid}")), "status");
    assert!(
        status
            .lines()
            .any(|line| line == "Uid:\t65534\t65534\t65534\t65534")
    );
    assert!(
        status
            .lines()
            .any(|line| line == "Gid:\t65534\t65534\t65534\t65534")
    );
    assert!(
        status
            .lines()
            .any(|line| line == "CapEff:\t0000000000000000")
    );
    assert!(
        fs::read_link(format!(
            "/proc/{sandboxed_pid}/fd/{}",
            inherited.as_raw_fd()
        ))
        .is_err()
    );
    let policy_result = start_sandbox_attack(&subgroup, inherited.as_raw_fd());
    assert_eq!(read(&subgroup, "memory.max"), (256 * MIB).to_string());
    println!("os_sandbox={policy_result} uid=65534 gid=65534 capabilities=empty");
    assert_eq!(invoke(&mut sandboxed, "ready"), Ok(()));
    let pids_result = PathBuf::from(format!("/tmp/pids-{}.txt", std::process::id()));
    let mut pids = Command::new(env::current_exe().unwrap())
        .arg("--pids")
        .arg(&subgroup)
        .arg(&pids_result)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    assert!(pids.wait().unwrap().success());
    let pids_output = fs::read_to_string(&pids_result).unwrap();
    fs::remove_file(pids_result).unwrap();
    let forked = pids_output
        .split("child_count=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(forked <= 62, "pids.max was not enforced: {pids_output}");
    assert!(
        pids_output.contains("pids_current=64"),
        "pids.max did not stop fork at 64: {pids_output}"
    );
    assert!(read(&subgroup, "pids.current").parse::<u64>().unwrap() <= 64);
    assert_eq!(read(&subgroup, "pids.max"), "64");
    println!("pids_limit=max64 effective=true {pids_output}");
    sandboxed.shutdown(Duration::from_millis(250)).unwrap();
    assert!(sandboxed.pid().is_none());
    assert!(groups(&root).is_empty());

    let before_crash = oom_count(&root);
    let mut crashed = spawn(&exe, &bundle, &root);
    let crashed_pid = crashed.pid().unwrap();
    assert_eq!(groups(&root).len(), 1);
    assert_eq!(unsafe { libc::kill(crashed_pid as i32, libc::SIGKILL) }, 0);
    let crash_result = invoke(&mut crashed, "ready");
    assert_eq!(crash_result, Err("worker_failed".into()));
    assert_eq!(oom_count(&root), before_crash);
    assert!(crashed.pid().is_none());
    assert!(groups(&root).is_empty());
    println!("ordinary_crash=worker_failed oom_delta=0 reaped=true subgroup_removed=true");

    let before_oom = oom_count(&root);
    let mut worker = spawn(&exe, &bundle, &root);
    let worker_pid = worker.pid().unwrap();
    let group = groups(&root).pop().unwrap();
    let mut helper = start_charge(&group);
    let helper_pid = helper.id();
    assert!(read(&group, "memory.current").parse::<u64>().unwrap() >= (96 * MIB) as u64);
    let oom_result = invoke(&mut worker, "pressure");
    let helper_status = helper.wait().unwrap();
    let after_oom = oom_count(&root);
    assert_eq!(oom_result, Err("worker_memory_limit".into()));
    assert!(after_oom > before_oom, "OOM counters did not increase");
    assert_eq!(helper_status.signal(), Some(libc::SIGKILL));
    assert!(worker.pid().is_none());
    assert!(groups(&root).is_empty());
    println!(
        "cgroup_oom=worker_memory_limit oom_delta={} helper_pid={} signal={:?} worker_pid={} reaped=true subgroup_removed=true",
        after_oom - before_oom,
        helper_pid,
        helper_status.signal(),
        worker_pid,
    );
}
