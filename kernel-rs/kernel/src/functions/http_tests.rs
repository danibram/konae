use super::*;
use crate::{
    functions::runtime::Runtime,
    functions::tests::{publish, release},
};
use konae_functions_executor::MemoryPolicy;
use rusqlite::Connection;
use std::convert::Infallible;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const HANDLER: &str = r#"
let calls=0;
export async function handle(request, context) {
  calls++;
  const path = new URL(request.url).pathname;
  if(path.endsWith('/spin')) { while(true) {} }
  if(path.endsWith('/pressure')) { const bytes=new Uint8Array(320*1024*1024).fill(7); return new Response(String(bytes[0])); }
  if(path.endsWith('/stream-check')) {
    const reader=request.body.getReader(); let count=0; let checksum=0;
    for(;;) { const {done,value}=await reader.read(); if(done) break; count+=value.byteLength; for(const byte of value) checksum+=byte; }
    return Response.json({count,checksum});
  }
  if(path.endsWith('/large-json')) { const value=await request.json(); return Response.json({paddingLength:value.padding.length}); }
  if(path.endsWith('/large-replay')) { const value=await request.json(); await context.actions.call('orders.update',{id:1,quantity:value.quantity}); await context.actions.call('audit.add',{quantity:value.quantity}); return Response.json({paddingLength:value.padding.length}); }
  if(path.endsWith('/early-action')) { await context.actions.call('orders.update',{id:1,quantity:99}); return Response.json({wrong:true}); }
  if(path.endsWith('/early-catch')) { try { await context.actions.call('orders.update',{id:1,quantity:99}); } catch {} return Response.json({wrong:true}); }
  if(path.endsWith('/catch')) { try { await context.actions.call('not.allowed', {}); } catch {} return Response.json({wrong:true}); }
  if(request.method === 'POST') {
    const input=await request.json();
    await context.actions.call('orders.update', {id:input.id, quantity:input.quantity});
    if(input.fail) throw Error('reject after write');
    await context.actions.call('audit.add',{quantity:input.quantity});
    if(input.disconnect) { while(true) {} }
    if(input.wait) {
      const until = Date.now()+200;
      while(Date.now()<until) {}
    }
  }
  const rows=await context.actions.call('orders.read',{});
  return Response.json({version:'A',calls,rows,cookie:request.headers.has('cookie'),authorization:request.headers.has('authorization')});
}
"#;
const STREAM_HANDLER: &str = r#"
export async function handle(request) {
  const reader=request.body.getReader(); let count=0; let checksum=0;
  for(;;) { const {done,value}=await reader.read(); if(done) break; count+=value.byteLength; for(const byte of value) checksum=(checksum+byte)>>>0; }
  return Response.json({count,checksum});
}
"#;

fn synthetic_body(
    length: usize,
) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, Infallible>> {
    futures_util::stream::unfold((0usize, length), |(offset, length)| async move {
        if offset == length {
            return None;
        }
        let count = (length - offset).min(konae_functions_executor::protocol::BODY_CHUNK_BYTES);
        Some((
            Ok(axum::body::Bytes::from(vec![b'x'; count])),
            (offset + count, length),
        ))
    })
}

async fn send_chunked_http(
    address: std::net::SocketAddr,
    host: &str,
    path: &str,
    length: usize,
) -> (u16, Value) {
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut sent = 0usize;
    let mut connected = true;
    while sent < length {
        let size = (length - sent).min(64 * 1024);
        if socket
            .write_all(format!("{size:x}\r\n").as_bytes())
            .await
            .is_err()
        {
            connected = false;
            break;
        }
        let chunk = vec![b'x'; size];
        if socket.write_all(&chunk).await.is_err() || socket.write_all(b"\r\n").await.is_err() {
            connected = false;
            break;
        }
        sent += size;
    }
    if connected {
        let _ = socket.write_all(b"0\r\n\r\n").await;
    }
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let boundary = response
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap_or_else(|| {
            panic!(
                "invalid HTTP response: {}",
                String::from_utf8_lossy(&response[..response.len().min(256)])
            )
        })
        + 4;
    let headers = std::str::from_utf8(&response[..boundary]).unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = &response[boundary..];
    (status, serde_json::from_slice(body).unwrap())
}

async fn start_partial_chunked_http(
    address: std::net::SocketAddr,
    host: &str,
    path: String,
) -> tokio::net::TcpStream {
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10000\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(&vec![b'z'; 64 * 1024]).await.unwrap();
    socket.write_all(b"\r\n").await.unwrap();
    socket
}

