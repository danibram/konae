use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState, DeployInput, SignupPolicy, database_lock, database_path,
    find_project_deployment, initialize_database, safe_relative_path,
};

/// Plans a release against the live database, a database the caller supplies,
/// or the initial package or empty database for a first release.
pub async fn validate_release(state: &AppState, input: &DeployInput, against: Option<&Path>) -> Result<crate::schema_change::Change> {
    if let Some(snapshot) = &input.initial_released {
        anyhow::ensure!(!state.deployments.read().await.values().any(|d| d.id == input.project.id), "A published snapshot can only enter an existing app through project_import.");
        let package = input.initial_package.as_ref().context("A published snapshot requires an initial package.")?;
        anyhow::ensure!(package.manifest.released.as_ref().is_some_and(|published| serde_json::to_value(published).ok() == serde_json::to_value(snapshot).ok()), "Published snapshot does not match the package.");
    }
    validate_import_release(state, input, against).await
}

/// The owner import door has already checked permission and package integrity.
pub async fn validate_import_release(state: &AppState, input: &DeployInput, against: Option<&Path>) -> Result<crate::schema_change::Change> {
    crate::sources::check_files(&input.project.sources().map_err(anyhow::Error::msg)?)?;
    let restored = input.restored()?;
    validate_release_inner(state, &restored, against, input.initial_released.is_some()).await
}

pub(crate) async fn validate_published_release(state: &AppState, input: &DeployInput, against: Option<&Path>) -> Result<crate::schema_change::Change> {
    validate_release_inner(state, input, against, true).await
}

