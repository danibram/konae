use super::*;
use std::{
    sync::{
        Barrier,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: std::path::PathBuf,
    db: Database,
}
impl Fixture {
    fn new() -> Self {
        let number = NEXT_DATABASE.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "konae-native-budget-{}-{number}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = open(&dir, "a").unwrap();
        Self { dir, db }
    }
    fn state(&self) -> (i64, i64, i64) {
        let conn = self.db.lock().unwrap();
        (
            conn.query_row("SELECT quantity FROM orders WHERE id=1 LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM audit LIMIT 1", [], |r| r.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM mutation_receipts LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap(),
        )
    }
    fn quantity(&self) -> i64 {
        self.state().0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn budget() -> Budget {
    Budget::new(Duration::from_secs(5))
}

#[test]
fn expired_budget_never_authenticates_reads_or_mutates() {
    let fixture = Fixture::new();
    let expired = Budget::until(Instant::now());
    let expected = DataError::Budget(BudgetError::DeadlineExceeded);
    assert_eq!(
        authenticate(&fixture.db, "a-alice", &expired),
        Err(expected)
    );
    assert_eq!(read(&fixture.db, "alice", 1, &expired), Err(expected));
    assert_eq!(
        update(&fixture.db, "alice", 1, 7, "expired", &expired),
        Err(expected)
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(expired.mutation_outcome(), MutationOutcomeState::Clean);
}

#[test]
fn mutex_wait_obeys_absolute_deadline() {
    let fixture = Fixture::new();
    let locked = fixture.db.lock().unwrap();
    let db = fixture.db.clone();
    let request = Budget::new(Duration::from_millis(60));
    let (send, receive) = mpsc::channel();
    let worker = thread::spawn(move || send.send(read(&db, "alice", 1, &request)).unwrap());
    let result = receive.recv_timeout(Duration::from_millis(300));
    drop(locked);
    worker.join().unwrap();
    assert_eq!(
        result.unwrap(),
        Err(DataError::Budget(BudgetError::DeadlineExceeded))
    );
}

#[test]
fn cancellation_wakes_mutex_wait() {
    let fixture = Fixture::new();
    let locked = fixture.db.lock().unwrap();
    let request = budget();
    let pending = request.clone();
    let db = fixture.db.clone();
    let (send, receive) = mpsc::channel();
    let worker = thread::spawn(move || {
        send.send(update(&db, "alice", 1, 7, "cancel-mutex", &pending))
            .unwrap()
    });
    thread::sleep(Duration::from_millis(20));
    assert_eq!(request.cancel(), MutationOutcomeState::Clean);
    let result = receive.recv_timeout(Duration::from_millis(200));
    drop(locked);
    worker.join().unwrap();
    assert_eq!(
        result.unwrap(),
        Err(DataError::Budget(BudgetError::Cancelled))
    );
    assert_eq!(fixture.state(), (2, 0, 0));
}

#[test]
fn sqlite_lock_350ms_does_not_commit_after_100ms_deadline() {
    let fixture = Fixture::new();
    let external = Connection::open(fixture.dir.join("a.sqlite")).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let holder = thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        external.execute_batch("ROLLBACK").unwrap();
    });
    let start = Instant::now();
    let request = Budget::new(Duration::from_millis(100));
    let result = update(&fixture.db, "alice", 1, 7, "blocked", &request);
    let elapsed = start.elapsed();
    holder.join().unwrap();
    assert_eq!(
        result,
        Err(DataError::Budget(BudgetError::DeadlineExceeded))
    );
    assert!(elapsed < Duration::from_millis(300), "elapsed={elapsed:?}");
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    println!(
        "sqlite-lock elapsed_ms={} quantity=2 audit=0 receipts=0",
        elapsed.as_millis()
    );
}

#[test]
fn cancellation_interrupts_sqlite_busy_wait() {
    let fixture = Fixture::new();
    let external = Connection::open(fixture.dir.join("a.sqlite")).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let request = budget();
    let cancel = request.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancel.cancel();
    });
    let start = Instant::now();
    let result = update(&fixture.db, "alice", 1, 7, "cancel-busy", &request);
    canceller.join().unwrap();
    assert_eq!(result, Err(DataError::Budget(BudgetError::Cancelled)));
    assert!(start.elapsed() < Duration::from_millis(300));
    external.execute_batch("ROLLBACK").unwrap();
    assert_eq!(fixture.state(), (2, 0, 0));
}

#[test]
fn sql_progress_interrupts_expensive_trigger_and_rolls_back() {
    let fixture = Fixture::new();
    fixture.db.lock().unwrap().execute_batch(
        "CREATE TRIGGER expensive_audit BEFORE INSERT ON audit BEGIN
         SELECT sum(x) FROM (WITH RECURSIVE count(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM count WHERE x<1000000000) SELECT x FROM count);
         END;").unwrap();
    let request = Budget::new(Duration::from_millis(40));
    let start = Instant::now();
    assert_eq!(
        update(&fixture.db, "alice", 1, 7, "expensive", &request),
        Err(DataError::Budget(BudgetError::DeadlineExceeded))
    );
    assert!(start.elapsed() < Duration::from_millis(300));
    assert_eq!(fixture.state(), (2, 0, 0));
    fixture
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER expensive_audit")
        .unwrap();
    assert!(update(&fixture.db, "alice", 1, 7, "expensive", &budget()).is_ok());
    assert_eq!(fixture.state(), (7, 1, 1));
}

#[test]
fn cancellation_interrupts_running_sql_progress() {
    let fixture = Fixture::new();
    let request = budget();
    let conn = RequestConnection::acquire(&fixture.db, &request).unwrap();
    let cancel = request.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancel.cancel();
    });
    let result: Result<i64> = conn.sql(|| conn.connection.query_row(
        "WITH RECURSIVE count(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM count WHERE x<1000000000) SELECT sum(x) FROM count LIMIT 1", [], |row| row.get(0)));
    canceller.join().unwrap();
    assert_eq!(result, Err(DataError::Budget(BudgetError::Cancelled)));
    drop(conn);
    assert_eq!(
        read(&fixture.db, "alice", 1, &budget())
            .unwrap()
            .unwrap()
            .quantity,
        2
    );
}