fn function_process_rss() -> (usize, usize) {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss=,command="])
        .output()
        .unwrap();
    let own = std::process::id().to_string();
    let mut kernel_rss_kib = 0usize;
    let mut worker_rss_kib = 0usize;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 {
            continue;
        }
        if fields[0] == own {
            kernel_rss_kib = fields[2].parse().unwrap_or_default();
        } else if fields[1] == own && line.contains("--function-worker") {
            worker_rss_kib += fields[2].parse::<usize>().unwrap_or_default();
        }
    }
    (kernel_rss_kib, worker_rss_kib)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_body_pump_keeps_a_small_queue_and_enforces_real_byte_eof() {
    let _serial = BODY_PUMP_TEST_LOCK.lock().await;
    let budget = Budget::new(Duration::from_secs(60));
    let (body, pump) = start_body_pump(Body::from_stream(synthetic_body(20 * 1024 * 1024)), budget);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(body.receiver.lock().unwrap().len() <= 2);
    assert!(
        body.state.lock().unwrap().length
            <= 3 * konae_functions_executor::protocol::BODY_CHUNK_BYTES
    );
    let drained = body.clone();
    tokio::task::spawn_blocking(move || drained.drain())
        .await
        .unwrap()
        .unwrap();
    assert!(body.complete());
    assert_eq!(body.summary().unwrap().0, 20 * 1024 * 1024);
    drop(pump);

    let budget = Budget::new(Duration::from_secs(60));
    let (body, pump) = start_body_pump(
        Body::from_stream(synthetic_body(MAX_FUNCTION_BODY_BYTES)),
        budget,
    );
    let drained = body.clone();
    tokio::task::spawn_blocking(move || drained.drain())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.summary().unwrap().0, 100_000_000);
    drop(pump);

    let budget = Budget::new(Duration::from_secs(60));
    let (body, pump) = start_body_pump(
        Body::from_stream(synthetic_body(MAX_FUNCTION_BODY_BYTES + 1)),
        budget,
    );
    let drained = body.clone();
    assert_eq!(
        tokio::task::spawn_blocking(move || drained.drain())
            .await
            .unwrap()
            .unwrap_err(),
        "body_too_large"
    );
    assert!(!body.complete());
    drop(pump);

    let (length, _, legacy, permits) = read_replay_body(
        Body::from_stream(synthetic_body(MAX_LEGACY_FUNCTION_BODY_BYTES)),
        true,
        &Budget::new(Duration::from_secs(5)),
    )
    .await
    .unwrap();
    assert_eq!(length, MAX_LEGACY_FUNCTION_BODY_BYTES);
    assert!(legacy.is_some());
    drop(permits);
    let (length, _, legacy, permits) = read_replay_body(
        Body::from_stream(synthetic_body(MAX_LEGACY_FUNCTION_BODY_BYTES + 1)),
        true,
        &Budget::new(Duration::from_secs(5)),
    )
    .await
    .unwrap();
    assert_eq!(length, MAX_LEGACY_FUNCTION_BODY_BYTES + 1);
    assert!(legacy.is_none());
    assert!(permits.is_empty());

    let mut slow_bodies = Vec::new();
    let mut slow_pumps = Vec::new();
    for _ in 0..3 {
        let stream = futures_util::stream::repeat_with(|| {
            Ok::<_, Infallible>(axum::body::Bytes::from(vec![b'z'; 64 * 1024]))
        });
        let (body, pump) = start_body_pump(
            Body::from_stream(stream),
            Budget::new(Duration::from_secs(60)),
        );
        slow_bodies.push(body);
        slow_pumps.push(pump);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    for body in &slow_bodies {
        assert!(body.receiver.lock().unwrap().len() <= 2);
        assert!(body.state.lock().unwrap().length <= 3 * 64 * 1024);
    }
    let (small, small_pump) =
        start_body_pump(Body::from("small"), Budget::new(Duration::from_secs(2)));
    let drained = small.clone();
    tokio::task::spawn_blocking(move || drained.drain())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(small.summary().unwrap().0, 5);
    drop(small_pump);
    for pump in &mut slow_pumps {
        pump.cancel().await;
    }
    drop(slow_pumps);
    drop(slow_bodies);
    tokio::task::yield_now().await;
    assert_eq!(
        function_body_budget().available_permits(),
        MAX_AGGREGATE_PENDING_BODY_BYTES
    );

    let stalled = futures_util::stream::once(async {
        Ok::<_, Infallible>(axum::body::Bytes::from_static(b"partial"))
    })
    .chain(futures_util::stream::pending());
    let (body, pump) = start_body_pump(
        Body::from_stream(stalled),
        Budget::new(Duration::from_millis(30)),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut pump = pump;
    pump.cancel().await;
    assert_eq!(body.state.lock().unwrap().error, Some("deadline_exceeded"));
    drop(body);
    tokio::task::yield_now().await;
    assert_eq!(
        function_body_budget().available_permits(),
        MAX_AGGREGATE_PENDING_BODY_BYTES
    );
}

#[tokio::test]
async fn receipt_prelookup_refuses_matching_v2_and_v3_without_returning_old_bodies() {
    let root =
        std::env::temp_dir().join(format!("konae-function-prelookup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("database.sqlite");
    let db = Connection::open(&database).unwrap();
    db.execute_batch(
        "CREATE TABLE __konae_function_receipts (actor_scope TEXT NOT NULL, function_name TEXT NOT NULL, idempotency_key TEXT NOT NULL, fingerprint TEXT NOT NULL, response_json TEXT NOT NULL, PRIMARY KEY(actor_scope,function_name,idempotency_key)) WITHOUT ROWID;
         CREATE TABLE orders(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL);
         CREATE TABLE audit(id INTEGER PRIMARY KEY);
         INSERT INTO orders VALUES(1,'bob');
         INSERT INTO audit VALUES(1);",
    )
    .unwrap();
    let actor_scope = serde_json::to_string(&Some("alice")).unwrap();
    let body = br#"{"id":1}"#;
    let path = "/orders/1?view=short";
    let headers = vec![
        ("accept".into(), "application/json".into()),
        ("content-type".into(), "application/json".into()),
    ];
    let v2 = function_data::request_fingerprint("POST", path, body, &headers).unwrap();
    let body_digest: [u8; 32] = Sha256::digest(body).into();
    let v3 =
        function_data::request_fingerprint_v3("POST", path, body.len(), &body_digest, &headers)
            .unwrap();
    let historical = serde_json::to_string(&Output {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: r#"{"secret":"historical"}"#.into(),
    })
    .unwrap();
    for (key, fingerprint) in [("v2", &v2), ("v3", &v3)] {
        db.execute(
            "INSERT INTO __konae_function_receipts VALUES(?1,'orders',?2,?3,?4)",
            rusqlite::params![actor_scope, key, fingerprint, historical],
        )
        .unwrap();
    }
    drop(db);

    for key in ["v2", "v3"] {
        let result = prelookup_receipt(
            &database,
            Some("alice"),
            "orders",
            key,
            "POST",
            path,
            &headers,
            Body::from(body.to_vec()),
            &Budget::new(Duration::from_secs(5)),
            Instant::now(),
        )
        .await
        .unwrap();
        let ReceiptPrelookup::Replay(timed) = result else {
            panic!("expected a matching receipt");
        };
        assert!(matches!(timed.outcome, FunctionOutcome::Replayed));
        assert!(matches!(
            outcome(function_data::IDEMPOTENCY_REPLAY_CODE),
            FunctionOutcome::Replayed
        ));
        assert_eq!(timed.response.status(), StatusCode::CONFLICT);
        let value: Value = serde_json::from_slice(
            &axum::body::to_bytes(timed.response.into_body(), 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["code"], function_data::IDEMPOTENCY_REPLAY_CODE);
        assert!(!value.to_string().contains("historical"));
    }

    let continued = prelookup_receipt(
        &database,
        Some("alice"),
        "orders",
        "missing",
        "POST",
        path,
        &headers,
        Body::from(body.to_vec()),
        &Budget::new(Duration::from_secs(5)),
        Instant::now(),
    )
    .await
    .unwrap();
    let ReceiptPrelookup::Continue(stream) = continued else {
        panic!("a new key must continue to the worker");
    };
    assert_eq!(
        axum::body::to_bytes(stream, 1024).await.unwrap().as_ref(),
        body
    );

    let oversized_legacy = match prelookup_receipt(
        &database,
        Some("alice"),
        "orders",
        "v2",
        "POST",
        path,
        &headers,
        Body::from(vec![b'x'; MAX_LEGACY_FUNCTION_BODY_BYTES + 1]),
        &Budget::new(Duration::from_secs(5)),
        Instant::now(),
    )
    .await
    {
        Err(response) => response,
        Ok(_) => panic!("an oversized legacy body must conflict"),
    };
    assert_eq!(oversized_legacy.status(), StatusCode::CONFLICT);
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(oversized_legacy.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["code"], "idempotency_conflict");

    let changed = match prelookup_receipt(
        &database,
        Some("alice"),
        "orders",
        "v3",
        "POST",
        path,
        &headers,
        Body::from(br#"{"id":2}"#.to_vec()),
        &Budget::new(Duration::from_secs(5)),
        Instant::now(),
    )
    .await
    {
        Err(response) => response,
        Ok(_) => panic!("a different request must conflict"),
    };
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(changed.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["code"], "idempotency_conflict");

    let db = Connection::open(&database).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM orders WHERE owner_id='alice' LIMIT 10",
            [],
            |row| { row.get::<_, i64>(0) }
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM audit LIMIT 1", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM __konae_function_receipts LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

static BODY_PUMP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires KONAE_TEST_FUNCTION_WORKER pointing to the built external JavaScript worker"]
async fn real_http_stream_accepts_100mb_and_refuses_the_next_byte_without_content_length() {
    let executable =
        std::env::var_os("KONAE_TEST_FUNCTION_WORKER").expect("KONAE_TEST_FUNCTION_WORKER");
    let policy = std::env::var_os("KONAE_TEST_FUNCTION_CGROUP")
        .map(|root| MemoryPolicy::LinuxCgroup {
            delegated_root: root.into(),
        })
        .unwrap_or(MemoryPolicy::DevelopmentPolling);
    let root = std::env::temp_dir().join(format!("konae-function-100mb-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::for_test(&root);
    state.functions = Arc::new(Runtime::configured(
        executable.into(),
        policy,
        Duration::from_secs(60),
        Duration::from_secs(1),
    ));
    let id = "http_functions_100mb";
    let mut payload = release(id, STREAM_HANDLER);
    payload["initialModules"] = json!({"functions":{"on":true}});
    payload["serverArtifact"]["manifest"]["functions"]["orders"]["access"] = json!("anonymous");
    payload["serverArtifact"]["manifest"]["functions"]["orders"]["actions"] = json!([]);
    let mut manifest = payload["serverArtifact"]["manifest"].clone();
    let mut bundles = payload["serverArtifact"]["bundles"].clone();
    for name in ["slow1", "slow2", "slow3"] {
        let mut function = manifest["functions"]["orders"].clone();
        function["entry"] = json!(format!("functions/{name}.ts"));
        function["mount"] = json!(format!("/api/{name}"));
        manifest["functions"][name] = function;
        bundles[name] = json!(STREAM_HANDLER);
        payload["project"]["files"][format!("functions/{name}.ts")] = json!(STREAM_HANDLER);
    }
    payload["serverArtifact"]["manifest"] = manifest.clone();
    payload["serverArtifact"]["bundles"] = bundles;
    payload["project"]["files"]["functions.json"] = json!(manifest.to_string());
    let deployed = publish(&state, &payload).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(crate::serve(listener, state.clone(), axum::Router::new()));
    let send = |length, declared: Option<&str>| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/orders")
            .header("host", &deployed.hostname);
        if let Some(declared) = declared {
            request = request.header(header::CONTENT_LENGTH, declared);
        }
        request
            .body(Body::from_stream(synthetic_body(length)))
            .unwrap()
    };
    let utf8 = Request::builder()
        .method("POST")
        .uri("/api/orders")
        .header("host", &deployed.hostname)
        .body(Body::from("é"))
        .unwrap();
    let utf8_response = crate::stack(state.clone(), axum::Router::new())
        .oneshot(utf8)
        .await
        .unwrap();
    let utf8_bytes = axum::body::to_bytes(utf8_response.into_body(), 1024)
        .await
        .unwrap();
    let utf8_value: Value = serde_json::from_slice(&utf8_bytes).unwrap();
    assert_eq!(utf8_value["count"], 2);
    assert_eq!(utf8_value["checksum"], 364);
    let (chunked_status, chunked_value) =
        send_chunked_http(address, &deployed.hostname, "/api/orders", 2).await;
    assert_eq!(chunked_status, 200);
    assert_eq!(chunked_value["count"], 2);
    assert_eq!(chunked_value["checksum"], 240);
    let (status, value) =
        send_chunked_http(address, &deployed.hostname, "/api/orders", 100_000_000).await;
    assert_eq!(status, 200);
    assert_eq!(value["count"], 100_000_000);
    assert_eq!(value["checksum"], 3_410_065_408u64);
    let (status, value) =
        send_chunked_http(address, &deployed.hostname, "/api/orders", 100_000_001).await;
    assert_eq!(status, 413);
    assert_eq!(
        value["error"],
        "The function request exceeds 100000000 bytes."
    );
    let response = crate::stack(state.clone(), axum::Router::new())
        .oneshot(send(100_000_001, Some("1")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["error"],
        "The function request exceeds 100000000 bytes."
    );

    let stalled = futures_util::future::join_all(["slow1", "slow2", "slow3"].map(|name| {
        start_partial_chunked_http(address, &deployed.hostname, format!("/api/{name}"))
    }))
    .await;
    let small_host = deployed.hostname.clone();
    let small =
        tokio::spawn(
            async move { send_chunked_http(address, &small_host, "/api/orders", 2).await },
        );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, workers_kib) = function_process_rss();
            if workers_kib > 0
                && std::process::Command::new("ps")
                    .args(["-axo", "ppid=,command="])
                    .output()
                    .ok()
                    .map(|output| {
                        String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .filter(|line| {
                                let fields = line.split_whitespace().collect::<Vec<_>>();
                                fields
                                    .first()
                                    .is_some_and(|ppid| ppid == &std::process::id().to_string())
                                    && line.contains("--function-worker")
                            })
                            .count()
                    })
                    .unwrap_or_default()
                    >= 4
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("three partial uploads and the small-request worker should be running");
    let (kernel_rss_kib, workers_rss_kib) = function_process_rss();
    assert!(kernel_rss_kib > 0);
    assert!(workers_rss_kib > 0);
    eprintln!(
        "three_partial_uploads_rss_kib kernel={kernel_rss_kib} workers={workers_rss_kib}; each upload has one 64KiB chunk pending"
    );
    let (small_status, small_value) = tokio::time::timeout(Duration::from_secs(3), small)
        .await
        .expect("small request should pass three stalled uploads")
        .unwrap();
    assert_eq!(small_status, 200);
    assert_eq!(small_value["count"], 2);
    drop(stalled);
    let (recovered_status, recovered_value) = tokio::time::timeout(
        Duration::from_secs(3),
        send_chunked_http(address, &deployed.hostname, "/api/orders", 2),
    )
    .await
    .expect("small request should pass after upload cancellation");
    assert_eq!(recovered_status, 200);
    assert_eq!(recovered_value["count"], 2);
    serving.abort();
    let _ = serving.await;
    crate::usage::flush();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn missing_worker_isolation_is_a_clear_service_refusal() {
    let response = super::executor_failure("sandbox_unavailable");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["code"], "sandbox_unavailable");
    assert!(value["error"].as_str().unwrap().contains("OS isolation"));
}

#[tokio::test]
async fn a_join_failure_reports_unknown_if_a_mutation_may_have_committed() {
    for (committed, expected) in [
        (false, StatusCode::INTERNAL_SERVER_ERROR),
        (true, StatusCode::CONFLICT),
    ] {
        let started = Instant::now();
        let budget = Budget::new(Duration::from_secs(1));
        let worker_budget = budget.clone();
        let join = tokio::task::spawn_blocking(move || {
            if committed {
                worker_budget.begin_commit().unwrap();
                worker_budget.committed();
            }
            panic!("fixture worker panic");
        })
        .await;
        assert!(join.unwrap_err().is_panic());
        let timed = interrupted(
            &budget,
            "function.executor-failed",
            started,
            &RunProgress::default(),
        );
        assert_eq!(timed.response.status(), expected);
        let body = axum::body::to_bytes(timed.response.into_body(), 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["code"],
            if committed {
                "mutation_outcome_unknown"
            } else {
                "function.executor-failed"
            }
        );
    }
}

#[tokio::test]
async fn admission_includes_requests_waiting_for_publication_and_releases_on_drop() {
    let from = crate::usage::day(crate::now_ms());
    let root = std::env::temp_dir().join(format!("konae-function-queue-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::for_test(&root);
    state.functions = Arc::new(Runtime::configured(
        "/nonexistent/worker".into(),
        MemoryPolicy::DevelopmentPolling,
        Duration::from_secs(2),
        Duration::from_secs(1),
    ));
    let id = "function_queue";
    let deployed = publish(&state, &release(id, "export function handle() {}")).await;
    let lock = crate::named_lock(&state, &format!("publish:{id}"), true).await;
    let reservations = (0..63)
        .map(|_| {
            state
                .functions
                .admit(&Budget::new(Duration::from_secs(2)))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let request = || {
        Request::builder()
            .uri("/api/orders")
            .header("host", &deployed.hostname)
            .body(Body::empty())
            .unwrap()
    };
    let service = crate::stack(state.clone(), axum::Router::new());
    let mut waiting = Box::pin(service.clone().oneshot(request()));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );
    let refused = tokio::time::timeout(Duration::from_millis(250), service.oneshot(request()))
        .await
        .unwrap()
        .unwrap();
    let body = axum::body::to_bytes(refused.into_body(), 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["code"], "queue_full");
    drop(waiting);
    assert!(
        state
            .functions
            .admit(&Budget::new(Duration::from_secs(2)))
            .is_ok()
    );
    drop(reservations);
    let mut waiting = Box::pin(crate::stack(state.clone(), axum::Router::new()).oneshot(request()));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );
    crate::maintenance::pause_for_test(&root, id, true);
    drop(lock);
    let response = waiting.await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "300");
    crate::usage::flush();
    let to = crate::usage::day(crate::now_ms());
    let usage = crate::usage::read_usage(&crate::layout::usage(&root, id), &from, &to).unwrap();
    assert_eq!(
        usage["totals"],
        json!([{"kind": "function", "name": "orders", "count": 2}])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires KONAE_TEST_FUNCTION_WORKER pointing to the built external JavaScript worker"]
async fn completed_function_calls_have_one_daily_count() {
    let executable = std::env::var_os("KONAE_TEST_FUNCTION_WORKER").unwrap();
    let root = std::env::temp_dir().join(format!("konae-function-usage-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::for_test(&root);
    let policy = std::env::var_os("KONAE_TEST_FUNCTION_CGROUP")
        .map(|root| MemoryPolicy::LinuxCgroup {
            delegated_root: root.into(),
        })
        .unwrap_or(MemoryPolicy::DevelopmentPolling);
    state.functions = Arc::new(Runtime::configured(
        executable.into(),
        policy,
        Duration::from_secs(5),
        Duration::from_secs(1),
    ));
    let deployment = publish(
        &state,
        &release(
            "function_usage",
            "export function handle() { return new Response('ok'); }",
        ),
    )
    .await;
    let service = crate::stack(state, axum::Router::new());
    let from = crate::usage::day(crate::now_ms());
    for (method, status) in [
        ("GET", StatusCode::OK),
        ("GET", StatusCode::OK),
        ("PUT", StatusCode::METHOD_NOT_ALLOWED),
    ] {
        let request = Request::builder()
            .method(method)
            .uri("/api/orders")
            .header("host", &deployment.hostname)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            service.clone().oneshot(request).await.unwrap().status(),
            status
        );
    }
    let to = crate::usage::day(crate::now_ms());
    let summary = crate::usage::admin_call(
        &root,
        &deployment,
        "usage_summary",
        &json!({"from": from, "to": to}),
    )
    .await
    .unwrap();
    let calls = summary["usage"]["days"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "function" && row["name"] == "orders")
        .map(|row| row["count"].as_u64().unwrap())
        .sum::<u64>();
    assert_eq!(calls, 3, "{summary}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires KONAE_TEST_FUNCTION_WORKER pointing to the built external JavaScript worker"]
async fn real_functions_http_auth_transaction_publish_deadline_and_usage() {
    const DEADLINE_MS: u64 = 3500;
    let executable =
        std::env::var_os("KONAE_TEST_FUNCTION_WORKER").expect("KONAE_TEST_FUNCTION_WORKER");
    if let Some(root) = std::env::var_os("KONAE_TEST_FUNCTION_CGROUP") {
        konae_functions_executor::validate_linux_worker_containment(&PathBuf::from(root)).unwrap();
    }
    let root = std::env::temp_dir().join(format!("konae-function-http-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::for_test(&root);
    let policy = std::env::var_os("KONAE_TEST_FUNCTION_CGROUP")
        .map(|root| MemoryPolicy::LinuxCgroup {
            delegated_root: root.into(),
        })
        .unwrap_or(MemoryPolicy::DevelopmentPolling);
    state.functions = Arc::new(Runtime::configured(
        executable.into(),
        policy,
        Duration::from_millis(DEADLINE_MS),
        Duration::from_millis(500),
    ));
    let id = "http_functions";
    let mut payload = release(id, HANDLER);
    payload["initialModules"] =
        json!({"functions":{"on":true},"auth":{"on":true},"actions":{"on":true}});
    payload["initialSignupPolicy"] = json!({"mode":"open","allowlist":[]});
    payload["project"]["files"]["schema.sql"] = json!(
        "CREATE TABLE IF NOT EXISTS orders(id INTEGER PRIMARY KEY,owner_id TEXT NOT NULL,quantity INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY,actor TEXT NOT NULL,quantity INTEGER NOT NULL);"
    );
    let actions = json!({"version":1,"actions":{
      "orders.read":{"kind":"query","access":"authenticated","statement":"SELECT id,quantity FROM orders WHERE owner_id=?1 LIMIT 10","bindings":[{"source":"actor","field":"id"}]},
      "orders.update":{"kind":"mutation","access":"authenticated","statement":"UPDATE orders SET quantity=?1 WHERE id=?2 AND owner_id=?3","guard":"Only the order owner can change it.","bindings":[{"source":"input","key":"quantity","type":"number"},{"source":"input","key":"id","type":"number"},{"source":"actor","field":"id"}]},
      "audit.add":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO audit(actor,quantity) VALUES(?1,?2)","bindings":[{"source":"actor","field":"id"},{"source":"input","key":"quantity","type":"number"}]}
    }});
    payload["project"]["files"]["actions.json"] = json!(actions.to_string());
    payload["serverArtifact"]["manifest"]["functions"]["orders"]["access"] = json!("authenticated");
    payload["serverArtifact"]["manifest"]["functions"]["orders"]["actions"] =
        json!(["audit.add", "orders.read", "orders.update"]);
    payload["project"]["files"]["functions.json"] =
        json!(payload["serverArtifact"]["manifest"].to_string());
    let deployed = publish(&state, &payload).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(crate::serve(listener, state.clone(), axum::Router::new()));
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/orders");
    let mut accounts = Vec::new();
    for email in ["alice@example.test", "bob@example.test"] {
        let response = client
            .post(format!("http://{address}/_konae/auth/signup"))
            .header("host", &deployed.hostname)
            .json(&json!({"email":email,"password":"correct horse battery"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let value: Value = response.json().await.unwrap();
        accounts.push((
            cookie,
            value["result"]["user"]["userId"]
                .as_str()
                .unwrap()
                .to_owned(),
        ));
    }
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    db.execute(
        "INSERT INTO orders VALUES(1,?1,2),(2,?2,4)",
        rusqlite::params![accounts[0].1, accounts[1].1],
    )
    .unwrap();
    drop(db);
    assert_eq!(
        client
            .get(&url)
            .header("host", &deployed.hostname)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let get = |suffix: &str, cookie: &str| {
        client
            .get(format!("{url}{suffix}"))
            .header("host", &deployed.hostname)
            .header("cookie", cookie)
    };
    let first = get("", &accounts[0].0).send().await.unwrap();
    let status = first.status();
    let first: Value = first.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["version"], "A");
    assert_eq!(first["calls"], 1);
    assert_eq!(first["rows"][0]["id"], 1);
    assert_eq!(first["cookie"], false);
    assert_eq!(first["authorization"], false);
    let second: Value = get("", &accounts[1].0)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second["calls"], 1);
    assert_eq!(second["rows"][0]["id"], 2);
    let post = |key: &str, input: Value| {
        client
            .post(&url)
            .header("host", &deployed.hostname)
            .header("cookie", &accounts[0].0)
            .header("idempotency-key", key)
            .header(header::ACCEPT, "application/json")
            .json(&input)
    };
    let large_json: Value = client
        .post(format!("{url}/large-json"))
        .header("host", &deployed.hostname)
        .header("cookie", &accounts[0].0)
        .header("idempotency-key", "large-json")
        .header(header::ACCEPT, "application/json")
        .json(&json!({"padding":"x".repeat(70_000)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(large_json["paddingLength"], 70_000);
    let large_request = |key: &str, padding: String| {
        client
            .post(format!("{url}/large-replay"))
            .header("host", &deployed.hostname)
            .header("cookie", &accounts[0].0)
            .header("idempotency-key", key)
            .header(header::ACCEPT, "application/json")
            .json(&json!({"quantity":7,"padding":padding}))
    };
    let large_first: Value = large_request("large-replay", "x".repeat(70_000))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let large_replay = large_request("large-replay", "x".repeat(70_000))
        .send()
        .await
        .unwrap();
    assert_eq!(large_replay.status(), StatusCode::CONFLICT);
    let large_replay: Value = large_replay.json().await.unwrap();
    assert_eq!(large_replay["code"], function_data::IDEMPOTENCY_REPLAY_CODE);
    assert_eq!(large_first["paddingLength"], 70_000, "{large_first}");
    let (left, right) = tokio::join!(
        large_request("large-concurrent", "z".repeat(70_000)).send(),
        large_request("large-concurrent", "z".repeat(70_000)).send(),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.status(), right.status());
    let (success, replay) = if left.status() == StatusCode::OK {
        (left, right)
    } else {
        (right, left)
    };
    assert_eq!(success.status(), StatusCode::OK);
    let replay_status = replay.status();
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay_status, StatusCode::CONFLICT, "{replay}");
    assert_eq!(replay["code"], function_data::IDEMPOTENCY_REPLAY_CODE);
    let changed_body = large_request("large-replay", "y".repeat(70_000))
        .send()
        .await
        .unwrap();
    assert_eq!(changed_body.status(), StatusCode::CONFLICT);
    assert_eq!(
        changed_body.json::<Value>().await.unwrap()["code"],
        "idempotency_conflict"
    );
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(db);
    let changed = json!({"id":1,"quantity":7});
    let first = post("same", changed.clone()).send().await.unwrap();
    let status = first.status();
    let first: Value = first.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{first}");
    Connection::open(crate::layout::database(&root, id))
        .unwrap()
        .execute("UPDATE orders SET owner_id=?1 WHERE id=1", [&accounts[1].1])
        .unwrap();
    let replay = post("same", changed.clone()).send().await.unwrap();
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["code"], function_data::IDEMPOTENCY_REPLAY_CODE);
    assert!(
        replay["error"]
            .as_str()
            .unwrap()
            .contains("authorized read")
    );
    let current: Value = get("", &accounts[0].0)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(current["rows"], json!([]));
    assert_eq!(
        Connection::open(crate::layout::database(&root, id))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    let legacy_fingerprint = function_data::request_fingerprint(
        "POST",
        "/api/orders",
        &serde_json::to_vec(&changed).unwrap(),
        &[
            ("accept".into(), "application/json".into()),
            ("content-type".into(), "application/json".into()),
        ],
    )
    .unwrap();
    Connection::open(crate::layout::database(&root, id))
        .unwrap()
        .execute(
            "UPDATE __konae_function_receipts SET fingerprint=?1 WHERE idempotency_key='same'",
            [legacy_fingerprint],
        )
        .unwrap();
    let old_replay = post("same", changed.clone()).send().await.unwrap();
    assert_eq!(old_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        old_replay.json::<Value>().await.unwrap()["code"],
        function_data::IDEMPOTENCY_REPLAY_CODE
    );
    Connection::open(crate::layout::database(&root, id))
        .unwrap()
        .execute("UPDATE orders SET owner_id=?1 WHERE id=1", [&accounts[0].1])
        .unwrap();
    let early_action = |suffix: &str, key: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/orders/{suffix}"))
            .header("host", &deployed.hostname)
            .header("cookie", &accounts[0].0)
            .header("idempotency-key", key)
            .header("content-type", "application/json")
            .body(Body::from_stream(synthetic_body(5 * 1024 * 1024)))
            .unwrap()
    };
    for (suffix, key) in [("early-action", "early"), ("early-catch", "early-catch")] {
        let refused = crate::stack(state.clone(), axum::Router::new())
            .oneshot(early_action(suffix, key))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(refused.into_body(), 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], "function.request-body-incomplete");
    }
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    assert_eq!(
        db.query_row("SELECT quantity FROM orders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(db);
    let oversized = Request::builder()
        .method("POST")
        .uri("/api/orders/early-catch")
        .header("host", &deployed.hostname)
        .header("cookie", &accounts[0].0)
        .header("idempotency-key", "oversized-no-write")
        .header("content-type", "application/json")
        .body(Body::from_stream(synthetic_body(100_000_001)))
        .unwrap();
    let response = crate::stack(state.clone(), axum::Router::new())
        .oneshot(oversized)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["error"],
        "The function request exceeds 100000000 bytes."
    );
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    assert_eq!(
        db.query_row("SELECT quantity FROM orders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(db);
    assert_eq!(
        post("same", json!({"id":1,"quantity":8}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        post("foreign", json!({"id":2,"quantity":8}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post("rollback", json!({"id":1,"quantity":9,"fail":true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_GATEWAY
    );
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    assert_eq!(
        db.query_row("SELECT quantity FROM orders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(db);
    let poisoned = get("/catch", &accounts[0].0).send().await.unwrap();
    assert_eq!(poisoned.status(), StatusCode::FORBIDDEN);
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let blocked = post("native-deadline", json!({"id":1,"quantity":99}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::GATEWAY_TIMEOUT);
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        db.query_row("SELECT quantity FROM orders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM __konae_function_receipts", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        4
    );
    drop(db);
    let request = Request::builder()
        .method("POST")
        .uri("/api/orders")
        .header("host", &deployed.hostname)
        .header("cookie", &accounts[0].0)
        .header("idempotency-key", "disconnect")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"id":1,"quantity":99,"disconnect":true}).to_string(),
        ))
        .unwrap();
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    db.busy_timeout(Duration::ZERO).unwrap();
    let disconnect =
        tokio::spawn(crate::stack(state.clone(), axum::Router::new()).oneshot(request));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match db.execute_batch("BEGIN IMMEDIATE; ROLLBACK;") {
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    break;
                }
                result => result.unwrap(),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    disconnect.abort();
    assert!(disconnect.await.unwrap_err().is_cancelled());
    let guard = tokio::time::timeout(
        Duration::from_millis(700),
        crate::named_lock(&state, &format!("write:{id}"), true),
    )
    .await
    .unwrap();
    assert_eq!(
        db.query_row("SELECT quantity FROM orders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM __konae_function_receipts", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        4
    );
    drop(guard);
    drop(db);
    let pending = tokio::spawn(post("wait", json!({"id":1,"quantity":6,"wait":true})).send());
    let mut acquired = false;
    for _ in 0..100 {
        if tokio::time::timeout(
            Duration::from_millis(2),
            crate::named_lock(&state, &format!("write:{id}"), true),
        )
        .await
        .is_err()
        {
            acquired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        acquired,
        "The A request must hold its native slot before publishing B"
    );
    payload.as_object_mut().unwrap().remove("initialModules");
    payload
        .as_object_mut()
        .unwrap()
        .remove("initialSignupPolicy");
    payload["serverArtifact"]["bundles"]["orders"] =
        json!(HANDLER.replace("version:'A'", "version:'B'"));
    let b = publish(&state, &payload).await;
    assert_ne!(b.artifact_version, deployed.artifact_version);
    let a_response = pending.await.unwrap().unwrap();
    let status = a_response.status();
    let a_response: Value = a_response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{a_response}");
    assert_eq!(a_response["version"], "A");
    let b_response: Value = get("", &accounts[0].0)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b_response["version"], "B");
    assert_eq!(b_response["calls"], 1);
    let replay = post("same", changed).send().await.unwrap();
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        replay.json::<Value>().await.unwrap()["code"],
        function_data::IDEMPOTENCY_REPLAY_CODE
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let after_idle: Value = get("", &accounts[0].0)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_idle["calls"], 1);
    let began = Instant::now();
    let deadline = get("/spin", &accounts[0].0).send().await.unwrap();
    assert_eq!(deadline.status(), StatusCode::GATEWAY_TIMEOUT);
    assert!(began.elapsed() < Duration::from_millis(DEADLINE_MS + 400));
    let recovered = get("", &accounts[0].0).send().await.unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let lock = crate::named_lock(&state, &format!("publish:{id}"), true).await;
    let waiting = get("", &accounts[0].0).send().await.unwrap();
    assert_eq!(waiting.status(), StatusCode::GATEWAY_TIMEOUT);
    drop(lock);
    let method = client
        .put(&url)
        .header("host", &deployed.hostname)
        .send()
        .await
        .unwrap();
    assert_eq!(method.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(method.headers()[header::ALLOW], "GET, POST");
    payload["serverArtifact"]["bundles"]["orders"] =
        json!("while (true) {} export function handle() {}");
    let c = publish(&state, &payload).await;
    let cold_timeout = get("", &accounts[0].0).send().await.unwrap();
    assert_eq!(cold_timeout.status(), StatusCode::GATEWAY_TIMEOUT);
    crate::usage::flush();
    let today = crate::usage::day(crate::now_ms());
    let summary = crate::usage::read_summary(&root, &b, &today, &today, true).unwrap();
    let latency = &summary["functionLatency"];
    let series = latency["series"].as_array().unwrap();
    assert_eq!(series.len(), 3, "{latency}");
    let a_series = series
        .iter()
        .find(|row| row["version"] == deployed.artifact_version)
        .unwrap();
    let b_series = series
        .iter()
        .find(|row| row["version"] == b.artifact_version)
        .unwrap();
    let c_series = series
        .iter()
        .find(|row| row["version"] == c.artifact_version)
        .unwrap();
    assert_eq!(a_series["samples"], 22, "{latency}");
    assert_eq!(a_series["timeouts"], 1, "{latency}");
    assert_eq!(a_series["replays"], 4, "{latency}");
    assert_eq!(b_series["samples"], 7, "{latency}");
    assert_eq!(b_series["timeouts"], 2, "{latency}");
    assert_eq!(b_series["coldStarts"], 2, "{latency}");
    assert_eq!(b_series["replays"], 1, "{latency}");
    assert_eq!(c_series["samples"], 1, "{latency}");
    assert_eq!(c_series["timeouts"], 1, "{latency}");
    assert_eq!(c_series["coldStarts"], 1, "{latency}");
    assert!(
        c_series["initialization"]["maxMs"].as_f64().unwrap() > 0.0,
        "{latency}"
    );
    assert!(
        c_series["execution"]["maxMs"].as_f64().unwrap() >= (DEADLINE_MS - 100) as f64,
        "{latency}"
    );
    assert!(
        b_series["initialization"]["maxMs"].as_f64().unwrap() > 0.0,
        "{latency}"
    );
    assert!(
        b_series["execution"]["maxMs"].as_f64().unwrap() >= (DEADLINE_MS - 100) as f64,
        "{latency}"
    );
    assert!(
        b_series["queue"]["maxMs"].as_f64().unwrap() >= DEADLINE_MS as f64,
        "{latency}"
    );
    if std::env::var_os("KONAE_TEST_FUNCTION_TIMINGS").is_some() {
        let mut rounds = Vec::new();
        for round in 0..3 {
            payload["serverArtifact"]["bundles"]["orders"] =
                json!(format!("// {round}\n{HANDLER}"));
            publish(&state, &payload).await;
            let mut samples = Vec::new();
            for _ in 0..6 {
                let started = Instant::now();
                let response = get("", &accounts[0].0).send().await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = response.bytes().await.unwrap();
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["calls"], 1);
                assert_eq!(body["rows"], json!([{"id": 1, "quantity": 6}]));
            }
            rounds.push(json!({"cold_ms": samples[0], "warm_ms": &samples[1..]}));
        }
        println!("FUNCTION_HTTP_TIMINGS {}", json!(rounds));
        crate::usage::flush();
    }
    if std::env::var_os("KONAE_TEST_FUNCTION_CGROUP").is_some() {
        let pressure = get("/pressure", &accounts[0].0).send().await.unwrap();
        assert_eq!(pressure.status(), StatusCode::BAD_GATEWAY);
        let failure: Value = pressure.json().await.unwrap();
        assert_eq!(failure["code"], "worker_memory_limit", "{failure}");
        let recovered = get("", &accounts[0].0).send().await.unwrap();
        assert_eq!(recovered.status(), StatusCode::OK);
        println!("kernel_http_oom=worker_memory_limit recovery=200");
    }
    if let Some(destination) = std::env::var_os("KONAE_TEST_PRESERVE_APP_DIR") {
        let destination = PathBuf::from(destination);
        let source = crate::layout::folder(&root, id);
        copy_tree(&source, &destination);
        std::fs::write(
            destination.parent().unwrap().join("test-cookie.txt"),
            &accounts[0].0,
        )
        .unwrap();
        std::fs::write(
            destination.parent().unwrap().join("test-host.txt"),
            &deployed.hostname,
        )
        .unwrap();
        println!("preserved_published_app={}", destination.display());
    }
    serving.abort();
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
