mod budget;
mod cache;
mod data;
mod runtime;
mod worker;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use budget::Budget;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct AppState {
    mode: String,
    enabled: bool,
    timeout: Duration,
    dbs: HashMap<String, data::Database>,
    cache: Mutex<cache::Cache>,
}
struct CancelOnDrop(Budget);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn response(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response()
}
fn error(status: u16, message: &str) -> Response {
    response(status, json!({"error": message}))
}
fn failure(code: &str, budget: &Budget) -> Response {
    if budget.cancel().may_have_committed() {
        return error(409, "mutation_outcome_unknown");
    }
    match code {
        "deadline_exceeded" | "cancelled" => error(504, "deadline_exceeded"),
        "mutation_outcome_unknown" | "idempotency_conflict" => error(409, code),
        "queue_full" | "cache_busy" => error(503, code),
        "idempotency_key_required" | "invalid_input" => error(400, code),
        "worker_failed" | "response_too_large" | "worker_memory_limit" | "invalid_response" => {
            error(502, code)
        }
        "operation_limit" => error(429, code),
        "capability_expired" => error(403, code),
        _ => error(500, "internal_error"),
    }
}
fn failed(error_value: anyhow::Error, budget: &Budget) -> Response {
    let code = if let Some(value) = error_value.downcast_ref::<data::DataError>() {
        value.code()
    } else if let Some(value) = error_value.downcast_ref::<budget::BudgetError>() {
        value.code()
    } else {
        match error_value.to_string().as_str() {
            "queue_full" => "queue_full",
            "cache_busy" => "cache_busy",
            _ => worker::error_code(&error_value),
        }
    };
    failure(code, budget)
}
fn cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}
fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|s| s.parse().expect("invalid numeric configuration"))
        .unwrap_or(default)
}

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--function-worker") {
        return worker::worker_main();
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(serve())
}

async fn serve() -> anyhow::Result<()> {
    let mode = std::env::var("MODE").unwrap_or_else(|_| "native".into());
    anyhow::ensure!(
        ["native", "deno"].contains(&mode.as_str()),
        "MODE must be native or deno"
    );
    let enabled = std::env::var("FUNCTIONS_ENABLED").unwrap_or_else(|_| "1".into()) != "0";
    let port = env_number("PORT", if mode == "native" { 43101 } else { 43102 });
    anyhow::ensure!(port <= u16::MAX as u64, "invalid port");
    let dir = PathBuf::from(
        std::env::var("DATA_DIR").unwrap_or_else(|_| format!("/tmp/konae-functions-{mode}")),
    );
    std::fs::create_dir_all(&dir)?;
    let mut dbs = HashMap::new();
    for app in ["a", "b"] {
        dbs.insert(app.to_owned(), data::open(&dir, app)?);
    }
    let capacity = usize::try_from(env_number("CACHE_CAPACITY", 4))?;
    anyhow::ensure!(
        (1..=64).contains(&capacity),
        "CACHE_CAPACITY must be between 1 and 64"
    );
    let bundles = std::env::var_os("APP_BUNDLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../app/dist"));
    let timeout_ms = env_number("REQUEST_TIMEOUT_MS", env_number("JS_TIMEOUT_MS", 100));
    anyhow::ensure!(
        timeout_ms > 0 && timeout_ms <= 60_000,
        "REQUEST_TIMEOUT_MS must be between 1 and 60000"
    );
    let timeout = Duration::from_millis(timeout_ms);
    let defaults = worker::WorkerLimits::default();
    let limits = worker::WorkerLimits {
        heap_bytes: usize::try_from(env_number("WORKER_HEAP_BYTES", defaults.heap_bytes as u64))?,
        response_bytes: usize::try_from(env_number(
            "WORKER_RESPONSE_BYTES",
            defaults.response_bytes as u64,
        ))?,
        rss_bytes: usize::try_from(env_number("WORKER_RSS_BYTES", defaults.rss_bytes as u64))?,
        operation_limit: usize::try_from(env_number(
            "WORKER_OPERATION_LIMIT",
            defaults.operation_limit as u64,
        ))?,
    };
    anyhow::ensure!(
        limits.validate(),
        "Worker limits require heap >= 1 MiB and <= RSS, response 1..2097152 bytes and operation limit 1..4096"
    );
    let state = Arc::new(AppState {
        mode: mode.clone(),
        enabled,
        timeout,
        dbs,
        cache: Mutex::new(cache::Cache::new(
            capacity,
            Duration::from_millis(env_number("CACHE_TTL_MS", 60_000)),
            timeout,
            bundles,
            limits,
        )),
    });
    if mode == "deno" && enabled {
        let reaper = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tick.tick().await;
                let reaper = reaper.clone();
                if tokio::task::spawn_blocking(move || reaper.cache.lock().unwrap().reap())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port as u16)).await?;
    eprintln!(
        "mode={mode} port={port} pid={} data={}",
        std::process::id(),
        dir.display()
    );
    axum::serve(listener, Router::new().fallback(route).with_state(state)).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    app: String,
    function: String,
    version: String,
}