#[test]
fn cancellation_before_commit_reverts_order_audit_and_receipt() {
    let fixture = Fixture::new();
    let request = budget();
    let result = update_inner(
        &fixture.db,
        "alice",
        1,
        7,
        "before-commit",
        &request,
        |step| {
            if matches!(step, MutationStep::BeforeCommit) {
                request.cancel();
            }
        },
    );
    assert_eq!(result, Err(DataError::Budget(BudgetError::Cancelled)));
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    assert_eq!(fixture.state(), (2, 0, 0));
}

#[test]
fn sqlite_commit_hook_vetoes_cancelled_commit_without_wrapper_check() {
    let fixture = Fixture::new();
    let request = budget();
    let conn = RequestConnection::acquire(&fixture.db, &request).unwrap();
    conn.connection
        .execute_batch(
            "BEGIN IMMEDIATE; UPDATE orders SET quantity=7 WHERE id=1 AND owner_id='alice'",
        )
        .unwrap();
    request.begin_commit().unwrap();
    assert_eq!(request.cancel(), MutationOutcomeState::Committing);
    conn.connection.execute_batch("COMMIT").unwrap_err();
    assert!(conn.connection.is_autocommit());
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    drop(conn);
    assert_eq!(fixture.state(), (2, 0, 0));
}

