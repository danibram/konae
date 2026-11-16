
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use rusqlite::Connection;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    find_project_deployment, initialize_database, layout, now_ms, open_auth, persist_release, runtime,
    schema_change, AppState, BuildArtifact, DeployInput, Deployment, Installed, ProjectInput, Settings,
};

/// The variable that confirms a schema change that loses data, as
/// `release_plan`'s confirmation does in Konae.
pub const CONFIRM_SCHEMA: &str = "KONAE_CONFIRM_SCHEMA";

/// Opens every folder under `apps/` and answers, for each, the app or why
/// the Kernel does not serve it. One folder that fails leaves the others
/// served.
pub async fn open_all(state: &AppState, confirmation: Option<&str>) -> Result<Vec<(String, Result<Deployment>)>> {
    let mut ids = Vec::new();
    let mut entries = tokio::fs::read_dir(layout::apps(&state.data_root)).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !tokio::fs::metadata(entry.path()).await.is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        if let Some(id) = entry.file_name().to_str() {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    let mut opened = Vec::new();
    for id in ids {
        let result = open(state, &id, confirmation).await;
        opened.push((id, result));
    }
    Ok(opened)
}

/// Opens one app folder and serves it. The record keeps the address an
/// earlier start gave the app; a folder without one gets `<slug>.<suffix>`,
/// or a random address when another app holds that one.
pub async fn open(state: &AppState, id: &str, confirmation: Option<&str>) -> Result<Deployment> {
    let root = &state.data_root;
    ensure!(runtime::valid_app_id(id), "apps/{id} is not named after an app ID. An app ID uses lowercase letters, digits, \"_\" and \"-\".");
    decide_files_module(root, id).await?;
    let mut settings = Settings::read(root, id).with_context(|| format!("apps/{id}/konae.json is missing or not readable"))?;
    ensure!(settings.id == id, "apps/{id}/konae.json names the app {}. Name the folder after the app's ID.", settings.id);
    let sources = layout::sources(root, id);
    let stored = match tokio::fs::read(layout::record(root, id)).await {
        Ok(bytes) => Some(serde_json::from_slice::<Installed>(&bytes).with_context(|| format!("apps/{id}/.konae/deployment.json is malformed"))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let released = if let Some(record) = stored.as_ref() {
        let recovered = crate::sources::capture_released_state(root, id, &settings, record)?;
        settings.released = Some(recovered.clone());
        Some(recovered)
    } else {
        settings.released.clone()
    };
    let dist = read_dist(&layout::dist(root, id))
        .with_context(|| format!("apps/{id}/dist/ is missing or not readable. Build it with npm run build"))?;
    let dist_digest = version_of(&dist);
    let rebuilt = released.as_ref().and_then(|snapshot| snapshot.artifact_digest.as_deref()).is_some_and(|digest| digest != dist_digest);
    let candidate_sources = state.execution_mode.is_test();
    let source_actions = || async { tokio::fs::read_to_string(sources.join("actions.json")).await.with_context(|| format!("apps/{id}/app/actions.json is missing")) };
    let source_schema = || async { tokio::fs::read_to_string(sources.join("schema.sql")).await.unwrap_or_default() };
    let source_transforms = || async { optional_text(&sources.join("schema-transforms.json")).await };
    let actions = if candidate_sources || rebuilt || (released.is_none() && stored.is_none()) {
        source_actions().await?
    } else if let Some(snapshot) = released.as_ref() {
        snapshot.actions.clone()
    } else if let Some(record) = stored.as_ref() {
        record.actions.clone()
    } else { source_actions().await? };
    let program = crate::action_shape::read(&actions)?;
    for (name, action) in &program.actions {
        let Some(file) = action.page.as_ref().and_then(|page| page.get("file")).and_then(Value::as_str) else { continue };
        ensure!(dist.contains_key(file), "Action {name} names {file} as its page file, but the Build Artifact does not contain it.");
    }
    let schema = if candidate_sources || rebuilt || (released.is_none() && stored.is_none()) {
        source_schema().await
    } else if let Some(snapshot) = released.as_ref() {
        snapshot.schema_sql.clone()
    } else {
        crate::sources::database_schema(root, id)?
    };
    let fresh_sources = released.is_none() && stored.is_none();
    let schema_transforms = if candidate_sources || rebuilt || fresh_sources {
        source_transforms().await?
    } else {
        released.as_ref().and_then(|snapshot| snapshot.schema_transforms.clone())
    };
    let mut manifest = if candidate_sources || rebuilt { settings.manifest.clone() } else { released.as_ref().map(|snapshot| snapshot.manifest.clone()).unwrap_or_else(|| settings.manifest.clone()) };
    manifest = crate::sources::manifest_without_runtime(&manifest);
    let visibility = if candidate_sources || rebuilt { settings.visibility.as_str() } else { released.as_ref().map(|snapshot| snapshot.visibility.as_str()).unwrap_or(&settings.visibility) };
    if let Some(object) = manifest.as_object_mut() {
        object.insert("visibility".into(), Value::String(visibility.to_owned()));
    }
    let encode = |bytes: &Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let applied_name = released.as_ref().map(|snapshot| snapshot.name.as_str()).or_else(|| stored.as_ref().map(|_| settings.name.as_str())).unwrap_or(&settings.name);
    let applied_slug = released.as_ref().map(|snapshot| snapshot.slug.as_str()).or_else(|| stored.as_ref().map(|_| settings.slug.as_str())).unwrap_or(&settings.slug);
    let input = DeployInput {
        project: ProjectInput {
            id: id.to_owned(),
        name: if candidate_sources || rebuilt { settings.name.clone() } else { applied_name.to_owned() },
        slug: if candidate_sources || rebuilt { settings.slug.clone() } else { applied_slug.to_owned() },
            files: {
                let mut files = HashMap::from([("actions.json".to_owned(), actions.clone()), ("schema.sql".to_owned(), schema.clone())]);
                if let Some(document) = &schema_transforms { files.insert("schema-transforms.json".to_owned(), document.clone()); }
                files
            },
            encodings: HashMap::new(),
            directories: settings.directories.clone(),
            manifest,
        },
        visibility: visibility.to_owned(),
        artifact: BuildArtifact {
            version: 1,
            entrypoint: "index.html".into(),
            encodings: dist.keys().map(|path| (path.clone(), "base64".to_owned())).collect(),
            files: dist.iter().map(|(path, bytes)| (path.clone(), encode(bytes))).collect(),
        },
        initial_signup_policy: None,
        initial_sign_in_methods: None,
        initial_modules: Some(settings.modules.clone()),
        initial_package: None,
        contact_email: settings.contact_email.clone().or_else(|| released.as_ref().and_then(|snapshot| snapshot.contact_email.clone())),
        schema_confirmation: None,
    };
    runtime::validate_release(state, &input, None).await?;
    let program: crate::ActionProgram = serde_json::from_str(&actions)?;
    let _database = crate::database_lock(state, id).await;
    let database = layout::database(root, id);
    tokio::fs::create_dir_all(layout::data(root, id)).await?;
    if !database.exists() {
        initialize_database(&database)?;
    }
    let plan = plan(state, &database, &schema, &program, schema_transforms.as_deref())?;
    if let Some(expected) = plan.confirmation() {
        if confirmation != Some(expected.as_str()) {
            let mut effects = plan.losses.iter().map(|loss| loss.sentence.clone()).collect::<Vec<_>>();
            if let Some(transformations) = &plan.transformations {
                effects.extend(transformations.iter().map(|item| format!("Transition {} changed {} existing row(s) in the dry run.", item.id, item.affected_rows)));
            }
            if plan.transformations.as_ref().is_some_and(|items| !items.is_empty()) {
                bail!("app/schema-transforms.json asks to change existing data. {} To apply these changes, start the Kernel with {CONFIRM_SCHEMA}={expected}.", effects.join(" "));
            }
            bail!("app/schema.sql asks for a change that loses data. {} To apply it, start the Kernel with {CONFIRM_SCHEMA}={expected}.", effects.join(" "));
        }
    }
    schema_change::apply_with_transforms(&database, &schema, &program, schema_transforms.as_deref(), schema_change::Mode::Apply(plan.confirmation().as_deref()))?;
    let existing = find_project_deployment(&*state.deployments.read().await, id);
    let (hostname, created_at, _taking) = match existing.as_ref() {
        Some(deployment) => (deployment.hostname.clone(), deployment.created_at, None),
        None if stored.is_some() => (stored.as_ref().unwrap().hostname.clone(), stored.as_ref().unwrap().created_at, None),
        None => {
            let (hostname, taking) = free_hostname(state, input.project.slug.as_str(), id).await;
            (hostname, now_ms(), Some(taking))
        }
    };
    let unchanged = !rebuilt && !candidate_sources;
    let released_at = if unchanged { released.as_ref().and_then(|snapshot| snapshot.released_at).or_else(|| stored.as_ref().and_then(|record| record.released_at)).or_else(|| existing.as_ref().map(|current| current.released_at)).unwrap_or_else(now_ms) } else { now_ms() };
    let source_digest = if candidate_sources || rebuilt || fresh_sources { Some(crate::sources::digest_folder(&sources).await?) } else { released.as_ref().and_then(|snapshot| snapshot.source_digest.clone()).or_else(|| stored.as_ref().and_then(|record| record.source_digest.clone())).or_else(|| existing.as_ref().and_then(|current| current.source_digest.clone())) };
    let artifact_version = if unchanged { stored.as_ref().map(|record| record.artifact_version.clone()).or_else(|| existing.as_ref().map(|current| current.artifact_version.clone())).unwrap_or_else(|| format!("v{}", &dist_digest[..32])) } else { format!("v{}", &dist_digest[..32]) };
    let policy = settings.signup_policy.clone();
    let sign_in_methods = settings.sign_in_methods.clone();
    let deployment = Deployment {
        id: id.to_owned(), project_id: id.to_owned(), project_name: input.project.name.clone(), slug: input.project.slug.clone(),
        url: crate::deployment_url(&state.public_origin, &hostname), created_at, released_at, source_digest,
        manifest: input.project.manifest.clone(), hostname, actions, artifact_version, signup_policy: policy,
        sign_in_methods, contact_email: input.contact_email.clone(), modules: settings.modules.clone(), released_schema: schema,
    };
    open_auth(state, &deployment)?;
    if rebuilt { persist_release(state, &deployment, &dist_digest, &settings.directories, schema_transforms.as_deref(), None).await?; }
    else if candidate_sources || fresh_sources { crate::persist_folder_snapshot_with_transforms(state, &deployment, &dist_digest, schema_transforms.as_deref()).await?; }
    else { crate::persist_folder_snapshot(state, &deployment, &dist_digest).await?; }
    let mut deployments = state.deployments.write().await;
    deployments.retain(|_, served| served.id != id);
    deployments.insert(deployment.hostname.clone(), deployment.clone());
    drop(deployments);
    crate::http::routes_published(&deployment);
    Ok(deployment)
}

pub async fn decide_files_module(root: &Path, id: &str) -> Result<Option<bool>> {
    let raw_settings: Value = serde_json::from_slice(&std::fs::read(layout::settings(root, id)).with_context(|| format!("apps/{id}/konae.json is missing or not readable"))?)?;
    if !raw_settings["modules"]["files"].is_null() {
        if raw_settings["manifest"]["fileAccess"].is_null() && raw_settings["released"]["manifest"]["fileAccess"].is_null() { return Ok(None); }
        let settings = Settings::read(root, id)?;
        settings.write(root).await?;
        return Ok(None);
    }
    let legacy_file_access = raw_settings["manifest"]["fileAccess"].as_str().or(raw_settings["released"]["manifest"]["fileAccess"].as_str());
    let used = crate::modules::uses_files(root, id, &json!({"fileAccess": legacy_file_access}));
    let mut settings = Settings::read(root, id)?;
    let files_state = settings.modules.get_mut("files").expect("files is switchable");
    files_state["on"] = json!(used);
    if legacy_file_access == Some("app-users") { files_state["fileAccess"] = json!("app-users"); }
    settings.write(root).await?;
    eprintln!("konae: decided files module for {id}: {}", if used { "on" } else { "off" });
    Ok(Some(used))
}

async fn optional_text(path: &Path) -> Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn plan(state: &AppState, database: &Path, schema: &str, program: &crate::ActionProgram, transforms: Option<&str>) -> Result<schema_change::Change> {
    let copy = state.data_root.join(format!("validate-{}.sqlite", Uuid::new_v4()));
    let result = (|| {
        Connection::open(database)?.execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()])?;
        schema_change::apply_with_transforms(&copy, schema, program, transforms, schema_change::Mode::Plan)
    })();
    let _ = std::fs::remove_file(&copy);
    result
}

/// Hostname for a first publish or a bare-folder open: `<slug>.<suffix>` when
/// free, otherwise `<slug>-<code>.<suffix>`. A republish keeps the address it
/// already has; this only runs when none is stored yet. Hold the second value
/// until the Deployment serves on the name.
pub async fn free_hostname(state: &AppState, slug: &str, id: &str) -> (String, crate::hostname::Taking) {
    crate::hostname::choose(state, slug, id).await
}

pub(crate) fn read_dist(dist: &Path) -> Result<HashMap<String, Vec<u8>>> {
    let mut files = HashMap::new();
    let mut pending = vec![dist.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry.path().strip_prefix(dist)?.components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                files.insert(relative, std::fs::read(entry.path())?);
            }
        }
    }
    Ok(files)
}