async fn with_cache<T: Send + 'static>(
    state: Arc<AppState>,
    budget: &Budget,
    work: impl FnOnce(&mut cache::Cache) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let budget = budget.clone();
    tokio::task::spawn_blocking(move || {
        budget.check()?;
        let mut cache = state
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("worker_failed"))?;
        budget.check()?;
        work(&mut cache)
    })
    .await?
}

async fn route(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let budget = Budget::new(state.timeout);
    let _cancel = CancelOnDrop(budget.clone());
    let deadline = tokio::time::Instant::from_std(budget.deadline());
    match tokio::time::timeout_at(deadline, admitted_route(state, request, &budget)).await {
        Ok(_) if std::time::Instant::now() >= budget.deadline() => {
            failure("deadline_exceeded", &budget)
        }
        Ok(response) => response,
        Err(_) => failure("deadline_exceeded", &budget),
    }
}

async fn admitted_route(state: Arc<AppState>, request: Request, budget: &Budget) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let method = parts.method.as_str();
    if path == "/health" && method == "GET" {
        return response(200, json!({"ok": true}));
    }
    if path == "/__stats" && method == "GET" {
        let mode = state.mode.clone();
        let enabled = state.enabled;
        return match with_cache(state.clone(), budget, move |cache| {
            Ok(cache.stats(&mode, enabled))
        })
        .await
        {
            Ok(stats) => response(200, stats),
            Err(error) => failed(error, budget),
        };
    }
    if path == "/__control/evict" && method == "POST" {
        return match with_cache(state.clone(), budget, |cache| Ok(cache.evict_all())).await {
            Ok(count) => response(200, json!({"evicted": count})),
            Err(error) => failed(error, budget),
        };
    }
    if path == "/__control/version" && method == "POST" {
        let Ok(bytes) = to_bytes(body, 16384).await else {
            return error(413, "body_too_large");
        };
        let Ok(v) = serde_json::from_slice::<Version>(&bytes) else {
            return error(400, "invalid_input");
        };
        if !["a", "b"].contains(&v.app.as_str())
            || !["orders", "preferences"].contains(&v.function.as_str())
            || v.version.is_empty()
            || v.version.len() > 64
        {
            return error(400, "invalid_input");
        }
        return match with_cache(state.clone(), budget, move |cache| {
            cache.version(&v.app, &v.function, &v.version)
        })
        .await
        {
            Ok(()) => response(200, json!({"ok": true})),
            Err(error) => failed(error, budget),
        };
    }
    if path == "/__control/loop" && method == "GET" {
        if state.mode != "deno" || !state.enabled {
            return error(404, "not_found");
        }
        let db = state.dbs["a"].clone();
        let command_budget = budget.clone();
        let receive = with_cache(state.clone(), budget, move |cache| {
            cache.dispatch("a", "orders", db, None, command_budget, None)
        })
        .await;
        return receive_response(receive, budget).await;
    }
    let segments: Vec<&str> = path.split('/').collect();
    let (app, function, id) = match segments.as_slice() {
        ["", app, "orders", id] if !id.is_empty() && (method == "GET" || method == "POST") => {
            (*app, "orders", Some(*id))
        }
        ["", app, "preferences"] if method == "GET" => (*app, "preferences", None),
        _ => return error(404, "not_found"),
    };
    let Some(db) = state.dbs.get(app) else {
        return error(404, "not_found");
    };
    if state.mode == "deno" && !state.enabled {
        return error(404, "not_found");
    }
    let token = cookie(&parts.headers, "session").unwrap_or_default();
    let auth_db = db.clone();
    let auth_budget = budget.clone();
    let actor = match tokio::task::spawn_blocking(move || {
        data::authenticate(&auth_db, &token, &auth_budget)
    })
    .await
    {
        Ok(Ok(Some(actor))) => actor,
        Ok(Ok(None)) => return error(401, "unauthorized"),
        Ok(Err(error)) => return failed(error.into(), budget),
        Err(error) => return failed(error.into(), budget),
    };
    let Ok(bytes) = to_bytes(body, 16384).await else {
        return error(413, "body_too_large");
    };
    let key = if function == "orders" && method == "POST" {
        let mut values = parts.headers.get_all("idempotency-key").iter();
        let Some(value) = values.next() else {
            return error(400, "idempotency_key_required");
        };
        let Ok(value) = value.to_str() else {
            return error(400, "invalid_input");
        };
        if value.is_empty() {
            return error(400, "idempotency_key_required");
        }
        if values.next().is_some()
            || value.len() > 128
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return error(400, "invalid_input");
        }
        Some(value.to_owned())
    } else {
        None
    };
    if state.mode == "deno" {
        let input = runtime::Input {
            url: format!("http://localhost{}", parts.uri),
            method: method.to_owned(),
            headers: parts
                .headers
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
                .collect(),
            body: bytes.to_vec(),
            body_len: bytes.len(),
            actor,
        };
        let app = app.to_owned();
        let db = db.clone();
        let command_budget = budget.clone();
        let receive = with_cache(state.clone(), budget, move |cache| {
            cache.dispatch(&app, function, db, Some(input), command_budget, key)
        })
        .await;
        return receive_response(receive, budget).await;
    }
    if function == "preferences" {
        let theme = cookie(&parts.headers, "theme").unwrap_or_else(|| "system".into());
        let theme = percent_encoding::percent_decode_str(&theme)
            .decode_utf8()
            .map(|s| s.into_owned())
            .unwrap_or(theme);
        let mut out = response(
            200,
            json!({"theme": theme, "language": parts.headers.get("accept-language").and_then(|v| v.to_str().ok()), "userId": actor}),
        );
        out.headers_mut().insert(
            "set-cookie",
            "seen=1; Path=/; HttpOnly; SameSite=Lax".parse().unwrap(),
        );
        return out;
    }
    let raw = percent_encoding::percent_decode_str(id.unwrap()).decode_utf8_lossy();
    if raw.is_empty() || raw.len() > 9 || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return error(400, "invalid_input");
    }
    let id: i64 = raw.parse().unwrap();
    if id < 1 {
        return error(400, "invalid_input");
    }
    let db = db.clone();
    let sql_budget = budget.clone();
    let result = if method == "GET" {
        tokio::task::spawn_blocking(move || data::read(&db, &actor, id, &sql_budget)).await
    } else {
        let quantity = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| {
            let obj = v.as_object()?;
            if obj.len() != 1 {
                return None;
            }
            let n = obj.get("quantity")?.as_f64()?;
            if n.fract() != 0.0 || !(1.0..=10.0).contains(&n) {
                return None;
            }
            Some(n as i64)
        });
        let Some(quantity) = quantity else {
            return error(400, "invalid_input");
        };
        let key = key.expect("POST key was validated");
        tokio::task::spawn_blocking(move || {
            data::update(&db, &actor, id, quantity, &key, &sql_budget).map(|outcome| outcome.order)
        })
        .await
    };
    if let Err(error) = budget.check() {
        return failed(error.into(), budget);
    }
    match result {
        Ok(Ok(Some(order))) => response(200, json!(order)),
        Ok(Ok(None)) => error(404, "not_found"),
        Ok(Err(error)) => failed(error.into(), budget),
        Err(error) => failed(error.into(), budget),
    }
}