async fn validate_release_inner(state: &AppState, input: &DeployInput, against: Option<&Path>, published: bool) -> Result<crate::schema_change::Change> {
    if let Some(initial_modules) = input.initial_modules.as_ref() {
        crate::modules::validate_state(initial_modules).map_err(anyhow::Error::new)?;
    }
    if let Some(initial_package) = input.initial_package.as_ref() {
        crate::modules::validate_state(&initial_package.manifest.modules).map_err(anyhow::Error::new)?;
    }
    anyhow::ensure!(
        input.artifact.version == 1
            && input.artifact.entrypoint == "index.html"
            && input.artifact.files.contains_key("index.html"),
        "Invalid Build Artifact."
    );
    anyhow::ensure!(
        ["public", "private"].contains(&input.visibility.as_str()),
        "Invalid visibility."
    );
    anyhow::ensure!(
        input.artifact.files.len() <= 1000,
        "Artifact has too many files."
    );
    for path in input.artifact.files.keys() {
        anyhow::ensure!(
            safe_relative_path(path)
                && !path.starts_with("_konae/")
                && !path.starts_with("oauth/")
                && !path.starts_with(".well-known/")
                && path != "mcp"
                && path != "server" && !path.starts_with("server/")
                && path != "functions" && !path.starts_with("functions/"),
            "Invalid or reserved artifact path."
        );
    }
    let artifact = crate::artifact::decode(&input.artifact).map_err(anyhow::Error::msg)?;
    let uses_files = crate::modules::artifact_uses_files(artifact.values().map(Vec::as_slice))
        || crate::modules::sources_use_files(input.project.files.values().map(String::as_bytes))
        || input.project.manifest["fileAccess"] == "app-users";
    let uses_auth = crate::modules::sources_use_auth(input.project.files.values().map(String::as_bytes))
        || crate::modules::artifact_uses_auth(artifact.values().map(Vec::as_slice));
    let action_files = input.project.files.iter().map(|(name, text)| (name.as_str(), text.as_bytes())).collect::<Vec<_>>();
    let uses_actions = crate::modules::named_sources_use_actions(action_files.iter().copied());
    let uses_http = crate::modules::sources_use_http(action_files.iter().copied());
    let uses_calls = crate::modules::sources_use_calls(action_files.iter().copied());
    crate::artifact::check_routing(&input.project.manifest).map_err(anyhow::Error::msg)?;
    crate::calls::check_connects(&input.project.manifest)?;
    let artifact = crate::artifact::decode(&input.artifact).map_err(anyhow::Error::msg)?;
    let sizes = artifact.iter().map(|(path, bytes)| (path.as_str(), bytes.len()));
    anyhow::ensure!(sizes.clone().map(|(_, bytes)| bytes).sum::<usize>() <= crate::size::ARTIFACT_BYTES, "{}", crate::size::artifact_refused(sizes));
    drop(artifact);
    crate::sources::check_files(&input.project.sources().map_err(anyhow::Error::msg)?)?;
    let empty_actions = r#"{"version":1,"actions":{}}"#;
    let actions_source = input.project.files.get("actions.json").map(String::as_str).unwrap_or(empty_actions);
    let package_modules = input.initial_package.as_ref().map(|package| &package.manifest.modules);
    let initial_modules = input.initial_modules.as_ref().or(package_modules);
    let mut candidate_modules = state.deployments.read().await.values().find(|deployment| deployment.id == input.project.id).map(|deployment| deployment.modules.clone()).unwrap_or_else(|| crate::modules::initial_for_release(initial_modules, uses_files, uses_auth || input.visibility != "public", uses_http, uses_calls));
    if input.project.manifest["fileAccess"] == "app-users" {
        let files = candidate_modules.get_mut("files").expect("files is switchable");
        files["on"] = serde_json::json!(true);
        files["fileAccess"] = serde_json::json!("app-users");
    }
    if state.deployments.read().await.values().all(|deployment| deployment.id != input.project.id)
        && initial_modules.is_none_or(|modules| !modules.contains_key("actions")) && uses_actions {
        candidate_modules.get_mut("actions").expect("actions is switchable")["on"] = json!(true);
    }
    for module in crate::modules::release_use_order() {
        let used = match module.as_str() { "files" => uses_files, "actions" => uses_actions, "http" => uses_http, "calls" => uses_calls, _ => false };
        if used { crate::modules::require_on(&candidate_modules, &module).map_err(anyhow::Error::new)?; }
    }
    let program = crate::action_shape::read(actions_source)?;
    let existing_modules = state.deployments.read().await.values().find(|deployment| deployment.id == input.project.id).map(|deployment| deployment.modules.clone());
    candidate_modules = existing_modules.unwrap_or(candidate_modules);
    if state.deployments.read().await.values().all(|deployment| deployment.id != input.project.id)
        && input.initial_modules.as_ref().is_none_or(|modules| !modules.contains_key("auth"))
        && (input.visibility != "public" || input.initial_signup_policy.is_some() || uses_auth) {
        candidate_modules.get_mut("auth").expect("auth is switchable")["on"] = json!(true);
    }
    if input.visibility != "public" && !crate::modules::is_on(&candidate_modules, "auth") {
        return Err(anyhow::Error::new(crate::modules::check_private_auth(false).unwrap_err()));
    }
    crate::modules::check_requirements(&candidate_modules).map_err(anyhow::Error::new)?;
    if uses_auth && !crate::modules::is_on(&candidate_modules, "auth") {
        return Err(anyhow::Error::new(crate::modules::require_on(&candidate_modules, "auth").unwrap_err()));
    }
    if uses_actions { crate::modules::require_on(&candidate_modules, "actions").map_err(anyhow::Error::new)?; }
    let functions = input.project.files.get("functions.json").map(|source| crate::functions::read(source)).transpose()?;
    if let Some(manifest) = &functions {
        let files = input.project.files.keys().cloned().collect();
        let actions: Value = serde_json::from_str(actions_source)?;
        let paths = input.artifact.files.keys().cloned().collect::<Vec<_>>();
        if published { crate::functions::validate_published(manifest, &candidate_modules, &actions["actions"], &paths)?; }
        else { crate::functions::validate(manifest, &files, &candidate_modules, &actions["actions"], &paths)?; }
    }
    crate::functions::validate_artifact(functions.as_ref(), input.server_artifact.as_ref())?;
    crate::calls::check_release(&crate::domain::Rules::of(state), &crate::http::read_secrets(&state.data_root, &input.project.id)?, &program)?;
    let _guard = database_lock(state, &format!("release:{}", input.project.id)).await;
    let existing = find_project_deployment(&*state.deployments.read().await, &input.project.id);
    anyhow::ensure!(
        existing.is_none() || (input.initial_signup_policy.is_none()
            && input.initial_package.is_none()),
        "Portable state can only be imported on the first deployment."
    );
    if let Some(policy) = input.initial_signup_policy.as_ref() {
        SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?;
    }
    let temp = state
        .data_root
        .join(format!("validate-{}.sqlite", Uuid::new_v4()));
    let result = (|| -> Result<crate::schema_change::Change> {
        if !crate::modules::is_on(&candidate_modules, "actions") { return Ok(crate::schema_change::Change::default()); }
        std::fs::create_dir_all(&state.data_root)?;
        if let Some(database) = against {
            std::fs::copy(database, &temp)?;
        } else if let Some(ref deployment) = existing {
            let live = database_path(state, &deployment.id);
            if live.try_exists()? {
                crate::copies::vacuum_into(&live, &temp)?;
            } else {
                initialize_database(&temp)?;
            }
        } else if let Some(package) = input.initial_package.as_ref() {
            crate::package::copy_database(state, package, &temp)?;
        } else {
            initialize_database(&temp)?;
        }
        crate::schema_change::apply_with_transforms(
            &temp,
            input
                .project
                .files
                .get("schema.sql")
                .map(String::as_str)
                .unwrap_or(""),
            &program,
            input.project.files.get("schema-transforms.json").map(String::as_str),
            crate::schema_change::Mode::Plan,
        )
    })();
    let _ = tokio::fs::remove_file(temp).await;
    result
}
