use konae_functions_executor::{
    Budget, MemoryPolicy, Worker,
    protocol::{Input, WorkerLimits},
};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

#[path = "../../functions-runner/tests/fixtures/worker.rs"]
mod fixtures;

struct Bundle(tempfile::TempDir);

impl Bundle {
    fn new(source: &str) -> Self {
        Self::with_sources(&[("orders", source)])
    }

    fn with_sources(sources: &[(&str, &str)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        for (name, source) in sources {
            fs::write(directory.path().join(format!("{name}.mjs")), source).unwrap();
        }
        Self(directory)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn worker(bundle: &Bundle, timeout: Duration) -> Worker {
    let bundles = fs::read_dir(bundle.path())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(path).unwrap())
        })
        .collect();
    let runner = Path::new(env!("CARGO_BIN_EXE_konae-functions-runner-wasm"));
    #[cfg(feature = "starling-runner")]
    let launcher = starling_launcher(bundle, runner);
    #[cfg(feature = "starling-runner")]
    let runner = launcher.as_path();
    Worker::spawn_with_policy(
        runner,
        &bundles,
        &Budget::new(timeout),
        WorkerLimits::default(),
        MemoryPolicy::DevelopmentPolling,
    )
    .unwrap()
}

#[cfg(feature = "starling-runner")]
fn starling_launcher(bundle: &Bundle, runner: &Path) -> std::path::PathBuf {
    use std::{env, os::unix::fs::PermissionsExt, process::Command};

    let cache = env::var("KONAE_STARLING_CACHE").expect("Set KONAE_STARLING_CACHE for the tests");
    let variable_names = [
        "KONAE_STARLING_CACHE",
        "KONAE_STARLING_BUILDER",
        "KONAE_STARLING_TOOLS",
        "KONAE_STARLING_WASMTIME",
        "KONAE_STARLING_NODE",
        "PATH",
    ];
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let mut invalid_import = false;
    for entry in fs::read_dir(bundle.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "mjs") {
            continue;
        }
        let prepared = Command::new(runner)
            .arg("--prepare")
            .arg(path)
            .output()
            .unwrap();
        let rejected = !prepared.status.success()
            && String::from_utf8_lossy(&prepared.stderr).contains("function_import_forbidden");
        assert!(
            prepared.status.success() || rejected,
            "{}",
            String::from_utf8_lossy(&prepared.stderr)
        );
        invalid_import |= rejected;
    }
    let mut script = String::from("#!/bin/sh\n");
    for name in variable_names {
        if !invalid_import && name != "KONAE_STARLING_CACHE" {
            continue;
        }
        if let Ok(value) = env::var(name) {
            script.push_str(&format!("export {name}={}\n", quote(&value)));
        }
    }
    assert!(Path::new(&cache).is_absolute());
    script.push_str(&format!(
        "exec {} \"$@\"\n",
        quote(runner.to_str().unwrap())
    ));
    let launcher = bundle.path().join("worker");
    fs::write(&launcher, script).unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    launcher
}

fn input(path: &str, actor: &str) -> Input {
    Input {
        url: format!("http://example.test{path}"),
        method: "GET".into(),
        headers: Vec::new(),
        body: Vec::new(),
        body_len: 0,
        actor: actor.into(),
    }
}

fn invoke(
    worker: &mut Worker,
    request: Input,
    timeout: Duration,
) -> anyhow::Result<konae_functions_executor::Output> {
    invoke_function(worker, "orders", request, timeout)
}

fn invoke_function(
    worker: &mut Worker,
    function: &str,
    request: Input,
    timeout: Duration,
) -> anyhow::Result<konae_functions_executor::Output> {
    worker.invoke_with(function, request, &Budget::new(timeout), &mut |_, _| {
        Err("forbidden".into())
    })
}

#[test]
fn handler_throw_reports_text_and_reuses_worker() {
    let bundle = Bundle::new(fixtures::HANDLER_THROW);
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    let error = invoke(
        &mut worker,
        input("/throw", "alice"),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "handler_error"
    );
    assert!(
        konae_functions_executor::error_detail(&error)
            .unwrap()
            .contains("handler exploded")
    );
    assert_eq!(worker.pid(), Some(pid));
    assert!(worker.is_synchronized());
    assert_eq!(
        invoke(&mut worker, input("/warm", "alice"), Duration::from_secs(2))
            .unwrap()
            .body,
        "warm"
    );
}

