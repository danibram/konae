use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    ActionProgram, AppState, DeployInput, Deployment, Settings, SignupPolicy, action_shape,
    artifact, calls, copies, database_lock, default_sign_in_methods, deployment_url, domain,
    error, file_store, find_project_deployment, folder, functions, http, initialize_database,
    is_private, layout, modules, named_lock, now_ms, open_auth, package, path, persist_release,
    refusal, refusal_error, runtime, safe_relative_path, schema_change,
    sign_in_methods_from_input, size, sources,
};
use super::records::Recorded;

/// Installs a release from its JSON payload: the project, the build and
/// what a first release imports.
pub async fn deploy(state: &AppState, body: &[u8]) -> Response {
    if body.len() > size::RELEASE_BYTES {
        return error(StatusCode::BAD_REQUEST, &size::release_refused(body.len()));
    }
    let input: DeployInput = match serde_json::from_slice(body).map_err(anyhow::Error::from).and_then(functions::parse_deploy_input) {
        Ok(value) => value,
        Err(error_value) => {
            if let Some(refused) = refusal::of(&error_value) { return refusal_error(refused); }
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid deployment: {error_value}"),
            );
        }
    };
    deploy_input(state, input).await
}

/// Installs a release: the project, the build and what a first release
/// imports. `app/`, `dist/`, the database and the record change together, or
/// none of them does.
pub async fn deploy_input(state: &AppState, input: DeployInput) -> Response {
    deploy_input_in(state, input, false).await
}

/// The owner import door calls this after checking its project permission and package.
pub async fn deploy_import_held(state: &AppState, input: DeployInput) -> Response {
    deploy_input_in(state, input, true).await
}