async fn receive_response(
    receive: anyhow::Result<tokio::sync::oneshot::Receiver<anyhow::Result<runtime::Output>>>,
    budget: &Budget,
) -> Response {
    let output = match receive {
        Ok(rx) => match rx.await {
            Ok(Ok(out)) => out,
            Ok(Err(error)) => return failed(error, budget),
            Err(_) => return failure("worker_failed", budget),
        },
        Err(error) => return failed(error, budget),
    };
    if let Err(error) = budget.check() {
        return failed(error.into(), budget);
    }
    if output.status >= 500 && budget.mutation_outcome().may_have_committed() {
        return failure("mutation_outcome_unknown", budget);
    }
    let mut builder = Response::builder().status(output.status);
    for (name, value) in output.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(output.body))
        .unwrap_or_else(|_| failure("worker_failed", budget))
}

#[cfg(test)]
mod http_tests {
    use super::*;

    struct Fixture {
        state: Arc<AppState>,
        directory: PathBuf,
    }
    impl Fixture {
        fn new(timeout: Duration) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("functions-http-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&directory).unwrap();
            let dbs = ["a", "b"]
                .into_iter()
                .map(|app| (app.to_owned(), data::open(&directory, app).unwrap()))
                .collect();
            Self {
                state: Arc::new(AppState {
                    mode: "native".into(),
                    enabled: true,
                    timeout,
                    dbs,
                    cache: Mutex::new(cache::Cache::new(
                        1,
                        Duration::from_secs(60),
                        timeout,
                        directory.clone(),
                        worker::WorkerLimits::default(),
                    )),
                }),
                directory,
            }
        }
        async fn post(
            &self,
            session: Option<&str>,
            key: Option<&str>,
            body: &str,
        ) -> (StatusCode, Value) {
            let mut request = Request::builder().method("POST").uri("/a/orders/1");
            if let Some(session) = session {
                request = request.header("cookie", format!("session={session}"));
            }
            if let Some(key) = key {
                request = request.header("idempotency-key", key);
            }
            let answer = route(
                State(self.state.clone()),
                request.body(Body::from(body.to_owned())).unwrap(),
            )
            .await;
            let status = answer.status();
            let bytes = to_bytes(answer.into_body(), 16384).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap())
        }
        fn audit_count(&self) -> i64 {
            self.state.dbs["a"]
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[tokio::test]
    async fn authentication_precedes_key_and_body_validation() {
        let fixture = Fixture::new(Duration::from_secs(2));
        let (status, body) = fixture.post(None, None, "{").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"error":"unauthorized"}));
        let (status, body) = fixture
            .post(Some("a-alice"), None, r#"{"quantity":7}"#)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({"error":"idempotency_key_required"}));
        assert_eq!(fixture.audit_count(), 0);
    }

    #[tokio::test]
    async fn a_replayed_key_has_one_audit_and_changed_input_conflicts() {
        let fixture = Fixture::new(Duration::from_secs(2));
        for _ in 0..2 {
            let (status, body) = fixture
                .post(Some("a-alice"), Some("same-key"), r#"{"quantity":7}"#)
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({"id":1,"ownerId":"alice","quantity":7}));
        }
        let (status, body) = fixture
            .post(Some("a-alice"), Some("same-key"), r#"{"quantity":8}"#)
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, json!({"error":"idempotency_conflict"}));
        assert_eq!(fixture.audit_count(), 1);
    }

    #[tokio::test]
    async fn a_native_sqlite_lock_expires_without_a_late_mutation() {
        let fixture = Fixture::new(Duration::from_millis(40));
        let blocker = rusqlite::Connection::open(fixture.directory.join("a.sqlite")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (status, body) = fixture
            .post(Some("a-alice"), Some("locked-key"), r#"{"quantity":7}"#)
            .await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(body, json!({"error":"deadline_exceeded"}));
        blocker.execute_batch("ROLLBACK").unwrap();
        let check_budget = Budget::new(Duration::from_secs(1));
        let db = fixture.state.dbs["a"].clone();
        let order = tokio::task::spawn_blocking(move || data::read(&db, "alice", 1, &check_budget))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(order.quantity, 2);
        assert_eq!(fixture.audit_count(), 0);
    }

    #[tokio::test]
    async fn worker_resource_and_response_failures_keep_their_codes() {
        for code in ["worker_memory_limit", "invalid_response"] {
            let budget = Budget::new(Duration::from_secs(1));
            let answer = failure(code, &budget);
            assert_eq!(answer.status(), StatusCode::BAD_GATEWAY);
            let body = to_bytes(answer.into_body(), 16384).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"error":code})
            );
        }
    }

    #[tokio::test]
    async fn a_timeout_after_committing_reports_an_unknown_outcome() {
        let budget = Budget::new(Duration::from_secs(1));
        budget.begin_commit().unwrap();
        let answer = failure("deadline_exceeded", &budget);
        assert_eq!(answer.status(), StatusCode::CONFLICT);
        let body = to_bytes(answer.into_body(), 16384).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"error":"mutation_outcome_unknown"})
        );
        assert!(budget.check().is_err());
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::{Child, Command, Stdio},
        thread,
        time::Instant,
    };

    struct Server {
        process: Child,
        port: u16,
        directory: PathBuf,
    }
    impl Server {
        fn start() -> Self {
            let server = Self::configured("deno", true, 60000);
            assert_eq!(send(server.port, "/a/orders/1").0, 200);
            server
        }
        fn configured(mode: &str, enabled: bool, ttl_ms: u64) -> Self {
            let executable = std::env::var("KONAE_SPIKE_TEST_BINARY")
                .expect("Set KONAE_SPIKE_TEST_BINARY to the built server executable");
            let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = reservation.local_addr().unwrap().port();
            let directory =
                std::env::temp_dir().join(format!("functions-queue-{}-{port}", std::process::id()));
            std::fs::create_dir(&directory).unwrap();
            drop(reservation);
            let process = Command::new(executable)
                .env("MODE", mode)
                .env("PORT", port.to_string())
                .env("DATA_DIR", &directory)
                .env("FUNCTIONS_ENABLED", if enabled { "1" } else { "0" })
                .env("REQUEST_TIMEOUT_MS", "1000")
                .env("CACHE_TTL_MS", ttl_ms.to_string())
                .env("CACHE_CAPACITY", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut server = Self {
                process,
                port,
                directory,
            };
            let limit = Instant::now() + Duration::from_secs(5);
            loop {
                assert!(
                    server.process.try_wait().unwrap().is_none(),
                    "server exited before health"
                );
                if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    break;
                }
                assert!(Instant::now() < limit, "server did not listen");
                thread::sleep(Duration::from_millis(10));
            }
            server
        }
        fn stats(&self) -> Value {
            send(self.port, "/__stats").1
        }
        fn wait_stats(&self, predicate: impl Fn(&Value) -> bool) -> Value {
            let limit = Instant::now() + Duration::from_secs(3);
            loop {
                let stats = self.stats();
                if predicate(&stats) {
                    return stats;
                }
                assert!(Instant::now() < limit, "unexpected stats: {stats}");
                thread::sleep(Duration::from_millis(5));
            }
        }
        fn queued_post(&self) -> TcpStream {
            let mut socket = connect(self.port);
            write!(socket, "POST /a/orders/1 HTTP/1.1\r\nHost: localhost\r\nCookie: session=a-alice\r\nIdempotency-Key: never-run\r\nContent-Length: 14\r\nConnection: close\r\n\r\n").unwrap();
            socket.flush().unwrap();
            socket
        }
        fn assert_untouched(&self, stats: &Value) {
            assert_eq!(stats["initializationAttempts"], 1, "{stats}");
            assert_eq!(stats["runtimesCreated"], 1, "{stats}");
            assert_eq!(stats["runtimesLive"], 0, "{stats}");
            assert_eq!(
                stats["instances"][0]["requests"], 2,
                "only warmup and loop may execute"
            );
            let db = rusqlite::Connection::open(self.directory.join("a.sqlite")).unwrap();
            let quantity: i64 = db
                .query_row("SELECT quantity FROM orders WHERE id=1", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let audit: i64 = db
                .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
                .unwrap();
            let receipts: i64 = db
                .query_row("SELECT COUNT(*) FROM mutation_receipts", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!((quantity, audit, receipts), (2, 0, 0));
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn connect(port: u16) -> TcpStream {
        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
    }
    fn read_response(mut socket: TcpStream) -> (u16, Value) {
        let mut text = String::new();
        socket.read_to_string(&mut text).unwrap();
        let (headers, body) = text.split_once("\r\n\r\n").expect("HTTP response headers");
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap())
    }
    fn send(port: u16, path: &str) -> (u16, Value) {
        let mut socket = connect(port);
        write!(socket, "GET {path} HTTP/1.1\r\nHost: localhost\r\nCookie: session=a-alice\r\nConnection: close\r\n\r\n").unwrap();
        read_response(socket)
    }

    #[test]
    #[ignore = "Requires KONAE_SPIKE_TEST_BINARY after cargo build"]
    fn an_expired_queued_http_mutation_never_executes() {
        let server = Server::start();
        let mut queued = server.queued_post();
        thread::sleep(Duration::from_millis(600));
        let port = server.port;
        let occupied = thread::spawn(move || send(port, "/__control/loop"));
        server.wait_stats(|s| s["instances"][0]["requests"] == 2);
        queued.write_all(br#"{"quantity":7}"#).unwrap();
        server.wait_stats(|s| s["instances"][0]["busy"] == 2);
        assert_eq!(
            read_response(queued),
            (504, json!({"error":"deadline_exceeded"}))
        );
        assert_eq!(
            occupied.join().unwrap(),
            (504, json!({"error":"deadline_exceeded"}))
        );
        let stats =
            server.wait_stats(|s| s["discardedJobs"] == 1 && s["instances"][0]["busy"] == 0);
        server.assert_untouched(&stats);
    }

    #[test]
    #[ignore = "Requires KONAE_SPIKE_TEST_BINARY after cargo build"]
    fn a_disconnected_queued_http_mutation_never_executes() {
        let server = Server::start();
        let port = server.port;
        let occupied = thread::spawn(move || send(port, "/__control/loop"));
        server.wait_stats(|s| s["instances"][0]["requests"] == 2);
        let mut queued = server.queued_post();
        queued.write_all(br#"{"quantity":7}"#).unwrap();
        server.wait_stats(|s| s["instances"][0]["busy"] == 2);
        drop(queued);
        assert_eq!(
            occupied.join().unwrap(),
            (504, json!({"error":"deadline_exceeded"}))
        );
        let stats =
            server.wait_stats(|s| s["discardedJobs"] == 1 && s["instances"][0]["busy"] == 0);
        server.assert_untouched(&stats);
    }

    fn worker_pids(server: &Server) -> Vec<i32> {
        let output = Command::new("pgrep")
            .args(["-P", &server.process.id().to_string()])
            .output()
            .unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect()
    }
    fn assert_reaped(pid: i32) {
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "worker {pid} still exists"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    fn control(server: &Server, path: &str, body: &str) -> (u16, Value) {
        let mut socket = connect(server.port);
        write!(socket, "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        read_response(socket)
    }

    #[test]
    #[ignore = "Requires KONAE_SPIKE_TEST_BINARY after cargo build"]
    fn native_and_disabled_functions_never_spawn_a_worker() {
        for (mode, enabled, expected) in [("native", true, 200), ("deno", false, 404)] {
            let server = Server::configured(mode, enabled, 60000);
            assert_eq!(send(server.port, "/a/orders/1").0, expected);
            let stats = server.stats();
            assert_eq!(stats["initializationAttempts"], 0);
            assert_eq!(stats["engineInitialized"], false);
            assert_eq!(stats["runtimesCreated"], 0);
            assert!(worker_pids(&server).is_empty());
        }
    }

    #[test]
    #[ignore = "Requires KONAE_SPIKE_TEST_BINARY after cargo build"]
    fn version_and_eviction_return_after_the_worker_is_reaped() {
        let server = Server::start();
        let pids = worker_pids(&server);
        assert_eq!(pids.len(), 1);
        assert_eq!(
            control(
                &server,
                "/__control/version",
                r#"{"app":"a","function":"orders","version":"v2"}"#
            )
            .0,
            200
        );
        assert_reaped(pids[0]);
        assert_eq!(server.stats()["runtimesLive"], 0);
        assert_eq!(send(server.port, "/a/orders/1").0, 200);
        let next = worker_pids(&server);
        assert_eq!(next.len(), 1);
        assert_ne!(next[0], pids[0]);
        assert_eq!(
            control(&server, "/__control/evict", ""),
            (200, json!({"evicted":1}))
        );
        assert_reaped(next[0]);
        assert_eq!(server.stats()["runtimesCreated"], 2);
    }

    #[test]
    #[ignore = "Requires KONAE_SPIKE_TEST_BINARY after cargo build"]
    fn ttl_removes_the_worker_process() {
        let server = Server::configured("deno", true, 500);
        assert_eq!(send(server.port, "/a/orders/1").0, 200);
        let pids = worker_pids(&server);
        assert_eq!(pids.len(), 1);
        server.wait_stats(|s| s["evictions"] == 1 && s["runtimesLive"] == 0);
        assert_reaped(pids[0]);
    }
}
