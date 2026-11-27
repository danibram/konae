use std::{path::PathBuf, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
};
use konae_functions_executor::MemoryPolicy;
use rusqlite::Connection;
use time::{Date, Month, Time};

use super::*;
use crate::functions::{
    read_sources,
    runtime::Runtime,
    tests::{publish, release},
};

const SOURCE: &str = r#"{"version":1,"functions":{
    "digest":{"entry":"functions/digest.ts","access":"scheduled"},
    "web":{"entry":"functions/web.ts","access":"anonymous","mount":"/web","methods":["POST"]}
}}"#;

#[test]
fn cron_sources_match_the_editor_validation_cases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../cron-cases.json")).unwrap();
    for case in cases {
        let result = read_sources(Some(SOURCE), Some(&case["crons"].to_string()));
        if let Some(expression) = case["expression"].as_str() {
            assert_eq!(
                result.unwrap().unwrap().crons["digest"].cron.text(),
                expression
            );
        } else {
            crate::refusal::assert_case(&case, result);
        }
    }
}

#[test]
fn matches_calendar_days_and_steps_in_utc() {
    let time = |day, hour, minute| {
        Date::from_calendar_date(2026, Month::November, day)
            .unwrap()
            .with_time(Time::from_hms(hour, minute, 0).unwrap())
            .assume_utc()
    };
    let weekends_or_first = Expression::try_from("0-30/15 8 1 11 6-7".to_owned()).unwrap();
    assert!(weekends_or_first.matches(time(1, 8, 0)));
    assert!(weekends_or_first.matches(time(8, 8, 15)));
    assert!(weekends_or_first.matches(time(7, 8, 30)));
    assert!(!weekends_or_first.matches(time(7, 8, 45)));
    assert!(!weekends_or_first.matches(time(2, 8, 0)));
    assert!(!weekends_or_first.matches(time(7, 9, 0)));
    let first = Expression::try_from("0 8 2 * *".to_owned()).unwrap();
    assert!(first.matches(time(2, 8, 0)));
    assert!(!first.matches(time(1, 8, 0)));
}

const HANDLER: &str = r#"
export async function handle(request, context) {
    const { searchParams } = new URL(request.url);
    if (request.method !== "POST" || context.actor.userId !== null) return new Response("", {status: 400});
    const id = searchParams.get("cron");
    const at = Number(searchParams.get("scheduledAt"));
    await context.actions.call("runs.add", {id, at});
    return new Response("", {status: id === "rollback" ? 500 : 200});
}
"#;

fn cron_release(id: &str) -> Value {
    let mut payload = release(id, HANDLER);
    payload["initialModules"] = json!({"functions":{"on":false},"actions":{"on":false}});
    payload["project"]["files"]["schema.sql"] =
        json!("CREATE TABLE IF NOT EXISTS runs(id TEXT, at INTEGER);");
    payload["project"]["files"]["actions.json"] = json!(json!({"version":1,"actions":{
        "runs.add":{"kind":"mutation","access":"scheduled","statement":"INSERT INTO runs(id, at) VALUES (?1, ?2)","bindings":[
            {"source":"input","key":"id","type":"string"},{"source":"input","key":"at","type":"number"}
        ]}
    }}).to_string());
    payload["project"]["files"]["functions.json"] = json!(
        json!({"version":1,"functions":{
            "orders":{"entry":"functions/orders.ts","access":"scheduled","actions":["runs.add"]}
        }})
        .to_string()
    );
    let crons = json!({"digest":{"cron":"* * * * *","function":"orders"},"rollback":{"cron":"* * * * *","function":"orders"}});
    payload["project"]["files"]["crons.json"] =
        json!(json!({"version":1,"crons":crons}).to_string());
    payload["serverArtifact"]["manifest"] = json!({"version":1,"crons":crons,"functions":{
        "orders":{"entry":"functions/orders.ts","access":"scheduled","mount":"/","methods":["POST"],"actions":["runs.add"]}
    }});
    payload
}