pub(crate) fn version_of(files: &HashMap<String, Vec<u8>>) -> String {
    let mut paths = files.keys().collect::<Vec<_>>();
    paths.sort();
    let mut all = Vec::new();
    for path in paths {
        all.extend_from_slice(path.as_bytes());
        all.push(0);
        all.extend_from_slice(crate::digest(&files[path]).as_bytes());
    }
    crate::digest(&all)
}

/// Re-reads an app's settings when its `konae.json` changes, so an
/// operator command run beside the Kernel reaches the app it serves.
pub fn follow_settings(state: AppState) {
    tokio::spawn(async move {
        let mut seen: HashMap<String, SystemTime> = HashMap::new();
        loop {
            let served = state.deployments.read().await.values().cloned().collect::<Vec<_>>();
            for deployment in served {
                let path = layout::settings(&state.data_root, &deployment.id);
                let Ok(modified) = tokio::fs::metadata(&path).await.and_then(|metadata| metadata.modified()) else { continue };
                match seen.insert(deployment.id.clone(), modified) {
                    Some(before) if before != modified => reload(&state, deployment).await,
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

async fn reload(state: &AppState, deployment: Deployment) {
    let settings = match Settings::read(&state.data_root, &deployment.id) {
        Ok(settings) => settings,
        Err(error_value) => {
            eprintln!("Kept the settings of {}: konae.json is not readable ({error_value:#}).", deployment.id);
            return;
        }
    };
    let mut deployments = state.deployments.write().await;
    let Some(current) = deployments.values().find(|item| item.id == deployment.id).cloned() else { return };
    let mut updated = current;
    updated.signup_policy = settings.signup_policy;
    updated.sign_in_methods = settings.sign_in_methods;
    updated.contact_email = settings.contact_email;
    updated.modules = settings.modules;
    deployments.insert(updated.hostname.clone(), updated);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("konae-folder-{}", Uuid::new_v4()))
    }

    fn refused(opened: Result<Deployment>) -> String {
        match opened {
            Ok(_) => panic!("the folder opened"),
            Err(error_value) => format!("{error_value:#}"),
        }
    }

    const ACTIONS: &str = r#"{"version":1,"actions":{"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT body FROM notes ORDER BY id LIMIT 50","bindings":[]}}}"#;

    fn write_folder(root: &Path, id: &str, schema: &str) {
        let folder = layout::folder(root, id);
        std::fs::create_dir_all(folder.join("app")).unwrap();
        std::fs::create_dir_all(folder.join("dist")).unwrap();
        let settings = Settings::new(id, "Notes", "notes", &serde_json::json!({"version": 1, "visibility": "public"}));
        std::fs::write(folder.join("konae.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        std::fs::write(folder.join("app/actions.json"), ACTIONS).unwrap();
        std::fs::write(folder.join("app/schema.sql"), schema).unwrap();
        std::fs::write(folder.join("dist/index.html"), "<!doctype html><main>Notes</main>").unwrap();
    }

    #[tokio::test]
    async fn an_exported_folder_is_served_and_keeps_its_address_and_rows() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let first = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(first.hostname, "notes.localhost");
        assert!(layout::record(&root, "notes_app").exists());
        Connection::open(layout::database(&root, "notes_app")).unwrap().execute("INSERT INTO notes(body) VALUES('kept')", []).unwrap();
        let again = open(&state, "notes_app", None).await.unwrap();
        assert_eq!((again.hostname.as_str(), again.artifact_version.as_str()), (first.hostname.as_str(), first.artifact_version.as_str()));
        let kept: String = Connection::open(layout::database(&root, "notes_app")).unwrap().query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(kept, "kept");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_folder_with_a_missing_page_template_is_refused() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE notes(id INTEGER PRIMARY KEY);");
        std::fs::write(layout::sources(&root, "notes_app").join("actions.json"), r#"{"version":1,"actions":{"posts.head":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts LIMIT 1","bindings":[],"page":{"path":"/posts","file":"post.html"}}}}"#).unwrap();
        let error = refused(open(&state, "notes_app", None).await);
        assert!(error.contains("Action posts.head names post.html as its page file, but the Build Artifact does not contain it."), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_folder_from_0_9_0_decides_file_module_state_once() {
        let root = scratch();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-mechanism-0.9.0");
        for relative in ["konae.json", "app/actions.json", "app/schema.sql"] {
            let destination = layout::folder(&root, "app_legacy_modules").join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(fixture.join(relative), destination).unwrap();
        }
        let dist = layout::dist(&root, "app_legacy_modules");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<!doctype html><title>Legacy app</title><main>Legacy app</main>").unwrap();
        let state = AppState::for_test(&root);
        let deployment = open(&state, "app_legacy_modules", None).await.unwrap();
        let served = crate::read_artifact(&state, &deployment, "index.html").await.unwrap();
        assert!(String::from_utf8(served).unwrap().contains("Legacy app"));
        let listed = crate::modules::read(&state, &deployment);
        assert_eq!(listed["modules"]["actions"]["state"]["on"], true);
        let rewritten: serde_json::Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "app_legacy_modules")).unwrap()).unwrap();
        assert_eq!(rewritten["modules"]["files"]["on"], false);
        assert_eq!(rewritten["modules"].as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>(), ["files".to_owned(), "seo".to_owned()].into());
        assert_eq!(rewritten["modules"]["seo"]["on"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_0_9_0_folder_with_uploads_keeps_files_served_after_migration() {
        use axum::{extract::{Path as AxumPath, State}, http::{HeaderMap, HeaderValue, StatusCode, header}};
        let root = scratch();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-files-0.9.0");
        for relative in ["konae.json", "app/actions.json", "app/schema.sql"] {
            let destination = layout::folder(&root, "legacy_uploads").join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            let source = if relative == "konae.json" { fixture.join(relative) } else { std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-mechanism-0.9.0").join(relative) };
            std::fs::copy(source, destination).unwrap();
        }
        let index = layout::files(&root, "legacy_uploads");
        std::fs::create_dir_all(&index).unwrap();
        std::fs::copy(fixture.join("files-index.json"), index.join("index.json")).unwrap();
        std::fs::copy(fixture.join("files-018f2b2c-4a10-7a01-8000-000000000001.blob"), index.join("018f2b2c-4a10-7a01-8000-000000000001.blob")).unwrap();
        let dist = layout::dist(&root, "legacy_uploads");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<!doctype html><title>Legacy uploads</title>").unwrap();
        let state = AppState::for_test(&root);
        let deployment = open(&state, "legacy_uploads", None).await.unwrap();
        assert_eq!(deployment.modules["files"]["on"], true);
        let rewritten: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "legacy_uploads")).unwrap()).unwrap();
        assert_eq!(rewritten["modules"]["files"]["on"], true);
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("legacy-uploads.localhost"));
        let response = crate::serve_file(State(state), AxumPath("018f2b2c-4a10-7a01-8000-000000000001".into()), headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), b"legacy file\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn follow_settings_reapplies_a_hand_edited_module_state() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        open(&state, "notes_app", None).await.unwrap();
        follow_settings(state.clone());
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut settings = Settings::read(&root, "notes_app").unwrap();
        settings.modules.get_mut("seo").unwrap()["on"] = serde_json::json!(true);
        settings.write(&root).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let current = state.deployments.read().await.values().next().unwrap().clone();
                if current.modules["seo"]["on"] == true { break current; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let current = state.deployments.read().await.values().next().unwrap().clone();
        assert_eq!(current.modules["seo"]["on"], true);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn restarting_a_source_ahead_folder_keeps_the_applied_access_actions_and_rows() {
        let root = scratch();
        let first_state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let first = open(&first_state, "notes_app", None).await.unwrap();
        Connection::open(layout::database(&root, "notes_app")).unwrap()
            .execute("INSERT INTO notes(body) VALUES('applied row')", []).unwrap();

        let mut draft = crate::sources::read(&root, "notes_app").await.unwrap().unwrap();
        draft["name"] = serde_json::json!("Draft name");
        draft["slug"] = serde_json::json!("draft-name");
        draft["files"]["actions.json"] = serde_json::json!(r#"{"version":1,"actions":{"draft.only":{"kind":"query","access":"anonymous","statement":"SELECT body FROM notes ORDER BY id LIMIT 50","bindings":[]}}}"#);
        draft["files"]["schema.sql"] = serde_json::json!("CREATE TABLE draft_only(id INTEGER PRIMARY KEY);");
        draft["manifest"]["visibility"] = serde_json::json!("private");
        draft["manifest"]["fileAccess"] = serde_json::json!("app-users");
        crate::sources::write(&root, "notes_app", &draft).await.unwrap();

        let restarted_state = AppState::for_test(&root);
        let running = open(&restarted_state, "notes_app", None).await.unwrap();
        assert_eq!(running.project_name, "Notes");
        assert_eq!(running.slug, "notes");
        assert_eq!(running.manifest["visibility"], "public");
        assert!(running.manifest.get("fileAccess").is_none());
        assert_eq!(running.actions, ACTIONS);
        assert!(running.released_schema.contains("CREATE TABLE IF NOT EXISTS notes"));
        let row: String = Connection::open(layout::database(&root, "notes_app")).unwrap()
            .query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(row, "applied row");
        assert_eq!(running.artifact_version, first.artifact_version);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn test_mode_applies_schema_only_source_edits_when_dist_is_unchanged() {
        let root = scratch();
        let mut state = AppState::for_test(&root);
        state.execution_mode = crate::execution::ExecutionMode::Test;
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let first = open(&state, "notes_app", None).await.unwrap();
        let database = layout::database(&root, "notes_app");
        Connection::open(&database).unwrap().execute("INSERT INTO notes(body) VALUES('kept')", []).unwrap();
        let original_artifact = first.artifact_version.clone();
        std::fs::write(layout::sources(&root, "notes_app").join("schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, extra TEXT);").unwrap();

        let restarted = open(&state, "notes_app", None).await.unwrap();

        assert_eq!(restarted.artifact_version, original_artifact);
        assert!(restarted.released_schema.contains("extra TEXT"));
        assert!(crate::table_columns(&Connection::open(&database).unwrap(), "notes").unwrap().iter().any(|column| column["name"] == "extra"));
        let kept: String = Connection::open(&database).unwrap().query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(kept, "kept");
        assert_eq!(restarted.source_digest.as_deref(), Some(crate::sources::digest_folder(&layout::sources(&root, "notes_app")).await.unwrap().as_str()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn test_mode_uses_and_persists_transform_source_edits_when_dist_is_unchanged() {
        let root = scratch();
        let mut state = AppState::for_test(&root);
        state.execution_mode = crate::execution::ExecutionMode::Test;
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        open(&state, "notes_app", None).await.unwrap();
        let database = layout::database(&root, "notes_app");
        let before = crate::schema_transform::fingerprint(&Connection::open(&database).unwrap()).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, extra TEXT);";
        let document = serde_json::json!({
            "version": 1,
            "transitions": [{
                "id": "add-extra",
                "predecessor": null,
                "before": before,
                "after": crate::schema_transform::fingerprint_schema(schema).unwrap(),
                "maxRows": 10,
                "statements": ["ALTER TABLE notes ADD COLUMN extra TEXT"]
            }]
        }).to_string();
        std::fs::write(layout::sources(&root, "notes_app").join("schema.sql"), schema).unwrap();
        std::fs::write(layout::sources(&root, "notes_app").join("schema-transforms.json"), &document).unwrap();

        let running = open(&state, "notes_app", None).await.unwrap();

        assert!(crate::table_columns(&Connection::open(&database).unwrap(), "notes").unwrap().iter().any(|column| column["name"] == "extra"));
        let settings = Settings::read(&root, "notes_app").unwrap();
        assert_eq!(settings.released.unwrap().schema_transforms.as_deref(), Some(document.as_str()));
        assert_eq!(running.artifact_version, format!("v{}", &version_of(&read_dist(&layout::dist(&root, "notes_app")).unwrap())[..32]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_start_keeps_the_release_time_until_the_files_change() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let first = open(&state, "notes_app", None).await.unwrap();
        assert!(first.released_at >= first.created_at);
        tokio::time::sleep(Duration::from_millis(5)).await;
        let same = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(same.released_at, first.released_at, "the same dist/ is the same release");
        std::fs::write(layout::dist(&root, "notes_app").join("index.html"), "<!doctype html><main>Notes 2</main>").unwrap();
        let changed = open(&state, "notes_app", None).await.unwrap();
        assert!(changed.released_at > first.released_at, "new files are a new release");
        assert_eq!(changed.created_at, first.created_at);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_start_names_the_sources_its_release_was_built_from() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let app = layout::sources(&root, "notes_app");
        let first = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(first.source_digest.as_deref(), Some(crate::sources::digest_folder(&app).await.unwrap().as_str()));
        std::fs::write(app.join("index.html"), "<main>edited</main>").unwrap();
        let same = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(same.source_digest, first.source_digest);
        std::fs::write(layout::dist(&root, "notes_app").join("index.html"), "<!doctype html><main>edited</main>").unwrap();
        let rebuilt = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(rebuilt.source_digest.as_deref(), Some(crate::sources::digest_folder(&app).await.unwrap().as_str()));
        assert_ne!(rebuilt.source_digest, first.source_digest);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_taken_slug_hostname_gets_a_code_suffix() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let first = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(first.hostname, "notes.localhost");
        write_folder(&root, "notes_other", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let path = layout::settings(&root, "notes_other");
        let mut settings: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings.slug = "notes".into();
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let second = open(&state, "notes_other", None).await.unwrap();
        assert!(second.hostname.starts_with("notes-"), "{}", second.hostname);
        assert!(second.hostname.ends_with(".localhost"), "{}", second.hostname);
        assert_ne!(second.hostname, first.hostname);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn folder_migrates_the_legacy_file_access_option() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let path = layout::settings(&root, "notes_app");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings.as_object_mut().unwrap().remove("modules");
        settings["manifest"]["fileAccess"] = serde_json::json!("app-users");
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let deployment = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(deployment.modules["files"]["on"], true);
        assert_eq!(deployment.modules["files"]["fileAccess"], "app-users");
        assert!(deployment.manifest.get("fileAccess").is_none());
        let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["modules"]["files"]["fileAccess"], "app-users");
        assert!(persisted["manifest"].get("fileAccess").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn the_settings_watcher_applies_operator_policy_without_applying_source_drafts() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let deployment = open(&state, "notes_app", None).await.unwrap();
        let mut source = crate::sources::read(&root, "notes_app").await.unwrap().unwrap();
        source["manifest"]["visibility"] = serde_json::json!("private");
        source["manifest"]["fileAccess"] = serde_json::json!("app-users");
        crate::sources::write(&root, "notes_app", &source).await.unwrap();
        assert_eq!(state.deployments.read().await[&deployment.hostname].manifest["visibility"], "public");
        follow_settings(state.clone());
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut settings = Settings::read(&root, "notes_app").unwrap();
        settings.signup_policy = crate::SignupPolicy::from_input(crate::SignupMode::Closed, &[]).unwrap();
        settings.write(&root).await.unwrap();
        for _ in 0..20 {
            if state.deployments.read().await[&deployment.hostname].signup_policy.mode == crate::SignupMode::Closed { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let running = state.deployments.read().await[&deployment.hostname].clone();
        assert_eq!(running.signup_policy.mode, crate::SignupMode::Closed);
        assert_eq!(running.manifest["visibility"], "public");
        assert!(running.manifest.get("fileAccess").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_change_that_loses_data_waits_for_its_confirmation() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, extra TEXT);");
        open(&state, "notes_app", None).await.unwrap();
        std::fs::write(layout::sources(&root, "notes_app").join("schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);").unwrap();
        open(&state, "notes_app", None).await.unwrap();
        std::fs::write(layout::dist(&root, "notes_app").join("index.html"), "<!doctype html><main>rebuilt</main>").unwrap();
        let refused = refused(open(&state, "notes_app", None).await);
        assert!(refused.contains("loses data") && refused.contains(CONFIRM_SCHEMA), "{refused}");
        let confirmation = refused.rsplit('=').next().unwrap().trim_end_matches('.').to_owned();
        open(&state, "notes_app", Some(&confirmation)).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_folder_that_breaks_a_release_check_is_refused() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        std::fs::write(layout::sources(&root, "notes_app").join("actions.json"), ACTIONS.replace(" LIMIT 50", "")).unwrap();
        let refused = refused(open(&state, "notes_app", None).await);
        assert!(refused.contains("LIMIT"), "{refused}");
        assert!(!layout::record(&root, "notes_app").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn editor_state(root: &Path) -> AppState {
        AppState {
            public_origin: "http://app.apps.example.com".into(),
            host_suffix: "apps.example.com".into(),
            ..AppState::for_test(root)
        }
    }

    fn is_coded(hostname: &str, slug: &str) -> bool {
        let code = hostname.strip_prefix(&format!("{slug}-")).and_then(|rest| rest.strip_suffix(".apps.example.com"));
        code.is_some_and(|code| code.len() == 6 && code.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    #[tokio::test]
    async fn a_first_publish_never_takes_the_editor_host() {
        let root = scratch();
        let state = editor_state(&root);
        let hostname = free_hostname(&state, "app", "app_test").await.0;
        assert!(is_coded(&hostname, "app"), "{hostname}");
        for slug in ["", "!!!", "日本語"] {
            let hostname = free_hostname(&state, slug, "app_test").await.0;
            assert!(is_coded(&hostname, "app"), "{slug:?} gave {hostname}");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_first_publish_never_takes_an_own_origin_or_a_claimed_name() {
        let root = scratch();
        let state = AppState { own_origins: vec!["http://mcp.apps.example.com".into()], ..editor_state(&root) };
        let hostname = free_hostname(&state, "mcp", "app_test").await.0;
        assert!(is_coded(&hostname, "mcp"), "{hostname}");
        let state = AppState { path_origin: Some("http://paths.apps.example.com".into()), ..editor_state(&root) };
        let hostname = free_hostname(&state, "paths", "app_test").await.0;
        assert!(is_coded(&hostname, "paths"), "{hostname}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_ordinary_slug_is_returned_bare() {
        let root = scratch();
        let state = editor_state(&root);
        assert_eq!(free_hostname(&state, "Notes", "app_test").await.0, "notes.apps.example.com");
        std::fs::remove_dir_all(root).ok();
    }
}
