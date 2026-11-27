use super::*;
use crate::files::{list_files, serve_file, upload_file};
use rusqlite::types::Value as SqlValue;

#[test]
fn the_transformation_journal_is_hidden_and_owner_sql_cannot_rewrite_it() {
    let root = std::env::temp_dir().join(format!("konae-schema-journal-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY); CREATE TABLE __konae_schema_state(format_version INTEGER NOT NULL); CREATE TABLE __konae_schema_history(id TEXT);").unwrap();
    drop(db);
    let tables = inspect_database(&path).unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0]["name"], "notes");
    assert!(run_admin_sql(&path, "SELECT * FROM __konae_schema_state").is_err());
    assert!(run_admin_sql(&path, "CREATE TABLE __konae_schema_state(id INTEGER)").is_err());
    assert!(run_admin_sql(&path, "DROP TABLE __konae_schema_history").is_err());
    assert!(run_admin_sql(&path, "ALTER TABLE notes RENAME TO __konae_schema_state").is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn deploy_input_reads_the_initial_signup_policy() {
    let input: DeployInput = serde_json::from_value(json!({
        "project": { "id": "app_1", "name": "App", "slug": "app", "files": {}, "manifest": {} },
        "visibility": "public",
        "artifact": { "version": 1, "entrypoint": "index.html", "files": { "index.html": "<main></main>" } },
        "initialSignupPolicy": { "mode": "closed", "allowlist": [] }
    })).unwrap();

    assert_eq!(input.initial_signup_policy.unwrap().mode, SignupMode::Closed);
}

#[tokio::test]
async fn a_release_with_a_missing_page_template_uses_the_shared_case() {
    let cases: Value = serde_json::from_str(include_str!("page-release-cases.json")).unwrap();
    let case = &cases[0];
    let root = std::env::temp_dir().join(format!("konae-release-page-file-{}", Uuid::new_v4()));
    let state = test_state(&root);
    let body = release_body(
        "app_missing_page_file",
        "CREATE TABLE IF NOT EXISTS posts(id INTEGER PRIMARY KEY, title TEXT NOT NULL);",
        json!({
            "posts.head": {
                "kind": "query", "access": "anonymous",
                "statement": "SELECT title FROM posts LIMIT 10", "bindings": [],
                "page": {"path": "/posts", "file": "post.html"}
            }
        }),
        "<main>posts</main>",
        None,
    );
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let text = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains(case["error"].as_str().unwrap()), "{text}");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_stored_deployment_on_the_editor_host_does_not_capture_the_owner_door() {
    let root = std::env::temp_dir().join(format!("konae-editor-host-{}", Uuid::new_v4()));
    let (state, _) = login_test_state(&root, "public").await;
    let mut deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    deployment.hostname = "app.apps.example.com".into();
    let state = AppState {
        public_origin: "http://app.apps.example.com".into(),
        host_suffix: "apps.example.com".into(),
        deployments: Arc::new(RwLock::new(HashMap::from([(
            deployment.hostname.clone(),
            deployment,
        )]))),
        ..state
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HOST,
        HeaderValue::from_static("app.apps.example.com"),
    );
    assert!(deployment_for_host(&state, &headers).await.is_none());
    assert!(deployment_for(&state, &headers).await.is_none());
    headers.insert(
        header::HOST,
        HeaderValue::from_static("app.apps.example.com:8080"),
    );
    assert!(deployment_for_host(&state, &headers).await.is_none());
    // A page on the editor host is not an app origin either.
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://app.apps.example.com"),
    );
    assert!(!security::from_app_origin(&state, &headers).await);
    let _ = std::fs::remove_dir_all(root);
}

pub async fn login_test_state(root: &Path, visibility: &str) -> (AppState, HeaderMap) {
    let app_id = Uuid::new_v4().to_string();
    let deployment = Deployment {
        id: app_id.clone(),
        project_id: app_id,
        project_name: "Login test".into(),
        slug: "app".into(),
        url: "http://login-test.localhost".into(),
        created_at: now_ms(),
        released_at: 0,
        source_digest: None,
        manifest: json!({"visibility": visibility}),
        hostname: "login-test.localhost".into(),
        actions: "{}".into(),
        artifact_version: "v1".into(),
        signup_policy: SignupPolicy::default(),
        sign_in_methods: default_sign_in_methods(),
        contact_email: None,
        modules: {
            let mut modules = modules::auth_enabled_defaults();
            modules.get_mut("actions").unwrap()["on"] = json!(true);
            modules.get_mut("files").unwrap()["on"] = json!(true);
            modules
        },
        released_schema: String::new(),
        server: None,
    };
    tokio::fs::create_dir_all(crate::layout::data(&root, &deployment.id))
        .await
        .unwrap();
    let state = AppState {
        deployments: Arc::new(RwLock::new(HashMap::from([(
            deployment.hostname.clone(),
            deployment,
        )]))),
        ..AppState::for_test(&root)
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HOST,
        HeaderValue::from_static("login-test.localhost"),
    );
    (state, headers)
}

fn action(kind: &str, statement: &str, bindings: Vec<ActionBinding>) -> ActionDefinition {
    ActionDefinition {
        kind: kind.into(),
        access: "anonymous".into(),
        statement: statement.into(),
        bindings,
        http: None,
        page: None,
        signal: None,
        guard: None,
        notify: None,
        schedule: None,
        subscribe: None,
        call: Default::default(),
        extra: Default::default(),
    }
}

fn apply_schema(
    path: &Path,
    schema: &str,
    program: &ActionProgram,
) -> Result<schema_change::Change> {
    schema_change::apply(path, schema, program, schema_change::Mode::Apply(None))
}

fn action_program(actions: &[(&str, &str)]) -> ActionProgram {
    ActionProgram {
        version: 1,
        actions: actions
            .iter()
            .map(|(name, statement)| ((*name).into(), action("query", statement, Vec::new())))
            .collect(),
    }
}

#[test]
fn actions_accept_one_constrained_statement_and_reject_structural_sql() {
    assert!(
        validate_action(
            "tasks.list",
            &action("query", "SELECT id FROM tasks LIMIT 10", vec![])
        )
        .is_ok()
    );
    assert!(
        validate_action(
            "tasks.create",
            &action("mutation", "INSERT INTO tasks(title) VALUES (?)", vec![])
        )
        .is_ok()
    );
    assert!(
        validate_action(
            "tasks.drop",
            &action("mutation", "DROP TABLE tasks", vec![])
        )
        .is_err()
    );
    assert!(
        validate_action("tasks.read", &action("query", "SELECT 1; SELECT 2", vec![])).is_err()
    );
    let mut granted = action("mutation", "DELETE FROM tasks", vec![]);
    granted.access = "granted".into();
    assert!(validate_action("tasks.delete", &granted).is_err());
    assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks", vec![]))
        .unwrap_err()
        .message
        .contains("reads rows and has no LIMIT"));
    assert!(validate_action("tasks.list", &action("query", "SELECT id FROM tasks WHERE title = 'LIMIT 5'", vec![])).is_err());
    assert!(validate_action("tasks.count", &action("query", "SELECT count(*) AS n FROM tasks", vec![])).is_ok());
    assert!(validate_action("votes.results", &action("query", "SELECT choice, count(*) AS n FROM votes GROUP BY choice", vec![])).is_err());
    assert!(validate_action("tasks.one", &action("query", "SELECT 1", vec![])).is_ok());
}

#[test]
fn action_inputs_are_named_typed_and_closed() {
    let definition = action(
        "mutation",
        "INSERT INTO tasks(title) VALUES (?)",
        vec![ActionBinding {
            source: "input".into(),
            key: Some("title".into()),
            value_type: Some("string".into()),
            max_length: Some(20),
            nullable: false,
            field: None,
            value: None,
        }],
    );
    assert_eq!(
        resolve_bindings(&definition, &json!({"title": "Ship it"}), None).unwrap(),
        vec![SqlValue::Text("Ship it".into())]
    );
    assert!(
        resolve_bindings(
            &definition,
            &json!({"title": "Ship it", "sql": "DROP TABLE tasks"}),
            None,
        )
        .unwrap_err()
        .contains("Unexpected")
    );
    assert!(
        resolve_bindings(&definition, &json!({"title": 42}), None)
            .unwrap_err()
            .contains("non-empty string")
    );
}

