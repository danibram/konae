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

    fn sources(&self) -> impl Iterator<Item = std::path::PathBuf> {
        fs::read_dir(self.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "mjs"))
    }
}

const RUNNER: &str = env!("CARGO_BIN_EXE_konae-functions-runner-wasm");

fn worker(bundle: &Bundle, timeout: Duration) -> Worker {
    worker_with_limits(bundle, timeout, WorkerLimits::default())
}

fn worker_with_limits(bundle: &Bundle, timeout: Duration, limits: WorkerLimits) -> Worker {
    #[cfg(feature = "starling-runner")]
    {
        let components = bundle
            .sources()
            .map(|source| {
                let native = source.with_extension("cwasm");
                if !native.exists() {
                    let portable = source.with_extension("wasm");
                    let reply = prepare(&source, &portable);
                    assert_eq!(reply["status"], "ok", "{reply}");
                    let output = std::process::Command::new(RUNNER)
                        .arg("--compile-component")
                        .arg(&native)
                        .stdin(fs::File::open(portable).unwrap())
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                (function_name(&source), fs::File::open(native).unwrap())
            })
            .collect();
        let runner = starling_launcher(bundle);
        Worker::spawn_prepared_with_policy(
            &runner,
            components,
            &Budget::new(timeout),
            limits,
            MemoryPolicy::DevelopmentPolling,
        )
        .unwrap()
    }
    #[cfg(feature = "quickjs-runner")]
    {
        spawn_source_worker(bundle, timeout, limits).unwrap()
    }
}

fn function_name(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}

fn spawn_source_worker(
    bundle: &Bundle,
    timeout: Duration,
    limits: WorkerLimits,
) -> anyhow::Result<Worker> {
    let bundles = bundle
        .sources()
        .map(|path| (function_name(&path), fs::read_to_string(path).unwrap()))
        .collect();
    Worker::spawn_with_policy(
        Path::new(RUNNER),
        &bundles,
        &Budget::new(timeout),
        limits,
        MemoryPolicy::DevelopmentPolling,
    )
}

#[cfg(feature = "starling-runner")]
#[test]
fn runtime_requires_prepared_components() {
    let bundle = Bundle::new("export function handle() { return new Response('ok'); }");
    let outcome = spawn_source_worker(&bundle, Duration::from_secs(2), WorkerLimits::default());
    let error = outcome
        .err()
        .expect("StarlingMonkey accepted an unprepared bundle");
    assert_ne!(
        konae_functions_executor::error_code(&error),
        "deadline_exceeded"
    );
}

#[cfg(feature = "starling-runner")]
fn prepare(source: &Path, portable: &Path) -> Value {
    let helper = std::env::var_os("KONAE_FUNCTIONS_PREPARER")
        .expect("Set KONAE_FUNCTIONS_PREPARER to the built Rust helper");
    let output = std::process::Command::new(helper)
        .args(["prepare", "--input"])
        .arg(source)
        .arg("--output")
        .arg(portable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[cfg(feature = "starling-runner")]
fn starling_launcher(bundle: &Bundle) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let Some(seed) = std::env::var_os("KONAE_STARLING_TEST_SEED") else {
        return RUNNER.into();
    };
    let seed: u64 = seed.to_str().unwrap().parse().unwrap();
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let launcher = bundle.path().join("worker");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexport KONAE_STARLING_TEST_SEED={seed}\nexec {} \"$@\"\n",
            quote(RUNNER)
        ),
    )
    .unwrap();
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

#[cfg(feature = "starling-runner")]
fn expected_random_pair(seed: u64) -> Value {
    let mut state = (seed ^ (seed >> 32)) as u32;
    if state == 0 {
        state = 0x6d2b79f5;
    }
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let high = state >> 5;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let low = state >> 6;
        (high as f64 * 67108864.0 + low as f64) / 9007199254740992.0
    };
    json!([next(), next()])
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

#[cfg(feature = "starling-runner")]
#[test]
fn handler_error_detail_is_bounded_and_uses_portable_frames() {
    let bundle = Bundle::new(
        "export async function handle() { return fail(); } async function fail() { throw new TypeError('🦊'.repeat(3000)); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(5));
    let error = invoke(&mut worker, input("/", "alice"), Duration::from_secs(5)).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "handler_error"
    );
    let detail = konae_functions_executor::error_detail(&error).unwrap();
    assert!(detail.len() <= 4096, "detail was {} bytes", detail.len());
    assert!(detail.starts_with("TypeError:"));
    assert!(detail.contains(".js:"), "{detail}");
    assert!(!detail.contains("/Users/"), "{detail}");
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
    #[cfg(feature = "starling-runner")]
    {
        let portable = bundle.path().join("orders.wasm");
        let reply = prepare(&bundle.path().join("orders.mjs"), &portable);
        assert_eq!(reply["status"], "bundle_contract");
        assert!(!portable.exists());
    }
    #[cfg(feature = "quickjs-runner")]
    {
        let error = spawn_source_worker(&bundle, Duration::from_secs(2), WorkerLimits::default())
            .err()
            .expect("QuickJS accepted an import");
        assert_eq!(
            konae_functions_executor::error_code(&error),
            "function_import_forbidden"
        );
    }
}

#[test]
fn dynamic_import_is_refused_before_loading_the_dependency() {
    let bundle = Bundle::new(
        "export async function handle() { try { await import('./unused.mjs'); return new Response('loaded'); } catch { return new Response('blocked'); } }",
    );
    let mut local_worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(
        &mut local_worker,
        input("/", "alice"),
        Duration::from_secs(2),
    )
    .unwrap_or_else(|error| panic!("{error}; {}", local_worker.diagnostics()));
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

#[cfg(feature = "starling-runner")]
#[test]
fn snapshot_clock_uses_one_build_time_and_live_handler_time() {
    let bundle = Bundle::new(
        r#"
        let counter = 0;
        const buildNow = Date.now();
        const buildDate = +new Date();
        const buildCall = Date();
        export function handle() {
          return Response.json({
            counter: ++counter, buildNow, buildDate, buildCall,
            handlerNow: Date.now(), handlerDate: +new Date(),
            handlerCall: Date(), explicit: +new Date(0),
          });
        }
    "#,
    );
    let mut worker = worker(&bundle, Duration::from_secs(5));
    let first: Value = serde_json::from_str(
        &invoke(&mut worker, input("/", "alice"), Duration::from_secs(5))
            .unwrap()
            .body,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    let second: Value = serde_json::from_str(
        &invoke(&mut worker, input("/", "alice"), Duration::from_secs(5))
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(first["counter"], 1);
    assert_eq!(second["counter"], 1);
    for value in [&first, &second] {
        assert_eq!(value["buildNow"], value["buildDate"]);
        assert_eq!(value["explicit"], 0);
        assert!(value["handlerNow"].as_i64().unwrap() >= value["buildNow"].as_i64().unwrap());
        assert!(
            value["handlerNow"]
                .as_i64()
                .unwrap()
                .abs_diff(value["handlerDate"].as_i64().unwrap())
                <= 5
        );
    }
    assert_eq!(first["buildNow"], second["buildNow"]);
    assert_eq!(first["buildCall"], second["buildCall"]);
    assert!(second["handlerNow"].as_i64().unwrap() > first["handlerNow"].as_i64().unwrap());
}

#[cfg(feature = "starling-runner")]
#[test]
fn random_seed_resets_each_invocation_and_worker() {
    unsafe { std::env::set_var("KONAE_STARLING_TEST_SEED", "305419896") };
    let bundle = Bundle::new(
        "export function handle() { return Response.json([Math.random(), Math.random()]); }",
    );
    let timeout = Duration::from_secs(5);
    let mut first_worker = worker(&bundle, timeout);
    let first: Value = serde_json::from_str(
        &invoke(&mut first_worker, input("/", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    let second: Value = serde_json::from_str(
        &invoke(&mut first_worker, input("/", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    let mut second_worker = worker(&bundle, timeout);
    let first_in_second_worker: Value = serde_json::from_str(
        &invoke(&mut second_worker, input("/", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    unsafe { std::env::remove_var("KONAE_STARLING_TEST_SEED") };

    let seed = 305419896;
    assert_eq!(first, expected_random_pair(seed));
    assert_eq!(second, expected_random_pair(seed + 1));
    assert_eq!(first_in_second_worker, expected_random_pair(seed));
}

#[cfg(feature = "starling-runner")]
#[test]
fn web_api_clones_and_microtasks_match_the_contract() {
    let bundle = Bundle::new(
        r#"
        export async function handle(request) {
          const path = new URL(request.url).pathname;
          if (path === '/buffered') {
            const original = new Response('é', { status: 201, statusText: 'Created' });
            const copy = original.clone();
            copy.headers.set('x-copy', 'yes');
            return Response.json({
              original: await original.text(), copy: await copy.text(),
              originalStatus: original.status, copyStatus: copy.status,
              originalHeader: original.headers.get('x-copy'), copyHeader: copy.headers.get('x-copy'),
            });
          }
          if (path === '/stream') {
            const original = new Response(new ReadableStream({ start(c) {
              c.enqueue(new TextEncoder().encode('A'));
              c.enqueue(new TextEncoder().encode('B'));
              c.close();
            } }));
            const copy = original.clone();
            const [left, right] = await Promise.all([original.text(), copy.text()]);
            return Response.json({ left, right });
          }
          if (path === '/used') {
            const response = new Response('done');
            await response.text();
            try { response.clone(); return new Response('clone accepted'); }
            catch (error) { return new Response(error.name); }
          }
          const order = [];
          queueMicrotask(() => order.push('micro'));
          order.push('sync');
          await Promise.resolve();
          return Response.json({ order, structuredClone: typeof structuredClone });
        }
    "#,
    );
    let mut worker = worker(&bundle, Duration::from_secs(5));
    let timeout = Duration::from_secs(5);
    let buffered: Value = serde_json::from_str(
        &invoke(&mut worker, input("/buffered", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(buffered["original"], "é");
    assert_eq!(buffered["copy"], "é");
    assert_eq!(buffered["originalStatus"], 201);
    assert_eq!(buffered["copyStatus"], 201);
    assert_eq!(buffered["originalHeader"], Value::Null);
    assert_eq!(buffered["copyHeader"], "yes");

    let streamed: Value = serde_json::from_str(
        &invoke(&mut worker, input("/stream", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(streamed, json!({"left": "AB", "right": "AB"}));
    assert_eq!(
        invoke(&mut worker, input("/used", "alice"), timeout)
            .unwrap()
            .body,
        "TypeError"
    );
    let globals: Value = serde_json::from_str(
        &invoke(&mut worker, input("/globals", "alice"), timeout)
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(globals["order"], json!(["sync", "micro"]));
    assert_eq!(globals["structuredClone"], "function");
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
    let mut local_worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(
        &mut local_worker,
        input("/", "alice"),
        Duration::from_secs(2),
    )
    .unwrap_or_else(|error| panic!("{error}; {}", local_worker.diagnostics()));
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
        "export function handle(request) { const url = new URL(request.url); const size = Number(url.searchParams.get('size') || 0); if (url.searchParams.get('stream')) { let sent = 0; return new Response(new ReadableStream({ pull(c) { if (sent >= size) return c.close(); const chunk = new Uint8Array(Math.min(65536, size - sent)); chunk.fill(120); sent += chunk.length; c.enqueue(chunk); } })); } return new Response('x'.repeat(size)); }",
    );
    let mut worker = worker(&bundle, Duration::from_secs(10));
    for size in [256 * 1024, 1024 * 1024, 2 * 1024 * 1024] {
        let result = invoke(
            &mut worker,
            input(&format!("/?size={size}"), "alice"),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(result.body.len(), size);
    }
    let error = invoke(
        &mut worker,
        input(&format!("/?size={}", 2 * 1024 * 1024 + 1), "alice"),
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "response_too_large"
    );
}

#[cfg(feature = "starling-runner")]
#[test]
fn reply_chunks_fit_guest_memory_and_preserve_limits() {
    let bundle = Bundle::new(
        r#"
        export function handle(request) {
          const path = new URL(request.url).pathname;
          if (path === '/split') return new Response(new ReadableStream({ start(c) {
            c.enqueue(Uint8Array.of(0xf0, 0x9f));
            c.enqueue(Uint8Array.of(0xa6, 0x8a));
            c.close();
          } }));
          if (path === '/malformed') return new Response(new ReadableStream({ start(c) {
            c.enqueue(Uint8Array.of(0xc3));
            c.enqueue(Uint8Array.of(0x28));
            c.close();
          } }));
          if (path === '/bom') return new Response('\uFEFFok');
          if (path === '/framing') return new Response('x', { headers: { 'content-length': '1' } });
          if (path === '/cancel') return new Response(new ReadableStream({
            pull(c) { c.enqueue(new Uint8Array(65536)); },
            cancel() { return Promise.resolve(); },
          }));
          let left = Number(new URL(request.url).searchParams.get('size'));
          let sent = 0;
          return new Response(new ReadableStream({
            pull(controller) {
              if (!left) return controller.close();
              const size = Math.min(65536, left);
              const bytes = new Uint8Array(size);
              bytes.fill(97 + sent++ % 26);
              controller.enqueue(bytes);
              left -= size;
            },
          }));
        }
    "#,
    );
    let timeout = Duration::from_secs(10);
    let mut worker = worker(&bundle, timeout);
    for size in [256 * 1024, 1024 * 1024, 2 * 1024 * 1024] {
        let result = invoke(&mut worker, input(&format!("/?size={size}"), ""), timeout)
            .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
        assert_eq!(result.body.len(), size);
        for (index, chunk) in result.body.as_bytes().chunks(65536).enumerate() {
            assert!(chunk.iter().all(|&byte| byte == b'a' + (index % 26) as u8));
        }
    }
    let split = invoke(&mut worker, input("/split", ""), timeout)
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(split.body, "🦊");
    let bom = invoke(&mut worker, input("/bom", ""), timeout).unwrap();
    assert_eq!(bom.body, "ok");
    let framing = invoke(&mut worker, input("/framing", ""), timeout).unwrap();
    assert_eq!(
        framing.headers,
        vec![
            ("content-length".into(), "1".into()),
            ("content-type".into(), "text/plain;charset=UTF-8".into()),
        ]
    );
    let malformed_bundle =
        Bundle::new(&fs::read_to_string(bundle.path().join("orders.mjs")).unwrap());
    let mut malformed_worker =
        worker_with_limits(&malformed_bundle, timeout, WorkerLimits::default());
    let malformed = invoke(&mut malformed_worker, input("/malformed", ""), timeout).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&malformed),
        "invalid_response"
    );
    let mut oversized_worker = worker_with_limits(&bundle, timeout, WorkerLimits::default());
    let error = invoke(&mut oversized_worker, input("/?size=2097153", ""), timeout).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "response_too_large"
    );
}

#[cfg(feature = "starling-runner")]
#[test]
fn reply_limit_and_cancel_are_independent_of_the_http_envelope() {
    let bundle = Bundle::new(
        r#"
        export function handle(request) {
          if (new URL(request.url).pathname === '/text') return new Response('x'.repeat(Number(new URL(request.url).searchParams.get('size'))));
          if (new URL(request.url).pathname === '/cancel') return new Response(new ReadableStream({
            pull(c) { c.enqueue(new Uint8Array(65536)); },
            cancel() { return Promise.resolve(); },
          }));
          const size = Number(new URL(request.url).searchParams.get('size'));
          return new Response(new Uint8Array(size));
        }
    "#,
    );
    let timeout = Duration::from_secs(10);
    let limits = WorkerLimits {
        response_bytes: 1024 * 1024,
        ..WorkerLimits::default()
    };
    let mut worker = worker_with_limits(&bundle, timeout, limits.clone());
    let exact = invoke(&mut worker, input("/?size=1048576", ""), timeout)
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(exact.body.len(), 1024 * 1024);
    let text = invoke(&mut worker, input("/text?size=1048576", ""), timeout)
        .unwrap_or_else(|error| panic!("{error}; {}", worker.diagnostics()));
    assert_eq!(text.body.len(), 1024 * 1024);
    let over = invoke(&mut worker, input("/?size=1048577", ""), timeout).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&over),
        "response_too_large"
    );
    let mut cancel_worker = worker_with_limits(&bundle, timeout, limits);
    let cancel = invoke(&mut cancel_worker, input("/cancel", ""), timeout).unwrap_err();
    assert_eq!(
        konae_functions_executor::error_code(&cancel),
        "response_too_large"
    );
}

#[cfg(feature = "starling-runner")]
#[test]
fn body_chunks_keep_their_bytes_under_the_default_heap() {
    use konae_functions_executor::{RequestBody, RequestBodyChunk};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    struct Upload(Mutex<usize>, AtomicBool);
    const LENGTH: usize = 100_000_000;

    impl RequestBody for Upload {
        fn read_chunk(&self, maximum: usize) -> Result<RequestBodyChunk, String> {
            let mut sent = self.0.lock().unwrap();
            if *sent == LENGTH {
                self.1.store(true, Ordering::Release);
                return Ok(RequestBodyChunk::new(Vec::new(), true));
            }
            let len = maximum.min(LENGTH - *sent);
            let byte = (*sent / 65536 % 256) as u8;
            *sent += len;
            Ok(RequestBodyChunk::new(vec![byte; len], false))
        }

        fn complete(&self) -> bool {
            self.1.load(Ordering::Acquire)
        }
    }

    let bundle = Bundle::new(
        r#"
        export async function handle(request) {
          const reader = request.body.getReader();
          let first;
          let last;
          let total = 0;
          let checksum = 0;
          for (;;) {
            const { value, done } = await reader.read();
            if (done) break;
            first ??= value;
            last = value[0];
            total += value.byteLength;
            for (let index = 0; index < value.length; index++) checksum = (checksum + value[index]) >>> 0;
          }
          return Response.json({ total, first: first[0], last, checksum });
        }
    "#,
    );
    let timeout = Duration::from_secs(60);
    let mut worker = worker(&bundle, timeout);
    let body = Arc::new(Upload(Mutex::new(0), AtomicBool::new(false)));
    let request = Input {
        method: "POST".into(),
        ..input("/", "")
    };
    let result = worker
        .invoke_with_body(
            "orders",
            request,
            body.clone(),
            &Budget::new(timeout),
            &mut |_, _| Err("forbidden".into()),
        )
        .unwrap_or_else(|error| {
            panic!(
                "After {} bytes: {error:#}, detail={:?}",
                body.0.lock().unwrap(),
                konae_functions_executor::error_detail(&error)
            )
        });
    assert_eq!(
        serde_json::from_str::<Value>(&result.body).unwrap(),
        json!({
            "total": LENGTH,
            "first": 0,
            "last": (LENGTH - 1) / 65536 % 256,
            "checksum": (0..LENGTH)
                .fold(0u32, |sum, index| sum.wrapping_add(((index / 65536) % 256) as u32))
        })
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
    if size <= 2 * 1024 * 1024 {
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
        export async function handle(request) {
            if (new URL(request.url).pathname === '/raw') {
                const boundary = 'konae-raw-boundary';
                const raw = new Blob([
                    `--${boundary}\r\nContent-Disposition: form-data; name="name"\r\n\r\nMadrid 🦊\r\n--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="binary.bin"\r\nContent-Type: application/octet-stream\r\n\r\n`,
                    new Uint8Array([0, 255]), `\r\n--${boundary}--\r\n`,
                ]);
                const rawRequest = new Request('https://example.test/raw', {
                    method: 'POST', headers: { 'content-type': `multipart/form-data; boundary=${boundary}` }, body: raw,
                });
                const form = await rawRequest.formData();
                const file = form.get('file');
                return Response.json({name: form.get('name'), filename: file.name, type: file.type,
                    binary: [...new Uint8Array(await file.arrayBuffer())]});
            }
            const form = new FormData();
            form.append('name', 'Madrid 🦊');
            form.append('file', new Blob([new Uint8Array([0, 255])], {type: 'application/octet-stream'}), 'binary.bin');
            const parsed = await new Request('https://example.test', { method: 'POST', body: form }).formData();
            const file = parsed.get('file');
            const controller = new AbortController();
            controller.abort('cancelled');
            return Response.json({
                name: parsed.get('name'), filename: file.name, type: file.type,
                binary: [...new Uint8Array(await file.arrayBuffer())], aborted: controller.signal.aborted,
                utf8: new TextDecoder().decode(new TextEncoder().encode('🇪🇸'))
            });
        }
    "#,
    );
    let mut local_worker = worker(&bundle, Duration::from_secs(2));
    let output = invoke(
        &mut local_worker,
        input("/", "alice"),
        Duration::from_secs(2),
    )
    .unwrap_or_else(|error| panic!("{error}; {}", local_worker.diagnostics()));
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({
            "name": "Madrid 🦊", "filename": "binary.bin", "type": "application/octet-stream",
            "binary": [0, 255], "aborted": true, "utf8": "🇪🇸"
        })
    );
    let output = invoke(
        &mut local_worker,
        input("/raw", "alice"),
        Duration::from_secs(2),
    )
    .unwrap_or_else(|error| panic!("{error}; {}", local_worker.diagnostics()));
    assert_eq!(
        serde_json::from_str::<Value>(&output.body).unwrap(),
        json!({"name": "Madrid 🦊", "filename": "binary.bin", "type": "application/octet-stream", "binary": [0, 255]})
    );
}
