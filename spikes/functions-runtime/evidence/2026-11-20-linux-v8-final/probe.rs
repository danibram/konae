use konae_functions_executor::{Budget, Input, MemoryPolicy, Worker, WorkerLimits, error_code};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const MIB: usize = 1024 * 1024;
const HELPER_BYTES: usize = 96 * MIB;

fn read(path: &Path, file: &str) -> String {
    fs::read_to_string(path.join(file))
        .unwrap()
        .trim()
        .to_owned()
}

fn fields(text: &str) -> Value {
    let mut values = serde_json::Map::new();
    for line in text.lines() {
        let (key, value) = line.split_once(' ').unwrap();
        values.insert(key.to_owned(), json!(value.parse::<u64>().unwrap()));
    }
    Value::Object(values)
}

fn invoke(worker: &mut Worker, route: &str) -> Result<konae_functions_executor::Output, String> {
    worker
        .invoke_with(
            Input {
                url: format!("http://probe.test/{route}"),
                method: "GET".into(),
                headers: vec![],
                body: vec![],
                actor: String::new(),
            },
            &Budget::new(Duration::from_secs(3)),
            &mut |_, _| panic!("unexpected operation"),
        )
        .map_err(|error| error_code(&error).to_owned())
}

fn spawn(exe: &Path, bundle: &Path, root: &Path, mib: usize) -> Worker {
    Worker::spawn_with_policy(
        exe,
        "pressure",
        bundle,
        &Budget::new(Duration::from_secs(3)),
        WorkerLimits {
            rss_bytes: mib * MIB,
            ..WorkerLimits::default()
        },
        MemoryPolicy::LinuxCgroup {
            delegated_root: root.to_owned(),
        },
    )
    .unwrap()
}

fn ready(worker: &mut Worker, calls: u64) -> Value {
    let response = invoke(worker, "ready").unwrap();
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(value["marker"], "attached");
    assert_eq!(value["calls"], calls);
    assert_eq!(value["deno"], "undefined");
    assert_eq!(value["fetch"], "undefined");
    value
}

fn process_path(pid: u32) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}"))
}

fn worker_group(root: &Path, pid: u32) -> PathBuf {
    let entries = groups(root);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let group = entries.into_iter().next().unwrap();
    assert_eq!(read(&group, "cgroup.procs"), pid.to_string());
    let expected = format!("0::/delegated/{}", group.file_name().unwrap().display());
    let proc = process_path(pid);
    assert_eq!(read(&proc, "cgroup"), expected);
    assert_eq!(read(&proc, "oom_score_adj"), "0");
    assert_no_caps(&proc);
    group
}

fn assert_no_caps(proc: &Path) {
    let status = read(proc, "status");
    for line in [
        "CapEff:\t0000000000000000",
        "CapBnd:\t0000000000000000",
        "NoNewPrivs:\t1",
    ] {
        assert!(status.lines().any(|entry| entry == line), "{line}");
    }
}

fn groups(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("konae-worker-")
        })
        .collect()
}