#[tokio::test]
async fn login_route_is_available_for_public_and_private_apps() {
    for visibility in ["public", "private"] {
        let root = std::env::temp_dir()
            .join(format!("konae-login-{visibility}-{}", Uuid::new_v4()));
        let (state, headers) = login_test_state(&root, visibility).await;
        let response = login_page(State(state), headers).await;
        assert_eq!(response.status(), StatusCode::OK, "visibility={visibility}");
        let bytes = axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("<h1>Sign in</h1>") && html.contains(r#"<p class="kicker">"#));
        assert!(html.contains("/_konae/auth/signup"));
        assert!(!html.contains("Account creation is open"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}

async fn set_policy(state: &AppState, mode: SignupMode, allowlist: &[&str]) {
    let entries = allowlist.iter().map(|e| e.to_string()).collect::<Vec<_>>();
    for deployment in state.deployments.write().await.values_mut() {
        deployment.signup_policy = SignupPolicy::from_input(mode, &entries).unwrap();
    }
}

async fn signup_status(state: &AppState, headers: &HeaderMap, email: &str) -> StatusCode {
    let body = json!({"email": email, "password": "correct horse battery"});
    let request = Request::builder().body(Body::from(body.to_string())).unwrap();
    signup(State(state.clone()), headers.clone(), request).await.status()
}

#[test]
fn signup_policy_matches_exact_emails_and_whole_domains() {
    let open = SignupPolicy::default();
    assert!(open.allows("anyone@example.test"));
    let closed = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
    assert!(!closed.allows("anyone@example.test"));
    let invitation: SignupPolicy = serde_json::from_value(json!({"mode": "invitation"})).unwrap();
    assert!(
        !invitation.allows("anyone@example.test"),
        "Invitation only refuses every plain signup"
    );
    let list = SignupPolicy::from_input(
        SignupMode::Allowlist,
        &[
            " Carol@Example.TEST ".into(),
            "@Team.Example".into(),
            "carol@example.test".into(),
        ],
    )
    .unwrap();
    assert_eq!(list.allowlist, vec!["carol@example.test", "@team.example"]);
    assert!(list.allows("CAROL@example.test"));
    assert!(list.allows("dana@TEAM.example"));
    assert!(!list.allows("dana@sub.team.example"));
    assert!(!list.allows("dana@evilteam.example"));
    assert!(!list.allows("team.example@attacker.test"));
    assert!(!list.allows("erin@example.test"));
    assert!(!list.allows("not-an-email"));
    let empty = SignupPolicy::from_input(SignupMode::Allowlist, &[]).unwrap();
    assert!(!empty.allows("carol@example.test"));
    for invalid in [
        "@",
        "@nodot",
        "two@@example.test",
        "@bad domain.test",
        "plain",
    ] {
        assert!(
            SignupPolicy::from_input(SignupMode::Allowlist, &[invalid.into()]).is_err(),
            "entry={invalid}"
        );
    }
    let too_many = (0..=MAX_ALLOWLIST_ENTRIES)
        .map(|i| format!("user{i}@example.test"))
        .collect::<Vec<_>>();
    assert!(SignupPolicy::from_input(SignupMode::Allowlist, &too_many).is_err());
}

#[tokio::test]
async fn signup_follows_the_deployment_policy() {
    let root = std::env::temp_dir().join(format!("konae-signup-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    assert_eq!(
        signup_status(&state, &headers, "open@example.test").await,
        StatusCode::OK
    );

    set_policy(&state, SignupMode::Closed, &[]).await;
    assert_eq!(
        signup_status(&state, &headers, "closed@example.test").await,
        StatusCode::FORBIDDEN
    );

    set_policy(
        &state,
        SignupMode::Allowlist,
        &["Listed@Example.test", "@team.example"],
    )
    .await;
    assert_eq!(
        signup_status(&state, &headers, "listed@EXAMPLE.test").await,
        StatusCode::OK
    );
    assert_eq!(
        signup_status(&state, &headers, "any@team.example").await,
        StatusCode::OK
    );
    assert_eq!(
        signup_status(&state, &headers, "other@example.test").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        signup_status(&state, &headers, "any@sub.team.example").await,
        StatusCode::FORBIDDEN
    );

    // Existing accounts still sign in when signup is closed.
    set_policy(&state, SignupMode::Closed, &[]).await;
    let body = json!({"email": "open@example.test", "password": "correct horse battery"});
    let request = Request::builder()
        .body(Body::from(body.to_string()))
        .unwrap();
    assert_eq!(
        login(State(state.clone()), headers.clone(), request)
            .await
            .status(),
        StatusCode::OK
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn operator_policy_updates_keep_saved_source_settings_across_restart() {
    let root =
        std::env::temp_dir().join(format!("konae-policy-preserves-draft-{}", Uuid::new_v4()));
    let (state, _) = login_test_state(&root, "public").await;
    let mut current = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .unwrap()
        .clone();
    current.modules.get_mut("auth").unwrap()["on"] = json!(true);
    current.modules.get_mut("files").unwrap()["on"] = json!(true);
    current.actions = "{\"version\":1,\"actions\":{}}".into();
    state
        .deployments
        .write()
        .await
        .insert(current.hostname.clone(), current.clone());
    Settings::of(&current).write(&root).await.unwrap();
    let draft = json!({"id":current.id,"name":"Unpublished draft name","slug":"app","files":{"index.html":"<main>app</main>","actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""},"directories":[],"manifest":{"version":1,"visibility":"private","fileAccess":"app-users"},"updatedAt":now_ms()});
    crate::sources::write(&root, &current.id, &draft)
        .await
        .unwrap();
    tokio::fs::create_dir_all(crate::layout::dist(&root, &current.id))
        .await
        .unwrap();
    tokio::fs::write(
        crate::layout::dist(&root, &current.id).join("index.html"),
        "<main>deployed</main>",
    )
    .await
    .unwrap();

    set_signup_policy(&state, &current.id, SignupMode::Closed, None)
        .await
        .unwrap();
    let saved = crate::sources::read(&root, &current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved["name"], "Unpublished draft name");
    assert_eq!(saved["manifest"]["visibility"], "private");
    assert!(saved["manifest"].get("fileAccess").is_none());

    let restarted = AppState::for_test(&root);
    let serving = folder::open(&restarted, &current.id, None).await.unwrap();
    assert_eq!(serving.project_name, current.project_name);
    assert_eq!(serving.manifest["visibility"], "public");
    assert_eq!(serving.modules["files"]["on"], true);
    assert!(serving.manifest.get("fileAccess").is_none());
    assert_eq!(serving.signup_policy.mode, SignupMode::Closed);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn login_page_hides_account_creation_when_signup_is_closed() {
    let root = std::env::temp_dir().join(format!("konae-login-policy-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "private").await;
    let page = |state: AppState| {
        let headers = headers.clone();
        async move {
            let response = login_page(State(state), headers).await;
            let bytes = axum::body::to_bytes(response.into_body(), 16_384).await.unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        }
    };
    set_policy(&state, SignupMode::Closed, &[]).await;
    let html = page(state.clone()).await;
    assert!(!html.contains("/_konae/auth/signup"));
    assert!(html.contains("not accepting new accounts"));
    set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
    let html = page(state.clone()).await;
    assert!(html.contains("/_konae/auth/signup"));
    assert!(html.contains("Only invited email addresses"));
    set_policy(&state, SignupMode::Invitation, &[]).await;
    let html = page(state.clone()).await;
    assert!(!html.contains("/_konae/auth/signup"));
    assert!(html.contains("Only invited people can create an account"));
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn private_apps_refuse_anonymous_actions_without_a_session() {
    let root = std::env::temp_dir().join(format!("konae-private-action-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "private").await;
    let program = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n", "bindings": []}}});
    for deployment in state.deployments.write().await.values_mut() {
        deployment.actions = program.to_string();
        initialize_database(&database_path(&state, &deployment.id)).unwrap();
    }
    let call = |state: AppState| {
        let headers = headers.clone();
        async move {
            invoke_action(
                State(state),
                AxumPath("items.count".into()),
                headers,
                Json(json!({})),
            )
            .await
            .status()
        }
    };
    assert_eq!(call(state.clone()).await, StatusCode::UNAUTHORIZED);
    for deployment in state.deployments.write().await.values_mut() {
        deployment.manifest = json!({"visibility": "public"});
    }
    assert_eq!(call(state.clone()).await, StatusCode::OK);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn runtime_mcp_and_http_action_return_the_same_named_operation_result() {
    let root = std::env::temp_dir().join(format!("konae-shared-action-{}", Uuid::new_v4()));
    let (state, _) = login_test_state(&root, "public").await;
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .unwrap()
        .clone();
    let program = json!({"version": 1, "actions": {"items.count": {
        "kind": "query", "access": "anonymous", "statement": "SELECT 7 AS count", "bindings": []
    }}});
    for value in state.deployments.write().await.values_mut() {
        value.actions = program.to_string();
        value.manifest = json!({"visibility": "public"});
        initialize_database(&database_path(&state, &value.id)).unwrap();
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, deployment.hostname.parse().unwrap());
    let response = invoke_action(
        State(state.clone()),
        AxumPath("items.count".into()),
        headers.clone(),
        Json(json!({})),
    )
    .await;
    let direct: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .unwrap(),
    )
    .unwrap();

    let request = Request::builder()
        .method("POST")
        .uri("/_konae/mcp")
        .header(header::HOST, deployment.hostname)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"actions_invoke","arguments":{"name":"items.count","input":{}}}}).to_string()))
        .unwrap();
    let response = runtime::runtime(State(state.clone()), request).await;
    let envelope: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        envelope["result"]["structuredContent"]["result"],
        direct["result"]
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn mcp_http_and_guard_refusal_use_transactional_notify_intents() {
    let root = std::env::temp_dir().join(format!("konae-notify-intents-entrypoints-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let program = json!({"version":1,"actions":{
        "items.mcp":{"kind":"mutation","access":"anonymous","statement":"INSERT INTO items(value) VALUES ('mcp')","bindings":[],"notify":{"to":"owner","subject":"MCP","body":"Stored"}},
        "items.http":{"kind":"mutation","access":"anonymous","statement":"INSERT INTO items(value) VALUES ('http')","bindings":[],"http":{"method":"POST","path":"/api/notify"},"notify":{"to":"owner","subject":"HTTP","body":"Stored"}},
        "items.guarded":{"kind":"mutation","access":"anonymous","guard":"No item changed.","statement":"UPDATE items SET value='miss' WHERE id=999","bindings":[],"notify":{"to":"owner","subject":"Guard","body":"Must not queue"}}
    }});
    for value in state.deployments.write().await.values_mut() {
        value.actions = program.to_string();
        value.modules.get_mut("http").unwrap()["on"] = json!(true);
        initialize_database(&database_path(&state, &value.id)).unwrap();
        Connection::open(database_path(&state, &value.id)).unwrap().execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT);").unwrap();
    }
    let deployment = state.deployments.read().await.values().next().unwrap().clone();
    let request = Request::builder().method("POST").uri("/_konae/mcp")
        .header(header::HOST, deployment.hostname.clone()).header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"actions_invoke","arguments":{"name":"items.mcp","input":{}}}}).to_string())).unwrap();
    let response = runtime::runtime(State(state.clone()), request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let route = integrations::http::call_route(&state, &deployment, "POST", "/api/notify", "", b"{}", &headers, "127.0.0.1".parse().unwrap()).await;
    let route_status = route.status();
    let route_body = axum::body::to_bytes(route.into_body(), 4096).await.unwrap();
    assert_eq!(route_status, StatusCode::OK, "{}", String::from_utf8_lossy(&route_body));
    let before_guard: i64 = Connection::open(database_path(&state, &deployment.id)).unwrap().query_row(
        "SELECT count(*) FROM __konae_notify_intents WHERE state='pending'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(before_guard, 2);
    let refused = invoke_action(State(state.clone()), AxumPath("items.guarded".into()), headers.clone(), Json(json!({}))).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let after_guard: i64 = Connection::open(database_path(&state, &deployment.id)).unwrap().query_row(
        "SELECT count(*) FROM __konae_notify_intents WHERE state='pending'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(after_guard, before_guard);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_deployment_that_does_not_say_public_serves_no_asset_without_a_login() {
    let root = std::env::temp_dir().join(format!("konae-fail-closed-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let id = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .unwrap()
        .id
        .clone();
    let artifact = layout::dist(&state.data_root, &id);
    tokio::fs::create_dir_all(&artifact).await.unwrap();
    tokio::fs::write(artifact.join("report.txt"), "secret")
        .await
        .unwrap();
    let read = |manifest: Value| {
        let (state, headers) = (state.clone(), headers.clone());
        async move {
            for deployment in state.deployments.write().await.values_mut() {
                deployment.manifest = manifest.clone();
            }
            let response = serve_key(state, headers, "report.txt".into()).await;
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    assert_eq!(read(json!({"visibility": "public"})).await, "secret");
    for manifest in [
        json!({}),
        json!({"visibility": null}),
        json!({"visibility": "Public"}),
        json!({"visibility": "open"}),
    ] {
        let deployment = Deployment {
            manifest: manifest.clone(),
            ..state
                .deployments
                .read()
                .await
                .values()
                .next()
                .unwrap()
                .clone()
        };
        assert!(is_private(&deployment), "{manifest}");
        let body = read(manifest.clone()).await;
        assert!(
            !body.contains("secret") && body.contains("<form"),
            "{manifest}"
        );
    }
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_scheduled_mutation_without_a_route_runs_only_on_its_schedule() {
    let root = std::env::temp_dir().join(format!("konae-scheduled-action-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let program = json!({"version": 1, "actions": {"holds.release": {
        "kind": "mutation", "access": "anonymous", "schedule": {"every": "15m"},
        "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL", "bindings": [{"source": "kernel", "field": "now"}]
    }}});
    for deployment in state.deployments.write().await.values_mut() {
        deployment.actions = program.to_string();
        deployment.manifest = json!({"visibility": "public"});
    }
    let response = invoke_action(
        State(state),
        AxumPath("holds.release".into()),
        headers,
        Json(json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&bytes).contains("Action holds.release runs only on its schedule.")
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn redeploy_adds_tables_without_losing_rows_and_rejects_destructive_schema() {
    let root = std::env::temp_dir().join(format!("konae-schema-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();

    let initial = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
    let original_action = action_program(&[("tasks.list", "SELECT id, title FROM tasks")]);
    apply_schema(&path, initial, &original_action).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("INSERT INTO tasks(title) VALUES ('Keep this row')", [])
        .unwrap();

    let extended = format!(
        "{initial} CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL); CREATE INDEX IF NOT EXISTS notes_body ON notes(body);"
    );
    let extended_actions = action_program(&[
        ("tasks.list", "SELECT id, title FROM tasks"),
        ("notes.list", "SELECT id, body FROM notes"),
    ]);
    apply_schema(&path, &extended, &extended_actions).unwrap();
    apply_schema(&path, &extended, &extended_actions).unwrap();

    let connection = Connection::open(&path).unwrap();
    let title: String = connection
        .query_row("SELECT title FROM tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(title, "Keep this row");
    assert!(connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='notes')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    drop(connection);

    assert!(apply_schema(&path, "DROP TABLE tasks;", &extended_actions).is_err());
    let connection = Connection::open(&path).unwrap();
    let title: String = connection
        .query_row("SELECT title FROM tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(title, "Keep this row");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn schemas_may_call_safe_functions_but_not_dangerous_ones() {
    let root = std::env::temp_dir().join(format!("konae-schema-functions-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();
    let schema = "CREATE TABLE IF NOT EXISTS items (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL CHECK (length(name) > 0),
              title TEXT NOT NULL DEFAULT '',
              slug TEXT GENERATED ALWAYS AS (lower(name)) VIRTUAL,
              created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS items_title ON items(lower(title));
            CREATE INDEX IF NOT EXISTS items_named ON items(name) WHERE length(name) > 0;";
    let actions = action_program(&[("items.list", "SELECT id, name, slug FROM items")]);
    apply_schema(&path, schema, &actions).unwrap();
    let connection = Connection::open(&path).unwrap();
    assert!(connection.execute("INSERT INTO items(name) VALUES ('')", []).is_err(), "the CHECK constraint runs");
    connection.execute("INSERT INTO items(name) VALUES ('Mixed')", []).unwrap();
    let slug: String = connection.query_row("SELECT slug FROM items", [], |row| row.get(0)).unwrap();
    assert_eq!(slug, "mixed");
    drop(connection);

    for function in DENIED_FUNCTIONS {
        let schema = format!(
            "CREATE TABLE IF NOT EXISTS evil_{function} (id INTEGER PRIMARY KEY, path TEXT CHECK ({function}(path) IS NULL));"
        );
        let error = apply_schema(&path, &schema, &actions).unwrap_err().to_string();
        assert_eq!(error, format!("schema.sql may only create tables and indexes, but it contains {function}()."), "{function}");
    }
    let evil = format!("{schema} CREATE INDEX IF NOT EXISTS evil_index ON items(load_extension(name));");
    let error = apply_schema(&path, &evil, &actions).unwrap_err().to_string();
    assert_eq!(error, "schema.sql may only create tables and indexes, but it contains load_extension().");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_schema_value_and_an_action_pattern_are_bounded_and_no_trigger_or_view_gets_in() {
    let root = std::env::temp_dir().join(format!("konae-sql-bounds-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();
    let actions = action_program(&[("items.list", "SELECT id FROM items LIMIT 5")]);
    apply_schema(
        &path,
        "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT);",
        &actions,
    )
    .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("INSERT INTO items(name) VALUES ('a')", [])
        .unwrap();
    drop(connection);
    // An index whose expression builds a 200 MB value per row hits the
    // value limit instead of the memory of the Kernel.
    let heavy = "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT); CREATE INDEX IF NOT EXISTS heavy ON items(length(hex(zeroblob(200000000))));";
    let error = apply_schema(&path, heavy, &actions)
        .unwrap_err()
        .to_string();
    assert!(error.contains("too big"), "{error}");

    let db = Connection::open(&path).unwrap();
    restrict_database(&db).unwrap();
    let pattern = |length: usize| format!("SELECT 'x' LIKE '{}'", "%".repeat(length));
    db.query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize), [], |row| {
        row.get::<_, bool>(0)
    })
    .unwrap();
    let long = db
        .query_row(&pattern(MAX_LIKE_PATTERN_BYTES as usize + 1), [], |row| {
            row.get::<_, bool>(0)
        })
        .unwrap_err()
        .to_string();
    assert!(long.contains("pattern too complex"), "{long}");
    // Studio and the schema change open this connection too.
    for statement in [
        "CREATE TRIGGER t AFTER INSERT ON items BEGIN DELETE FROM items; END",
        "CREATE VIEW v AS SELECT 1",
        "CREATE TEMP TRIGGER t AFTER INSERT ON items BEGIN SELECT 1; END",
        "CREATE TEMP VIEW v AS SELECT 1",
    ] {
        assert!(
            db.execute_batch(statement)
                .unwrap_err()
                .to_string()
                .contains("not authorized"),
            "{statement}"
        );
    }
    drop(db);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn named_locks_leave_no_entry_once_released() {
    let root = std::env::temp_dir().join(format!("konae-locks-{}", Uuid::new_v4()));
    let state = test_state(&root);
    let count = |state: &AppState| state.db_locks.lock().unwrap().len();
    for i in 0..100 {
        drop(database_lock(&state, &format!("transfer:{i}")).await);
    }
    assert_eq!(count(&state), 0);

    let first = database_lock(&state, "shared").await;
    let waiter = {
        let state = state.clone();
        tokio::spawn(async move {
            let _second = database_lock(&state, "shared").await;
        })
    };
    tokio::task::yield_now().await;
    assert_eq!(count(&state), 1);
    drop(first);
    waiter.await.unwrap();
    assert_eq!(count(&state), 0, "the last holder removes the entry");

    // A caller that gives up while waiting removes the entry too.
    let held = database_lock(&state, "contended").await;
    let gave_up = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        database_lock(&state, "contended"),
    )
    .await;
    assert!(gave_up.is_err());
    assert_eq!(count(&state), 1, "the holder keeps its entry");
    drop(held);
    assert_eq!(count(&state), 0);
}

#[test]
fn redeploy_rolls_back_schema_when_an_action_does_not_prepare() {
    let root = std::env::temp_dir().join(format!("konae-schema-action-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();
    let invalid_action = action_program(&[("tasks.list", "SELECT missing FROM tasks")]);
    assert!(
        apply_schema(
            &path,
            "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);",
            &invalid_action,
        )
        .is_err()
    );
    let connection = Connection::open(&path).unwrap();
    assert!(!connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    let _ = std::fs::remove_dir_all(root);
}

fn test_state(root: &Path) -> AppState {
    AppState::for_test(&root)
}

fn input_binding(key: &str, value_type: &str, max_length: Option<usize>) -> ActionBinding {
    ActionBinding {
        source: "input".into(),
        key: Some(key.into()),
        value_type: Some(value_type.into()),
        max_length,
        nullable: false,
        field: None,
        value: None,
    }
}

#[test]
fn nullable_inputs_preserve_types_and_required_inputs() {
    for value_type in ["string", "number", "boolean"] {
        let mut binding = input_binding("value", value_type, Some(4));
        binding.nullable = true;
        let definition = action("query", "SELECT ?1", vec![binding]);
        assert!(validate_action("inputs.echo", &definition).is_ok());
        for input in [json!({}), json!({"value": null})] {
            assert_eq!(
                resolve_bindings(&definition, &input, None).unwrap(),
                vec![SqlValue::Null]
            );
        }
        assert!(
            resolve_bindings(&definition, &json!({"other": null}), None)
                .unwrap_err()
                .contains("Unexpected")
        );
        let wrong = match value_type {
            "string" => json!({"value": 5}),
            "number" => json!({"value": "5"}),
            _ => json!({"value": 0}),
        };
        assert!(resolve_bindings(&definition, &wrong, None).is_err());
        let required = action(
            "query",
            "SELECT ?1",
            vec![input_binding("value", value_type, Some(4))],
        );
        assert!(resolve_bindings(&required, &json!({}), None).is_err());
        assert!(resolve_bindings(&required, &json!({"value": null}), None).is_err());
    }
    let mut binding = input_binding("value", "string", Some(4));
    binding.nullable = true;
    let definition = action("query", "SELECT ?1", vec![binding]);
    assert!(resolve_bindings(&definition, &json!({"value": ""}), None).is_err());
    assert!(resolve_bindings(&definition, &json!({"value": "longer"}), None).is_err());
    assert_eq!(
        resolve_bindings(&definition, &json!({"value": "ship"}), None).unwrap(),
        vec![SqlValue::Text("ship".into())]
    );
    for source in ["actor", "kernel", "literal"] {
        let mut binding = input_binding("value", "string", None);
        binding.source = source.into();
        binding.nullable = true;
        assert!(
            validate_action("inputs.echo", &action("query", "SELECT ?1", vec![binding])).is_err()
        );
    }
}

#[test]
fn mutation_returning_fails_schema_validation_without_executing() {
    let root = std::env::temp_dir().join(format!("konae-returning-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();
    let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT, \"RETURNING\" INTEGER);";
    for sql in [
        "INSERT INTO notes(body) VALUES ('note') RETURNING id",
        "UPDATE notes SET body='note' returning body",
        "DELETE FROM notes RETURNING *",
    ] {
        let program = ActionProgram {
            version: 1,
            actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]),
        };
        assert!(
            apply_schema(&path, schema, &program)
                .unwrap_err()
                .to_string()
                .contains("cannot use RETURNING")
        );
    }
    for sql in [
        "INSERT INTO notes(body) VALUES ('RETURNING')",
        "UPDATE notes SET body='note' /* RETURNING id */",
        "UPDATE notes SET \"RETURNING\"=1",
    ] {
        let program = ActionProgram {
            version: 1,
            actions: HashMap::from([("notes.write".into(), action("mutation", sql, vec![]))]),
        };
        apply_schema(&path, schema, &program).unwrap();
    }
    let count: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn artifact_content_types_follow_the_extension() {
    for (path, expected) in [
        ("index.html", "text/html; charset=utf-8"),
        ("docs/page.HTM", "text/html; charset=utf-8"),
        ("app.js", "text/javascript; charset=utf-8"),
        ("chunk.mjs", "text/javascript; charset=utf-8"),
        ("styles.css", "text/css; charset=utf-8"),
        ("data.json", "application/json"),
        ("app.js.map", "application/json"),
        ("site.webmanifest", "application/manifest+json"),
        ("robots.txt", "text/plain; charset=utf-8"),
        ("logo.svg", "image/svg+xml"),
        ("a.png", "image/png"),
        ("a.jpg", "image/jpeg"),
        ("a.JPEG", "image/jpeg"),
        ("a.gif", "image/gif"),
        ("a.webp", "image/webp"),
        ("favicon.ico", "image/x-icon"),
        ("module.wasm", "application/wasm"),
        ("font.woff2", "font/woff2"),
        ("font.woff", "font/woff"),
        ("font.ttf", "font/ttf"),
        ("font.otf", "font/otf"),
        ("sitemap.xml", "application/xml; charset=utf-8"),
        ("feed.rss", "application/rss+xml; charset=utf-8"),
        ("feed.atom", "application/atom+xml; charset=utf-8"),
        ("notes.md", "text/markdown; charset=utf-8"),
        ("rows.csv", "text/csv; charset=utf-8"),
        ("photo.avif", "image/avif"),
        ("menu.pdf", "application/pdf"),
        ("archive.zip", "application/octet-stream"),
        ("LICENSE", "application/octet-stream"),
        ("dir.html/file", "application/octet-stream"),
    ] {
        assert_eq!(content_type(path), expected, "path={path}");
    }
    let response = artifact_response(b"x".to_vec(), "x.bin");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/octet-stream");
}

#[test]
fn number_inputs_bind_integers_as_integer_and_fractions_as_real() {
    let definition = action(
        "mutation",
        "INSERT INTO items(label) VALUES (?)",
        vec![input_binding("value", "number", None)],
    );
    assert_eq!(
        resolve_bindings(&definition, &json!({"value": 5}), None).unwrap(),
        vec![SqlValue::Integer(5)]
    );
    assert_eq!(
        resolve_bindings(&definition, &json!({"value": -3}), None).unwrap(),
        vec![SqlValue::Integer(-3)]
    );
    assert_eq!(
        resolve_bindings(&definition, &json!({"value": 5.5}), None).unwrap(),
        vec![SqlValue::Real(5.5)]
    );
    assert_eq!(
        resolve_bindings(&definition, &json!({"value": u64::MAX}), None).unwrap(),
        vec![SqlValue::Real(u64::MAX as f64)]
    );

    let root = std::env::temp_dir().join(format!("konae-number-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute("CREATE TABLE items(label TEXT)", [])
        .unwrap();
    let params = resolve_bindings(&definition, &json!({"value": 5}), None).unwrap();
    run_mutation(&path, &definition, params, None).unwrap();
    let stored: String = Connection::open(&path)
        .unwrap()
        .query_row("SELECT label FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, "5");
    let _ = std::fs::remove_dir_all(root);
}

/// ADR-0012: a notice that lists or addresses rows reads them back
/// through the Kernel's RETURNING clause; the answer stays changes and

#[test]
fn a_mutation_reads_back_its_changed_rows_for_the_notice_only() {
    let program: ActionProgram = serde_json::from_value(json!({"version": 1, "actions": {
        "tasks.remind": {"kind": "mutation", "access": "authenticated", "schedule": {"every": "1h"},
            "statement": "UPDATE tasks SET reminded_at = ?1 WHERE reminded_at IS NULL; -- hourly",
            "bindings": [{"source": "kernel", "field": "now"}],
            "notify": {"to": {"column": "assignee"}, "changes": ["title"], "subject": "Due", "body": "{changes}"}}
    }})).unwrap();
    let definition = &program.actions["tasks.remind"];
    let root = std::env::temp_dir().join(format!("konae-returning-rows-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap().execute_batch(
        "CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, assignee TEXT, reminded_at INTEGER);
             INSERT INTO tasks(title, assignee, reminded_at) VALUES ('a', 'ana', NULL), ('b', 'ben', NULL), ('c', 'ana', 5);",
    ).unwrap();
    let ran = run_mutation(&path, definition, vec![SqlValue::Integer(9)], Some(notify::MutationContext {
        app: "test-app".into(), release: "release-1".into(), action: "tasks.remind".into(), input: json!({"source":"test"}),
        account: Some("account-1".into()), actor_id: Some("actor-1".into()), at: 9,
    })).unwrap();
    assert_eq!(ran.answer, json!({"changes": 2, "lastInsertId": 0}));
    let mut rows: Vec<Value> = ran.rows.iter().cloned().map(Value::Object).collect();
    rows.sort_by_key(|row| row["title"].to_string());
    assert_eq!(
        rows,
        vec![
            json!({"title": "a", "assignee": "ana"}),
            json!({"title": "b", "assignee": "ben"})
        ]
    );
    assert!(ran.notify_intent.is_some());
    let stored: String = Connection::open(&path).unwrap().query_row(
        "SELECT payload_json FROM __konae_notify_intents WHERE id=?1 AND state='pending'",
        [ran.notify_intent.as_deref().unwrap()], |row| row.get(0),
    ).unwrap();
    let payload: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(payload["release"], "release-1");
    assert_eq!(payload["action"], "tasks.remind");
    assert_eq!(payload["input"]["source"], "test");
    assert_eq!(payload["account"], "account-1");
    assert_eq!(payload["actor_id"], "actor-1");
    assert_eq!(payload["rows"].as_array().unwrap().len(), 2);
    let reminded: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM tasks WHERE reminded_at = 9",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reminded, 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn notify_intent_capture_failure_rolls_back_the_mutation_and_leaves_no_intent() {
    let root = std::env::temp_dir().join(format!("konae-notify-capture-failure-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap().execute_batch(
        "CREATE TABLE items(value TEXT);
         CREATE TABLE __konae_notify_intents(id TEXT);",
    ).unwrap();
    let action: ActionDefinition = serde_json::from_value(json!({
        "kind":"mutation","access":"anonymous","statement":"INSERT INTO items(value) VALUES ('must rollback')","bindings":[],
        "notify":{"to":"owner","subject":"Changed","body":"Stored"}
    })).unwrap();
    let outcome = run_mutation(&path, &action, Vec::new(), Some(notify::MutationContext {
        app:"capture-app".into(), release:"release-2".into(), action:"items.create".into(), input:json!({}),
        account:None, actor_id:None, at:10,
    }));
    assert!(outcome.is_err());
    let db = Connection::open(&path).unwrap();
    let item_count: i64 = db.query_row("SELECT count(*) FROM items", [], |row| row.get(0)).unwrap();
    assert_eq!(item_count, 0, "the SQL change rolls back when effect capture fails");
    let intent_count: i64 = db.query_row("SELECT count(*) FROM __konae_notify_intents", [], |row| row.get(0)).unwrap();
    assert_eq!(intent_count, 0);
    assert!(app_db::open(&path).unwrap().query_row("SELECT payload_json FROM __konae_notify_intents", [], |row| row.get::<_, String>(0)).is_err(),
        "app SQL cannot read the internal notify table");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn string_max_length_counts_utf16_code_units() {
    let two = action(
        "mutation",
        "SELECT ?",
        vec![input_binding("title", "string", Some(2))],
    );
    let one = action(
        "mutation",
        "SELECT ?",
        vec![input_binding("title", "string", Some(1))],
    );

    let emoji = json!({"title": "\u{1F600}"});
    assert!(resolve_bindings(&two, &emoji, None).is_ok());
    assert!(
        resolve_bindings(&one, &emoji, None)
            .unwrap_err()
            .contains("too long")
    );
    assert!(resolve_bindings(&two, &json!({"title": "\u{e9}\u{e9}"}), None).is_ok());
}

async fn deploy_files(state: &AppState, files: Value) -> Value {
    deploy_with_visibility(state, files, "public").await
}

async fn deploy_with_visibility(state: &AppState, files: Value, visibility: &str) -> Value {
    let body = json!({
        "project": {
            "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
            "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
        },
        "visibility": visibility,
        "artifact": {"version": 1, "entrypoint": "index.html", "files": files}
    });
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
}

#[tokio::test]
async fn a_first_release_takes_the_slug_hostname_when_free() {
    let root = std::env::temp_dir().join(format!("konae-slug-host-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
    assert_eq!(first["hostname"], "republish.localhost");
    assert_eq!(first["url"], "http://republish.localhost");
    let again = deploy_files(&state, json!({"index.html": "<p>two</p>"})).await;
    assert_eq!(again["hostname"], "republish.localhost");
    let body = json!({
        "project": {
            "id": "app_other", "name": "Other", "slug": "republish", "manifest": {},
            "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
        },
        "visibility": "public",
        "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>other</p>"}}
    });
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let other = serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone();
    let host = other["hostname"].as_str().unwrap();
    assert!(host.starts_with("republish-"), "{host}");
    assert!(host.ends_with(".localhost"), "{host}");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_browser_navigation_to_a_page_counts_one_page_view_by_the_file_served() {
    let root = std::env::temp_dir().join(format!("konae-page-views-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    deploy_files(
        &state,
        json!({"index.html": "<p>home</p>", "about.html": "<p>about</p>", "app.js": "1"}),
    )
    .await;
    let (host, deployment) = state
        .deployments
        .read()
        .await
        .iter()
        .next()
        .map(|(h, d)| (h.clone(), d.clone()))
        .unwrap();
    let get = |path: &str, extra: &[(&'static str, &'static str)]| {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
        for (name, value) in extra {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        serve_key(state.clone(), headers, path.into())
    };
    let navigation = [("sec-fetch-dest", "document")];
    assert_eq!(
        get("index.html", &navigation).await.status(),
        StatusCode::OK
    );
    assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);
    assert_eq!(get("about", &navigation).await.status(), StatusCode::OK);

    get("index.html", &[]).await;
    get("index.html", &[("sec-fetch-dest", "script")]).await;
    get("app.js", &navigation).await;
    get(
        "index.html",
        &[
            ("sec-fetch-dest", "document"),
            ("sec-purpose", "prefetch;prerender"),
        ],
    )
    .await;
    assert_eq!(
        get("missing", &navigation).await.status(),
        StatusCode::NOT_FOUND
    );
    usage::flush();
    let today = usage::day(now_ms());
    let usage =
        usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
    let views: Vec<_> = usage["totals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "page_view")
        .cloned()
        .collect();
    assert_eq!(
        views,
        vec![
            json!({"kind": "page_view", "name": "about.html", "count": 2}),
            json!({"kind": "page_view", "name": "index.html", "count": 1}),
        ]
    );

    for deployment in state.deployments.write().await.values_mut() {
        deployment.manifest["routing"] = json!("spa");
    }
    assert_eq!(get("items/3", &navigation).await.status(), StatusCode::OK);
    assert_eq!(
        get("missing-chunk.js", &[("sec-fetch-dest", "script")])
            .await
            .status(),
        StatusCode::OK
    );
    usage::flush();
    let usage =
        usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
    let index = usage["totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "page_view" && row["name"] == "index.html")
        .cloned();
    assert_eq!(
        index,
        Some(json!({"kind": "page_view", "name": "index.html", "count": 2}))
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn seo_lists_only_titled_pages_and_revalidates_the_chosen_address() {
    let root = std::env::temp_dir().join(format!("konae-seo-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let mut state = test_state(&root);
    state.public_origin = "https://konae.example.test".into();
    state.guards =
        Arc::new(Guards::new(Vec::new()).with_statement_wait(std::time::Duration::from_millis(5)));
    deploy_files(&state, json!({
        "index.html": "<!doctype html><html><head><title>Home</title><meta name=\"description\" content=\"Start here\"></head><body>Home</body></html>",
        "about.html": "<!doctype html><html><head><title>A \"small\" <app> &amp; more</title><meta property=\"og:title\" content=\"Owner title\"></head><body>About</body></html>",
        "post.html": "<!doctype html><html><head><title>Post template</title></head><body>Post</body></html>",
        "untitled.html": "<html><body>Missing title</body></html>",
        "robots.txt": "User-agent: special\nDisallow: /private\n",
        "card.png": "image"
    })).await;
    let (host, deployment) = {
        let mut deployments = state.deployments.write().await;
        let (host, deployment) = deployments.iter_mut().next().unwrap();
        deployment.actions = r#"{"version":1,"actions":{"posts.by":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts WHERE slug = ?1 LIMIT 1","bindings":[{"source":"input","key":"slug","type":"string"}],"page":{"path":"/posts/:slug","file":"post.html","list":"posts.list"}},"posts.list":{"kind":"query","access":"anonymous","statement":"SELECT 'listed' AS slug, ?1 AS updated LIMIT 1","bindings":[{"source":"kernel","field":"now"}]}}}"#.into();
        (host.clone(), deployment.clone())
    };
    if let Err(error_value) = action_shape::read(&deployment.actions) {
        panic!("{error_value}");
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
    let _off = serve_key(state.clone(), headers.clone(), "index.html".into()).await;
    {
        let mut deployments = state.deployments.write().await;
        let app = deployments.get_mut(&host).unwrap();
        app.modules.get_mut("seo").unwrap()["on"] = json!(true);
        app.modules.get_mut("seo").unwrap()["canonical"] = Value::Null;
        app.modules.get_mut("seo").unwrap()["image"] = json!("card.png");
    }
    let home = serve_key(state.clone(), headers.clone(), "index.html".into()).await;
    let home_body = axum::body::to_bytes(home.into_body(), 100_000)
        .await
        .unwrap();
    let home_html = String::from_utf8(home_body.to_vec()).unwrap();
    for card in ["og:title", "og:description", "og:url", "og:image"] {
        assert!(home_html.contains(card), "missing {card}");
    }
    let about = serve_key(state.clone(), headers.clone(), "about".into()).await;
    let old_etag = about.headers()[header::ETAG].to_str().unwrap().to_owned();
    let about_body = axum::body::to_bytes(about.into_body(), 100_000)
        .await
        .unwrap();
    let about_html = String::from_utf8(about_body.to_vec()).unwrap();
    assert!(about_html.contains("<title>A \"small\" <app> &amp; more</title>"));
    assert!(about_html.contains("content=\"Owner title\""));
    assert!(
        about_html.contains("href=\"https://republish.localhost/about\""),
        "{about_html}"
    );
    let mut navigation_headers = headers.clone();
    navigation_headers.insert(
        header::IF_NONE_MATCH,
        HeaderValue::from_str(&old_etag).unwrap(),
    );
    navigation_headers.insert("sec-fetch-dest", HeaderValue::from_static("document"));
    let navigation_revalidation =
        serve_key(state.clone(), navigation_headers, "about".into()).await;
    assert_eq!(navigation_revalidation.status(), StatusCode::NOT_MODIFIED);
    let robots = serve_key(state.clone(), headers.clone(), "robots.txt".into()).await;
    assert!(
        String::from_utf8(
            axum::body::to_bytes(robots.into_body(), 100_000)
                .await
                .unwrap()
                .to_vec()
        )
        .unwrap()
        .contains("Disallow: /private")
    );
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(state.guards.statement_permit(&deployment.id).await.unwrap());
    }
    let blocked_sitemap = serve_key(state.clone(), headers.clone(), "sitemap.xml".into()).await;
    let blocked_sitemap = String::from_utf8(
        axum::body::to_bytes(blocked_sitemap.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!blocked_sitemap.contains("/posts/listed"));
    drop(held);
    let sitemap = serve_key(state.clone(), headers.clone(), "sitemap.xml".into()).await;
    let sitemap_etag = sitemap.headers()[header::ETAG].to_str().unwrap().to_owned();
    let sitemap = String::from_utf8(
        axum::body::to_bytes(sitemap.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(sitemap.contains("/about"));
    assert!(sitemap.contains("/posts/listed"));
    assert!(!sitemap.contains("/post</loc>"));
    assert!(!sitemap.contains("untitled"));
    let mut sitemap_headers = headers.clone();
    sitemap_headers.insert(
        header::IF_NONE_MATCH,
        HeaderValue::from_str(&sitemap_etag).unwrap(),
    );
    let sitemap_revalidation =
        serve_key(state.clone(), sitemap_headers, "sitemap.xml".into()).await;
    assert_eq!(sitemap_revalidation.status(), StatusCode::NOT_MODIFIED);
    let template = serve_key(state.clone(), headers.clone(), "post.html".into()).await;
    assert_eq!(template.headers()["x-robots-tag"], "noindex");
    let listed = modules::read(&state, &deployment);
    assert_eq!(
        listed["modules"]["seo"]["current"]["canonical"]["reason"],
        "Generated hostname is the default."
    );
    assert!(
        listed["modules"]["seo"]["current"]["excluded"]
            .to_string()
            .contains("untitled.html")
    );
    let domain_file =
        crate::layout::private(&root, &deployment.id).join(crate::domain::RECORD_FILE);
    tokio::fs::create_dir_all(domain_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &domain_file,
        serde_json::to_vec(
            &json!({"hostname":"app.example.test","token":"test","verified":true,"createdAt":0}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    headers.insert(
        header::IF_NONE_MATCH,
        HeaderValue::from_str(&old_etag).unwrap(),
    );
    let changed = serve_key(state.clone(), headers, "about.html".into()).await;
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = String::from_utf8(
        axum::body::to_bytes(changed.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(changed.contains("https://app.example.test/about"));
    state
        .deployments
        .write()
        .await
        .get_mut(&host)
        .unwrap()
        .manifest["routing"] = json!("spa");
    let fallback = serve_key(
        state.clone(),
        HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]),
        "deep/page".into(),
    )
    .await;
    assert_eq!(fallback.headers()["x-robots-tag"], "noindex");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn page_queries_wait_through_database_swap_and_keep_the_new_wal() {
    let root = std::env::temp_dir().join(format!("konae-page-swap-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    deploy_files(&state, json!({"index.html":"<title>Home</title>","post.html":"<html><head><title>Template</title></head><body></body></html>"})).await;
    let (host, deployment) = {
        let mut deployments = state.deployments.write().await;
        let (host, deployment) = deployments.iter_mut().next().unwrap();
        deployment.actions = r#"{"version":1,"actions":{"posts.head":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts WHERE slug = ?1 LIMIT 1","bindings":[{"source":"input","key":"slug","type":"string"}],"page":{"path":"/posts/:slug","file":"post.html"}}}}"#.into();
        deployment.artifact_version = format!("{}-page-swap", deployment.artifact_version);
        (host.clone(), deployment.clone())
    };
    let routes = http::route_table(&deployment);
    assert!(routes.has_pages());
    let live = crate::layout::database(&root, &deployment.id);
    let staged = crate::layout::private(&root, &deployment.id).join("page-swap-staged.sqlite");
    let replaced = crate::layout::private(&root, &deployment.id).join("page-swap-replaced.sqlite");
    for path in [&live, &staged] {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch("CREATE TABLE posts(slug TEXT PRIMARY KEY, title TEXT);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO posts VALUES('hello', ?1)",
                [if path == &live { "Old" } else { "New" }],
            )
            .unwrap();
    }
    let lock = database_lock(&state, &deployment.id).await;
    let (at_gap_tx, at_gap_rx) = std::sync::mpsc::channel();
    let (continue_tx, continue_rx) = std::sync::mpsc::channel();
    let swap = tokio::task::spawn_blocking(move || {
        swap_database_with(&live, &staged, &replaced, || {
            at_gap_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
        })
    });
    tokio::task::spawn_blocking(move || at_gap_rx.recv().unwrap())
        .await
        .unwrap();
    let headers = HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]);
    let request = tokio::spawn(serve_key_with_routes(
        state.clone(),
        headers,
        "posts/hello".into(),
        Some(routes),
    ));
    let blocked = tokio::time::timeout(std::time::Duration::from_millis(20), async {
        while !request.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err();
    continue_tx.send(()).unwrap();
    swap.await.unwrap().unwrap();
    assert!(
        blocked,
        "the page request completed inside the database swap"
    );
    let writer = Connection::open(&crate::layout::database(&root, &deployment.id)).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    writer
        .execute("INSERT INTO posts VALUES('later', 'WAL row')", [])
        .unwrap();
    assert!(
        crate::layout::database(&root, &deployment.id)
            .with_extension("sqlite-wal")
            .exists()
            || {
                let mut sidecar = crate::layout::database(&root, &deployment.id)
                    .as_os_str()
                    .to_owned();
                sidecar.push("-wal");
                std::path::PathBuf::from(sidecar).exists()
            }
    );
    drop(lock);
    let response = request.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("New"), "{body}");
    writer
        .execute("UPDATE posts SET title=NULL WHERE slug='hello'", [])
        .unwrap();
    let headers = HeaderMap::from_iter([(header::HOST, HeaderValue::from_str(&host).unwrap())]);
    let untitled = serve_key_with_routes(
        state.clone(),
        headers,
        "posts/hello".into(),
        Some(http::route_table(&deployment)),
    )
    .await;
    assert_eq!(untitled.headers()["x-robots-tag"], "noindex");
    let untitled = String::from_utf8(
        axum::body::to_bytes(untitled.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(untitled.contains("name=\"konae-page-params\""));
    assert!(untitled.contains("&quot;slug&quot;:&quot;hello&quot;"));
    let reopened = Connection::open(crate::layout::database(&root, &deployment.id)).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT title FROM posts WHERE slug='later'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "WAL row"
    );
    drop(writer);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_file_the_browser_holds_answers_304_until_the_next_publish() {
    let root = std::env::temp_dir().join(format!("konae-etag-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "1"})).await;
    for deployment in state.deployments.write().await.values_mut() {
        deployment.manifest["visibility"] = json!("public");
    }
    let (host, deployment) = state
        .deployments
        .read()
        .await
        .iter()
        .next()
        .map(|(h, d)| (h.clone(), d.clone()))
        .unwrap();
    let get = |path: &str, extra: Vec<(&'static str, String)>| {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
        for (name, value) in extra {
            headers.insert(name, HeaderValue::from_str(&value).unwrap());
        }
        serve_key(state.clone(), headers, path.into())
    };
    let first = get("app.js", vec![]).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "public, no-cache");
    let tag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(tag, format!("\"{}\"", deployment.artifact_version.clone()));

    let again = get("app.js", vec![("if-none-match", tag.clone())]).await;
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(again.headers()[header::ETAG], tag.as_str());
    assert!(
        axum::body::to_bytes(again.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    let weak = get(
        "app.js",
        vec![("if-none-match", format!("\"other\", W/{tag}"))],
    )
    .await;
    assert_eq!(weak.status(), StatusCode::NOT_MODIFIED);

    // A navigation still resolves its file, so the page view counts.
    let page = get(
        "index.html",
        vec![
            ("if-none-match", tag.clone()),
            ("sec-fetch-dest", "document".into()),
        ],
    )
    .await;
    assert_eq!(page.status(), StatusCode::NOT_MODIFIED);
    usage::flush();
    let today = usage::day(now_ms());
    let usage =
        usage::read_usage(&crate::layout::usage(&root, &deployment.id), &today, &today).unwrap();
    assert!(
        usage["totals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "page_view" && row["name"] == "index.html")
    );

    // A publish changes the tag, so the old one gets the new bytes.
    deploy_files(&state, json!({"index.html": "<p>home</p>", "app.js": "2"})).await;
    for deployment in state.deployments.write().await.values_mut() {
        deployment.manifest["visibility"] = json!("public");
    }
    let fresh = get("app.js", vec![("if-none-match", tag.clone())]).await;
    assert_eq!(fresh.status(), StatusCode::OK);
    assert_ne!(fresh.headers()[header::ETAG], tag.as_str());
    assert_eq!(
        &axum::body::to_bytes(fresh.into_body(), 1024).await.unwrap()[..],
        b"2"
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn a_private_apps_files_stay_out_of_shared_caches() {
    let deployment = Settings::new("app", "App", "app", &json!({"visibility": "private"}))
        .deployment(
            Installed {
                hostname: "app.localhost".into(),
                created_at: 1,
                released_at: None,
                source_digest: None,
                actions: "{}".into(),
                artifact_version: "v1".into(),
                released: None,
            },
            "http://localhost",
        );
    assert_eq!(
        ArtifactCache::of(&deployment).control(),
        "private, no-cache"
    );
}

#[test]
fn a_record_without_a_release_time_reads_its_first_publish() {
    let installed: Installed =
        serde_json::from_value(json!({"hostname": "app.localhost", "createdAt": 7, "actions": "{}", "artifactVersion": "v1"})).unwrap();
    let deployment = Settings::new("app", "App", "app", &json!({"visibility": "public"})).deployment(installed, "http://localhost");
    assert_eq!(deployment.released_at, 7);
    assert_eq!(serde_json::to_value(&deployment).unwrap()["releasedAt"], json!(7));
}

#[tokio::test]
async fn a_republish_moves_the_release_time_and_keeps_the_first_publish() {
    let root = std::env::temp_dir().join(format!("konae-released-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
    assert!(first["releasedAt"].as_u64().unwrap() >= first["createdAt"].as_u64().unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = deploy_files(&state, json!({"index.html": "<p>two</p>"})).await;
    assert_eq!(second["createdAt"], first["createdAt"]);
    assert!(second["releasedAt"].as_u64().unwrap() > first["releasedAt"].as_u64().unwrap());
    let id = second["id"].as_str().unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(crate::layout::record(&root, id)).unwrap()).unwrap();
    assert_eq!(record["releasedAt"], second["releasedAt"]);
    let loaded = load_deployments(&root, "http://localhost").await.unwrap();
    assert_eq!(json!(loaded.values().next().unwrap().released_at), second["releasedAt"]);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn deployment_loading_recovers_legacy_files_after_an_operator_settings_write() {
    let root = std::env::temp_dir().join(format!("konae-load-legacy-files-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    deploy_files(
        &state,
        json!({"index.html":"<script>miniapp.files.list()</script>"}),
    )
    .await;
    let id = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .unwrap()
        .id
        .clone();
    let path = layout::settings(&root, &id);
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    raw.as_object_mut().unwrap().remove("modules");
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    Settings::read(&root, &id)
        .unwrap()
        .write(&root)
        .await
        .unwrap();
    let loaded = load_deployments(&root, "http://localhost").await.unwrap();
    assert!(loaded.values().next().unwrap().modules["files"]["on"] == true);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn konae_load_decides_legacy_auth_before_existing_accounts_sign_in() {
    let root = std::env::temp_dir().join(format!("konae-legacy-auth-load-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let released = deploy_files(&state, json!({"index.html": "<p>Members</p>"})).await;
    let id = released["id"].as_str().unwrap();
    let original = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
    accounts::app_store(&state, &original).unwrap().signup("member@example.test", "correct horse battery").unwrap();
    let settings_path = layout::settings(&root, id);
    let mut old: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    old["modules"].as_object_mut().unwrap().remove("auth");
    std::fs::write(&settings_path, serde_json::to_vec(&old).unwrap()).unwrap();

    let loaded = load_deployments(&root, "http://localhost").await.unwrap();
    let deployment = loaded.values().find(|deployment| deployment.id == id).unwrap();
    assert_eq!(deployment.modules["auth"]["on"], true);
    let auth = accounts::app_store(&state, deployment).unwrap();
    let session = auth.login("member@example.test", "correct horse battery").unwrap();
    assert!(auth.authenticate(Some(&session.token)).unwrap().is_some());
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_release_names_the_digest_of_the_sources_it_carried() {
    let root = std::env::temp_dir().join(format!("konae-source-digest-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
    let sources = [
        ("actions.json", "{\"version\":1,\"actions\":{}}"),
        ("schema.sql", ""),
    ];
    let expected = sources::digest(sources.iter().map(|(path, text)| (*path, text.as_bytes())));
    assert_eq!(first["sourceDigest"], json!(expected));
    let id = first["id"].as_str().unwrap();
    let record: Value =
        serde_json::from_slice(&std::fs::read(crate::layout::record(&root, id)).unwrap()).unwrap();
    assert_eq!(record["sourceDigest"], json!(expected));
    // A write that is not a release keeps the digest.
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    persist_deployment(&state, &deployment).await.unwrap();
    let loaded = load_deployments(&root, "http://localhost").await.unwrap();
    assert_eq!(
        loaded.values().next().unwrap().source_digest.as_deref(),
        Some(expected.as_str())
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn a_record_without_a_source_digest_names_none() {
    let installed: Installed =
        serde_json::from_value(json!({"hostname": "app.localhost", "createdAt": 7, "actions": "{}", "artifactVersion": "v1"})).unwrap();
    let deployment = Settings::new("app", "App", "app", &json!({"visibility": "public"})).deployment(installed, "http://localhost");
    assert_eq!(deployment.source_digest, None);
    assert!(serde_json::to_value(&deployment).unwrap().get("sourceDigest").is_none());
}

#[tokio::test]
async fn republish_stops_serving_files_missing_from_the_new_artifact() {
    let root = std::env::temp_dir().join(format!("konae-republish-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let first = deploy_files(
        &state,
        json!({"index.html": "<p>one</p>", "old.js": "1", "assets/old.css": "a{}"}),
    )
    .await;
    let id = first["id"].as_str().unwrap().to_owned();
    let deployment = state.deployments.read().await.values().next().cloned().unwrap();
    assert_eq!(read_artifact(&state, &deployment, "old.js").await.unwrap(), b"1");

    deploy_files(&state, json!({"index.html": "<p>two</p>", "new.js": "2"})).await;
    let deployment = state.deployments.read().await.values().next().cloned().unwrap();
    assert_eq!(deployment.id, id);
    assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>two</p>");
        assert_eq!(read_artifact(&state, &deployment, "new.js").await.unwrap(), b"2");
    assert!(read_artifact(&state, &deployment, "old.js").await.is_err());
    assert!(read_artifact(&state, &deployment, "assets/old.css").await.is_err());
    assert!(staged_or_retired(&root, &id).is_empty(), "only dist/ holds files");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn new_deployments_pick_a_policy_from_visibility_and_redeploys_keep_it() {
    for (visibility, expected) in [
        ("public", SignupMode::Open),
        ("private", SignupMode::Closed),
    ] {
        let root = std::env::temp_dir().join(format!("konae-policy-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        let first =
            deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), visibility).await;
        assert_eq!(
            first["signupPolicy"]["mode"],
            json!(expected),
            "visibility={visibility}"
        );

        set_policy(&state, SignupMode::Allowlist, &["@example.test"]).await;
        let second =
            deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), visibility).await;
        assert_eq!(
            second["signupPolicy"],
            json!({"mode": "allowlist", "allowlist": ["@example.test"]})
        );
        let id = second["id"].as_str().unwrap();
        let stored = Settings::read(&root, id).unwrap();
        assert_eq!(stored.signup_policy.mode, SignupMode::Allowlist);
        let mut legacy = state
            .deployments
            .read()
            .await
            .values()
            .find(|deployment| deployment.id == id)
            .unwrap()
            .clone();
        legacy.manifest["runtime"] = json!({"database":{"mode":"declared-actions"}});
        assert!(released_settings(&legacy).manifest.get("runtime").is_none());
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}

#[tokio::test]
async fn an_operation_completes_a_legacy_applied_snapshot_from_live_files_and_database() {
    let root = std::env::temp_dir().join(format!(
        "konae-legacy-operation-snapshot-{}",
        Uuid::new_v4()
    ));
    let id = "app_legacy_operation";
    let settings = Settings::new(
        id,
        "Applied",
        "applied",
        &json!({"version":1,"visibility":"public"}),
    );
    settings.write(&root).await.unwrap();
    tokio::fs::create_dir_all(crate::layout::dist(&root, id))
        .await
        .unwrap();
    tokio::fs::write(
        crate::layout::dist(&root, id).join("index.html"),
        "old applied artifact",
    )
    .await
    .unwrap();
    let database = crate::layout::database(&root, id);
    tokio::fs::create_dir_all(database.parent().unwrap())
        .await
        .unwrap();
    rusqlite::Connection::open(&database).unwrap().execute_batch("CREATE TABLE \"live notes\"(id INTEGER PRIMARY KEY, title TEXT); CREATE INDEX \"live notes title\" ON \"live notes\"(title); INSERT INTO \"live notes\" VALUES(1,'kept');").unwrap();
    let installed = Installed {
        hostname: "applied.localhost".into(), created_at: 1, released_at: Some(2), source_digest: Some("old-source".into()),
        actions: r#"{"version":1,"actions":{"applied.read":{"kind":"query","access":"anonymous","statement":"SELECT title FROM \"live notes\" LIMIT 5","bindings":[]}}}"#.into(),
        artifact_version: "v-old".into(), released: None,
    };
    tokio::fs::create_dir_all(crate::layout::private(&root, id))
        .await
        .unwrap();
    tokio::fs::write(
        crate::layout::record(&root, id),
        serde_json::to_vec(&installed).unwrap(),
    )
    .await
    .unwrap();
    let state = test_state(&root);
    let deployment = settings.deployment(installed, "http://localhost");
    state
        .deployments
        .write()
        .await
        .insert(deployment.hostname.clone(), deployment.clone());

    let mut operated = deployment;
    operated.signup_policy = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
    persist_deployment(&state, &operated).await.unwrap();

    let migrated = Settings::read(&root, id).unwrap().released.unwrap();
    assert_eq!(migrated.signup_policy.mode, SignupMode::Closed);
    assert!(
        migrated
            .schema_sql
            .contains("CREATE TABLE IF NOT EXISTS \"live notes\"")
    );
    assert!(
        migrated
            .schema_sql
            .contains("CREATE INDEX IF NOT EXISTS \"live notes title\"")
    );
    let expected_digest = crate::folder::version_of(&HashMap::from([(
        "index.html".to_owned(),
        b"old applied artifact".to_vec(),
    )]));
    assert_eq!(
        migrated.artifact_digest.as_deref(),
        Some(expected_digest.as_str())
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn republishing_a_public_app_as_private_closes_an_open_signup() {
    // (first visibility, policy the owner set, second visibility, expected)
    let cases = [
        (
            "public",
            None,
            "private",
            json!({"mode": "closed", "allowlist": []}),
        ),
        (
            "public",
            Some((SignupMode::Allowlist, "@example.test")),
            "private",
            json!({"mode": "allowlist", "allowlist": ["@example.test"]}),
        ),
        // The owner opened a private app on purpose; a redeploy keeps that.
        (
            "private",
            Some((SignupMode::Open, "")),
            "private",
            json!({"mode": "open", "allowlist": []}),
        ),
        (
            "private",
            None,
            "public",
            json!({"mode": "closed", "allowlist": []}),
        ),
    ];
    for (first, chosen, second, expected) in cases {
        let root = std::env::temp_dir().join(format!("konae-flip-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        deploy_with_visibility(&state, json!({"index.html": "<p>one</p>"}), first).await;
        if second == "private" {
            let mut deployment = state
                .deployments
                .read()
                .await
                .values()
                .next()
                .unwrap()
                .clone();
            deployment.modules.get_mut("auth").unwrap()["on"] = json!(true);
            Settings::of(&deployment).write(&root).await.unwrap();
            state
                .deployments
                .write()
                .await
                .insert(deployment.hostname.clone(), deployment);
        }
        if let Some((mode, entry)) = chosen {
            let entries: Vec<&str> = if entry.is_empty() {
                vec![]
            } else {
                vec![entry]
            };
            set_policy(&state, mode, &entries).await;
        }
        let republished =
            deploy_with_visibility(&state, json!({"index.html": "<p>two</p>"}), second).await;
        assert_eq!(
            republished["signupPolicy"], expected,
            "{first} {chosen:?} -> {second}"
        );
        let id = republished["id"].as_str().unwrap();
        let stored = Settings::read(&root, id).unwrap();
        assert_eq!(
            serde_json::to_value(&stored.signup_policy).unwrap(),
            expected
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}

#[tokio::test]
async fn signup_checks_input_first_and_gives_one_answer_for_closed_and_unlisted() {
    let root = std::env::temp_dir().join(format!("konae-signup-oracle-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let attempt = |email: &'static str, password: &'static str| {
        let state = state.clone();
        let headers = headers.clone();
        async move {
            let body = json!({"email": email, "password": password});
            let request = Request::builder()
                .body(Body::from(body.to_string()))
                .unwrap();
            let response = signup(State(state), headers, request).await;
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap()["error"].clone(),
            )
        }
    };
    set_policy(&state, SignupMode::Allowlist, &["listed@example.test"]).await;
    // A weak password gets the same answer whether or not the address is listed.
    let listed = attempt("listed@example.test", "x").await;
    let unlisted = attempt("other@example.test", "x").await;
    assert_eq!(listed.0, StatusCode::BAD_REQUEST);
    assert_eq!(listed, unlisted);
    assert_eq!(
        attempt("not-an-email", "correct horse battery").await.0,
        StatusCode::BAD_REQUEST
    );
    let refused = attempt("other@example.test", "correct horse battery").await;
    assert_eq!(refused.0, StatusCode::FORBIDDEN);
    set_policy(&state, SignupMode::Closed, &[]).await;
    assert_eq!(
        attempt("listed@example.test", "correct horse battery").await,
        refused
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn app_databases_are_capped_in_size() {
    let root = std::env::temp_dir().join(format!("konae-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    initialize_database(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    restrict_database(&db).unwrap();
    db.authorizer::<fn(AuthContext<'_>) -> Authorization>(None).unwrap();
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    let pages: u64 = db.query_row("PRAGMA max_page_count", [], |r| r.get(0)).unwrap();
    assert_eq!(pages * page_size, MAX_APP_DATABASE_BYTES);
    drop(db);

    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE notes(body TEXT)").unwrap();
    limit_database_size(&db, 64 * 1024).unwrap();
    let mut full = None;
    for _ in 0..100 {
        if let Err(error_value) = db.execute("INSERT INTO notes VALUES (?1)", ["x".repeat(4000)]) {
            full = Some(error_value.to_string());
            break;
        }
    }
    assert!(full.expect("the cap must stop writes").contains("full"));
    let _ = std::fs::remove_dir_all(root);
}

fn staged_or_retired(root: &Path, id: &str) -> Vec<String> {
    std::fs::read_dir(layout::private(root, id))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("dist-") || name.starts_with("server-"))
        .collect()
}

fn release_body(
    id: &str,
    schema: &str,
    actions: Value,
    page: &str,
    confirmation: Option<String>,
) -> Value {
    json!({
        "project": {
            "id": id, "name": "Steps", "slug": "steps", "manifest": {},
            "files": {"actions.json": json!({"version": 1, "actions": actions}).to_string(), "schema.sql": schema}
        },
        "visibility": "public",
        "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": page}},
        "schemaConfirmation": confirmation
    })
}

#[tokio::test]
async fn a_static_release_with_only_seo_creates_no_action_database_and_serves_its_page() {
    let root = std::env::temp_dir().join(format!("konae-static-release-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = AppState::for_test(&root);
    let id = "app_static_home";
    let body = json!({
        "project":{"id":id,"name":"Home","slug":"home","manifest":{"version":1,"visibility":"public"},"files":{"index.html":"<!doctype html><title>Konae</title>"}},
        "visibility":"public",
        "initialModules":{"seo":{"on":true}},
        "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<!doctype html><title>Konae</title>"}}
    });
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let deployment = state.deployments.read().await.values().find(|deployment| deployment.id == id).unwrap().clone();
    assert_eq!(deployment.modules["seo"]["on"], true);
    assert_eq!(deployment.modules["actions"]["on"], false);
    assert!(!database_path(&state, id).exists());
    assert!(!crate::layout::identity(&root, id).exists());
    let html = read_artifact(&state, &deployment, "index.html").await.unwrap();
    let html = String::from_utf8(html).unwrap();
    assert!(html.contains("<title>Konae</title>"));
    assert!(!html.contains("runtime.js"));
    let reloaded = load_deployments(&root, "http://localhost").await.unwrap();
    assert_eq!(reloaded.values().find(|deployment| deployment.id == id).unwrap().modules["seo"]["on"], true);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_first_release_refuses_unknown_module_options_from_an_import_package() {
    let root =
        std::env::temp_dir().join(format!("konae-invalid-package-modules-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = AppState::for_test(&root);
    let mut body = release_body(
        "app_bad_package_modules",
        "",
        json!({}),
        "<p>Home</p>",
        None,
    );
    body["initialPackage"] = json!({
        "manifest": {
            "format": "konae-package", "version": 1, "packageId": "pkg_bad_modules", "createdAt": 1,
            "deploymentId": "dep_source", "projectId": "project_source", "visibility": "public",
            "modules": {
                "actions": {"on": false},
                "seo": {"on": true, "canonical": "path", "image": null, "futureOption": true}
            },
            "publishedRevision": 1, "contents": [], "excluded": []
        },
        "components": {}
    });
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let message = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        message.contains("Unknown option futureOption for module seo."),
        "{message}"
    );
    assert!(!crate::layout::settings(&root, "app_bad_package_modules").exists());
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn deployment_loading_skips_one_app_with_an_unknown_module_option() {
    let root = std::env::temp_dir().join(format!(
        "konae-load-invalid-module-option-{}",
        Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    for (id, hostname, invalid) in [
        ("app_bad_option", "bad.localhost", true),
        ("app_good_option", "good.localhost", false),
    ] {
        tokio::fs::create_dir_all(crate::layout::private(&root, id))
            .await
            .unwrap();
        let mut settings = Settings::new(id, id, id, &json!({"visibility":"public"}));
        if invalid {
            settings.modules.get_mut("seo").unwrap()["futureOption"] = json!(true);
        }
        settings.write(&root).await.unwrap();
        let installed = Installed {
            hostname: hostname.into(),
            created_at: 1,
            released_at: Some(1),
            source_digest: None,
            actions: r#"{"version":1,"actions":{}}"#.into(),
            artifact_version: "v1".into(),
            released: None,
        };
        tokio::fs::write(
            crate::layout::record(&root, id),
            serde_json::to_vec(&installed).unwrap(),
        )
        .await
        .unwrap();
    }
    let loaded = load_deployments(&root, "http://localhost").await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(
        loaded
            .values()
            .any(|deployment| deployment.id == "app_good_option")
    );
    assert!(
        !loaded
            .values()
            .any(|deployment| deployment.id == "app_bad_option")
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_release_with_schema_and_actions_off_gets_the_module_refusal() {
    let root = std::env::temp_dir().join(format!("konae-schema-actions-off-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = AppState::for_test(&root);
    let mut body = release_body(
        "app_schema_actions_off",
        "CREATE TABLE notes(id INTEGER PRIMARY KEY);",
        json!({}),
        "<main>Home</main>",
        None,
    );
    body["initialModules"] = json!({"actions":{"on":false}});
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let message = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{message}");
    assert!(message.contains("Module actions is off"), "{message}");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn an_actions_key_in_another_json_file_does_not_turn_on_actions() {
    let root = std::env::temp_dir().join(format!("konae-other-json-actions-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = AppState::for_test(&root);
    let mut body = release_body(
        "app_other_json_actions",
        "",
        json!({}),
        "<main>Home</main>",
        None,
    );
    body["initialModules"] = json!({"actions":{"on":false}});
    body["project"]["files"]["public/keys.json"] = json!("{\"actions\":{\"jump\":\"Space\"}}");
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let status = response.status();
    let message = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, StatusCode::OK, "{message}");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn release_checks_files_before_actions_when_both_modules_are_off() {
    let root = std::env::temp_dir().join(format!("konae-files-before-actions-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = AppState::for_test(&root);
    let mut body = release_body(
        "app_files_before_actions",
        "",
        json!({"notes.list":{"kind":"query","statement":"SELECT 1"}}),
        "<main>Home</main>",
        None,
    );
    body["initialModules"] = json!({"actions":{"on":false},"files":{"on":false}});
    body["artifact"]["files"]["app.js"] = json!("miniapp.files.list()");
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let refused = response_json(response).await;
    assert_eq!(refused["code"], "module.off");
    assert_eq!(
        refused["error"],
        "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_static_whole_document_can_release_without_a_browser_sdk_module() {
    let root = std::env::temp_dir().join(format!("konae-static-whole-document-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = AppState::for_test(&root);
    let mut body = release_body(
        "app_static_whole_document",
        "",
        json!({}),
        "<!doctype html><title>Static</title>",
        None,
    );
    body["initialModules"] =
        json!({"actions":{"on":false},"auth":{"on":false},"files":{"on":false}});
    body["project"]["files"]
        .as_object_mut()
        .unwrap()
        .remove("actions.json");
    body["project"]["files"]
        .as_object_mut()
        .unwrap()
        .remove("schema.sql");
    let response = deploy(&state, body.to_string().as_bytes()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn release_swaps_fresh_baseline_and_existing_data_transformations() {
    let root = std::env::temp_dir().join(format!("konae-release-transform-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root))
        .await
        .unwrap();
    let state = test_state(&root);
    let id = "app_transform_release";
    let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);";
    let actions = json!({
        "notes.list": {"kind":"query", "access":"anonymous", "statement":"SELECT body FROM notes ORDER BY id LIMIT 10", "bindings": []}
    });
    let v1 = json!({
        "id": "baseline", "predecessor": null,
        "before": format!("sha256:{}", "0".repeat(64)),
        "after": schema_transform::fingerprint_schema(schema).unwrap(),
        "maxRows": 10, "statements": ["CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)"]
    });
    let document = json!({"version": 1, "transitions": [v1.clone()]}).to_string();
    let mut first = release_body(id, schema, actions.clone(), "<p>first</p>", None);
    first["project"]["files"]["schema-transforms.json"] = json!(document);
    let response = deploy(&state, first.to_string().as_bytes()).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    async fn read_notes(state: &AppState) -> (StatusCode, String) {
        let deployment = state
            .deployments
            .read()
            .await
            .values()
            .next()
            .unwrap()
            .clone();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, deployment.hostname.parse().unwrap());
        let response = invoke_action(
            State(state.clone()),
            AxumPath("notes.list".into()),
            headers,
            Json(json!({})),
        )
        .await;
        let status = response.status();
        let body = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 100_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        (status, body)
    }

    assert_eq!(
        read_notes(&state).await.0,
        StatusCode::OK,
        "fresh baseline schema must reach the live database"
    );
    let database = database_path(&state, id);
    Connection::open(&database)
        .unwrap()
        .execute("INSERT INTO notes(body) VALUES('before')", [])
        .unwrap();
    let fingerprint = schema_transform::fingerprint(&Connection::open(&database).unwrap()).unwrap();
    let v2 = json!({
        "id": "rewrite", "predecessor": "baseline",
        "before": fingerprint, "after": schema_transform::fingerprint_schema(schema).unwrap(),
        "maxRows": 1, "statements": ["UPDATE notes SET body='after' WHERE id=1"]
    });
    let update_document = json!({"version": 1, "transitions": [v1, v2]}).to_string();
    let mut second = release_body(id, schema, actions, "<p>second</p>", None);
    second["project"]["files"]["schema-transforms.json"] = json!(update_document);
    let input: DeployInput = serde_json::from_value(second.clone()).unwrap();
    let confirmation = runtime::validate_release(&state, &input, None)
        .await
        .unwrap()
        .confirmation()
        .unwrap();
    second["schemaConfirmation"] = json!(confirmation);
    let response = deploy(&state, second.to_string().as_bytes()).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (status, body) = read_notes(&state).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("after"),
        "the release must swap the staged transformed data: {body}"
    );

    let restarted = test_state(&root);
    let deployment = folder::open(&restarted, id, None).await.unwrap();
    let (status, body) = read_notes(&restarted).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("after"),
        "the transformed rows must survive a restart: {body}"
    );
    let db = Connection::open(database_path(&restarted, id)).unwrap();
    let history = schema_transform::load_history(&db).unwrap();
    assert_eq!(history.entries.len(), 2);
    assert!(history.entries[0].baseline_skipped);
    assert_eq!(history.entries[1].id, "rewrite");
    assert_eq!(history.entries[1].affected_rows, 1);
    assert_eq!(deployment.id, id);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_pending_call_continuation_blocks_an_incompatible_release_until_finished() {
    let root = std::env::temp_dir().join(format!("konae-pending-schema-release-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let id = "app_pending_schema";
    let initial_schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, legacy TEXT);";
    let actions = json!({"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT id FROM notes LIMIT 10","bindings":[]}});
    let first = release_body(id, initial_schema, actions.clone(), "<p>A</p>", None);
    let response = deploy(&state, first.to_string().as_bytes()).await;
    let first_status = response.status();
    let first_body = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
    assert_eq!(first_status, StatusCode::OK, "{first_body}");
    let deployment_a = state.deployments.read().await.values().find(|item| item.id == id).unwrap().clone();
    let database = database_path(&state, id);
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch(include_str!("integrations/calls-app.sql")).unwrap();
    let continuation = json!({"action":"notes.finish", "definition":{
        "kind":"mutation", "access":"anonymous", "bindings":[],
        "statement":"UPDATE notes SET legacy='finished' WHERE id=1"
    }}).to_string();
    connection.execute("INSERT INTO __konae_calls (id,deployment,action,continuation,input,state,next_attempt_at,created_at,updated_at) VALUES('job-a', ?1, 'remote', ?2, '{}', 'pending', 0, 1, 1)", rusqlite::params![id, continuation]).unwrap();
    drop(connection);

    let changed_schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);";
    let candidate = release_body(id, changed_schema, actions, "<p>B</p>", None);
    let input: DeployInput = serde_json::from_value(candidate.clone()).unwrap();
    let refused = runtime::validate_release(&state, &input, None).await.unwrap_err();
    assert_eq!(crate::refusal::of(&refused).unwrap().code, "schema.pending-work");
    assert_eq!(state.deployments.read().await.values().find(|item| item.id == id).unwrap().artifact_version, deployment_a.artifact_version);
    assert!(std::fs::read_to_string(crate::layout::dist(&root, id).join("index.html")).unwrap().contains("A"));

    Connection::open(&database).unwrap().execute("UPDATE __konae_calls SET state='delivered',then_state='done' WHERE id='job-a'", []).unwrap();
    let confirmation = runtime::validate_release(&state, &input, None).await.unwrap().confirmation().unwrap();
    let mut candidate = candidate;
    candidate["schemaConfirmation"] = json!(confirmation);
    assert_eq!(deploy(&state, candidate.to_string().as_bytes()).await.status(), StatusCode::OK);
    assert!(std::fs::read_to_string(crate::layout::dist(&root, id).join("index.html")).unwrap().contains("B"));
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn static_get_keeps_body_and_etag_together_across_rollback_and_paused_publish() {
    async fn get(state: AppState, host: &str) -> (StatusCode, String, String) {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        let response = crate::serve_key(state, headers, "index.html".into()).await;
        let status = response.status();
        let etag = response.headers().get(header::ETAG).and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
        let body = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        (status, body, etag)
    }
    let root = std::env::temp_dir().join(format!("konae-publish-get-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);";
    let actions = json!({"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT id FROM notes LIMIT 10","bindings":[]}});
    let first = release_body("app_publish_get", schema, actions.clone(), "<p>A</p>", None);
    let response = deploy(&state, first.to_string().as_bytes()).await;
    let first_status = response.status();
    let first_body = String::from_utf8(axum::body::to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
    assert_eq!(first_status, StatusCode::OK, "{first_body}");
    let first_deployment = state.deployments.read().await.values().find(|item| item.id == "app_publish_get").unwrap().clone();
    let original = get(state.clone(), &first_deployment.hostname).await;
    assert_eq!(original.0, StatusCode::OK);
    assert!(original.1.contains("A"));

    let failed = release_body("app_publish_get", schema, actions.clone(), "<p>rollback</p>", None);
    FAILING_RELEASE_STEPS.lock().unwrap().push(("app_publish_get".into(), "swap_sources"));
    assert_ne!(deploy(&state, failed.to_string().as_bytes()).await.status(), StatusCode::OK);
    FAILING_RELEASE_STEPS.lock().unwrap().retain(|(id, _)| id != "app_publish_get");
    let after_rollback = get(state.clone(), &first_deployment.hostname).await;
    assert_eq!((after_rollback.1.as_str(), after_rollback.2.as_str()), (original.1.as_str(), original.2.as_str()));

    let second = release_body("app_publish_get", schema, actions.clone(), "<p>B</p>", None);
    let held = database_lock(&state, "app_publish_get").await;
    let publishing_state = state.clone();
    let publishing = tokio::spawn(async move { deploy(&publishing_state, second.to_string().as_bytes()).await.status() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let blocked_get_state = state.clone();
    let hostname = first_deployment.hostname.clone();
    let pending_get = tokio::spawn(async move { get(blocked_get_state, &hostname).await });
    let other = release_body("app_publish_other", schema, actions, "<p>other app</p>", None);
    assert_eq!(deploy(&state, other.to_string().as_bytes()).await.status(), StatusCode::OK);
    let other_host = state.deployments.read().await.values().find(|item| item.id == "app_publish_other").unwrap().hostname.clone();
    let other_response = get(state.clone(), &other_host).await;
    assert_eq!(other_response.0, StatusCode::OK);
    assert!(other_response.1.contains("other app"));
    drop(held);

    assert_eq!(publishing.await.unwrap(), StatusCode::OK);
    let after_publish = pending_get.await.unwrap();
    assert_eq!(after_publish.0, StatusCode::OK);
    assert!(after_publish.1.contains("B"));
    assert_ne!(after_publish.2, original.2);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn a_release_that_fails_at_any_step_leaves_the_old_actions_on_the_old_rows() {
    let list = |table: &str, column: &str| json!({"kind": "query", "access": "anonymous", "statement": format!("SELECT {column} FROM {table} ORDER BY id LIMIT 50"), "bindings": []});
    let both = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\nCREATE TABLE IF NOT EXISTS tags(id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n";
    let notes_only =
        "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\n";
    for step in [
        "stage_local_artifact",
        "stage_server",
        "schema_change",
        "stage_sources",
        "persist_deployment",
        "swap_sources",
        "switch_dist",
        "switch_server",
    ] {
        let root = std::env::temp_dir().join(format!("konae-release-step-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root))
            .await
            .unwrap();
        let state = test_state(&root);
        let id = format!("app_{step}");
        let mut first = release_body(
            &id,
            both,
            json!({"notes.list": list("notes", "body"), "tags.list": list("tags", "name")}),
            "<p>one</p>",
            None,
        );
        functions::tests::add_function(
            &mut first,
            "export function handle() { return new Response('A'); }",
        );
        let response = deploy(&state, first.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{step}: {}",
            String::from_utf8_lossy(&bytes)
        );
        Connection::open(database_path(&state, &id)).unwrap()
            .execute_batch("INSERT INTO notes(body) VALUES ('kept note'); INSERT INTO tags(name) VALUES ('old tag');").unwrap();
        let live = state
            .deployments
            .read()
            .await
            .values()
            .next()
            .cloned()
            .unwrap();
        let app = crate::layout::sources(&root, &id);
        let first_actions = std::fs::read(app.join("actions.json")).unwrap();
        assert!(
            !crate::layout::private(&root, &id)
                .join("sources.json")
                .exists()
        );

        let mut second = release_body(
            &id,
            notes_only,
            json!({"notes.list": list("notes", "body")}),
            "<p>two</p>",
            None,
        );
        functions::tests::add_function(
            &mut second,
            "export function handle() { return new Response('B'); }",
        );
        second.as_object_mut().unwrap().remove("initialModules");
        let input: DeployInput = serde_json::from_value(second.clone()).unwrap();
        let confirmation = runtime::validate_release(&state, &input, None)
            .await
            .unwrap()
            .confirmation();
        assert!(confirmation.is_some(), "dropping tags loses a row");
        second["schemaConfirmation"] = json!(confirmation);
        FAILING_RELEASE_STEPS
            .lock()
            .unwrap()
            .push((id.clone(), step));
        assert_ne!(
            deploy(&state, second.to_string().as_bytes()).await.status(),
            StatusCode::OK,
            "{step}"
        );
        FAILING_RELEASE_STEPS
            .lock()
            .unwrap()
            .retain(|(app, _)| app != &id);

        let call = |name: &'static str| {
            let state = state.clone();
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, live.hostname.parse().unwrap());
            async move {
                let response = invoke_action(
                    State(state),
                    AxumPath(name.into()),
                    headers,
                    Json(json!({})),
                )
                .await;
                let status = response.status();
                (
                    status,
                    String::from_utf8(
                        axum::body::to_bytes(response.into_body(), 100_000)
                            .await
                            .unwrap()
                            .to_vec(),
                    )
                    .unwrap(),
                )
            }
        };
        let (status, body) = call("tags.list").await;
        assert_eq!(status, StatusCode::OK, "{step}: {body}");
        assert!(body.contains("old tag"), "{step}: {body}");
        assert!(call("notes.list").await.1.contains("kept note"), "{step}");
        let serving = state
            .deployments
            .read()
            .await
            .values()
            .next()
            .cloned()
            .unwrap();
        assert_eq!(serving.artifact_version, live.artifact_version, "{step}");
        let record: Value =
            serde_json::from_slice(&std::fs::read(crate::layout::record(&root, &id)).unwrap())
                .unwrap();
        assert_eq!(
            record["artifactVersion"],
            json!(live.artifact_version),
            "{step}: the record names the release memory serves"
        );
        assert_eq!(
            read_artifact(&state, &serving, "index.html").await.unwrap(),
            b"<p>one</p>",
            "{step}"
        );
        assert_eq!(
            std::fs::read(crate::layout::dist(&root, &id).join("index.html")).unwrap(),
            b"<p>one</p>",
            "{step}: dist/ is the first release's"
        );
        assert_eq!(
            std::fs::read(app.join("actions.json")).unwrap(),
            first_actions,
            "{step}: app/ is the first release's"
        );
        assert!(
            !crate::layout::private(&root, &id)
                .join("sources.json")
                .exists(),
            "{step}: the release does not write editor metadata outside app/"
        );
        let restored = functions::read_artifact(&layout::server(&root, &id))
            .unwrap()
            .unwrap();
        assert!(restored.bundles["orders"].contains("'A'"), "{step}");
        assert_eq!(serving.server, live.server, "{step}");
        let mut restarted = test_state(&root);
        restarted.execution_mode = execution::ExecutionMode::Live;
        let reopened = folder::open(&restarted, &id, None).await.unwrap();
        assert_eq!(reopened.server, live.server, "{step}");
        assert!(
            staged_or_retired(&root, &id).is_empty(),
            "{step}: no staged files stay"
        );
        let staged_sources: Vec<_> = std::fs::read_dir(crate::layout::private(&root, &id))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("app-"))
            .collect();
        assert!(staged_sources.is_empty(), "{step}: {staged_sources:?}");
        let leftovers: Vec<_> = std::fs::read_dir(crate::layout::private(&root, &id))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("release-"))
            .collect();
        assert!(leftovers.is_empty(), "{step}: {leftovers:?}");

        // The same release goes through once nothing fails.
        assert_eq!(
            deploy(&state, second.to_string().as_bytes()).await.status(),
            StatusCode::OK,
            "{step}"
        );
        assert_ne!(
            std::fs::read(app.join("actions.json")).unwrap(),
            first_actions,
            "{step}: app/ is the second release's"
        );
        assert_eq!(call("tags.list").await.0, StatusCode::NOT_FOUND, "{step}");
        assert!(call("notes.list").await.1.contains("kept note"), "{step}");
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}

#[tokio::test]
async fn a_failed_record_write_removes_the_new_artifact_version() {
    let root = std::env::temp_dir().join(format!("konae-orphan-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
    let state = test_state(&root);
    let first = deploy_files(&state, json!({"index.html": "<p>one</p>"})).await;
    let id = first["id"].as_str().unwrap().to_owned();
    let record = crate::layout::record(&root, &id);
    tokio::fs::remove_file(&record).await.unwrap();
    tokio::fs::create_dir_all(record.join("blocker")).await.unwrap();
    let body = json!({
        "project": {
            "id": "app_republish", "name": "Republish", "slug": "republish", "manifest": {},
            "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}
        },
        "visibility": "public",
        "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>two</p>"}}
    });
    assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(staged_or_retired(&root, &id).is_empty(), "the staged files are gone");
    let deployment = state.deployments.read().await.values().next().cloned().unwrap();
    assert_eq!(read_artifact(&state, &deployment, "index.html").await.unwrap(), b"<p>one</p>");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

#[tokio::test]
    async fn restoring_a_backup_first_backs_up_the_live_database() {
        let root = std::env::temp_dir().join(format!("konae-restore-{}", Uuid::new_v4()));
        let (state, _) = login_test_state(&root, "public").await;
    let deployment_id = state.deployments.read().await.values().next().unwrap().id.clone();
    let id = deployment_id.as_str();
    let path = database_path(&state, id);
    initialize_database(&path).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('old');")
        .unwrap();
    let backup = create_backup(&state, id, "Manual backup").await.unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("INSERT INTO notes VALUES ('newer')", [])
        .unwrap();
    restore_backup(&state, id, backup["id"].as_str().unwrap()).await.unwrap();

    let backups = list_backups(&state, id).await.unwrap();
    let before = backups
        .iter()
        .find(|item| item["label"] == "Before restore")
        .expect("restore must keep a copy of the live database");
    let snapshot = crate::layout::backups(&root, &id)
        .join(format!("{}.sqlite", before["id"].as_str().unwrap()));
    let rows: i64 = Connection::open(snapshot)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 2);
    let rows: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    let _ = tokio::fs::remove_dir_all(root).await;
}

pub fn signed_in(
    state: &AppState,
    deployment_id: &str,
    headers: &HeaderMap,
    email: &str,
) -> (HeaderMap, String) {
    let auth =
        AuthService::open(crate::layout::identity(&state.data_root, deployment_id)).unwrap();
    let session = auth.signup(email, "correct horse battery").unwrap();
    let mut headers = headers.clone();
    headers.insert(
        header::COOKIE,
        format!("konae_app_session={}", session.token)
            .parse()
            .unwrap(),
    );
    (headers, session.user.user_id)
}

pub async fn response_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn upload_refuses_to_overwrite_an_unreadable_file_index() {
    let root = std::env::temp_dir().join(format!("konae-upload-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
    let files = crate::layout::files(&root, &deployment.id);
    tokio::fs::create_dir_all(&files).await.unwrap();
    tokio::fs::write(files.join("index.json"), "{not json")
        .await
        .unwrap();
    let body =
        json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
    let request = Request::builder()
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = upload_file(State(state.clone()), headers.clone(), request).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        tokio::fs::read(files.join("index.json")).await.unwrap(),
        b"{not json"
    );

    tokio::fs::remove_file(files.join("index.json"))
        .await
        .unwrap();
    let request = Request::builder()
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = upload_file(State(state), headers, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_json(response).await["result"]
            .get("sha256")
            .is_none()
    );
    let index: Vec<Value> =
        serde_json::from_slice(&tokio::fs::read(files.join("index.json")).await.unwrap()).unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(index[0]["visibility"], "private");
    assert_eq!(index[0]["sha256"], digest(b"hi"));
    let leftovers = std::fs::read_dir(&files)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        })
        .count();
    assert_eq!(leftovers, 0);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn files_off_refuses_upload_listing_and_read_with_the_module_sentence() {
    let root = std::env::temp_dir().join(format!("konae-files-off-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let mut deployments = state.deployments.write().await;
    deployments
        .values_mut()
        .next()
        .unwrap()
        .modules
        .get_mut("files")
        .unwrap()["on"] = json!(false);
    drop(deployments);
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    let (headers, _) = signed_in(&state, &deployment.id, &headers, "owner@example.test");
    let expected = "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.";
    let upload = Request::builder()
        .body(Body::from(
            json!({"name":"a.txt","type":"text/plain","dataUrl":"data:text/plain;base64,aGk="})
                .to_string(),
        ))
        .unwrap();
    for response in [
        upload_file(State(state.clone()), headers.clone(), upload).await,
        list_files(State(state.clone()), headers.clone()).await,
        serve_file(
            State(state.clone()),
            AxumPath("missing".into()),
            headers.clone(),
        )
        .await,
    ] {
        let body = response_json(response).await;
        assert_eq!(body["error"], expected);
        assert_eq!(body["code"], "module.off");
    }
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn release_refuses_a_capsule_that_uses_files_while_the_module_is_off() {
    let state = AppState::for_test(
        &std::env::temp_dir().join(format!("konae-files-release-off-{}", Uuid::new_v4())),
    );
    let body = json!({
        "project": {"id":"files_off","name":"Files off","slug":"files-off","manifest":{},"files":{"actions.json":"{\"version\":1,\"actions\":{}}","schema.sql":""}},
        "visibility":"public",
        "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<main>app</main>","app.js":"miniapp.files.upload(file)"}},
        "initialModules":{"files":{"on":false}}
    });
    let response = deploy(&state, body.to_string().as_bytes()).await;
    let refused = response_json(response).await;
    assert_eq!(
        refused["error"],
        "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run."
    );
    assert_eq!(refused["code"], "module.off");
}

#[tokio::test]
async fn files_are_private_to_the_uploader_unless_marked_public() {
    let root = std::env::temp_dir().join(format!("konae-files-{}", Uuid::new_v4()));
    let (state, headers) = login_test_state(&root, "public").await;
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    let (alice, alice_id) = signed_in(&state, &deployment.id, &headers, "alice@example.test");
    let (bob, _) = signed_in(&state, &deployment.id, &headers, "bob@example.test");
    let body = |visibility: Option<&str>| {
        let mut value = json!({"name": "a.txt", "type": "text/plain", "dataUrl": "data:text/plain;base64,aGk="});
        if let Some(visibility) = visibility {
            value["visibility"] = json!(visibility);
        }
        Request::builder()
            .body(Body::from(value.to_string()))
            .unwrap()
    };

    let anonymous = upload_file(State(state.clone()), headers.clone(), body(None)).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let private_file =
        response_json(upload_file(State(state.clone()), alice.clone(), body(None)).await).await;
    assert_eq!(private_file["result"]["visibility"], "private");
    assert_eq!(private_file["result"]["ownerId"], alice_id);
    let private_id = private_file["result"]["id"].as_str().unwrap().to_owned();

    let public_file =
        response_json(upload_file(State(state.clone()), alice.clone(), body(Some("public"))).await)
            .await;
    let public_id = public_file["result"]["id"].as_str().unwrap().to_owned();

    let alice_list = response_json(list_files(State(state.clone()), alice.clone()).await).await;
    assert_eq!(alice_list["result"].as_array().unwrap().len(), 2);
    assert!(
        alice_list["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["ownerId"] == alice_id)
    );

    let bob_list = response_json(list_files(State(state.clone()), bob.clone()).await).await;
    assert_eq!(bob_list["result"].as_array().unwrap().len(), 1);
    assert_eq!(bob_list["result"][0]["id"], public_id);
    assert!(bob_list["result"][0].get("ownerId").is_none());

    let visitor_list = response_json(list_files(State(state.clone()), headers.clone()).await).await;
    assert_eq!(visitor_list["result"][0]["id"], public_id);
    assert!(visitor_list["result"][0].get("ownerId").is_none());

    assert_eq!(
        serve_file(
            State(state.clone()),
            AxumPath(private_id.clone()),
            bob.clone()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        serve_file(
            State(state.clone()),
            AxumPath(private_id.clone()),
            headers.clone()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        serve_file(State(state.clone()), AxumPath(private_id), alice.clone())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        serve_file(State(state.clone()), AxumPath(public_id.clone()), headers)
            .await
            .status(),
        StatusCode::OK
    );

    let legacy_id = Uuid::new_v4().to_string();
    let directory = crate::layout::files(&root, &deployment.id);
    tokio::fs::write(directory.join(format!("{legacy_id}.blob")), b"hi")
        .await
        .unwrap();
    let mut index: Vec<Value> =
        serde_json::from_slice(&tokio::fs::read(directory.join("index.json")).await.unwrap())
            .unwrap();
    index.push(json!({"id": legacy_id, "name": "old.txt", "type": "text/plain", "size": 2, "createdAt": 1}));
    tokio::fs::write(
        directory.join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .await
    .unwrap();
    let after_legacy = response_json(list_files(State(state.clone()), alice).await).await;
    assert!(
        after_legacy["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["id"] != legacy_id)
    );
    assert_eq!(
        serve_file(State(state.clone()), AxumPath(legacy_id), bob)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let (private_state, private_headers) = login_test_state(&root.join("private"), "private").await;
    let private_deployment = private_state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    assert_eq!(
        list_files(State(private_state.clone()), private_headers.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (member, _) = signed_in(
        &private_state,
        &private_deployment.id,
        &private_headers,
        "member@example.test",
    );
    assert_eq!(
        list_files(State(private_state), member).await.status(),
        StatusCode::OK
    );

    let _ = tokio::fs::remove_dir_all(root).await;
}

fn query_definition(statement: &str) -> ActionDefinition {
    let program: ActionProgram = serde_json::from_value(json!({"version": 1, "actions": {
        "rows.read": {"kind": "query", "access": "anonymous", "statement": statement, "bindings": []}
    }})).unwrap();
    program.actions["rows.read"].clone()
}

fn converted_after<T>(work: impl FnOnce() -> T) -> (T, usize) {
    COLUMNS_CONVERTED.with(|count| count.set(0));
    let value = work();
    (value, COLUMNS_CONVERTED.with(|count| count.get()))
}

const THREE_BLOBS: &str = "SELECT zeroblob(700000) a, zeroblob(700000) b, zeroblob(700000) c";

#[test]
fn an_action_result_stops_being_read_once_it_passes_2_mib() {
    let root = std::env::temp_dir().join(format!("konae-row-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap();
    let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(THREE_BLOBS), Vec::new()));
    assert_eq!(outcome.unwrap_err().to_string(), "Query result exceeds 2 MiB or 1000 rows.");
    assert_eq!(converted, 1, "the column that passes the cap is the last one read");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn browse_refuses_an_oversized_row_before_converting_its_values() {
    let root = std::env::temp_dir().join(format!("konae-browse-row-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap().execute_batch(
        "CREATE TABLE items(payload BLOB); INSERT INTO items VALUES (zeroblob(1100000));",
    ).unwrap();
    let (outcome, converted) = converted_after(|| browse_table(&path, &json!({"tableName": "items"})));
    assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
    assert_eq!(converted, 1, "the row id is accounted before the oversized payload is read");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn browse_keeps_its_row_shape_for_ordinary_values() {
    let root = std::env::temp_dir().join(format!("konae-browse-row-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap().execute_batch(
        "CREATE TABLE items(name TEXT, done INTEGER); INSERT INTO items VALUES ('task', 1);",
    ).unwrap();
    let (outcome, _) = converted_after(|| browse_table(&path, &json!({"tableName": "items"})));
    let result = outcome.unwrap();
    assert_eq!(result["totalRows"], 1);
    assert_eq!(result["rows"], json!([{"rowId": 1, "values": {"name": "task", "done": 1}}]));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn action_result_budget_counts_column_names_before_converting_values() {
    let root = std::env::temp_dir().join(format!("konae-column-name-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap();
    let alias = "a".repeat(16_000);
    let statement = format!("WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM rows WHERE n < 140) SELECT NULL AS \"{alias}\" FROM rows");
    let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(&statement), Vec::new()));
    assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
    assert_eq!(converted, 130, "the oversized repeated column name is refused before value conversion");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn action_result_budget_counts_controls_in_lossy_text_before_conversion() {
    let root = std::env::temp_dir().join(format!("konae-invalid-text-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap();
    let statement = "SELECT CAST(zeroblob(350000) || x'ff' AS TEXT) AS payload";
    let (outcome, converted) = converted_after(|| run_query(&path, &query_definition(statement), Vec::new()));
    assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
    assert_eq!(converted, 0, "the NUL escapes pass the byte cap before lossy text conversion");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn notify_refuses_an_oversized_returning_row_and_rolls_back() {
    let root = std::env::temp_dir().join(format!("konae-notify-row-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE items(payload BLOB);")
        .unwrap();
    let definition: ActionDefinition = serde_json::from_value(json!({
        "kind": "mutation", "access": "anonymous",
        "statement": "INSERT INTO items(payload) VALUES (zeroblob(1100000))",
        "bindings": [],
        "notify": {"to": "owner", "changes": ["payload"], "subject": "Changed", "body": "{changes}"}
    }))
    .unwrap();
    let (outcome, converted) = converted_after(|| run_mutation(&path, &definition, Vec::new(), Some(notify::MutationContext {
        app: "test-app".into(), release: "release-1".into(), action: "items.insert".into(), input: json!({}),
        account: None, actor_id: None, at: 1,
    })));
    assert_eq!(outcome.err().unwrap().to_string(), QUERY_RESULT_TOO_LARGE);
    assert_eq!(
        converted, 0,
        "the oversized returned payload is rejected before conversion"
    );
    let count: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "the mutation rolls back when its notice result exceeds the budget"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn owner_sql_stops_reading_a_result_once_it_passes_2_mib() {
    let root = std::env::temp_dir().join(format!("konae-admin-row-cap-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap();
    let (outcome, converted) = converted_after(|| run_admin_sql(&path, THREE_BLOBS));
    assert_eq!(outcome.unwrap_err().to_string(), "SQL result exceeds 2 MiB.");
    assert_eq!(converted, 1, "the column that passes the cap is the last one read");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_result_under_the_cap_comes_back_unchanged() {
    let root = std::env::temp_dir().join(format!("konae-row-small-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.sqlite");
    Connection::open(&path).unwrap();
    let sql = "SELECT x'00ff10' AS b, 'héllo \"q\"' AS t, 7 AS n, 1.5 AS r, NULL AS z";
    let expected = json!({"b": "00ff10", "t": "héllo \"q\"", "n": 7, "r": 1.5, "z": null});
    assert_eq!(run_query(&path, &query_definition(sql), Vec::new()).unwrap(), vec![expected.clone()]);
    assert_eq!(run_admin_sql(&path, sql).unwrap()["rows"], json!([expected]));
    let _ = std::fs::remove_dir_all(root);
}

async fn statement_cap_state(root: &Path) -> (AppState, HeaderMap, String) {
    let (mut state, headers) = login_test_state(root, "public").await;
    let program = json!({"version": 1, "actions": {
        "items.count": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n", "bindings": []},
        "items.broken": {"kind": "query", "access": "anonymous", "statement": "SELECT * FROM missing_table", "bindings": []},
        "items.route": {"kind": "query", "access": "anonymous", "statement": "SELECT 1 AS n LIMIT 1", "bindings": [], "http": {"method": "GET", "path": "/api/count"}}
    }});
    let mut id = String::new();
    for deployment in state.deployments.write().await.values_mut() {
        deployment.actions = program.to_string();
        deployment.manifest = json!({"visibility": "public"});
        deployment.modules.get_mut("http").unwrap()["on"] = json!(true);
        initialize_database(&database_path(&state, &deployment.id)).unwrap();
        id = deployment.id.clone();
    }
    state.guards = Arc::new(Guards::new(Vec::new()).with_statement_wait(std::time::Duration::from_millis(60)));
    (state, headers, id)
}

async fn hold_statements(
    state: &AppState,
    id: &str,
    count: usize,
) -> Vec<crate::limits::StatementPermit> {
    let mut held = Vec::new();
    for _ in 0..count {
        held.push(state.guards.statement_permit(id).await.unwrap());
    }
    held
}

async fn call_action(state: &AppState, headers: &HeaderMap, name: &str) -> Response {
    invoke_action(
        State(state.clone()),
        AxumPath(name.into()),
        headers.clone(),
        Json(json!({})),
    )
    .await
}

#[tokio::test]
async fn a_fifth_action_statement_on_one_deployment_is_refused_while_another_is_served() {
    let root = std::env::temp_dir().join(format!("konae-statement-cap-{}", Uuid::new_v4()));
    let (state, headers, id) = statement_cap_state(&root).await;
    let _elsewhere = hold_statements(&state, "another-deployment", 4).await;
    assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK, "another Deployment's slots do not count");
    let mut held = hold_statements(&state, &id, 4).await;
    let refused = call_action(&state, &headers, "items.count").await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().contains_key(header::RETRY_AFTER));
    let body = axum::body::to_bytes(refused.into_body(), 4096).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], "rate-limited");
    held.pop();
    assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK, "a freed slot lets the next call run");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn an_action_statement_that_errors_releases_its_permit() {
    let root = std::env::temp_dir().join(format!("konae-statement-error-{}", Uuid::new_v4()));
    let (state, headers, id) = statement_cap_state(&root).await;
    let _held = hold_statements(&state, &id, 3).await;
    for _ in 0..5 {
        assert_eq!(call_action(&state, &headers, "items.broken").await.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(call_action(&state, &headers, "items.count").await.status(), StatusCode::OK);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn the_runtime_endpoint_answers_a_refused_statement_with_the_rate_limited_tool_error() {
    let root = std::env::temp_dir().join(format!("konae-statement-runtime-{}", Uuid::new_v4()));
    let (state, _, id) = statement_cap_state(&root).await;
    let _held = hold_statements(&state, &id, 4).await;
    let request = Request::builder()
        .method("POST")
        .uri("/_konae/mcp")
        .header(header::HOST, "login-test.localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "actions_invoke", "arguments": {"name": "items.count"}}}).to_string()))
        .unwrap();
    let response = runtime::runtime(State(state.clone()), request).await;
    assert!(response.headers().contains_key(header::RETRY_AFTER));
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(body["result"]["content"][0]["text"], "rate-limited");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn an_http_route_answers_a_refused_statement_with_the_rate_limited_response() {
    let root = std::env::temp_dir().join(format!("konae-statement-route-{}", Uuid::new_v4()));
    let (state, headers, id) = statement_cap_state(&root).await;
    let deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .cloned()
        .unwrap();
    let client: std::net::IpAddr = "192.0.2.1".parse().unwrap();
    let _held = hold_statements(&state, &id, 4).await;
    let refused = http::call_route(
        &state,
        &deployment,
        "GET",
        "/api/count",
        "",
        b"",
        &headers,
        client,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().contains_key(header::RETRY_AFTER));
    let body = axum::body::to_bytes(refused.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"],
        "rate-limited"
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}
