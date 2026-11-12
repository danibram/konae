//! The Kernel alone serves each app folder as it finds it (ADR-0001). At
//! startup it reads `konae.json`, `app/` and `dist/` of every folder under
//! `apps/`, runs on them the checks a release gets, applies the schema
//! change `app/schema.sql` asks of `data/database.sqlite`, and writes the
//! record it serves the app by. An exported folder carries no record, so
//! this is how it starts to answer.
//!
//! While it serves, the Kernel follows each app's `konae.json`, so an
//! operator command that changes the Signup Policy takes effect without a
//! restart.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use rusqlite::Connection;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    find_project_deployment, initialize_database, layout, now_ms, open_auth, persist_deployment, runtime,
    safe_slug, schema_change, AppState, BuildArtifact, DeployInput, Deployment, Installed, ProjectInput, Settings,
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
    let settings = Settings::read(root, id).with_context(|| format!("apps/{id}/konae.json is missing or not readable"))?;
    ensure!(settings.id == id, "apps/{id}/konae.json names the app {}. Name the folder after the app's ID.", settings.id);
    let sources = layout::sources(root, id);
    let actions = tokio::fs::read_to_string(sources.join("actions.json"))
        .await
        .with_context(|| format!("apps/{id}/app/actions.json is missing"))?;
    let schema = tokio::fs::read_to_string(sources.join("schema.sql")).await.unwrap_or_default();
    let dist = read_dist(&layout::dist(root, id))
        .with_context(|| format!("apps/{id}/dist/ is missing or not readable. Build it with npm run build"))?;
    let artifact_version = format!("v{}", &version_of(&dist)[..32]);
    let mut manifest = settings.manifest.clone();
    if let Some(object) = manifest.as_object_mut() {
        object.insert("visibility".into(), Value::String(settings.visibility.clone()));
    }
    let encode = |bytes: &Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let input = DeployInput {
        project: ProjectInput {
            id: id.to_owned(),
            name: settings.name.clone(),
            slug: settings.slug.clone(),
            files: HashMap::from([("actions.json".to_owned(), actions.clone()), ("schema.sql".to_owned(), schema.clone())]),
            manifest,
        },
        visibility: settings.visibility.clone(),
        artifact: BuildArtifact {
            version: 1,
            entrypoint: "index.html".into(),
            encodings: dist.keys().map(|path| (path.clone(), "base64".to_owned())).collect(),
            files: dist.iter().map(|(path, bytes)| (path.clone(), encode(bytes))).collect(),
        },
        initial_signup_policy: None,
        initial_sign_in_methods: None,
        initial_package: None,
        contact_email: settings.contact_email.clone(),
        schema_confirmation: None,
    };
    // The checks of a release: paths, size, routing, connects, the actions and their secrets.
    runtime::validate_release(state, &input, None).await?;
    let program: crate::ActionProgram = serde_json::from_str(&actions)?;
    let _database = crate::database_lock(state, id).await;
    let database = layout::database(root, id);
    tokio::fs::create_dir_all(layout::data(root, id)).await?;
    if !database.exists() {
        initialize_database(&database)?;
    }
    let plan = plan(state, &database, &schema, &program)?;
    if let Some(expected) = plan.confirmation() {
        if confirmation != Some(expected.as_str()) {
            let losses = plan.losses.iter().map(|loss| loss.sentence.as_str()).collect::<Vec<_>>().join(" ");
            bail!("app/schema.sql asks for a change that loses data. {losses} To apply it, start the Kernel with {CONFIRM_SCHEMA}={expected}.");
        }
    }
    schema_change::apply(&database, &schema, &program, schema_change::Mode::Apply(plan.confirmation().as_deref()))?;
    let existing = find_project_deployment(&*state.deployments.read().await, id);
    let (hostname, created_at) = match existing.as_ref() {
        Some(deployment) => (deployment.hostname.clone(), deployment.created_at),
        None => (free_hostname(state, &settings.slug).await, now_ms()),
    };
    let deployment = settings.deployment(Installed { hostname, created_at, actions, artifact_version }, &state.public_origin);
    open_auth(state, &deployment)?;
    persist_deployment(state, &deployment).await?;
    let mut deployments = state.deployments.write().await;
    deployments.retain(|_, served| served.id != id);
    deployments.insert(deployment.hostname.clone(), deployment.clone());
    drop(deployments);
    crate::http::routes_published(&deployment);
    Ok(deployment)
}

/// The change `schema` asks of the live database, read from a copy.
fn plan(state: &AppState, database: &Path, schema: &str, program: &crate::ActionProgram) -> Result<schema_change::Change> {
    let copy = state.data_root.join(format!("validate-{}.sqlite", Uuid::new_v4()));
    let result = (|| {
        Connection::open(database)?.execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()])?;
        schema_change::apply(&copy, schema, program, schema_change::Mode::Plan)
    })();
    let _ = std::fs::remove_file(&copy);
    result
}

async fn free_hostname(state: &AppState, slug: &str) -> String {
    let slug = Some(safe_slug(slug)).filter(|slug| !slug.is_empty()).unwrap_or_else(|| "app".into());
    let wanted = format!("{slug}.{}", state.host_suffix);
    if !state.deployments.read().await.contains_key(&wanted) {
        return wanted;
    }
    let short_id = Uuid::new_v4().simple().to_string().chars().take(6).collect::<String>();
    format!("{slug}-{short_id}.{}", state.host_suffix)
}

/// Every file under `dist/`, by its path inside it.
fn read_dist(dist: &Path) -> Result<HashMap<String, Vec<u8>>> {
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

/// A digest of the files, so the same `dist/` keeps its version across starts.
fn version_of(files: &HashMap<String, Vec<u8>>) -> String {
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
    let installed = Installed {
        hostname: deployment.hostname.clone(),
        created_at: deployment.created_at,
        actions: deployment.actions.clone(),
        artifact_version: deployment.artifact_version.clone(),
    };
    let updated = settings.deployment(installed, &state.public_origin);
    state.deployments.write().await.insert(updated.hostname.clone(), updated);
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
    async fn folder_preserves_file_access_and_refuses_an_invalid_policy() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);");
        let path = layout::settings(&root, "notes_app");
        let mut settings: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings.manifest["fileAccess"] = serde_json::json!("app-users");
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let deployment = open(&state, "notes_app", None).await.unwrap();
        assert_eq!(deployment.manifest["fileAccess"], "app-users");
        assert_eq!(Settings::of(&deployment).manifest["fileAccess"], "app-users");
        settings.manifest["fileAccess"] = serde_json::json!("other");
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        reload(&state, deployment.clone()).await;
        assert_eq!(state.deployments.read().await[&deployment.hostname].manifest["fileAccess"], "app-users");
        assert!(refused(open(&state, "notes_app", None).await).contains(crate::artifact::FILE_ACCESS_MESSAGE));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_change_that_loses_data_waits_for_its_confirmation() {
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, extra TEXT);");
        open(&state, "notes_app", None).await.unwrap();
        std::fs::write(layout::sources(&root, "notes_app").join("schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);").unwrap();
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
}
