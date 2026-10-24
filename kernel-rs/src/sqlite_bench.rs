//! Measures what a Server Action costs on its Database, through the same lock
//! and run path as the action route. Ignored by default:
//! `cargo test --release sqlite_bench -- --ignored --nocapture`.

use super::*;
use rusqlite::params;
use std::time::{Duration, Instant};

fn state(root: &Path) -> AppState {
    AppState {
        deployments: Arc::new(RwLock::new(HashMap::new())),
        db_locks: LockMap::default(),
        s3: None,
        artifact_bucket: "artifacts".into(),
        data_bucket: "data".into(),
        data_root: root.into(),
        copies: Arc::new(copies::CopyStatus::default()),
        admin_key: "bench".into(),
        accounts: accounts::AccountSettings::local(),
        public_origin: "http://localhost".into(),
        host_suffix: "localhost".into(),
        guards: Arc::new(Guards::new(Vec::new())),
    }
}

fn action(kind: &str, statement: &str) -> Arc<ActionDefinition> {
    Arc::new(serde_json::from_value(json!({"kind": kind, "access": "anonymous", "statement": statement, "bindings": []})).unwrap())
}

const ID: &str = "bench";

async fn run(state: &AppState, action: &ActionDefinition, params: Vec<SqlValue>) -> Duration {
    let started = Instant::now();
    let _guard = action_lock(state, ID, &action.kind).await;
    run_action(database_path(state, ID), action, params).await.unwrap();
    started.elapsed()
}

fn percentile(samples: &mut [Duration], p: f64) -> f64 {
    samples.sort();
    samples[((samples.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1000.0
}

fn report(label: &str, mut samples: Vec<Duration>) {
    let n = samples.len();
    println!(
        "{label}: n={n} p50={:.3}ms p95={:.3}ms p99={:.3}ms",
        percentile(&mut samples, 0.5),
        percentile(&mut samples, 0.95),
        percentile(&mut samples, 0.99)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore]
async fn sqlite_bench() {
    let root = std::env::temp_dir().join(format!("littlebox-sqlite-bench-{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join("deployments").join(ID)).unwrap();
    let state = state(&root);
    let path = database_path(&state, ID);
    initialize_database(&path).unwrap();
    {
        let mut db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL, owner TEXT NOT NULL, created_at INTEGER NOT NULL); CREATE INDEX items_owner ON items(owner, created_at);").unwrap();
        let tx = db.transaction().unwrap();
        for i in 0..20_000 {
            tx.execute("INSERT INTO items(name, owner, created_at) VALUES (?1, ?2, ?3)", params![format!("item {i}"), format!("user{}", i % 200), i]).unwrap();
        }
        tx.commit().unwrap();
    }
    let by_id = action("query", "SELECT id, name FROM items WHERE id = ?1 LIMIT 1");
    let list = action("query", "SELECT id, name, created_at FROM items WHERE owner = ?1 ORDER BY created_at DESC LIMIT 50");
    let insert = action("mutation", "INSERT INTO items(name, owner, created_at) VALUES ('new', ?1, 0)");

    // One caller at a time.
    let mut samples = Vec::new();
    for i in 0..2_000 {
        samples.push(run(&state, &by_id, vec![SqlValue::Integer(1 + i % 20_000)]).await);
    }
    report("sequential read by id", samples);
    let mut samples = Vec::new();
    for i in 0..1_000 {
        samples.push(run(&state, &insert, vec![SqlValue::Text(format!("user{}", i % 200))]).await);
    }
    report("sequential insert", samples);

    // 32 readers listing rows while one writer inserts.
    let started = Instant::now();
    let mut readers = Vec::new();
    for reader in 0..32 {
        let (state, list) = (state.clone(), list.clone());
        readers.push(tokio::spawn(async move {
            let mut samples = Vec::new();
            for i in 0..200 {
                samples.push(run(&state, &list, vec![SqlValue::Text(format!("user{}", (reader * 7 + i) % 200))]).await);
            }
            samples
        }));
    }
    let writer = {
        let (state, insert) = (state.clone(), insert.clone());
        tokio::spawn(async move {
            let mut samples = Vec::new();
            for i in 0..300 {
                samples.push(run(&state, &insert, vec![SqlValue::Text(format!("user{}", i % 200))]).await);
            }
            samples
        })
    };
    let mut reads = Vec::new();
    for reader in readers {
        reads.extend(reader.await.unwrap());
    }
    let writes = writer.await.unwrap();
    let wall = started.elapsed();
    println!("concurrent: {} reads + {} writes in {:.0}ms = {:.0} actions/s", reads.len(), writes.len(), wall.as_secs_f64() * 1000.0, (reads.len() + writes.len()) as f64 / wall.as_secs_f64());
    report("concurrent list (32 readers)", reads);
    report("concurrent insert (1 writer)", writes);
    let _ = std::fs::remove_dir_all(root);
}
