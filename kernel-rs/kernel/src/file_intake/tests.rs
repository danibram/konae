use crate::{app_data::database_path, file_intake::AcceptOutcome};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    case: String,
    value: Value,
    valid: bool,
}

#[test]
fn declaration_cases_match_shared_file() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("../file-intake-cases.json")).unwrap();
    for case in cases {
        let result = super::validate_declaration(&case.value.to_string());
        assert_eq!(result.is_ok(), case.valid, "{}: {result:?}", case.case);
    }
}

#[test]
fn path_normalization_refuses_escape_and_empty_parts() {
    for path in ["/absolute", "../up", "a/../b", "a//b", "a\\b", "a/./b"] {
        assert!(
            super::accept::normalize_relative_path(path).is_err(),
            "{path}"
        );
    }
    assert_eq!(
        super::accept::normalize_relative_path("camera/2026-11-27.jpg").unwrap(),
        "camera/2026-11-27.jpg"
    );
}

#[tokio::test]
async fn accept_replays_replacements_and_commits_one_action_effect_per_version() {
    use rusqlite::Connection;
    use std::io::Write;

    let root = std::env::temp_dir().join(format!("konae-intake-{}", uuid::Uuid::new_v4()));
    let (state, _) = crate::tests::login_test_state(&root, "public").await;
    let mut deployment = state
        .deployments
        .read()
        .await
        .values()
        .next()
        .unwrap()
        .clone();
    deployment
        .modules
        .insert("file-intake".into(), serde_json::json!({"on": true}));
    deployment.actions = serde_json::json!({"version":1,"actions":{"records.accept":{"kind":"mutation","access":"anonymous","statement":"INSERT INTO received(version_id) VALUES(?1)","bindings":[{"source":"input","key":"versionId","type":"string"}]}}}).to_string();
    deployment.manifest["fileIntake"] = serde_json::json!({
        "version":1,
        "sources":[{"id":"camera","version":1,"replay":"delivery-key-or-current-path-sha256"}],
        "consumers":[{"id":"record","source":"camera","action":"records.accept","input":{}}]
    });
    let draft_sources = crate::layout::sources(&root, &deployment.id);
    tokio::fs::create_dir_all(&draft_sources).await.unwrap();
    tokio::fs::write(
        draft_sources.join("file-intake.json"),
        r#"{"version":1,"sources":[{"id":"draft-only","version":1}],"consumers":[]}"#,
    )
    .await
    .unwrap();
    state
        .deployments
        .write()
        .await
        .insert(deployment.hostname.clone(), deployment.clone());
    let database = database_path(&state, &deployment.id);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE received(version_id TEXT NOT NULL);")
        .unwrap();
    drop(connection);
    let scratch = root.join("staged");
    let mut staged = std::fs::File::create(&scratch).unwrap();
    staged.write_all(b"first bytes").unwrap();
    drop(staged);

    let mut foreign = deployment.clone();
    foreign.id = "another-app".into();
    assert!(
        super::accept::accept_staged(
            &state,
            &foreign,
            "camera",
            &scratch,
            "incoming/photo.jpg",
            None
        )
        .await
        .is_err()
    );

    crate::maintenance::pause_for_test(&root, &deployment.id, true);
    let paused = super::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/photo.jpg",
        None,
    )
    .await
    .unwrap_err();
    assert!(paused.to_string().contains("maintenance"));
    crate::maintenance::pause_for_test(&root, &deployment.id, false);

    let connection = Connection::open(&database).unwrap();
    super::accept::ensure_schema(&connection).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_intake BEFORE INSERT ON __konae_intake_jobs
         BEGIN SELECT RAISE(ABORT, 'forced refusal'); END;",
        )
        .unwrap();
    let refused = super::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/photo.jpg",
        None,
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("could not be committed"));
    assert!(
        state
            .files
            .store
            .list(&deployment.id)
            .await
            .unwrap()
            .is_empty()
    );
    connection
        .execute_batch("DROP TRIGGER refuse_intake;")
        .unwrap();
    drop(connection);

    let first = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/photo.jpg",
        Some("transfer-1"),
    )
    .await
    .unwrap();
    let replay = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/photo.jpg",
        Some("transfer-1"),
    )
    .await
    .unwrap();
    assert_eq!(
        replay,
        AcceptOutcome {
            version_id: first.version_id.clone(),
            created: false
        }
    );
    std::fs::write(&scratch, b"replacement bytes").unwrap();
    assert!(
        super::accept::accept_staged(
            &state,
            &deployment,
            "camera",
            &scratch,
            "incoming/photo.jpg",
            Some("transfer-1")
        )
        .await
        .is_err()
    );
    let replacement = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/photo.jpg",
        Some("transfer-2"),
    )
    .await
    .unwrap();
    assert_ne!(replacement.version_id, first.version_id);

    std::fs::write(&scratch, b"fallback current").unwrap();
    let fallback_first = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/fallback.jpg",
        None,
    )
    .await
    .unwrap();
    let fallback_retry = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/fallback.jpg",
        None,
    )
    .await
    .unwrap();
    assert_eq!(fallback_retry.version_id, fallback_first.version_id);
    std::fs::write(&scratch, b"fallback replacement").unwrap();
    let fallback_replacement = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/fallback.jpg",
        None,
    )
    .await
    .unwrap();
    std::fs::write(&scratch, b"fallback current").unwrap();
    let old_content_again = super::accept::accept_staged(
        &state,
        &deployment,
        "camera",
        &scratch,
        "incoming/fallback.jpg",
        None,
    )
    .await
    .unwrap();
    assert_ne!(fallback_first.version_id, fallback_replacement.version_id);
    assert_ne!(
        fallback_replacement.version_id,
        old_content_again.version_id
    );

    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM __konae_intake_versions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        5
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM __konae_intake_deliveries",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM __konae_intake_jobs", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert!(crate::files::valid_file_id(&first.version_id));
    connection.execute("UPDATE __konae_intake_jobs SET state='leased',leased_until=9223372036854775807 WHERE version_id=?1", [&first.version_id]).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_intake_effect BEFORE INSERT ON __konae_intake_effects BEGIN SELECT RAISE(ABORT,'simulated interrupted consumer'); END;").unwrap();
    drop(connection);
    super::worker::resume_import(&database).unwrap();
    assert!(super::worker::run_one(&state, &deployment).await.is_err());
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM received", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    connection
        .execute_batch("DROP TRIGGER fail_intake_effect;")
        .unwrap();
    drop(connection);
    super::worker::resume_import(&database).unwrap();
    for _ in 0..2 {
        assert!(super::worker::run_one(&state, &deployment).await.unwrap());
    }
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM received", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM __konae_intake_effects", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(connection);
    let references = crate::modules::blob_references(&root, &deployment.id).unwrap();
    assert_eq!(references.len(), 5);
    assert!(
        references
            .iter()
            .all(|reference| reference.module == "file-intake")
    );

    let package_root = root.join("package");
    let source_store = state.files.store.clone();
    let moved_root = root.join("moved");
    let moved_root_for_worker = moved_root.clone();
    let package_data_root = root.clone();
    let app_id = deployment.id.clone();
    let modules = deployment.modules.clone();
    let limits = state.files.limits;
    let (imported_refs, moved_store, moved_database) = tokio::task::spawn_blocking(move || {
        let components = crate::package::snapshot(
            &crate::layout::data(&package_data_root, &app_id),
            &package_root,
            &crate::file_store::BlockingFiles::new(source_store, &app_id),
            &modules,
        )
        .unwrap();
        assert!(components.iter().any(|(name, _)| *name == "database"));
        let archive = components
            .iter()
            .find(|(name, _)| *name == "files")
            .unwrap()
            .1
            .clone();
        let exported_database = components
            .iter()
            .find(|(name, _)| *name == "database")
            .unwrap()
            .1
            .clone();
        let moved_database = moved_root.join("apps/moved-app/data/database.sqlite");
        std::fs::create_dir_all(moved_database.parent().unwrap()).unwrap();
        std::fs::copy(&exported_database, &moved_database).unwrap();
        let imported_refs = crate::package::references_for_archive(
            &archive,
            Some(&exported_database),
            &package_data_root.join("import-stage"),
        )
        .unwrap();
        let moved_store: std::sync::Arc<dyn crate::file_store::FileStore> =
            std::sync::Arc::new(crate::file_store::LocalFolder::new(&moved_root));
        let moved_blobs = crate::file_store::BlockingFiles::new(moved_store.clone(), "moved-app");
        crate::package::unpack_files_with_references(
            &archive,
            None,
            Some((&moved_root.join("apps/moved-app/data/files"), &moved_blobs)),
            &limits,
            &imported_refs,
        )
        .unwrap();
        (imported_refs, moved_store, moved_database)
    })
    .await
    .unwrap();
    assert_eq!(imported_refs.len(), 5);
    for reference in &imported_refs {
        let old =
            crate::file_store::read_all(state.files.store.as_ref(), &deployment.id, &reference.id)
                .await
                .unwrap();
        let new = crate::file_store::read_all(moved_store.as_ref(), "moved-app", &reference.id)
            .await
            .unwrap();
        assert_eq!(old, new);
    }
    crate::file_intake::resume_import(&moved_database).unwrap();
    let mut moved_state = state.clone();
    moved_state.data_root = moved_root_for_worker;
    let mut moved_deployment = deployment.clone();
    moved_deployment.id = "moved-app".into();
    moved_deployment.project_id = "moved-project".into();
    for _ in 0..3 {
        assert!(
            super::worker::run_one(&moved_state, &moved_deployment)
                .await
                .unwrap()
        );
    }
    assert!(
        !super::worker::run_one(&moved_state, &moved_deployment)
            .await
            .unwrap()
    );
    let moved_connection = Connection::open(&moved_database).unwrap();
    assert_eq!(
        moved_connection
            .query_row("SELECT count(*) FROM __konae_intake_versions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        5
    );
    assert_eq!(
        moved_connection
            .query_row("SELECT count(*) FROM received", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        moved_connection
            .query_row("SELECT count(*) FROM __konae_intake_effects", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}