#[test]
fn cancellation_after_commit_is_unknown_and_replay_does_not_mutate() {
    let fixture = Fixture::new();
    let request = budget();
    let result = update_inner(
        &fixture.db,
        "alice",
        1,
        7,
        "lost-response",
        &request,
        |step| {
            if matches!(step, MutationStep::AfterCommit) {
                request.cancel();
            }
        },
    );
    assert_eq!(result, Err(DataError::MutationOutcomeUnknown));
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Committed);
    assert_eq!(request.failure_code(), Some("mutation_outcome_unknown"));
    assert_eq!(fixture.state(), (7, 1, 1));
    let replay = update(&fixture.db, "alice", 1, 7, "lost-response", &budget()).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.order.unwrap().quantity, 7);
    assert_eq!(fixture.state(), (7, 1, 1));
    println!("post-commit cancellation=mutation_outcome_unknown replay=true audit=1");
}

#[test]
fn replay_survives_reopen_and_returns_original_response() {
    let fixture = Fixture::new();
    update(&fixture.db, "alice", 1, 7, "first", &budget()).unwrap();
    update(&fixture.db, "alice", 1, 8, "second", &budget()).unwrap();
    let reopened = open(&fixture.dir, "a").unwrap();
    let replay = update(&reopened, "alice", 1, 7, "first", &budget()).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.order.unwrap().quantity, 7);
    assert_eq!(fixture.state(), (8, 2, 2));
}

#[test]
fn payload_conflict_does_not_write() {
    let fixture = Fixture::new();
    update(&fixture.db, "alice", 1, 7, "same", &budget()).unwrap();
    assert_eq!(
        update(&fixture.db, "alice", 1, 8, "same", &budget()),
        Err(DataError::IdempotencyConflict)
    );
    assert_eq!(
        update(&fixture.db, "alice", 2, 7, "same", &budget()),
        Err(DataError::IdempotencyConflict)
    );
    assert_eq!(fixture.state(), (7, 1, 1));
}

#[test]
fn receipt_scope_separates_actors_and_apps() {
    let fixture = Fixture::new();
    let b = open(&fixture.dir, "b").unwrap();
    for (db, actor, id) in [
        (&fixture.db, "alice", 1),
        (&fixture.db, "bob", 2),
        (&b, "alice", 1),
    ] {
        let outcome = update(db, actor, id, 7, "shared-key", &budget()).unwrap();
        assert!(!outcome.replayed);
        assert_eq!(outcome.order.unwrap().owner_id, actor);
    }
    assert_eq!(fixture.state(), (7, 2, 2));
}

#[test]
fn not_found_leaves_no_receipt_and_allows_retry_if_row_becomes_visible() {
    let fixture = Fixture::new();
    let request = budget();
    assert_eq!(
        update(&fixture.db, "alice", 2, 7, "absent", &request).unwrap(),
        MutationOutcome {
            order: None,
            replayed: false,
        }
    );
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(
        read(&fixture.db, "bob", 2, &budget())
            .unwrap()
            .unwrap()
            .quantity,
        4
    );
    fixture
        .db
        .lock()
        .unwrap()
        .execute("UPDATE orders SET owner_id='alice' WHERE id=2", [])
        .unwrap();
    let retry = update(&fixture.db, "alice", 2, 7, "absent", &budget()).unwrap();
    assert!(!retry.replayed);
    assert_eq!(retry.order.unwrap().quantity, 7);
    assert_eq!(fixture.state(), (2, 1, 1));
}

#[test]
fn legacy_not_found_receipt_replays_without_a_committed_mutation() {
    let fixture = Fixture::new();
    fixture.db.lock().unwrap().execute(
        "INSERT INTO mutation_receipts VALUES ('alice', 'orders.update', 'legacy', 2, 7, 'null')",
        [],
    ).unwrap();
    let request = budget();
    assert_eq!(
        update(&fixture.db, "alice", 2, 7, "legacy", &request).unwrap(),
        MutationOutcome {
            order: None,
            replayed: true
        }
    );
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    assert_eq!(fixture.state(), (2, 0, 1));
}

