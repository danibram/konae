
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    find_project_deployment, initialize_database, layout, now_ms, open_auth, persist_release, runtime,
    schema_change, AppState, BuildArtifact, DeployInput, Deployment, Installed, ProjectInput, Settings,
};

/// The variable that confirms a schema change that loses data, as
/// `release_plan`'s confirmation does in Konae.
pub const CONFIRM_SCHEMA: &str = "KONAE_CONFIRM_SCHEMA";

#[derive(Clone, Copy)]
pub enum ModuleSource { Installed, Folder }

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
        let result = match open(state, &id, confirmation).await {
            Err(error) if error.is::<crate::functions::ConfigurationError>() => return Err(error),
            result => result,
        };
        opened.push((id, result));
    }
    Ok(opened)
}

/// Opens one app folder and serves it. The record keeps the address an
/// earlier start gave the app; a folder without one gets `<slug>.<suffix>`,
/// or a random address when another app holds that one.
pub async fn open(state: &AppState, id: &str, confirmation: Option<&str>) -> Result<Deployment> {
    let _release_guard = crate::named_lock(state, &format!("publish:{id}"), true).await;
    let root = &state.data_root;
    ensure!(runtime::valid_app_id(id), "apps/{id} is not named after an app ID. An app ID uses lowercase letters, digits, \"_\" and \"-\".");
    let mut settings = Settings::read(root, id).with_context(|| format!("apps/{id}/konae.json is missing or not readable"))?;
    ensure!(settings.id == id, "apps/{id}/konae.json names the app {}. Name the folder after the app's ID.", settings.id);
    decide_app_modules(root, id, ModuleSource::Folder).await?;
    settings = Settings::read(root, id)?;
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
    let server_artifact = crate::functions::read_artifact(&layout::server(root, id))?;
    let server = crate::functions::validate_artifact(server_artifact.as_ref().map(|a| &a.manifest), server_artifact.as_ref())?;
    let prepared = crate::functions::prepare_artifact(state, server_artifact.as_ref()).await
        .with_context(|| format!("Could not prepare published functions for app {id}"))?;
    if server.as_ref().is_some_and(|published| !published.manifest.functions.is_empty())
        && !state.functions.has_runner()
    {
        eprintln!("konae: app {id} publishes server functions, but KONAE_FUNCTIONS_RUNNER is unset. Set it to the functions runner executable to serve function requests.");
    }
    let server_changed = released.as_ref().is_some_and(|r| r.server != server);
    let rebuilt = server_changed || released.as_ref().and_then(|snapshot| snapshot.artifact_digest.as_deref()).is_some_and(|digest| digest != dist_digest);
    let candidate_sources = state.execution_mode.is_test();
    let actions_enabled = crate::modules::is_on(&settings.modules, "actions");
    let source_actions = || async { tokio::fs::read_to_string(sources.join("actions.json")).await.unwrap_or_else(|_| r#"{"version":1,"actions":{}}"#.to_owned()) };
    let source_schema = || async { tokio::fs::read_to_string(sources.join("schema.sql")).await.unwrap_or_default() };
    let source_transforms = || async { optional_text(&sources.join("schema-transforms.json")).await };
    let actions = if candidate_sources || rebuilt || (released.is_none() && stored.is_none()) {
        source_actions().await
    } else if let Some(snapshot) = released.as_ref() {
        snapshot.actions.clone()
    } else if let Some(record) = stored.as_ref() {
        record.actions.clone()
    } else { source_actions().await };
    let program = crate::action_shape::read(&actions)?;
    for (name, action) in &program.actions {
        let Some(file) = action.page.as_ref().and_then(|page| page.get("file")).and_then(Value::as_str) else { continue };
        ensure!(dist.contains_key(file), "Action {name} names {file} as its page file, but the Build Artifact does not contain it.");
    }
    let schema = if !actions_enabled {
        String::new()
    } else if candidate_sources || rebuilt || (released.is_none() && stored.is_none()) {
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
            let mut files = HashMap::new();
                files.insert("actions.json".to_owned(), actions.clone());
                files.insert("schema.sql".to_owned(), schema.clone());
                if let Some(document) = &schema_transforms { files.insert("schema-transforms.json".to_owned(), document.clone()); }
                let function_source = if candidate_sources || rebuilt || fresh_sources {
                    optional_text(&sources.join("functions.json")).await?
                } else {
                    server.as_ref().map(|published| serde_json::to_string(&published.manifest)).transpose()?
                };
                if let Some(source) = function_source {
                    let declared = crate::functions::read(&source)?;
                    for function in declared.functions.values() {
                        if candidate_sources || rebuilt || fresh_sources {
                            if let Ok(text) = tokio::fs::read_to_string(sources.join(&function.entry)).await { files.insert(function.entry.clone(), text); }
                        }
                    }
                    files.insert("functions.json".into(), source);
                }
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
        server_artifact,
        initial_released: None,
        initial_signup_policy: None,
        initial_sign_in_methods: None,
        initial_modules: Some(settings.modules.clone()),
        initial_package: None,
        contact_email: settings.contact_email.clone().or_else(|| released.as_ref().and_then(|snapshot| snapshot.contact_email.clone())),
        schema_confirmation: None,
    };
    if candidate_sources || rebuilt || fresh_sources { runtime::validate_release(state, &input, None).await?; }
    else { crate::deployments::validate_published_release(state, &input, None).await?; }
    let program: crate::ActionProgram = serde_json::from_str(&actions)?;
    let _database = crate::database_lock(state, id).await;
    let database = layout::database(root, id);
    if actions_enabled || server.is_some() {
        tokio::fs::create_dir_all(layout::data(root, id)).await?;
    }
    if actions_enabled && !database.exists() {
        initialize_database(&database)?;
    }
    let plan = if actions_enabled { plan(state, &database, &schema, &program, schema_transforms.as_deref())? } else { schema_change::Change::default() };
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
    if actions_enabled { schema_change::apply_with_transforms(&database, &schema, &program, schema_transforms.as_deref(), schema_change::Mode::Apply(plan.confirmation().as_deref()))?; }
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
    let revision_digest = crate::functions::revision_digest(&dist_digest, server.as_ref());
    let artifact_version = if unchanged { stored.as_ref().map(|record| record.artifact_version.clone()).or_else(|| existing.as_ref().map(|current| current.artifact_version.clone())).unwrap_or_else(|| format!("v{}", &revision_digest[..32])) } else { format!("v{}", &revision_digest[..32]) };
    let policy = settings.signup_policy.clone();
    let sign_in_methods = settings.sign_in_methods.clone();
    let deployment = Deployment {
        id: id.to_owned(), project_id: id.to_owned(), project_name: input.project.name.clone(), slug: input.project.slug.clone(),
        url: crate::deployment_url(&state.public_origin, &hostname), created_at, released_at, source_digest,
        manifest: input.project.manifest.clone(), hostname, actions, artifact_version, signup_policy: policy,
        sign_in_methods, contact_email: input.contact_email.clone(), modules: settings.modules.clone(), released_schema: schema, server,
    };
    if crate::modules::is_on(&deployment.modules, "auth") { open_auth(state, &deployment)?; }
    if rebuilt { persist_release(state, &deployment, &dist_digest, &settings.directories, schema_transforms.as_deref(), None).await?; }
    else if candidate_sources || fresh_sources { crate::persist_folder_snapshot_with_transforms(state, &deployment, &dist_digest, schema_transforms.as_deref()).await?; }
    else { crate::persist_folder_snapshot(state, &deployment, &dist_digest).await?; }
    let mut deployments = state.deployments.write().await;
    deployments.retain(|_, served| served.id != id);
    deployments.insert(deployment.hostname.clone(), deployment.clone());
    drop(deployments);
    state.functions.retain_prepared(id, prepared);
    crate::http::routes_published(&deployment);
    state.functions.release_changed(id, &deployment.artifact_version).await;
    Ok(deployment)
}

pub async fn decide_app_modules(root: &Path, id: &str, source: ModuleSource) -> Result<Option<std::collections::BTreeMap<String, Value>>> {
    ensure!(runtime::valid_app_id(id), "apps/{id} is not named after an app ID. An app ID uses lowercase letters, digits, \"_\" and \"-\".");
    let raw_settings: Value = serde_json::from_slice(&std::fs::read(layout::settings(root, id)).with_context(|| format!("apps/{id}/konae.json is missing or not readable"))?)?;
    ensure!(raw_settings["id"].as_str() == Some(id), "apps/{id}/konae.json names the app {}. Name the folder after the app's ID.", raw_settings["id"].as_str().unwrap_or(""));
    let mut settings = Settings::read(root, id)?;
    let mut changed = false;
    for module in crate::modules::defaults().keys() {
        match module.as_str() {
            "auth" => {
                if raw_settings["modules"]["auth"].is_null() {
                    let used = crate::modules::uses_auth(root, id);
                    settings.modules.get_mut("auth").expect("auth is switchable")["on"] = json!(used);
                    changed = true;
                }
            }
            "files" => {
                let legacy_file_access = raw_settings["released"]["manifest"]["fileAccess"].as_str();
                let used = crate::modules::uses_files(root, id, &json!({"fileAccess": legacy_file_access}));
                if raw_settings["modules"]["files"]["on"] != true
                    && (raw_settings["modules"]["files"].is_null() || used || legacy_file_access.is_some())
                {
                    let files = settings.modules.get_mut("files").expect("files is switchable");
                    files["on"] = json!(used || legacy_file_access == Some("app-users"));
                    if legacy_file_access == Some("app-users") { files["fileAccess"] = json!("app-users"); }
                    changed = true;
                }
            }
            "actions" => {
                if raw_settings["modules"]["actions"].is_null() {
                    let used = match source { ModuleSource::Installed => crate::modules::uses_actions(root, id), ModuleSource::Folder => crate::modules::folder_uses_actions(root, id) };
                    settings.modules.get_mut("actions").expect("actions is switchable")["on"] = json!(used);
                    changed = true;
                }
            }
            "http" => {
                if raw_settings["modules"]["http"].is_null() {
                    let used = match source { ModuleSource::Installed => crate::modules::uses_http(root, id), ModuleSource::Folder => crate::modules::folder_uses_http(root, id) };
                    settings.modules.get_mut("http").expect("http is switchable")["on"] = json!(used);
                    changed = true;
                }
            }
            "calls" => {
                if raw_settings["modules"]["calls"].is_null() {
                    let used = match source { ModuleSource::Installed => crate::modules::uses_calls(root, id), ModuleSource::Folder => crate::modules::folder_uses_calls(root, id) };
                    settings.modules.get_mut("calls").expect("calls is switchable")["on"] = json!(used);
                    changed = true;
                }
            }
            _ => {}
        }
    }
    if !changed { return Ok(None); }
    settings.write(root).await?;
    eprintln!("konae: decided legacy module state for {id}");
    Ok(Some(settings.modules))
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
        crate::copies::vacuum_into(database, &copy)?;
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
    use rusqlite::Connection;

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

    #[tokio::test]
    async fn legacy_module_decisions_read_installed_actions_and_persist_them() {
        let root = scratch();
        for (id, fixture, expected_http, expected_calls) in [
            ("legacy_http", "module-http-route-0.9.0", true, false),
            ("legacy_calls", "module-calls-call-0.9.0", false, true),
            ("legacy_actions_only", "module-actions-only-0.9.0", false, false),
        ] {
            std::fs::create_dir_all(layout::sources(&root, id)).unwrap();
            let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures").join(fixture);
            let actions = std::fs::read_to_string(fixture.join("app/actions.json")).unwrap();
            std::fs::write(layout::sources(&root, id).join("actions.json"), r#"{"version":1,"actions":{}}"#).unwrap();
            let mut settings = Settings::new(id, "Legacy", id, &json!({"visibility":"public"}));
            settings.modules.remove("calls");
            settings.modules.remove("http");
            settings.write(&root).await.unwrap();
            std::fs::create_dir_all(layout::private(&root, id)).unwrap();
            std::fs::write(layout::record(&root, id), serde_json::to_vec(&json!({"actions":actions})).unwrap()).unwrap();
            decide_app_modules(&root, id, ModuleSource::Installed).await.unwrap();
            let saved: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, id)).unwrap()).unwrap();
            assert_eq!(saved["modules"]["http"]["on"], expected_http, "{id}");
            assert_eq!(saved["modules"]["calls"]["on"], expected_calls, "{id}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn folder_open_decides_legacy_actions_http_and_calls_from_the_folder_sources() {
        let root = scratch();
        for (id, actions, module) in [
            ("legacy_action_source", r#"{"version":1,"actions":{"notes.list":{"kind":"query","access":"anonymous","statement":"SELECT 1 AS n LIMIT 1","bindings":[]}}}"#, "actions"),
            ("legacy_http_source", r#"{"version":1,"actions":{"hooks.receive":{"kind":"query","access":"anonymous","statement":"SELECT 1 AS n LIMIT 1","bindings":[],"http":{"method":"GET","path":"/hooks"}}}}"#, "http"),
            ("legacy_call_source", r#"{"version":1,"actions":{"notify.send":{"kind":"call","access":"anonymous","method":"POST","url":"https://example.test/hook","bindings":[]}}}"#, "calls"),
        ] {
            let folder = layout::folder(&root, id);
            std::fs::create_dir_all(folder.join("app")).unwrap();
            std::fs::create_dir_all(folder.join("dist")).unwrap();
            std::fs::write(folder.join("app/actions.json"), actions).unwrap();
            std::fs::write(folder.join("app/schema.sql"), "").unwrap();
            std::fs::write(folder.join("dist/index.html"), "<!doctype html><title>Legacy</title>").unwrap();
            let mut settings = Settings::new(id, "Legacy", id, &json!({"visibility":"public"}));
            if module != "actions" { settings.modules.insert("actions".into(), crate::modules::defaults()["actions"].clone()); settings.modules.get_mut("actions").unwrap()["on"] = json!(true); }
            settings.modules.remove("http");
            settings.modules.remove("calls");
            if module == "actions" { settings.modules.remove("actions"); }
            let released = crate::ReleasedSettings {
                name: "Legacy".into(), slug: id.into(), visibility: "public".into(), signup_policy: crate::SignupPolicy::initial("public"),
                sign_in_methods: crate::default_sign_in_methods(), contact_email: None, manifest: json!({"visibility":"public"}),
                actions: r#"{"version":1,"actions":{}}"#.into(), schema_sql: String::new(), schema_transforms: None, server: None, source_digest: None,
                artifact_digest: Some("v0.9.0-artifact".into()), released_at: Some(2),
            };
            settings.released = Some(released.clone());
            settings.write(&root).await.unwrap();
            std::fs::create_dir_all(layout::private(&root, id)).unwrap();
            let installed = Installed { hostname: format!("{id}.localhost"), created_at: 1, released_at: Some(2), source_digest: None, actions: released.actions.clone(), artifact_version: "v0.9.0".into(), released: Some(released) };
            std::fs::write(layout::record(&root, id), serde_json::to_vec(&installed).unwrap()).unwrap();
        }
        let mut state = AppState::for_test(&root);
        state.execution_mode = crate::execution::ExecutionMode::Live;
        for (id, module) in [("legacy_action_source", "actions"), ("legacy_http_source", "http"), ("legacy_call_source", "calls")] {
            let deployment = open(&state, id, None).await.unwrap();
            assert!(crate::modules::is_on(&deployment.modules, module), "{id} {module}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn load_deployments_keeps_legacy_signed_routes_and_calls_live() {
        use axum::http::StatusCode;

        let root = scratch();
        let route_fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-http-route-0.9.0");
        let calls_fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-calls-call-0.9.0");
        let actions_fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-actions-only-0.9.0");
        for (id, fixture) in [("legacy_http", route_fixture), ("legacy_calls", calls_fixture), ("legacy_actions_only", actions_fixture)] {
            let source = layout::sources(&root, id);
            std::fs::create_dir_all(&source).unwrap();
            let dist = layout::dist(&root, id);
            std::fs::create_dir_all(&dist).unwrap();
            std::fs::write(dist.join("index.html"), "<!doctype html><title>Legacy app</title><main>Still serving</main>").unwrap();
            let actions = std::fs::read_to_string(fixture.join("app/actions.json")).unwrap();
            std::fs::copy(fixture.join("app/actions.json"), source.join("actions.json")).unwrap();
            let schema = std::fs::read_to_string(fixture.join("app/schema.sql")).unwrap();
            std::fs::write(source.join("schema.sql"), &schema).unwrap();
            let mut settings = Settings::new(id, "Legacy", id, &json!({"visibility":"public"}));
            settings.modules.remove("http");
            settings.modules.remove("calls");
            settings.write(&root).await.unwrap();
            if id == "legacy_http" {
                std::fs::create_dir_all(layout::data(&root, id)).unwrap();
                initialize_database(&layout::database(&root, id)).unwrap();
                Connection::open(layout::database(&root, id)).unwrap().execute_batch(&schema).unwrap();
                crate::http::secrets_call(&crate::http::secrets_store(&root, id).unwrap(), id, None, "secret_set", &json!({"name":"legacy_hook","value":"legacy-route-secret-1234"})).unwrap();
            }
            std::fs::create_dir_all(layout::private(&root, id)).unwrap();
            let installed = Installed { hostname: format!("{id}.localhost"), created_at: 1, released_at: Some(2), source_digest: None, actions, artifact_version: "v0.9.0".into(), released: None };
            std::fs::write(layout::record(&root, id), serde_json::to_vec(&installed).unwrap()).unwrap();
        }

        let mut state = AppState::for_test(&root);
        state.execution_mode = crate::execution::ExecutionMode::Test;
        state.deployments = std::sync::Arc::new(tokio::sync::RwLock::new(crate::load_deployments(&root, "http://localhost").await.unwrap()));
        let loaded = state.deployments.read().await;
        let modules = |id: &str| loaded.values().find(|deployment| deployment.id == id).unwrap().modules.clone();
        assert!(modules("legacy_http")["http"]["on"] == true);
        assert!(modules("legacy_calls")["calls"]["on"] == true);
        assert!(modules("legacy_actions_only")["actions"]["on"] == true);
        assert!(modules("legacy_actions_only")["http"]["on"] == false);
        assert!(modules("legacy_actions_only")["calls"]["on"] == false);
        drop(loaded);

        let body = br#"{"name":"delivered"}"#;
        let signed = crate::http::SignedRequest { method: "POST", path: "/hooks/events", query: "", body };
        let timestamp = now_ms() / 1000;
        let signature = crate::http::sign("legacy-route-secret-1234", timestamp, "legacy-nonce", &signed);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(crate::serve(listener, state.clone(), axum::Router::new()));
        let static_page = reqwest::Client::new().get(format!("http://{address}/index.html"))
            .header("host", "legacy_actions_only.localhost").send().await.unwrap();
        assert_eq!(static_page.status(), StatusCode::OK);
        assert!(static_page.text().await.unwrap().contains("Still serving"));
        let response = reqwest::Client::new().post(format!("http://{address}/hooks/events"))
            .header("host", "legacy_http.localhost")
            .header("content-type", "application/json")
            .header("X-Konae-Signature", signature)
            .body(body.to_vec()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        server.abort();
        let stored: String = Connection::open(layout::database(&root, "legacy_http")).unwrap().query_row("SELECT name FROM events", [], |row| row.get(0)).unwrap();
        assert_eq!(stored, "delivered");

        let caller = state.deployments.read().await.values().find(|deployment| deployment.id == "legacy_calls").unwrap().clone();
        assert_eq!(crate::calls::enqueue(&state, &caller, "notice.send", &json!({})).await.status(), StatusCode::OK);
        assert_eq!(crate::calls::run_once(&state).await.unwrap(), 1);
        assert_eq!(crate::calls::list_captured_requests_for_app(&state, "legacy_calls").await.unwrap().len(), 1);
        let saved: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "legacy_http")).unwrap()).unwrap();
        assert_eq!(saved["modules"]["http"]["on"], true);
        let _ = std::fs::remove_dir_all(root);
    }

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
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_folder_with_functions_and_no_actions_has_room_for_usage() {
        let root = scratch();
        let id = "function_only";
        let state = AppState::for_test(&root);
        let input: DeployInput = serde_json::from_value(crate::functions::tests::release(
            id, "export function handle() { return new Response('ok'); }",
        )).unwrap();
        let sources = layout::sources(&root, id);
        std::fs::create_dir_all(sources.join("functions")).unwrap();
        for (name, source) in input.project.files {
            std::fs::write(sources.join(name), source).unwrap();
        }
        let dist = layout::dist(&root, id);
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), &input.artifact.files["index.html"]).unwrap();
        crate::functions::stage(&layout::server(&root, id), input.server_artifact.as_ref()).await.unwrap();
        let mut settings = Settings::new(id, "Function only", id, &json!({"visibility": "public"}));
        settings.modules.get_mut("functions").unwrap()["on"] = json!(true);
        settings.write(&root).await.unwrap();

        assert!(!layout::data(&root, id).exists());
        let deployment = open(&state, id, None).await.unwrap();
        assert!(layout::data(&root, id).is_dir());
        assert!(!crate::modules::is_on(&deployment.modules, "actions"));
        assert!(!layout::database(&root, id).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_static_folder_without_actions_or_schema_creates_no_databases_or_sdk() {
        use axum::{extract::{Path as AxumPath, State}, http::{HeaderMap, HeaderValue, StatusCode, header}, Json};
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "static_app", "");
        let sources = layout::sources(&root, "static_app");
        std::fs::remove_file(sources.join("actions.json")).unwrap();
        std::fs::remove_file(sources.join("schema.sql")).unwrap();
        std::fs::write(layout::dist(&root, "static_app").join("index.html"), "<!doctype html><title>Static</title>").unwrap();

        let deployment = open(&state, "static_app", None).await.unwrap();
        assert!(!crate::layout::database(&root, "static_app").exists());
        assert_eq!(crate::http::api_keys_call(&state, &deployment, "api_keys_list", &json!({})).await.unwrap()["keys"], json!([]));
        assert!(!crate::layout::identity(&root, "static_app").exists());
        assert!(!crate::google::client_set(&state, &deployment).id);
        let secrets = crate::http::read_secrets(&root, "static_app").unwrap();
        let list = crate::http::secrets_call(&secrets, "static_app", Some(&deployment), "secrets_list", &json!({})).unwrap();
        assert_eq!(list["secrets"], json!([]));
        assert!(!crate::layout::secrets(&root, "static_app").exists());
        assert_eq!(crate::calls::admin_call(&state, &deployment, &json!({})).await.unwrap()["calls"], json!([]));
        assert!(!root.join(crate::calls::CALLS_DATABASE).exists());
        assert!(!crate::modules::is_on(&deployment.modules, "actions"));
        let html = crate::read_artifact(&state, &deployment, "index.html").await.unwrap();
        assert!(!String::from_utf8(html).unwrap().contains("runtime.js"));

        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("notes.localhost"));
        let response = crate::invoke_action(State(state), AxumPath("missing.action".into()), headers, Json(json!({}))).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], "Module actions is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn load_deployments_migrates_an_old_action_folder_and_http_returns_its_rows() {
        use axum::{http::{HeaderValue, Request, StatusCode, header}, routing::post, Router};
        use tower::ServiceExt;
        let root = scratch();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-actions-0.9.0");
        for relative in ["konae.json", "app/actions.json", "app/schema.sql"] {
            let destination = layout::folder(&root, "legacy_actions").join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(fixture.join(relative), destination).unwrap();
        }
        let mut settings: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "legacy_actions")).unwrap()).unwrap();
        settings["id"] = json!("legacy_actions");
        settings["slug"] = json!("legacy-actions");
        std::fs::write(layout::settings(&root, "legacy_actions"), serde_json::to_vec(&settings).unwrap()).unwrap();
        std::fs::create_dir_all(layout::dist(&root, "legacy_actions")).unwrap();
        std::fs::write(layout::dist(&root, "legacy_actions").join("index.html"), "<!doctype html><title>Legacy actions</title>").unwrap();
        std::fs::create_dir_all(layout::private(&root, "legacy_actions")).unwrap();
        let actions = std::fs::read_to_string(layout::sources(&root, "legacy_actions").join("actions.json")).unwrap();
        let installed = crate::Installed { hostname: "legacy-actions.localhost".into(), created_at: 1, released_at: Some(2), source_digest: None, actions, artifact_version: "v1".into(), released: None };
        std::fs::write(layout::record(&root, "legacy_actions"), serde_json::to_vec(&installed).unwrap()).unwrap();
        let database = layout::database(&root, "legacy_actions");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        crate::initialize_database(&database).unwrap();
        Connection::open(&database).unwrap().execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL); INSERT INTO notes(body) VALUES('kept row');").unwrap();

        let mut state = AppState::for_test(&root);
        state.deployments = std::sync::Arc::new(tokio::sync::RwLock::new(crate::load_deployments(&root, "http://localhost").await.unwrap()));
        let deployment = state.deployments.read().await["legacy-actions.localhost"].clone();
        assert!(deployment.modules["actions"]["on"] == true);
        assert!(deployment.actions.contains("notes.route") && deployment.actions.contains("notes.tick"));
        let app = Router::new().route("/_konae/actions/{name}", post(crate::invoke_action)).with_state(state);
        let response = app.oneshot(Request::builder().method("POST").uri("/_konae/actions/notes.list").header(header::HOST, HeaderValue::from_static("legacy-actions.localhost")).header(header::CONTENT_TYPE, "application/json").body(axum::body::Body::from("{}")).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
        assert_eq!(body["result"][0]["body"], "kept row");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn load_deployments_keeps_a_legacy_database_and_turns_actions_on_when_its_schema_is_unknown() {
        let root = scratch();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-actions-empty-0.9.0");
        for relative in ["konae.json", "app/actions.json", "app/schema.sql"] {
            let destination = layout::folder(&root, "legacy_empty_actions").join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(fixture.join(relative), destination).unwrap();
        }
        let settings_path = layout::settings(&root, "legacy_empty_actions");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        settings["id"] = json!("legacy_empty_actions");
        settings["slug"] = json!("legacy-empty-actions");
        std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        std::fs::create_dir_all(layout::dist(&root, "legacy_empty_actions")).unwrap();
        std::fs::write(layout::dist(&root, "legacy_empty_actions").join("index.html"), "<!doctype html><title>Legacy static</title><main>Still here</main>").unwrap();
        let actions = std::fs::read_to_string(layout::sources(&root, "legacy_empty_actions").join("actions.json")).unwrap();
        let installed = crate::Installed { hostname: "legacy-empty-actions.localhost".into(), created_at: 1, released_at: Some(2), source_digest: None, actions, artifact_version: "v1".into(), released: None };
        std::fs::create_dir_all(layout::private(&root, "legacy_empty_actions")).unwrap();
        std::fs::write(layout::record(&root, "legacy_empty_actions"), serde_json::to_vec(&installed).unwrap()).unwrap();
        let database = layout::database(&root, "legacy_empty_actions");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        Connection::open(&database).unwrap().execute_batch("CREATE TABLE old_rows(value TEXT NOT NULL); INSERT INTO old_rows VALUES('keep');").unwrap();

        let mut state = AppState::for_test(&root);
        state.deployments = std::sync::Arc::new(tokio::sync::RwLock::new(crate::load_deployments(&root, "http://localhost").await.unwrap()));
        assert!(state.deployments.read().await["legacy-empty-actions.localhost"].modules["actions"]["on"].as_bool().unwrap());
        let deployment = open(&state, "legacy_empty_actions", None).await.unwrap();
        assert!(crate::modules::is_on(&deployment.modules, "actions"));
        let page = crate::read_artifact(&state, &deployment, "index.html").await.unwrap();
        assert!(String::from_utf8(page).unwrap().contains("Still here"));
        let kept: String = Connection::open(&database).unwrap().query_row("SELECT value FROM old_rows", [], |row| row.get(0)).unwrap();
        assert_eq!(kept, "keep");
        assert!(database.is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_folder_with_a_missing_page_template_is_refused() {
        let cases: serde_json::Value = serde_json::from_str(include_str!("../page-release-cases.json")).unwrap();
        let expected = cases[0]["error"].as_str().unwrap();
        let root = scratch();
        let state = AppState::for_test(&root);
        write_folder(&root, "notes_app", "CREATE TABLE notes(id INTEGER PRIMARY KEY);");
        std::fs::write(layout::sources(&root, "notes_app").join("actions.json"), r#"{"version":1,"actions":{"posts.head":{"kind":"query","access":"anonymous","statement":"SELECT title FROM posts LIMIT 1","bindings":[],"page":{"path":"/posts","file":"post.html"}}}}"#).unwrap();
        let error = refused(open(&state, "notes_app", None).await);
        assert!(error.contains(expected), "{error}");
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
        assert_eq!(listed["modules"]["actions"]["state"]["on"], false);
        let rewritten: serde_json::Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "app_legacy_modules")).unwrap()).unwrap();
        assert_eq!(rewritten["modules"]["files"]["on"], false);
        assert_eq!(rewritten["modules"].as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>(), ["actions".to_owned(), "auth".to_owned(), "calls".to_owned(), "files".to_owned(), "functions".to_owned(), "http".to_owned(), "seo".to_owned()].into());
        assert!(!layout::database(&root, "app_legacy_modules").exists());
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
        assert_eq!(deployment.modules["auth"]["on"], true);
        let rewritten: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "legacy_uploads")).unwrap()).unwrap();
        assert_eq!(rewritten["modules"]["files"]["on"], true);
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("legacy-uploads.localhost"));
        let response = crate::files::serve_file(State(state), AxumPath("018f2b2c-4a10-7a01-8000-000000000001".into()), headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), b"legacy file\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn konae_deployment_loading_migrates_legacy_files_before_serving_and_keeps_unused_files_off() {
        use axum::{http::{HeaderMap, HeaderValue, Request, StatusCode, header}, Router};
        use tower::ServiceExt;
        let root = scratch();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures");
        let install_fixture = |id: &str, fixture: &str, file_fixture: bool| {
            let source = fixtures.join(fixture);
            let folder = layout::folder(&root, id);
            std::fs::create_dir_all(folder.join("app")).unwrap();
            let mut settings: Value = serde_json::from_slice(&std::fs::read(source.join("konae.json")).unwrap()).unwrap();
            settings["id"] = json!(id);
            std::fs::write(layout::settings(&root, id), serde_json::to_vec(&settings).unwrap()).unwrap();
            let actions = fixtures.join("module-mechanism-0.9.0/app/actions.json");
            let schema = fixtures.join("module-mechanism-0.9.0/app/schema.sql");
            std::fs::copy(actions, folder.join("app/actions.json")).unwrap();
            std::fs::copy(schema, folder.join("app/schema.sql")).unwrap();
            let dist = layout::dist(&root, id);
            std::fs::create_dir_all(&dist).unwrap();
            std::fs::write(dist.join("index.html"), "<title>Legacy app</title>").unwrap();
            if file_fixture {
                let data = layout::files(&root, id);
                std::fs::create_dir_all(&data).unwrap();
                std::fs::copy(source.join("files-index.json"), data.join("index.json")).unwrap();
                std::fs::copy(source.join("files-018f2b2c-4a10-7a01-8000-000000000001.blob"), data.join("018f2b2c-4a10-7a01-8000-000000000001.blob")).unwrap();
            }
            std::fs::create_dir_all(layout::private(&root, id)).unwrap();
            let installed = crate::Installed { hostname: format!("{id}.localhost"), created_at:1, released_at:Some(2), source_digest:None, actions:include_str!("../../../test-fixtures/module-mechanism-0.9.0/app/actions.json").into(), artifact_version:"v1".into(), released:None };
            std::fs::write(layout::record(&root, id), serde_json::to_vec(&installed).unwrap()).unwrap();
        };
        install_fixture("legacy_uploads", "module-files-0.9.0", true);
        install_fixture("legacy_static", "module-mechanism-0.9.0", false);
        let mut state = AppState::for_test(&root);
        let loaded = crate::load_deployments(&root, "http://localhost").await.unwrap();
        assert_eq!(loaded["legacy_uploads.localhost"].modules["files"]["on"], true);
        assert_eq!(loaded["legacy_uploads.localhost"].modules["auth"]["on"], true);
        assert_eq!(loaded["legacy_static.localhost"].modules["files"]["on"], false);
        assert_eq!(loaded["legacy_static.localhost"].modules["actions"]["on"], false);
        assert!(!layout::database(&root, "legacy_static").exists());
        state.deployments = std::sync::Arc::new(tokio::sync::RwLock::new(loaded));
        let auth = crate::auth::AuthService::open(layout::identity(&root, "legacy_uploads")).unwrap();
        let session = auth.signup("uploader@example.test", "correct horse battery").unwrap();
        let index_path = layout::files(&root, "legacy_uploads").join("index.json");
        let mut index: Value = serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
        index[0]["visibility"] = json!("private");
        index[0]["ownerId"] = json!(session.user.user_id);
        std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("legacy_uploads.localhost"));
        headers.insert(header::COOKIE, format!("konae_app_session={}", session.token).parse().unwrap());
        let request = Request::builder().uri("/_konae/files/018f2b2c-4a10-7a01-8000-000000000001").body(axum::body::Body::empty()).unwrap();
        let mut request = request;
        *request.headers_mut() = headers;
        let response = crate::stack(state.clone(), Router::new()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(axum::body::to_bytes(response.into_body(), 1024).await.unwrap().as_ref(), b"legacy file\n");
        let persisted: Value = serde_json::from_slice(&std::fs::read(layout::settings(&root, "legacy_uploads")).unwrap()).unwrap();
        assert_eq!(persisted["modules"]["files"]["on"], true);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn module_decision_refuses_mismatched_and_traversal_ids_before_writing() {
        let root = scratch();
        write_folder(&root, "shop", "");
        write_folder(&root, "shop_test", "");
        let shop_path = layout::settings(&root, "shop");
        let shop_before = std::fs::read(&shop_path).unwrap();
        let copied_path = layout::settings(&root, "shop_test");
        let mut copied: Value = serde_json::from_slice(&std::fs::read(&copied_path).unwrap()).unwrap();
        copied["id"] = json!("shop");
        std::fs::write(&copied_path, serde_json::to_vec(&copied).unwrap()).unwrap();
        assert!(decide_app_modules(&root, "shop_test", ModuleSource::Installed).await.is_err());
        assert_eq!(std::fs::read(shop_path).unwrap(), shop_before);

        let outside = root.join("x").join("konae.json");
        assert!(decide_app_modules(&root, "../../x", ModuleSource::Installed).await.is_err());
        assert!(!outside.exists());
        let _ = std::fs::remove_dir_all(root);
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
        open(&state, "notes_app", None).await.unwrap();
        let path = layout::settings(&root, "notes_app");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings.as_object_mut().unwrap().remove("modules");
        settings["released"]["manifest"]["fileAccess"] = serde_json::json!("app-users");
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