#[tokio::test]
async fn cron_release_keeps_published_rules_when_reopening_with_an_edited_draft() {
    let root = std::env::temp_dir().join(format!("konae-cron-folder-{}", uuid::Uuid::new_v4()));
    let id = "cron-folder";
    let state = AppState::for_test(&root);
    let payload = cron_release(id);
    let mut stale = payload.clone();
    stale["serverArtifact"]["manifest"]["crons"]["digest"]["cron"] = json!("0 8 * * *");
    let refused = crate::deploy(&state, stale.to_string().as_bytes()).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let message = axum::body::to_bytes(refused.into_body(), 2048)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&message)
            .contains("Server Artifact manifest does not match crons.json.")
    );

    let deployment = publish(&state, &payload).await;
    assert!(crate::modules::is_on(&deployment.modules, "functions"));
    assert!(crate::modules::is_on(&deployment.modules, "actions"));
    assert!(!crate::modules::is_on(&deployment.modules, "auth"));
    let app = crate::layout::sources(&root, id);
    std::fs::write(app.join("crons.json"), r#"{"version":1,"crons":{}}"#).unwrap();
    let reopened = crate::folder::open(&state, id, None).await.unwrap();
    assert_eq!(reopened.server, deployment.server);

    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_str(&deployment.hostname).unwrap());
    let response = crate::invoke_action(
        State(state.clone()),
        Path("runs.add".into()),
        headers.clone(),
        Json(json!({"id":"direct","at":0})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let request = Request::builder()
        .method("POST")
        .uri("/?cron=digest")
        .header("host", &deployment.hostname)
        .body(Body::empty())
        .unwrap();
    assert!(super::super::http::dispatch(&state, request).await.is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn adding_crons_to_a_deployment_records_the_capabilities_in_the_same_release() {
    let root = std::env::temp_dir().join(format!("konae-cron-republish-{}", uuid::Uuid::new_v4()));
    let id = "cron-republish";
    let state = AppState::for_test(&root);
    let mut candidate = cron_release(id);
    let mut base = candidate.clone();
    base["project"]["files"] = json!({});
    base.as_object_mut().unwrap().remove("serverArtifact");
    let deployment = publish(&state, &base).await;
    assert!(!crate::modules::is_on(&deployment.modules, "functions"));
    assert!(!crate::modules::is_on(&deployment.modules, "actions"));
    let before = crate::Settings::read(&root, id).unwrap().modules;

    candidate.as_object_mut().unwrap().remove("initialModules");
    let plan = serde_json::from_value(candidate.clone()).unwrap();
    crate::runtime::validate_release(&state, &plan, None)
        .await
        .unwrap();
    let settings = crate::Settings::read(&root, id).unwrap();
    assert_eq!(settings.modules, before);

    let deployment = publish(&state, &candidate).await;
    let settings = crate::Settings::read(&root, id).unwrap();
    assert!(crate::modules::is_on(&settings.modules, "functions"));
    assert!(crate::modules::is_on(&settings.modules, "actions"));
    let read = crate::modules::read(&state, &deployment);
    assert_eq!(read["modules"]["functions"]["state"]["on"], true);
    assert_eq!(read["modules"]["actions"]["state"]["on"], true);
    assert_eq!(
        read["modules"]["functions"]["current"],
        json!({"functions":["orders"],"crons":["digest","rollback"]})
    );
    assert!(!crate::modules::is_on(&settings.modules, "auth"));
    let database = Connection::open(crate::layout::database(&root, id)).unwrap();
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM runs", [], |row| row.get::<_, u64>(0))
            .unwrap(),
        0
    );
    drop(database);

    base.as_object_mut().unwrap().remove("initialModules");
    base["project"]["files"]["schema.sql"] = candidate["project"]["files"]["schema.sql"].clone();
    let removed = publish(&state, &base).await;
    assert_eq!(
        crate::modules::read(&state, &removed)["modules"]["functions"]["current"]["crons"],
        json!([])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_first_release_preserves_an_explicit_actions_setting_for_http_only_dependencies() {
    let root = std::env::temp_dir().join(format!("konae-cron-mixed-{}", uuid::Uuid::new_v4()));
    let state = AppState::for_test(&root);
    let mut payload = cron_release("cron-mixed");
    let functions = json!({
        "orders":{"entry":"functions/orders.ts","access":"scheduled","mount":"/","methods":["POST"],"actions":[]},
        "web":{"entry":"functions/web.js","access":"anonymous","mount":"/web","methods":["POST"],"actions":["runs.add"]}
    });
    let files = &mut payload["project"]["files"];
    files["functions.json"] = json!(json!({"version":1,"functions":functions}).to_string());
    files["functions/web.js"] = json!("export function handle() {}");
    let mut actions: Value = serde_json::from_str(files["actions.json"].as_str().unwrap()).unwrap();
    actions["actions"]["runs.add"]["access"] = json!("anonymous");
    files["actions.json"] = json!(actions.to_string());
    payload["serverArtifact"]["manifest"]["functions"] = functions;
    payload["serverArtifact"]["bundles"]["web"] = json!("export function handle() {}");

    let input = serde_json::from_value(payload.clone()).unwrap();
    let error = crate::runtime::validate_release(&state, &input, None)
        .await
        .unwrap_err();
    let refusal = crate::refusal::of(&error).unwrap();
    assert_eq!(refusal.code, "module.requires");
    assert_eq!(
        refusal.message,
        "Module functions requires actions, which is off. Turn actions on first."
    );
    payload["initialModules"]
        .as_object_mut()
        .unwrap()
        .remove("actions");
    let deployment = publish(&state, &payload).await;
    assert!(crate::modules::is_on(&deployment.modules, "actions"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn an_unavailable_runner_still_uses_the_minute_once_across_a_restart() {
    let root = std::env::temp_dir().join(format!("konae-cron-admission-{}", uuid::Uuid::new_v4()));
    let state = AppState::for_test(&root);
    let id = "cron-admission";
    let deployment = publish(&state, &cron_release(id)).await;
    let at = (slot(deployment.released_at) as u64 + 1) * MINUTE_MS;
    let error = run(&state, &deployment, "digest", at, false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("function.runner-unavailable"));
    let restarted = AppState::for_test(&root);
    let deployment = crate::folder::open(&restarted, id, None).await.unwrap();
    assert_eq!(
        run(&restarted, &deployment, "digest", at, false)
            .await
            .unwrap()["ran"],
        false
    );
    let stats =
        Connection::open(crate::layout::data(&root, id).join(usage::USAGE_DATABASE)).unwrap();
    assert_eq!(
        stats
            .query_row(
                "SELECT count(*) FROM scheduled_runs WHERE action = 'cron:digest'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    drop(stats);
    crate::usage::flush();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires KONAE_TEST_FUNCTION_WORKER pointing to the built JavaScript worker"]
async fn real_cron_runs_through_the_worker_once_per_minute_and_rolls_back_on_failure() {
    let executable = PathBuf::from(std::env::var_os("KONAE_TEST_FUNCTION_WORKER").unwrap());
    let root = std::env::temp_dir().join(format!("konae-cron-runner-{}", uuid::Uuid::new_v4()));
    let mut state = AppState::for_test(&root);
    state.functions = Arc::new(Runtime::configured(
        executable,
        MemoryPolicy::DevelopmentPolling,
        Duration::from_secs(5),
        Duration::from_secs(1),
    ));
    let id = "cron-runner";
    let deployment = publish(&state, &cron_release(id)).await;
    let at = (slot(deployment.released_at) as u64 + 1) * MINUTE_MS;
    let time = OffsetDateTime::from_unix_timestamp((at / 1000) as i64).unwrap();
    let running = Arc::default();
    tick(&state, &deployment, &running, at, time);
    tokio::time::timeout(Duration::from_secs(12), async {
        while !running.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    for job in ["digest", "rollback"] {
        assert_eq!(
            run(&state, &deployment, job, at, false).await.unwrap()["ran"],
            false
        );
    }
    let db = Connection::open(crate::layout::database(&root, id)).unwrap();
    let rows = || {
        db.query_row("SELECT count(*), max(id), max(at) FROM runs", [], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .unwrap()
    };
    assert_eq!(rows(), (1, "digest".into(), at));
    let stats =
        Connection::open(crate::layout::data(&root, id).join(usage::USAGE_DATABASE)).unwrap();
    let outcome = |id: &str| {
        stats
            .query_row(
                "SELECT outcome FROM scheduled_runs WHERE action = ?1 ORDER BY id DESC LIMIT 1",
                [run_name(id)],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    assert_eq!(outcome("digest"), "ok");
    assert_eq!(outcome("rollback"), "failed");

    state.execution_mode = crate::execution::ExecutionMode::Test;
    assert_eq!(crate::schedule::paused_schedules(&deployment).len(), 2);
    assert_eq!(
        crate::schedule::run_manually(&state, &deployment, "cron:digest")
            .await
            .unwrap()["status"],
        200
    );
    assert_eq!(rows().0, 2);
    drop((db, stats));
    crate::usage::flush();
    std::fs::remove_dir_all(root).unwrap();
}