#[test]
fn concurrent_retries_on_distinct_connections_write_once() {
    let fixture = Fixture::new();
    let second = open(&fixture.dir, "a").unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = [fixture.db.clone(), second]
        .into_iter()
        .map(|db| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                update(&db, "alice", 1, 7, "concurrent", &budget()).unwrap()
            })
        })
        .collect();
    barrier.wait();
    let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|out| out.replayed).count(), 1);
    assert_eq!(outcomes[0].order, outcomes[1].order);
    assert_eq!(fixture.state(), (7, 1, 1));
}

#[test]
fn audit_failure_rolls_back_receipt_and_allows_safe_retry() {
    let fixture = Fixture::new();
    fixture.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    let request = budget();
    assert_eq!(
        update(&fixture.db, "alice", 1, 7, "retry-audit", &request),
        Err(DataError::StorageError)
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(request.mutation_outcome(), MutationOutcomeState::Clean);
    fixture
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert!(update(&fixture.db, "alice", 1, 7, "retry-audit", &budget()).is_ok());
    assert_eq!(fixture.state(), (7, 1, 1));
}

#[test]
fn validates_key_and_native_ranges_before_mutation() {
    let fixture = Fixture::new();
    assert_eq!(
        update(&fixture.db, "alice", 1, 7, "", &budget()),
        Err(DataError::IdempotencyKeyRequired)
    );
    for key in ["bad key".to_owned(), "x".repeat(129), "\n".to_owned()] {
        assert_eq!(
            update(&fixture.db, "alice", 1, 7, &key, &budget()),
            Err(DataError::InvalidInput)
        );
    }
    assert_eq!(
        update(&fixture.db, "alice", 0, 7, "valid", &budget()),
        Err(DataError::InvalidInput)
    );
    assert_eq!(
        update(&fixture.db, "alice", 1, 11, "valid", &budget()),
        Err(DataError::InvalidInput)
    );
    assert_eq!(fixture.quantity(), 2);
}

#[test]
fn cancellation_and_commit_phase_share_one_state_across_threads() {
    for _ in 0..100 {
        let request = budget();
        let begin = request.clone();
        let barrier = Arc::new(Barrier::new(2));
        let start = barrier.clone();
        let worker = thread::spawn(move || {
            start.wait();
            begin.begin_commit()
        });
        barrier.wait();
        let observed = request.cancel();
        let entered = worker.join().unwrap();
        if observed == MutationOutcomeState::Clean {
            assert_eq!(entered, Err(BudgetError::Cancelled));
        }
        if entered.is_ok() {
            assert_eq!(observed, MutationOutcomeState::Committing);
        }
        assert_eq!(request.cancel(), request.mutation_outcome());
    }
}

#[test]
fn deadline_during_sqlite_commit_completion_reports_unknown_and_replays() {
    let fixture = Fixture::new();
    // SQLite llama este hook después de confirmar el WAL y antes de devolver COMMIT.
    fixture.db.lock().unwrap().wal_hook(Some(|_, _| {
        thread::sleep(Duration::from_millis(80));
        Ok(())
    }));
    let request = Budget::new(Duration::from_millis(40));
    assert_eq!(request.deadline(), request.clone().deadline());
    let result = update(
        &fixture.db,
        "alice",
        1,
        7,
        "commit-crossed-deadline",
        &request,
    );
    assert_eq!(result, Err(DataError::MutationOutcomeUnknown));
    assert!(request.mutation_outcome().may_have_committed());
    assert_eq!(fixture.state(), (7, 1, 1));
    fixture.db.lock().unwrap().wal_hook(None);
    let replay = update(
        &fixture.db,
        "alice",
        1,
        7,
        "commit-crossed-deadline",
        &budget(),
    )
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(fixture.state(), (7, 1, 1));
    println!(
        "sqlite-commit completion_after_deadline=true outcome=mutation_outcome_unknown replay=true audit=1"
    );
}