fn assert_clean(worker: &Worker, root: &Path, pid: u32) {
    assert!(worker.pid().is_none());
    assert!(!process_path(pid).exists(), "worker {pid} still in /proc");
    assert!(groups(root).is_empty());
}

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn charge_group(group: &Path) {
    fs::write(group.join("cgroup.procs"), std::process::id().to_string()).unwrap();
    let memory = vec![7u8; HELPER_BYTES];
    println!("ready");
    let _ = io::stdin().read(&mut [0]);
    std::hint::black_box(&memory);
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

fn main() {
    if env::args_os().nth(1).as_deref() == Some("--charge".as_ref()) {
        charge_group(Path::new(&env::args_os().nth(2).unwrap()));
        return;
    }
    let [exe, bundle, root, outer]: [PathBuf; 4] = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    assert_eq!(
        read(Path::new("/proc/self"), "cgroup"),
        "0::/delegated/supervisor"
    );
    assert_no_caps(Path::new("/proc/self"));
    assert!(groups(&root).is_empty());
    let outer_before = fields(&read(&outer, "memory.events.local"));
    let parent_before = fields(&read(&root, "memory.events.local"));
    let before = fields(&read(&root, "memory.events"));
    let mut samples = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let mut worker = spawn(&exe, &bundle, &root, 256);
        let init_ms = milliseconds(start);
        let response = ready(&mut worker, 1);
        let cold_ms = milliseconds(start);
        let pid = worker.pid().unwrap();
        let group = worker_group(&root, pid);
        assert_eq!(read(&group, "memory.max"), (256 * MIB).to_string());
        assert_eq!(read(&group, "memory.swap.max"), "0");
        assert_eq!(read(&group, "pids.max"), "64");
        assert_eq!(read(&group, "memory.oom.group"), "1");
        assert!(group.join("cgroup.kill").is_file());
        let mut warm = Vec::new();
        for count in 2..=6 {
            let start = Instant::now();
            ready(&mut worker, count);
            warm.push(milliseconds(start));
        }
        samples.push(json!({
            "pid": pid, "subgroup": group.file_name().unwrap().to_str().unwrap(),
            "memory_max": 256 * MIB, "swap_max": 0, "pids_max": 64, "oom_group": 1,
            "memory_current_bytes": read(&group, "memory.current").parse::<u64>().unwrap(),
            "memory_peak_bytes": read(&group, "memory.peak").parse::<u64>().unwrap(),
            "response": response, "init_ms": init_ms, "cold_ms": cold_ms, "warm_ms": warm,
        }));
        if samples.len() == 3 {
            let mut helper = start_charge(&group);
            let helper_pid = helper.id();
            let charged_bytes = read(&group, "memory.current").parse::<u64>().unwrap();
            assert!(charged_bytes >= HELPER_BYTES as u64);
            assert!(
                read(&group, "cgroup.procs")
                    .lines()
                    .any(|entry| entry == helper_pid.to_string())
            );
            let start = Instant::now();
            let result = invoke(&mut worker, "pressure");
            let elapsed_ms = milliseconds(start);
            let helper_status = helper.try_wait().unwrap();
            if helper_status.is_none() {
                drop(helper.stdin.take());
                helper.wait().unwrap();
            }
            let after = fields(&read(&root, "memory.events"));
            let local_after = fields(&read(&root, "memory.events.local"));
            let outer_after = fields(&read(&outer, "memory.events.local"));
            let pid_exists = process_path(pid).exists();
            let subgroup_exists = group.exists();
            println!(
                "{}",
                json!({
                    "pressure": {
                        "pid": pid, "bytes_requested": 320 * MIB,
                    "helper_pid": helper_pid, "helper_requested_bytes": HELPER_BYTES,
                    "helper_signal": helper_status.and_then(|status| status.signal()),
                    "charged_before_request_bytes": charged_bytes,
                        "result": result.as_ref().err(), "elapsed_ms": elapsed_ms,
                        "pid_exists": pid_exists, "subgroup_exists": subgroup_exists,
                        "delegated_events_before": before, "delegated_events_after": after,
                        "delegated_local_before": parent_before, "delegated_local_after": local_after,
                        "container_local_before": outer_before, "container_local_after": outer_after,
                    },
                    "supervisor_ipc_samples": samples,
                })
            );
            assert_eq!(result.unwrap_err(), "worker_failed");
            assert_eq!(helper_status.and_then(|status| status.signal()), Some(9));
            assert_eq!(local_after, parent_before);
            assert_eq!(outer_after, outer_before);
            for field in ["max", "oom", "oom_kill", "oom_group_kill"] {
                assert!(
                    after[field].as_u64().unwrap() > before[field].as_u64().unwrap(),
                    "{field}"
                );
            }
        } else {
            worker.shutdown(Duration::from_millis(250)).unwrap();
        }
        assert_clean(&worker, &root, pid);
    }
    let mut recovered = spawn(&exe, &bundle, &root, 256);
    let pid = recovered.pid().unwrap();
    let response = ready(&mut recovered, 1);
    let group = worker_group(&root, pid);
    recovered.shutdown(Duration::from_millis(250)).unwrap();
    assert_clean(&recovered, &root, pid);
    println!(
        "{}",
        json!({
            "recovery": {"pid": pid, "subgroup": group.file_name().unwrap().to_str().unwrap(),
                "response": response, "reaped": true, "subgroup_removed": true}
        })
    );
}
