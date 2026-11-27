use super::*;
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    program: Arc<ActionProgram>,
    function: Function,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("konae-function-data-{}", uuid::Uuid::new_v4()));
        let path = crate::layout::database(&root, "app");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::initialize_database(&path).unwrap();
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE orders(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK(quantity BETWEEN 1 AND 10));
             CREATE TABLE audit(id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL REFERENCES orders(id), actor_id TEXT NOT NULL, quantity INTEGER NOT NULL);
             CREATE TABLE leaked(secret TEXT);
             INSERT INTO orders VALUES(1,'alice',2),(2,'bob',4);").unwrap();
        let program: ActionProgram = serde_json::from_value(json!({"version":1,"actions":{
            "orders.read":{"kind":"query","access":"authenticated","statement":"SELECT id,quantity FROM orders WHERE owner_id=?1 LIMIT 10","bindings":[{"source":"actor","field":"id"}]},
            "orders.update":{"kind":"mutation","access":"authenticated","statement":"UPDATE orders SET quantity=?1 WHERE id=?2 AND owner_id=?3","guard":"Only the order owner can change it.","bindings":[{"source":"input","key":"quantity","type":"number"},{"source":"input","key":"id","type":"number"},{"source":"actor","field":"id"}]},
            "audit.add":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO audit(order_id,actor_id,quantity) VALUES(?1,?2,?3)","bindings":[{"source":"input","key":"id","type":"number"},{"source":"actor","field":"id"},{"source":"input","key":"quantity","type":"number"}]}
        }})).unwrap();
        let function = Function {
            entry: "functions/orders.ts".into(),
            mount: "/orders".into(),
            methods: vec!["GET".into(), "POST".into()],
            access: "authenticated".into(),
            actions: program.actions.keys().cloned().collect(),
        };
        Self {
            root,
            path,
            program: Arc::new(program),
            function,
        }
    }
    fn config(&self, method: &str, key: &str) -> InvocationConfig {
        let body = br#"{"quantity":7}"#;
        let headers = vec![
            ("content-type".into(), "application/json".into()),
            ("accept".into(), "application/json".into()),
        ];
        InvocationConfig {
            app_id: "test-app".into(),
            release_id: "test-release".into(),
            database: Some(self.path.clone()),
            program: self.program.clone(),
            function_name: "orders".into(),
            function: self.function.clone(),
            actor: Some("alice".into()),
            method: method.into(),
            path: "/orders/1?view=short".into(),
            body_len: body.len(),
            fingerprint: request_fingerprint(method, "/orders/1?view=short", body, &headers)
                .unwrap(),
            headers,
            idempotency_key: Some(key.into()),
        }
    }
    fn run(&self, key: &str) -> Invocation {
        match admit(self.config("POST", key), budget()).unwrap() {
            Admission::Run(invocation) => invocation,
            Admission::Replay(_) => panic!("unexpected replay"),
        }
    }
    fn refresh(config: &mut InvocationConfig, body: &[u8]) {
        config.body_len = body.len();
        config.fingerprint =
            request_fingerprint(&config.method, &config.path, body, &config.headers).unwrap();
    }
    fn state(&self) -> (i64, i64, i64) {
        let db = Connection::open(&self.path).unwrap();
        (
            db.query_row("SELECT quantity FROM orders WHERE id=1 LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap(),
            db.query_row("SELECT count(*) FROM audit LIMIT 1", [], |r| r.get(0))
                .unwrap(),
            db.query_row("SELECT count(*) FROM leaked LIMIT 1", [], |r| r.get(0))
                .unwrap(),
        )
    }
    fn receipts(&self) -> i64 {
        Connection::open(&self.path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM __konae_function_receipts LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        crate::app_db::forget(&self.path);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn budget() -> Budget {
    Budget::new(Duration::from_secs(5))
}
fn output(status: u16) -> Output {
    Output {
        status,
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("set-cookie".into(), "seen=1; Path=/; HttpOnly".into()),
        ],
        body: if status == 204 {
            String::new()
        } else {
            "{\"ok\":true}".into()
        },
    }
}
fn change(invocation: &mut Invocation) {
    invocation
        .action("orders.update", &json!({"id":1,"quantity":7}))
        .unwrap();
    invocation
        .action("audit.add", &json!({"id":1,"quantity":7}))
        .unwrap();
}
fn same_output(left: &Output, right: &Output) {
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

#[test]
fn commits_all_named_actions_and_receipt_marker_together() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("commit");
    assert_eq!(
        invocation.action("orders.read", &json!({})).unwrap(),
        json!([{"id":1,"quantity":2}])
    );
    change(&mut invocation);
    assert_eq!(
        invocation.action("orders.read", &json!({})).unwrap(),
        json!([{"id":1,"quantity":7}])
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    let expected = output(201);
    same_output(&invocation.finish(expected.clone()).unwrap(), &expected);
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
    let stored: String = Connection::open(&fixture.path)
        .unwrap()
        .query_row(
            "SELECT response_json FROM __konae_function_receipts LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, RECEIPT_RESPONSE_MARKER);
    match admit(fixture.config("POST", "commit"), budget()).unwrap() {
        Admission::Replay(error) => {
            assert_eq!(error.code, IDEMPOTENCY_REPLAY_CODE);
            assert_eq!(error.status, 409);
        }
        _ => panic!("expected replay"),
    }
}

#[test]
fn legacy_response_receipt_is_read_without_migration_or_response_replay() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("legacy");
    change(&mut invocation);
    invocation.finish(output(201)).unwrap();
    let legacy_response = serde_json::to_string(&output(201)).unwrap();
    let db = Connection::open(&fixture.path).unwrap();
    db.execute(
        "UPDATE __konae_function_receipts SET response_json=?1 WHERE idempotency_key='legacy'",
        [&legacy_response],
    )
    .unwrap();
    drop(db);

    let fingerprint = lookup_receipt(&fixture.path, Some("alice"), "orders", "legacy", &budget())
        .unwrap()
        .unwrap();
    assert_eq!(fingerprint, fixture.config("POST", "legacy").fingerprint);
    assert!(matches!(
        admit(fixture.config("POST", "legacy"), budget()).unwrap(),
        Admission::Replay(error) if error.code == IDEMPOTENCY_REPLAY_CODE
    ));
    let preserved: String = Connection::open(&fixture.path)
        .unwrap()
        .query_row(
            "SELECT response_json FROM __konae_function_receipts LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved, legacy_response);
    assert_eq!(fixture.state(), (7, 1, 0));
}

#[test]
fn transaction_recheck_refuses_a_receipt_after_row_access_changes() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("revoked-row");
    change(&mut invocation);
    invocation.finish(output(201)).unwrap();
    Connection::open(&fixture.path)
        .unwrap()
        .execute("UPDATE orders SET owner_id='bob' WHERE id=1", [])
        .unwrap();

    let replay = admit(fixture.config("POST", "revoked-row"), budget()).unwrap();
    let Admission::Replay(error) = replay else {
        panic!("expected a replay refusal");
    };
    assert_eq!(error.code, IDEMPOTENCY_REPLAY_CODE);
    assert_eq!(error.status, 409);
    assert!(error.message.contains("authorized read"));
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn abort_or_drop_after_update_reverts_every_action() {
    let fixture = Fixture::new();
    {
        let mut invocation = fixture.run("dropped");
        change(&mut invocation);
    }
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
    let mut invocation = fixture.run("aborted");
    change(&mut invocation);
    invocation.abort().unwrap();
    assert_eq!(
        invocation
            .action("orders.update", &json!({"id":1,"quantity":8}))
            .unwrap_err()
            .code,
        "function.invocation-closed"
    );
    assert_eq!(
        invocation.finish(output(200)).unwrap_err().code,
        "function.invocation-closed"
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
}

#[test]
fn error_response_rolls_back_but_redirect_commits_once() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("status400");
    change(&mut invocation);
    same_output(&invocation.finish(output(400)).unwrap(), &output(400));
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
    let mut invocation = fixture.run("status303");
    change(&mut invocation);
    let mut redirect = output(303);
    redirect
        .headers
        .push(("location".into(), "/orders/1".into()));
    invocation.finish(redirect.clone()).unwrap();
    assert_eq!(fixture.state(), (7, 1, 0));
    match admit(fixture.config("POST", "status303"), budget()).unwrap() {
        Admission::Replay(error) => assert_eq!(error.code, IDEMPOTENCY_REPLAY_CODE),
        _ => panic!("expected redirect replay"),
    }
}

#[test]
fn native_failure_poisons_transaction_even_when_caller_catches_it() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("poison");
    invocation
        .action("orders.update", &json!({"id":1,"quantity":7}))
        .unwrap();
    let refused = invocation
        .action("audit.add", &json!({"id":999,"quantity":7}))
        .unwrap_err();
    assert_eq!(refused.code, "function.action-failed");
    assert_eq!(
        invocation.action("orders.read", &json!({})).unwrap_err(),
        refused
    );
    assert_eq!(invocation.finish(output(200)).unwrap_err(), refused);
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
}

#[test]
fn guard_refusal_and_actor_input_are_the_kernel_rules() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("guard");
    let error = invocation
        .action("orders.update", &json!({"id":2,"quantity":7}))
        .unwrap_err();
    assert_eq!(error.message, "Only the order owner can change it.");
    assert_eq!(invocation.finish(output(200)).unwrap_err(), error);
    let mut invocation = fixture.run("actor");
    let error = invocation
        .action("orders.update", &json!({"id":1,"quantity":7,"actor":"bob"}))
        .unwrap_err();
    assert_eq!(error.message, "Unexpected action input: actor");
    assert_eq!(fixture.state(), (2, 0, 0));
}

#[test]
fn get_rejects_mutation_and_uses_read_only_connection() {
    let fixture = Fixture::new();
    let mut config = fixture.config("GET", "");
    config.idempotency_key = None;
    let mut invocation = match admit(config, budget()).unwrap() {
        Admission::Run(i) => i,
        _ => panic!(),
    };
    invocation.action("orders.read", &json!({})).unwrap();
    assert!(
        invocation
            .connection
            .as_ref()
            .unwrap()
            .db
            .is_readonly("main")
            .unwrap()
    );
    assert_eq!(
        invocation
            .action("orders.update", &json!({"id":1,"quantity":7}))
            .unwrap_err()
            .code,
        "function.get-mutation"
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
}

#[test]
fn no_actions_never_opens_database_even_for_post() {
    let fixture = Fixture::new();
    let absent = fixture.root.join("never-created.sqlite");
    let mut config = fixture.config("POST", "");
    config.function.actions.clear();
    config.database = Some(absent.clone());
    config.idempotency_key = None;
    let invocation = match admit(config, budget()).unwrap() {
        Admission::Run(i) => i,
        _ => panic!(),
    };
    assert!(invocation.connection.is_none());
    invocation.finish(output(200)).unwrap();
    assert!(!absent.exists());
}

#[test]
fn receipt_scope_uses_actor_function_and_payload_but_not_version() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("scope");
    change(&mut invocation);
    invocation.finish(output(201)).unwrap();
    let mut changed = fixture.config("POST", "scope");
    Fixture::refresh(&mut changed, br#"{"quantity":8}"#);
    assert_eq!(
        admit(changed, budget()).err().unwrap().code,
        "idempotency_conflict"
    );
    let mut changed = fixture.config("POST", "scope");
    changed.path = "/orders/2".into();
    Fixture::refresh(&mut changed, br#"{"quantity":7}"#);
    assert_eq!(
        admit(changed, budget()).err().unwrap().code,
        "idempotency_conflict"
    );
    let mut new_version = fixture.config("POST", "scope");
    new_version.function.entry = "functions/orders-v2.ts".into();
    assert!(matches!(
        admit(new_version, budget()).unwrap(),
        Admission::Replay(_)
    ));
    let mut actor = fixture.config("POST", "scope");
    actor.actor = Some("bob".into());
    assert!(matches!(admit(actor, budget()).unwrap(), Admission::Run(_)));
    let mut function = fixture.config("POST", "scope");
    function.function_name = "other".into();
    assert!(matches!(
        admit(function, budget()).unwrap(),
        Admission::Run(_)
    ));
    let mut unauthenticated = fixture.config("POST", "scope");
    unauthenticated.actor = None;
    assert_eq!(
        admit(unauthenticated, budget()).err().unwrap().code,
        "authentication-required"
    );
}

#[test]
fn deadline_before_commit_rolls_back_and_after_commit_is_unknown() {
    let fixture = Fixture::new();
    let request = budget();
    let mut invocation = match admit(fixture.config("POST", "cancel"), request.clone()).unwrap() {
        Admission::Run(i) => i,
        _ => panic!(),
    };
    change(&mut invocation);
    request.cancel();
    assert_eq!(
        invocation.finish(output(200)).unwrap_err().code,
        "cancelled"
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 0);
    let request = Budget::new(Duration::from_millis(40));
    let mut invocation = match admit(fixture.config("POST", "late"), request.clone()).unwrap() {
        Admission::Run(i) => i,
        _ => panic!(),
    };
    change(&mut invocation);
    invocation
        .connection
        .as_ref()
        .unwrap()
        .db
        .wal_hook(Some(|_, _| {
            thread::sleep(Duration::from_millis(80));
            Ok(())
        }));
    assert_eq!(
        invocation.finish(output(200)).unwrap_err().code,
        "mutation_outcome_unknown"
    );
    assert!(request.mutation_outcome().may_have_committed());
    assert_eq!(fixture.state(), (7, 1, 0));
    assert!(matches!(
        admit(fixture.config("POST", "late"), budget()).unwrap(),
        Admission::Replay(_)
    ));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn receipt_lookup_respects_budget_while_its_connection_is_locked() {
    let fixture = Fixture::new();
    let locked = Connection::open(&fixture.path).unwrap();
    locked
        .execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")
        .unwrap();

    let budget = Budget::new(Duration::from_millis(25));
    let error =
        lookup_receipt(&fixture.path, Some("alice"), "orders", "locked", &budget).unwrap_err();
    assert_eq!(error.code, "deadline_exceeded");
}

#[test]
fn expired_budget_refuses_receipt_and_write_lock_wait_is_bounded() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("existing");
    change(&mut invocation);
    invocation.finish(output(200)).unwrap();
    assert_eq!(
        admit(
            fixture.config("POST", "existing"),
            Budget::until(Instant::now())
        )
        .err()
        .unwrap()
        .code,
        "deadline_exceeded"
    );
    let locked = Connection::open(&fixture.path).unwrap();
    locked.execute_batch("BEGIN IMMEDIATE").unwrap();
    let start = Instant::now();
    let error = admit(
        fixture.config("POST", "locked"),
        Budget::new(Duration::from_millis(50)),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "deadline_exceeded");
    assert!(start.elapsed() < Duration::from_millis(300));
    locked.execute_batch("ROLLBACK").unwrap();
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn invalid_output_does_not_commit() {
    let fixture = Fixture::new();
    for (key, bad) in [
        (
            "status",
            Output {
                status: 101,
                ..output(200)
            },
        ),
        (
            "header",
            Output {
                headers: vec![("x-test".into(), "ok\r\ninjected: true".into())],
                ..output(200)
            },
        ),
        (
            "length",
            Output {
                headers: vec![("content-length".into(), "999".into())],
                ..output(200)
            },
        ),
        (
            "size",
            Output {
                body: "x".repeat(MAX_RESPONSE_BYTES),
                ..output(200)
            },
        ),
    ] {
        let mut invocation = fixture.run(key);
        change(&mut invocation);
        assert!(invocation.finish(bad).is_err());
        assert_eq!(fixture.state(), (2, 0, 0));
        assert_eq!(fixture.receipts(), 0);
    }
}

#[test]
fn forged_receipt_schema_and_attached_trigger_are_rejected() {
    let fixture = Fixture::new();
    let db = Connection::open(&fixture.path).unwrap();
    db.execute_batch("CREATE TABLE __konae_function_receipts(secret TEXT)")
        .unwrap();
    assert_eq!(
        admit(fixture.config("POST", "forged"), budget())
            .err()
            .unwrap()
            .code,
        "function.receipts-invalid"
    );
    db.execute_batch("DROP TABLE __konae_function_receipts")
        .unwrap();
    let invocation = fixture.run("schema");
    invocation.finish(output(200)).unwrap();
    db.execute_batch("CREATE TRIGGER leak_receipt AFTER INSERT ON __konae_function_receipts BEGIN INSERT INTO leaked(secret) SELECT response_json FROM __konae_function_receipts; END").unwrap();
    assert_eq!(
        admit(fixture.config("POST", "trigger"), budget())
            .err()
            .unwrap()
            .code,
        "function.receipts-invalid"
    );
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn trusted_receipt_flag_does_not_authorize_trigger_or_view_access() {
    let fixture = Fixture::new();
    let invocation = fixture.run("schema");
    invocation.finish(output(200)).unwrap();
    let raw = Connection::open(&fixture.path).unwrap();
    raw.execute_batch("CREATE VIEW receipt_view AS SELECT response_json FROM __konae_function_receipts;
        CREATE TRIGGER receipt_trigger AFTER INSERT ON __konae_function_receipts BEGIN INSERT INTO leaked(secret) SELECT response_json FROM __konae_function_receipts; END;").unwrap();
    let conn = RequestConnection::open(&fixture.path, false, &budget()).unwrap();
    let view: Result<String> = conn.internal(|| {
        conn.sql(|| {
            conn.db
                .query_row("SELECT response_json FROM receipt_view LIMIT 1", [], |r| {
                    r.get(0)
                })
        })
    });
    assert!(view.is_err());
    let insertion = conn.internal(|| {
        conn.sql(|| {
            conn.db.execute(
                "INSERT INTO __konae_function_receipts VALUES('null','fn','injected','f','{}')",
                [],
            )
        })
    });
    assert!(insertion.is_err());
    raw.execute_batch("DROP TRIGGER receipt_trigger; CREATE TRIGGER receipt_trigger AFTER INSERT ON __konae_function_receipts BEGIN INSERT INTO leaked(secret) VALUES(NEW.response_json); END;").unwrap();
    let direct = conn.internal(|| {
        conn.sql(|| {
            conn.db.execute(
                "INSERT INTO __konae_function_receipts VALUES('null','fn','new-row','f','{}')",
                [],
            )
        })
    });
    assert!(direct.is_err());
    assert!(!conn.internal.load(Ordering::SeqCst));
    assert_eq!(fixture.state(), (2, 0, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn reserved_receipts_survive_schema_change_and_never_enter_editable_ddl() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("portable");
    change(&mut invocation);
    invocation.finish(output(200)).unwrap();
    let schema = crate::import::live_schema(&fixture.path).unwrap();
    assert!(!schema.contains(RECEIPTS));
    let schema = crate::sources::database_schema(&fixture.root, "app").unwrap();
    assert!(!schema.contains(RECEIPTS));
    let schema = "CREATE TABLE IF NOT EXISTS orders(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK(quantity BETWEEN 1 AND 10)); CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL REFERENCES orders(id), actor_id TEXT NOT NULL, quantity INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS leaked(secret TEXT); CREATE TABLE IF NOT EXISTS added(id INTEGER PRIMARY KEY);";
    crate::schema_change::apply(
        &fixture.path,
        schema,
        &fixture.program,
        crate::schema_change::Mode::Apply(None),
    )
    .unwrap();
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
    assert!(matches!(
        admit(fixture.config("POST", "portable"), budget()).unwrap(),
        Admission::Replay(_)
    ));
    assert!(
        crate::run_admin_sql(
            &fixture.path,
            "SELECT response_json FROM __konae_function_receipts LIMIT 1"
        )
        .is_err()
    );
}

#[test]
fn row_budget_and_statement_validation_use_kernel_helpers() {
    let fixture = Fixture::new();
    let mut program:ActionProgram=serde_json::from_value(json!({"version":1,"actions":{
        "test.huge":{"kind":"query","access":"anonymous","statement":"SELECT hex(zeroblob(600000)) AS a,hex(zeroblob(600000)) AS b LIMIT 1","bindings":[]}
    }})).unwrap();
    let mut config = fixture.config("GET", "");
    config.function.actions = vec!["test.huge".into()];
    config.program = Arc::new(program);
    let mut invocation = match admit(config, budget()).unwrap() {
        Admission::Run(i) => i,
        _ => panic!(),
    };
    let error = invocation.action("test.huge", &json!({})).unwrap_err();
    assert!(
        error
            .message
            .contains("Query result exceeds 2 MiB or 1000 rows.")
    );
    program=serde_json::from_value(json!({"version":1,"actions":{
        "test.unbounded":{"kind":"query","access":"anonymous","statement":"SELECT id FROM orders","bindings":[]}
    }})).unwrap();
    let expected =
        crate::validate_action("test.unbounded", &program.actions["test.unbounded"]).unwrap_err();
    let mut config = fixture.config("GET", "");
    config.function.actions = vec!["test.unbounded".into()];
    config.program = Arc::new(program);
    let error = admit(config, budget()).err().unwrap();
    assert_eq!(error.code, expected.code);
    assert_eq!(error.message, expected.message);
}

#[test]
fn concurrent_identical_post_is_admitted_as_a_replay_without_mutating_again() {
    let fixture = Fixture::new();
    let mut first = fixture.run("concurrent");
    change(&mut first);
    let second_config = fixture.config("POST", "concurrent");
    let second = thread::spawn(move || admit(second_config, budget()));
    thread::sleep(Duration::from_millis(20));
    first.finish(output(201)).unwrap();
    assert!(matches!(
        second.join().unwrap().unwrap(),
        Admission::Replay(_)
    ));
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn visible_headers_and_query_belong_to_fingerprint_but_order_and_login_do_not() {
    let fixture = Fixture::new();
    let mut invocation = fixture.run("headers");
    change(&mut invocation);
    invocation.finish(output(201)).unwrap();
    let mut changed = fixture.config("POST", "headers");
    changed.headers[0].1 = "text/plain".into();
    Fixture::refresh(&mut changed, br#"{"quantity":7}"#);
    assert_eq!(
        admit(changed, budget()).err().unwrap().code,
        "idempotency_conflict"
    );
    let mut changed = fixture.config("POST", "headers");
    changed.path = "/orders/1?view=other".into();
    Fixture::refresh(&mut changed, br#"{"quantity":7}"#);
    assert_eq!(
        admit(changed, budget()).err().unwrap().code,
        "idempotency_conflict"
    );
    let mut reordered = fixture.config("POST", "headers");
    reordered.headers.reverse();
    reordered.headers[0].0 = "Accept".into();
    reordered.headers.extend([
        ("Cookie".into(), "session=new-login".into()),
        ("Host".into(), "another-machine".into()),
        ("Authorization".into(), "Bearer new-token".into()),
        ("Idempotency-Key".into(), "headers".into()),
    ]);
    Fixture::refresh(&mut reordered, br#"{"quantity":7}"#);
    assert!(matches!(
        admit(reordered, budget()).unwrap(),
        Admission::Replay(_)
    ));
    assert_eq!(fixture.state(), (7, 1, 0));
    assert_eq!(fixture.receipts(), 1);
}

#[test]
fn the_same_key_in_another_app_has_an_independent_receipt() {
    let first = Fixture::new();
    let second = Fixture::new();
    for fixture in [&first, &second] {
        let mut invocation = fixture.run("same-key");
        change(&mut invocation);
        invocation.finish(output(201)).unwrap();
    }
    assert_eq!(first.state(), (7, 1, 0));
    assert_eq!(second.state(), (7, 1, 0));
    assert_eq!(first.receipts(), 1);
    assert_eq!(second.receipts(), 1);
}