#[test]
fn invoke_selects_the_named_bundle() {
    let bundle = Bundle::with_sources(fixtures::NAMED_BUNDLES);
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    for (function, expected) in [("orders", "orders"), ("users", "users")] {
        let output = invoke_function(
            &mut worker,
            function,
            input("/", "alice"),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output.body, expected);
        assert_eq!(worker.pid(), Some(pid));
    }
}

#[test]
fn static_import_is_refused_before_module_evaluation() {
    let bundle = Bundle::new(
        "import './unused.mjs'; globalThis.marker = true; export function handle() { return new Response('ran'); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let error = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "function_import_forbidden"
    );
}

#[test]
fn dynamic_import_is_refused_before_loading_the_dependency() {
    let bundle = Bundle::new(
        "export async function handle() { try { await import('./unused.mjs'); return new Response('loaded'); } catch { return new Response('blocked'); } }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(output.body, "blocked");
}

#[test]
fn globals_and_context_capabilities_do_not_cross_requests() {
    let bundle = Bundle::new(
        "globalThis.count = (globalThis.count || 0) + await Promise.resolve(1); export function handle(request, context) { return Response.json({ count: globalThis.count, actor: context.actor.userId, actionCall: typeof context.actions.call, hasData: 'data' in context, hasGenericCall: 'call' in context }); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    for actor in ["alice", "bob"] {
        let output = invoke(&mut worker, input("/", actor), Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
        let body: Value = serde_json::from_str(&output.body).unwrap();
        assert_eq!(
            body,
            json!({
                "count": 1,
                "actor": actor,
                "actionCall": "function",
                "hasData": false,
                "hasGenericCall": false
            })
        );
        assert_eq!(worker.pid(), Some(pid));
    }
}

#[test]
fn wasmtime_epoch_reuses_a_synchronized_worker_after_a_loop() {
    let bundle = Bundle::new(
        "export function handle(request) { if (new URL(request.url).pathname === '/spin') { while (true) {} } return Response.json({ alive: true }); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    let error = invoke(
        &mut worker,
        input("/spin", "alice"),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "deadline_exceeded"
    );
    assert_eq!(worker.pid(), Some(pid));
    let output = invoke(&mut worker, input("/ok", "bob"), Duration::from_secs(2)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({"alive": true})
    );
    assert_eq!(worker.pid(), Some(pid));
}

#[test]
fn cooperative_cancellation_reuses_the_runner_worker() {
    let bundle = Bundle::new(
        "export async function handle(request, context) { if (new URL(request.url).pathname === '/cancel') { await context.actions.call('wait', {}); } return Response.json({ alive: true }); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    let budget = Budget::new(Duration::from_secs(2));
    let mut callback_entered = false;
    let mut callback = |operation: &str, _args: Value| {
        assert_eq!(operation, "action");
        callback_entered = true;
        budget.cancel();
        Err("cancelled".into())
    };

    let error = worker
        .invoke_with("orders", input("/cancel", "alice"), &budget, &mut callback)
        .unwrap_err();
    assert!(callback_entered);
    assert_eq!(konae_functions_executor::error_code(&error), "cancelled");
    assert_eq!(worker.pid(), Some(pid));

    let output = invoke(&mut worker, input("/ok", "bob"), Duration::from_secs(2)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({"alive": true})
    );
    assert_eq!(worker.pid(), Some(pid));
}

#[test]
fn shutdown_hook_runs_on_retirement_and_cannot_fail_the_request() {
    assert_shutdown_reports_failure(
        "export function shutdown() { throw new Error('cleanup failed'); }",
    );
}

fn assert_shutdown_reports_failure(hook: &str) {
    let bundle = Bundle::new(&format!(
        "export function handle() {{ return new Response('ok'); }} {hook}"
    ));
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap();
    assert_eq!(output.body, "ok");
    assert!(!worker.shutdown_invoked());

    let error = worker.shutdown(Duration::from_millis(250)).unwrap_err();
    assert!(
        worker.shutdown_invoked(),
        "{error}; {}",
        worker.diagnostics()
    );
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "shutdown_failed"
    );
}

#[test]
fn shutdown_watchdog_interrupts_a_synchronous_infinite_loop() {
    assert_shutdown_reports_failure("export function shutdown() { while (true) {} }");
}

#[test]
fn shutdown_deadline_covers_a_pending_hook_that_ignores_cancellation() {
    assert_shutdown_reports_failure(
        "export async function shutdown() { await new Promise(() => {}); }",
    );
}

#[test]
fn no_host_apis_or_raw_ipc_imports_are_visible_to_javascript() {
    let bundle = Bundle::new(
        r#"
        export function handle() {
            const names = ['Deno', 'process', 'fetch', 'require', 'WebAssembly',
                'setTimeout', 'crypto', '__konae', 'op_worker_call', 'op_worker_body_read',
                'op_worker_failure'];
            return Response.json(names.filter(name => globalThis[name] !== undefined));
        }
    "#,
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(output.body, "[]");
}

#[test]
fn linear_memory_limit_refuses_a_large_allocation() {
    let bundle = Bundle::new(
        r#"
        export function handle() {
            const bytes = new Uint8Array(80 * 1024 * 1024);
            bytes.fill(42);
            return Response.json(bytes[0]);
        }
    "#,
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let error = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "worker_memory_limit"
    );
}

#[test]
fn response_limit_refuses_an_oversized_body() {
    let bundle = Bundle::new(
        "export function handle(request) { if (new URL(request.url).pathname === '/large') return new Response('x'.repeat(2 * 1024 * 1024 + 1)); return new Response('warm'); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(4));
    let error = invoke(
        &mut worker,
        input("/large", "alice"),
        Duration::from_secs(4),
    )
    .unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "response_too_large"
    );
}

#[test]
#[ignore = "run under /usr/bin/time to record peak RSS for a selected response size"]
fn response_memory_probe() {
    let size = std::env::var("KONAE_RESPONSE_PROBE_BYTES")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let bundle = Bundle::new(&format!(
        "export function handle() {{ return new Response('x'.repeat({size})); }}"
    ));
    let mut worker = worker(&bundle, Duration::from_secs(10));
    let pid = worker.pid().unwrap();
    let sampling = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let peak_kib = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sample = sampling.clone();
    let peak = peak_kib.clone();
    let sampler = std::thread::spawn(move || {
        while sample.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(output) = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &pid.to_string()])
                .output()
            {
                if let Ok(kib) = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<usize>()
                {
                    peak.fetch_max(kib, std::sync::atomic::Ordering::Relaxed);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let result = invoke(&mut worker, input("/", "alice"), Duration::from_secs(10));
    sampling.store(false, std::sync::atomic::Ordering::Relaxed);
    sampler.join().unwrap();
    if size <= 256 * 1024 {
        assert_eq!(result.unwrap().body.len(), size);
    } else {
        assert_eq!(
            konae_functions_executor::error_code(&result.unwrap_err()),
            "response_too_large"
        );
    }
    eprintln!(
        "response_probe_bytes={size} peak_worker_rss_kib={}",
        peak_kib.load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[test]
fn web_apis_preserve_utf8_and_form_data_files() {
    let bundle = Bundle::new(
        r#"
        export async function handle() {
            const form = new FormData();
            form.append('name', 'Madrid 🦊');
            form.append('file', new Blob([new Uint8Array([0, 255])]), 'binary.bin');
            const parsed = await new Request('https://example.test', { method: 'POST', body: form }).formData();
            const file = parsed.get('file');
            const controller = new AbortController();
            controller.abort('cancelled');
            return Response.json({
                name: parsed.get('name'), filename: file.name,
                binary: [...new Uint8Array(await file.arrayBuffer())], aborted: controller.signal.aborted,
                utf8: new TextDecoder().decode(new TextEncoder().encode('🇪🇸'))
            });
        }
    "#,
    );
    let mut worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({
            "name": "Madrid 🦊", "filename": "binary.bin", "binary": [0, 255], "aborted": true, "utf8": "🇪🇸"
        })
    );
}
