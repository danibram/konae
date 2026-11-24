use konae_functions_executor::{
    Budget, MemoryPolicy, Worker,
    protocol::{Input, WorkerLimits},
};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use std::thread;
use std::{collections::BTreeMap, path::Path, time::Duration};

#[path = "fixtures/worker.rs"]
mod fixtures;

type Bundle = BTreeMap<String, String>;

fn bundle(source: &str) -> Bundle {
    BTreeMap::from([("orders".into(), source.into())])
}

fn worker(bundles: &Bundle, timeout: Duration) -> Worker {
    Worker::spawn_with_policy(
        Path::new(env!("CARGO_BIN_EXE_konae-functions-runner")),
        bundles,
        &Budget::new(timeout),
        WorkerLimits::default(),
        MemoryPolicy::DevelopmentPolling,
    )
    .unwrap()
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
fn webcrypto_vectors_match_on_each_invocation() {
    let source = fixtures::webcrypto_vectors();
    let timeout = Duration::from_secs(10);
    let mut worker = worker(&bundle(&source), timeout);
    for (path, expected) in [
        ("/vectors", "passed"),
        ("/reuse", "true"),
        ("/vectors", "passed"),
    ] {
        let reply = invoke(&mut worker, input(path, "alice"), timeout).unwrap_or_else(|error| {
            panic!(
                "{path}: {error:#}; detail={:?}",
                konae_functions_executor::error_detail(&error)
            )
        });
        assert_eq!(reply.body, expected, "{path}");
    }
}

#[test]
fn handler_throw_reports_text_and_reuses_worker() {
    let bundles = bundle(fixtures::HANDLER_THROW);
    let mut worker = worker(&bundles, Duration::from_secs(2));
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

    let output = invoke(&mut worker, input("/warm", "alice"), Duration::from_secs(2)).unwrap();
    assert_eq!(output.body, "warm");
    assert_eq!(worker.pid(), Some(pid));
}

#[test]
fn invoke_selects_the_named_bundle() {
    let bundles = fixtures::NAMED_BUNDLES
        .iter()
        .map(|(name, source)| ((*name).into(), (*source).into()))
        .collect();
    let mut worker = worker(&bundles, Duration::from_secs(2));
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

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a strict Linux cgroup delegation"]
fn strict_worker_survives_the_short_lived_spawn_thread() {
    let executable = Path::new(env!("CARGO_BIN_EXE_konae-functions-runner")).to_owned();
    let bundles = bundle("export function handle() { return new Response('alive'); }");
    let root = std::env::var("KONAE_FUNCTIONS_CGROUP").expect("strict cgroup root required");
    let mut worker = thread::spawn(move || {
        Worker::spawn_with_policy(
            &executable,
            &bundles,
            &Budget::new(Duration::from_secs(10)),
            WorkerLimits::default(),
            MemoryPolicy::LinuxCgroup {
                delegated_root: root.into(),
            },
        )
        .unwrap()
    })
    .join()
    .unwrap();
    let pid = worker.pid().unwrap();
    thread::sleep(Duration::from_millis(100));
    let output =
        invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap_or_else(|error| {
            panic!("strict worker {pid} died after its spawn thread exited: {error}")
        });
    assert_eq!(output.body, "alive");
    assert_eq!(worker.pid(), Some(pid));
    drop(worker);
}

#[test]
fn static_import_is_refused_before_module_evaluation() {
    let bundles = bundle(
        "import './unused.mjs'; globalThis.marker = true; export function handle() { return new Response('ran'); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let error = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "function_import_forbidden"
    );
}

#[test]
fn dynamic_import_is_refused_before_loading_the_dependency() {
    let bundles = bundle(
        "export async function handle() { try { await import('./unused.mjs'); return new Response('loaded'); } catch { return new Response('blocked'); } }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(output.body, "blocked");
}

#[test]
fn globals_and_context_capabilities_do_not_cross_requests() {
    let bundles = bundle(
        "globalThis.count = (globalThis.count || 0) + 1; export function handle(request, context) { return Response.json({ count: globalThis.count, actor: context.actor.userId, actionCall: typeof context.actions.call, hasData: 'data' in context, hasGenericCall: 'call' in context }); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
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
fn request_timers_can_be_cancelled_and_idle_intervals_are_cleared() {
    let bundles = bundle(
        "export async function handle(request) { const path = new URL(request.url).pathname; if (path === '/cancel') { const result = await new Promise(resolve => { const late = setTimeout(() => resolve('late'), 25); setTimeout(() => { clearTimeout(late); resolve('cleared'); }, 0); }); return new Response(result); } setInterval(() => {}, 1); return new Response('done'); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let cancelled = invoke(
        &mut worker,
        input("/cancel", "alice"),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(cancelled.body, "cleared");
    let completed = invoke(&mut worker, input("/idle", "alice"), Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("idle interval survived response: {error}"));
    assert_eq!(completed.body, "done");
    let next = invoke(&mut worker, input("/next", "bob"), Duration::from_secs(2)).unwrap();
    assert_eq!(next.body, "done");
}

#[test]
fn request_web_apis_are_scoped_and_clone_transfers_detach() {
    let bundles = bundle(
        "export async function handle() { const first = performance.now(); await new Promise(resolve => setTimeout(resolve, 2)); const second = performance.now(); const buffer = new ArrayBuffer(4); const map = new Map(); map.set('self', map); const copy = structuredClone({ map, buffer }, { transfer: [buffer] }); let callableRejected = false; try { structuredClone(() => {}); } catch (error) { callableRejected = error.name === 'DataCloneError'; } return Response.json({ monotonic: second >= first, cycle: copy.map.get('self') === copy.map, detached: buffer.byteLength === 0, transferred: copy.buffer.byteLength === 4, callableRejected }); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    for actor in ["alice", "bob"] {
        let output = invoke(&mut worker, input("/", actor), Duration::from_secs(2)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&output.body).unwrap(),
            json!({"monotonic": true, "cycle": true, "detached": true, "transferred": true, "callableRejected": true})
        );
    }
}

#[test]
fn request_microtasks_run_before_the_handler_returns() {
    let bundles = bundle(
        "export async function handle() { const order = []; queueMicrotask(() => order.push('microtask')); Promise.resolve().then(() => order.push('promise')); await new Promise(resolve => queueMicrotask(() => resolve(7))); return Response.json({ order, value: 7 }); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let output =
        invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap_or_else(|error| {
            panic!(
                "{error}; detail={:?}",
                konae_functions_executor::error_detail(&error)
            )
        });
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({"order": ["microtask", "promise"], "value": 7})
    );
}

#[test]
fn web_crypto_entropy_changes_across_requests() {
    let bundles = bundle(
        "export function handle() { const bytes = new Uint8Array(32); crypto.getRandomValues(bytes); return Response.json({ uuid: crypto.randomUUID(), bytes: Array.from(bytes) }); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let one = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap();
    let two = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap();
    assert_ne!(one.body, two.body);
}

#[test]
fn top_level_timer_and_crypto_effects_fail_initialization() {
    for source in [
        "setTimeout(() => {}, 0); export function handle() { return new Response('bad'); }",
        "try { crypto.randomUUID(); } catch {} export function handle() { return new Response('bad'); }",
        "try { await crypto.subtle.digest('SHA-256', new Uint8Array(1)); } catch {} export function handle() {}",
    ] {
        let bundles = bundle(source);
        let mut worker = Worker::spawn_with_policy(
            Path::new(env!("CARGO_BIN_EXE_konae-functions-runner")),
            &bundles,
            &Budget::new(Duration::from_secs(2)),
            WorkerLimits::default(),
            MemoryPolicy::DevelopmentPolling,
        )
        .unwrap();
        let error = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2))
            .expect_err("top-level effect must fail function initialization");
        assert_eq!(
            konae_functions_executor::error_code(&error),
            "unsupported_runtime_api"
        );
    }
}

#[test]
fn timer_string_callbacks_are_refused() {
    let bundles = bundle(
        "export function handle() { try { setTimeout('globalThis.executed = true', 0); } catch (error) { return new Response(JSON.stringify({ name: error.name, executed: globalThis.executed === true })); } return new Response('bad'); }",
    );
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let output = invoke(&mut worker, input("/", "alice"), Duration::from_secs(2)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({"name": "TypeError", "executed": false})
    );
}

#[test]
fn v8_watchdog_reuses_a_synchronized_worker_after_a_loop() {
    let bundle = bundle(
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
    let bundle = bundle(
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
    assert_eq!(worker.pid(), Some(pid), "{}", worker.diagnostics());

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
    let bundle = bundle(&format!(
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
fn both_runners_follow_the_portable_web_contract() {
    let bundles = bundle(include_str!("fixtures/function-contract.js"));
    let timeout = Duration::from_secs(5);
    let mut worker = worker(&bundles, timeout);
    let output = invoke(&mut worker, input("/", "alice"), timeout).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({
            "initializedAt": 0,
            "fetch": "function",
            "wasm": "undefined",
            "requestSignal": true,
            "appSignal": "fixture"
        })
    );
    for (path, code) in [
        ("/fetch", "unsupported_runtime_api"),
        ("/timeout", "unsupported_runtime_api"),
        ("/invalid-response", "invalid_response"),
    ] {
        let error = invoke(&mut worker, input(path, "alice"), timeout).unwrap_err();
        assert_eq!(
            konae_functions_executor::error_code(&error),
            code,
            "{path}: {error:#}"
        );
    }
}