async fn deploy_input_in(state: &AppState, input: DeployInput, restoring_import: bool) -> Response {
    let validation = if restoring_import { runtime::validate_import_release(state, &input, None).await }
        else { runtime::validate_release(state, &input, None).await };
    if let Err(error_value) = validation {
        return match refusal::of(&error_value) {
            Some(refused) => refusal_error(refused),
            None => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        };
    }
    if input.artifact.version != 1
        || input.artifact.entrypoint != "index.html"
        || !input.artifact.files.contains_key("index.html")
    {
        return error(StatusCode::BAD_REQUEST, "Invalid Build Artifact.");
    }
    if input.visibility != "public" && input.visibility != "private" {
        return error(
            StatusCode::BAD_REQUEST,
            "Visibility must be public or private.",
        );
    }
    let server = match functions::validate_artifact(input.server_artifact.as_ref().map(|a| &a.manifest), input.server_artifact.as_ref()) {
        Ok(server) => server,
        Err(refused) => return refusal_error(&refused),
    };
    let prepared = match functions::prepare_artifact(state, input.server_artifact.as_ref()).await {
        Ok(prepared) => prepared,
        Err(error_value) => {
            if let Some(refused) = crate::refusal::of(&error_value) {
                return refusal_error(refused);
            }
            return error(StatusCode::SERVICE_UNAVAILABLE, &format!("Function preparation failed: {error_value:#}"));
        }
    };
    let _generation = state.copies.generations.read().await;
    let _release_guard = if restoring_import { None } else { Some(named_lock(state, &format!("publish:{}", input.project.id), true).await) };
    let validation = if restoring_import { runtime::validate_import_release(state, &input, None).await }
        else { runtime::validate_release(state, &input, None).await };
    if let Err(error_value) = validation {
        return match refusal::of(&error_value) {
            Some(refused) => refusal_error(refused),
            None => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        };
    }
    let release_sources = match input.project.sources() {
        Ok(files) => files,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    let draft = input.project.clone();
    let draft_visibility = input.visibility.clone();
    let imported_snapshot = input.initial_released.clone();
    let input = match input.restored() {
        Ok(input) => input,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let actions = input.project.files.get("actions.json").cloned().unwrap_or_else(|| r#"{"version":1,"actions":{}}"#.to_owned());
    let program = match action_shape::read(&actions) {
        Ok(program) => program,
        Err(error_value) => match refusal::of(&error_value) {
            Some(refused) => return refusal_error(refused),
            None => return error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        },
    };
    for (name, action) in &program.actions {
        let Some(file) = action.page.as_ref().and_then(|page| page.get("file")).and_then(Value::as_str) else { continue };
        if !input.artifact.files.contains_key(file) {
            return refusal_error(&refusal::refuse("page.file-missing", format!("Action {name} names {file} as its page file, but the Build Artifact does not contain it.")));
        }
    }
    if !runtime::valid_app_id(&input.project.id) {
        return error(StatusCode::BAD_REQUEST, "Invalid project ID.");
    }
    if let Err(error_value) = http::read_secrets(&state.data_root, &input.project.id).and_then(|db| {
        calls::check_release(&domain::Rules::of(&state), &db, &program)
    }) {
        return match refusal::of(&error_value) {
            Some(refused) => refusal_error(refused),
            None => error(StatusCode::BAD_REQUEST, &error_value.to_string()),
        };
    }
    if !input.artifact.files.keys().all(|path| safe_relative_path(path)) {
        return error(
            StatusCode::BAD_REQUEST,
            "Artifact contains an invalid path.",
        );
    }
    if let Err(message) = artifact::check_routing(&input.project.manifest) {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    if let Err(refused) = calls::check_connects(&input.project.manifest) {
        return refusal_error(&refused);
    }
    let artifact_files = match artifact::decode(&input.artifact) {
        Ok(files) => files,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    let deployments = state.deployments.read().await;
    let existing = find_project_deployment(&deployments, &input.project.id);
    drop(deployments);
    if existing.is_some()
        && (input.initial_signup_policy.is_some()
            || input.initial_sign_in_methods.is_some()
            || input.initial_modules.is_some()
            || input.initial_package.is_some())
    {
        return error(
            StatusCode::CONFLICT,
            "Portable state can only be imported on the first deployment.",
        );
    }
    let package_policy = input.initial_package.as_ref().and_then(|package| package.manifest.signup_policy.as_ref());
    let initial_signup_policy = match input.initial_signup_policy.as_ref().or(package_policy) {
        Some(policy) => match SignupPolicy::from_input(policy.mode, &policy.allowlist) {
            Ok(policy) => Some(policy),
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };



    let package_methods = input.initial_package.as_ref().and_then(|package| package.manifest.sign_in_methods.as_ref());
    let initial_sign_in_methods = match input.initial_sign_in_methods.as_ref().or(package_methods) {
        Some(methods) => match sign_in_methods_from_input(methods) {
            Ok(methods) => Some(methods),
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };

    let id = input.project.id.clone();
    let release_directories = input.project.directories.clone();


    let (hostname, _taking) = match existing.as_ref() {
        Some(item) => (item.hostname.clone(), None),
        None => {
            let (hostname, taking) = folder::free_hostname(state, &input.project.slug, &input.project.id).await;
            (hostname, Some(taking))
        }
    };
    let url = existing
        .as_ref()
        .map(|item| item.url.clone())
        .unwrap_or_else(|| deployment_url(&state.public_origin, &hostname));
    let released_schema = input.project.files.get("schema.sql").cloned().unwrap_or_default();
    let released_transforms = input.project.files.get("schema-transforms.json").cloned();
    let mut manifest = input.project.manifest;
    let legacy_file_access = manifest["fileAccess"] == "app-users";
    manifest["visibility"] = Value::String(input.visibility.clone());
    let artifact_version = format!("v{}", Uuid::new_v4().simple());
    let deployment = Deployment {
        id: id.clone(),
        project_id: input.project.id,
        project_name: input.project.name,
        slug: input.project.slug.clone(),
        url,
        created_at: existing
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or_else(now_ms),
        released_at: now_ms(),
        source_digest: imported_snapshot.as_ref().and_then(|s| s.source_digest.clone()).or_else(|| Some(sources::digest(release_sources.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice()))))),
        manifest,
        hostname: hostname.clone(),
        actions,
        artifact_version: artifact_version.clone(),
        signup_policy: match existing.as_ref() {
            Some(item) => item.signup_policy.for_republish(is_private(item), &input.visibility),
            None => initial_signup_policy.unwrap_or_else(|| SignupPolicy::initial(&input.visibility)),
        },
        sign_in_methods: match existing.as_ref() {
            Some(item) => item.sign_in_methods.clone(),
            None => initial_sign_in_methods.unwrap_or_else(default_sign_in_methods),
        },
        modules: {
            let uses_files = modules::artifact_uses_files(artifact_files.values().map(Vec::as_slice))
                || modules::sources_use_files(input.project.files.values().map(String::as_bytes))
                || legacy_file_access;
            let uses_auth = modules::sources_use_auth(input.project.files.values().map(String::as_bytes))
                || modules::artifact_uses_auth(artifact_files.values().map(Vec::as_slice));
            let action_files = input.project.files.iter().map(|(name, text)| (name.as_str(), text.as_bytes())).collect::<Vec<_>>();
            let uses_actions = modules::named_sources_use_actions(action_files.iter().copied());
            let uses_http = modules::sources_use_http(action_files.iter().copied());
            let uses_calls = modules::sources_use_calls(action_files.iter().copied());
            let package_modules = input.initial_package.as_ref().map(|package| &package.manifest.modules);
            let initial_modules = input.initial_modules.as_ref().or(package_modules);
            let mut modules = existing.as_ref().map(|item| item.modules.clone()).unwrap_or_else(|| modules::initial_for_release(initial_modules, uses_files, uses_auth || input.visibility != "public", uses_http, uses_calls));
            if legacy_file_access {
                let files = modules.get_mut("files").expect("files is switchable");
                files["on"] = json!(true);
                files["fileAccess"] = json!("app-users");
            }
            if existing.is_none() && initial_modules.is_none_or(|state| !state.contains_key("actions")) && uses_actions {
                modules.get_mut("actions").expect("actions is switchable")["on"] = json!(true);
            }
            if existing.is_none() && initial_modules.is_none_or(|state| !state.contains_key("http")) && uses_http { modules.get_mut("http").expect("http is switchable")["on"] = json!(true); }
            if existing.is_none() && initial_modules.is_none_or(|state| !state.contains_key("calls")) && uses_calls { modules.get_mut("calls").expect("calls is switchable")["on"] = json!(true); }
            modules::complete(&mut modules);
            modules
        },
        contact_email: input.contact_email.clone(),
        released_schema,
        server,
    };
    let data = layout::data(&state.data_root, &id);
    if let Err(error_value) = tokio::fs::create_dir_all(&data).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }


    if let Err(error_value) = stage_local_artifact(&state, &id, &artifact_version, &artifact_files)
        .await
        .and_then(|()| release_step(&id, "stage_local_artifact"))
    {
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    if let Err(e) = functions::stage(&staged_server(state, &id, &artifact_version), input.server_artifact.as_ref()).await.and_then(|()| release_step(&id, "stage_server")) {
        remove_staged_dist(state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let subject = format!("{id} revision {artifact_version}");
    let journal_release = || copies::Pending::begin(&state.data_root, &subject, "installing the database, sources and release record");
    let initializes_stores = input.initial_package.is_some()
        || (modules::is_on(&deployment.modules, "actions") && !layout::database(&state.data_root, &id).exists())
        || (modules::is_on(&deployment.modules, "auth") && !layout::identity(&state.data_root, &id).exists());
    let preparing = if initializes_stores {
        match journal_release() {
            Ok(pending) => Some(pending),
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    } else { None };
    let mut notices = Vec::new();
    if let Some(package) = input.initial_package.as_ref() {
        if let Err(error_value) = package::install(&state, package, &id)
            .await
        {
            remove_staged_dist(&state, &id, &artifact_version).await;
            let _ = tokio::fs::remove_dir_all(&data).await;
            file_store::forget_deployment(state.files.store.as_ref(), &id).await;
            return error(StatusCode::BAD_REQUEST, &format!("Package import failed: {error_value:#}"));
        }

        match path::arrive(&state, &deployment, package.manifest.path.as_ref()).await {
            Ok(lines) => notices = lines,
            Err(error_value) => {
                remove_staged_dist(&state, &id, &artifact_version).await;
                let _ = tokio::fs::remove_dir_all(&data).await;
                return error(StatusCode::BAD_REQUEST, &format!("Package import failed: {error_value:#}"));
            }
        }
    }
    let db_path = layout::database(&state.data_root, &id);
    let schema = input
        .project
        .files
        .get("schema.sql")
        .cloned()
        .unwrap_or_default();
    let actions_enabled = modules::is_on(&deployment.modules, "actions");
    if actions_enabled && !db_path.exists() {
        if let Err(error_value) = initialize_database(&db_path) {
            remove_staged_dist(&state, &id, &artifact_version).await;
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Database initialization failed: {error_value}"),
            );
        }
    }
    let identity_path = layout::identity(&state.data_root, &id);
    let identity = if !modules::is_on(&deployment.modules, "auth") || identity_path.exists() {
        Ok(())
    } else {
        open_auth(&state, &deployment).map(drop).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))
    };
    if let Err(error_value) = identity.and_then(|()| if modules::is_on(&deployment.modules, "auth") { release_step(&id, "open_auth") } else { Ok(()) }) {
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }


    let database_guard = database_lock(&state, &id).await;
    let staged_database = layout::private(&state.data_root, &id).join(format!("release-{artifact_version}.sqlite"));
    let staged = if actions_enabled { stage_database(&db_path, &staged_database, &schema, &program, input.project.files.get("schema-transforms.json").map(String::as_str), input.schema_confirmation.as_deref()) } else { Ok(schema_change::Change::default()) }
        .and_then(|change| release_step(&id, "schema_change").map(|()| change));
    let change = match staged {
        Ok(change) => change,
        Err(error_value) => {
            remove_database_files(&staged_database);
            remove_staged_dist(&state, &id, &artifact_version).await;
            return match refusal::of(&error_value) {
                Some(refused) => refusal_error(refused),
                None => error(StatusCode::BAD_REQUEST, &format!("Schema update failed: {error_value}")),
            };
        }
    };

    let source_metadata = json!({
        "name": deployment.project_name, "slug": deployment.slug, "manifest": deployment.manifest,
        "directories": input.project.directories, "updatedAt": now_ms(),
    });
    let staged_sources = match sources::stage_release(&state.data_root, &id, &release_sources, &source_metadata).await {
        Ok(staged) => staged,
        Err(error_value) => {
            remove_database_files(&staged_database);
            remove_staged_dist(&state, &id, &artifact_version).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The sources of this release could not be written beside app/: {error_value:#}"));
        }
    };
    if let Err(error_value) = release_step(&id, "stage_sources") {
        staged_sources.discard().await;
        remove_database_files(&staged_database);
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }


    let recorded = Recorded::read(&state.data_root, &id).await;
    let replaced = layout::private(&state.data_root, &id).join(format!("release-{artifact_version}-replaced.sqlite"));
    let has_transformation_changes = change.transformations.as_ref().is_some_and(|items| !items.is_empty());
    let pending = match preparing {
        Some(pending) => pending,
        None => match journal_release() {
            Ok(pending) => pending,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    };
    let swapped = if !actions_enabled || change.statements.is_empty() && !has_transformation_changes {
        remove_database_files(&staged_database);
        false
    } else if let Err(error_value) = swap_database(&db_path, &staged_database, &replaced) {
        staged_sources.discard().await;
        remove_database_files(&staged_database);
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The changed database could not take the place of the live one: {error_value}"));
    } else {
        true
    };
    let undo_database = || {
        if swapped {
            if let Err(error_value) = restore_database(&db_path, &replaced) {
                eprintln!("Could not put back the database of {id} after a failed release: {error_value}");
            }
        }
    };
    if let Err(error_value) = async {
        persist_release(&state, &deployment, &folder::version_of(&artifact_files), &release_directories, released_transforms.as_deref(), Some(&deployment.modules)).await?;
        if imported_snapshot.is_some() {
            let mut settings = Settings::read(&state.data_root, &id)?;
            settings.name = draft.name.clone(); settings.slug = draft.slug.clone();
            settings.manifest = sources::manifest_without_runtime(&draft.manifest);
            settings.visibility = draft_visibility.clone();
            settings.write(&state.data_root).await?;
        }
        release_step(&id, "persist_deployment")
    }.await
    {
        recorded.put_back().await;
        undo_database();
        staged_sources.discard().await;
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }


    let swapped_sources = match staged_sources.swap().await {
        Err(error_value) => Err((error_value, false)),
        Ok(()) => release_step(&id, "swap_sources").map_err(|error_value| (error_value, true)),
    };
    if let Err((error_value, swapped_app)) = swapped_sources {
        if swapped_app { staged_sources.put_back().await; } else { staged_sources.discard().await; }
        recorded.put_back().await;
        undo_database();
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The new sources could not take the place of app/: {error_value:#}"));
    }
    if let Err(error_value) = switch_dist(&state, &id, &artifact_version).await {
        staged_sources.put_back().await;
        recorded.put_back().await;
        undo_database();
        remove_staged_dist(&state, &id, &artifact_version).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The new files could not take the place of dist/: {error_value}"));
    }
    state
        .deployments
        .write()
        .await
        .insert(hostname, deployment.clone());
    state.functions.retain_prepared(&id, prepared);
    drop(database_guard);
    if swapped {
        remove_database_files(&replaced);
    }
    staged_sources.finish().await;
    if let Err(e) = pending.finish() { return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()); }
    http::routes_published(&deployment);
    state.functions.release_changed(&id, &deployment.artifact_version).await;
    remove_stale_dist(&state, &id).await;
    let mut result = match serde_json::to_value(&deployment) {
        Ok(value) => value,
        Err(error_value) => return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
    };
    if !notices.is_empty() {
        result["notices"] = json!(notices);
    }
    Json(json!({"result": result})).into_response()
}

pub async fn stage_local_artifact(
    state: &AppState,
    id: &str,
    version: &str,
    files: &HashMap<String, Vec<u8>>,
) -> Result<()> {
    let root = staged_dist(state, id, version);
    for (path, contents) in files {
        let local_path = root.join(path);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&local_path, contents).await?;
    }
    Ok(())
}

/// Where a release's files wait until they become `dist/`.
pub fn staged_dist(state: &AppState, id: &str, version: &str) -> PathBuf {
    layout::private(&state.data_root, id).join(format!("dist-{version}"))
}

/// Moves the staged files into `dist/` and the previous ones aside, for
/// `remove_stale_dist` to delete. When the second move fails, the
/// previous files go back to `dist/`.
pub fn staged_server(state: &AppState, id: &str, version: &str) -> PathBuf {
    layout::private(&state.data_root, id).join(format!("server-{version}"))
}

pub async fn switch_dist(state: &AppState, id: &str, version: &str) -> std::io::Result<()> {
    let mut changed: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    for (live, staged, label, step) in [
        (layout::dist(&state.data_root, id), staged_dist(state, id, version), "dist", "switch_dist"),
        (layout::server(&state.data_root, id), staged_server(state, id, version), "server", "switch_server"),
    ] {
        // Local static-only callers do not stage a server directory.
        if label == "server" && !staged.exists() { continue; }
        let retired = layout::private(&state.data_root, id).join(format!("{label}-retired-{}", Uuid::new_v4().simple()));
        let moved = async {
            let had_live = tokio::fs::try_exists(&live).await?;
            if had_live { tokio::fs::rename(&live, &retired).await?; }
            changed.push((live.clone(), retired.clone(), had_live));
            release_step(id, step).map_err(|e| std::io::Error::other(e.to_string()))?;
            tokio::fs::rename(&staged, &live).await
        }.await;
        if let Err(error) = moved {
            for (live, retired, existed) in changed.into_iter().rev() {
                let _ = tokio::fs::remove_dir_all(&live).await;
                if existed { tokio::fs::rename(retired, live).await?; }
            }
            return Err(error);
        }
    }
    Ok(())
}



fn stage_database(live: &Path, staged: &Path, schema: &str, program: &ActionProgram, transforms: Option<&str>, confirmation: Option<&str>) -> Result<schema_change::Change> {
    remove_database_files(staged);
    copies::vacuum_into(live, staged)?;
    let change = schema_change::apply_with_transforms(staged, schema, program, transforms, schema_change::Mode::Apply(confirmation))?;


    Connection::open(staged)?.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    Ok(change)
}

fn database_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_database_files(path: &Path) {
    for file in [path.to_path_buf(), database_sidecar(path, "-wal"), database_sidecar(path, "-shm")] {
        let _ = std::fs::remove_file(file);
    }
}



fn swap_database(live: &Path, staged: &Path, replaced: &Path) -> std::io::Result<()> {
    swap_database_with(live, staged, replaced, || {})
}

pub(crate) fn swap_database_with(live: &Path, staged: &Path, replaced: &Path, between_renames: impl FnOnce()) -> std::io::Result<()> {
    std::fs::rename(live, replaced)?;
    for suffix in ["-wal", "-shm"] {
        let from = database_sidecar(live, suffix);
        if from.exists() {
            if let Err(error_value) = std::fs::rename(&from, database_sidecar(replaced, suffix)) {
                let _ = restore_database(live, replaced);
                return Err(error_value);
            }
        }
    }
    between_renames();
    if let Err(error_value) = std::fs::rename(staged, live) {
        let _ = restore_database(live, replaced);
        return Err(error_value);
    }
    Ok(())
}


fn restore_database(live: &Path, replaced: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        let from = database_sidecar(replaced, suffix);
        if from.exists() {
            std::fs::rename(&from, database_sidecar(live, suffix))?;
        } else {
            let _ = std::fs::remove_file(database_sidecar(live, suffix));
        }
    }
    std::fs::rename(replaced, live)
}

#[cfg(test)]
pub(crate) static FAILING_RELEASE_STEPS: std::sync::Mutex<Vec<(String, &'static str)>> = std::sync::Mutex::new(Vec::new());

/// Fails a release at `step` when a test asked for it.
#[cfg(test)]
fn release_step(id: &str, step: &'static str) -> Result<()> {
    if FAILING_RELEASE_STEPS.lock().unwrap().iter().any(|(app, failing)| app == id && *failing == step) {
        anyhow::bail!("The test failed this release at {step}.");
    }
    Ok(())
}

#[cfg(not(test))]
fn release_step(_id: &str, _step: &'static str) -> Result<()> {
    Ok(())
}

/// Best-effort removal of a release that never went live.
pub async fn remove_staged_dist(state: &AppState, id: &str, version: &str) {
    let _ = tokio::fs::remove_dir_all(staged_dist(state, id, version)).await;
    let _ = tokio::fs::remove_dir_all(staged_server(state, id, version)).await;
}

/// Deletes the retired or staged release folders beside `dist/`. Failures
/// are logged because the new release is already live.
pub async fn remove_stale_dist(state: &AppState, id: &str) {
    if let Ok(mut entries) = tokio::fs::read_dir(layout::private(&state.data_root, id)).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.file_name().to_str().is_some_and(|name| name.starts_with("dist-") || name.starts_with("server-")) {
                continue;
            }
            let removed = match entry.file_type().await {
                Ok(kind) if kind.is_dir() => tokio::fs::remove_dir_all(entry.path()).await,
                _ => tokio::fs::remove_file(entry.path()).await,
            };
            if let Err(error_value) = removed {
                eprintln!("Could not remove stale artifact {}: {error_value}", entry.path().display());
            }
        }
    }
}
