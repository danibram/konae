use konae_functions_executor::{
    Budget, MemoryPolicy, Worker,
    protocol::{Input, WorkerLimits},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    time::Duration,
};

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
fn handler_throw_reports_text_and_reuses_worker() {
    let bundles = bundle("export function handle(request) { if (new URL(request.url).pathname === '/throw') throw new Error('handler exploded'); return new Response('warm'); }");
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let pid = worker.pid().unwrap();

    let error = invoke(&mut worker, input("/throw", "alice"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(konae_functions_executor::error_code(&error), "handler_error");
    assert!(konae_functions_executor::error_detail(&error).unwrap().contains("handler exploded"));
    assert_eq!(worker.pid(), Some(pid));
    assert!(worker.is_synchronized());

    let output = invoke(&mut worker, input("/warm", "alice"), Duration::from_secs(2)).unwrap();
    assert_eq!(output.body, "warm");
    assert_eq!(worker.pid(), Some(pid));
}

#[test]
fn invoke_selects_the_named_bundle() {
    let bundles = BTreeMap::from([
        ("orders".into(), "export function handle() { return new Response('orders'); }".into()),
        ("users".into(), "export function handle() { return new Response('users'); }".into()),
    ]);
    let mut worker = worker(&bundles, Duration::from_secs(2));
    let pid = worker.pid().unwrap();
    for (function, expected) in [("orders", "orders"), ("users", "users")] {
        let output = invoke_function(&mut worker, function, input("/", "alice"), Duration::from_secs(2)).unwrap();
        assert_eq!(output.body, expected);
        assert_eq!(worker.pid(), Some(pid));
    }
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
