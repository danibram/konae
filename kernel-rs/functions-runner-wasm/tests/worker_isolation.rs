use konae_functions_executor::{
    Budget, MemoryPolicy, Worker,
    protocol::{Input, WorkerLimits},
};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

struct Bundle(tempfile::TempDir);

impl Bundle {
    fn new(source: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("orders.mjs"), source).unwrap();
        Self(directory)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn worker(bundle: &Bundle, timeout: Duration) -> Worker {
    let runner = Path::new(env!("CARGO_BIN_EXE_konae-functions-runner-wasm"));
    #[cfg(feature = "starling-runner")]
    let launcher = starling_launcher(bundle, runner);
    #[cfg(feature = "starling-runner")]
    let runner = launcher.as_path();
    Worker::spawn_with_policy(
        runner,
        "orders",
        bundle.path(),
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
    let prepared = Command::new(runner)
        .arg("--prepare")
        .arg(bundle.path().join("orders.mjs"))
        .output()
        .unwrap();
    let invalid_import = !prepared.status.success()
        && String::from_utf8_lossy(&prepared.stderr).contains("function_import_forbidden");
    assert!(
        prepared.status.success() || invalid_import,
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
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
    worker.invoke_with(request, &Budget::new(timeout), &mut |_, _| {
        Err("forbidden".into())
    })
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
        .invoke_with(input("/cancel", "alice"), &budget, &mut callback)
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
