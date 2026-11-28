use super::*;

pub const MAX_FILES: usize = 10_000;
pub const TOO_MANY_FILES: &str = "This deployment has reached its limit of 10,000 files. Delete files before uploading more.";

pub async fn list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "files") { return refusal_error(&refused); }
    let actor = match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_private(&deployment) && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    let directory = crate::layout::files(&state.data_root, &deployment.id);
    let files = match read_file_index(&directory).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let visible = files
        .iter()
        .filter(|file| file_readable(&deployment, file, actor_id))
        .map(|file| file_view(&deployment, file, actor_id))
        .collect::<Vec<_>>();
    Json(json!({"result": visible})).into_response()
}

/// Finds indexed metadata for one ID. Read routes treat a missing or invalid
/// index like a missing file.
pub async fn find_file_metadata(state: &AppState, deployment: &str, id: &str) -> Option<Value> {
    if !valid_file_id(id) {
        return None;
    }
    let path = crate::layout::files(&state.data_root, deployment).join("index.json");
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice::<Vec<Value>>(&bytes)
        .ok()?
        .into_iter()
        .find(|file| file["id"].as_str() == Some(id))
}

/// Reads the index under the Deployment's lock. A missing index is an empty one.
pub(super) async fn read_file_index(directory: &Path) -> Result<Vec<Value>, Response> {
    match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes)
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid.")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
    }
}

/// The refusal when one more file of `size` bytes would not fit.
pub(super) async fn file_room(state: &AppState, app: &str, files: &[Value], size: u64) -> Option<Response> {
    let root = state.data_root.clone();
    let app = app.to_owned();
    let inventory = tokio::task::spawn_blocking(move || crate::modules::blob_references(&root, &app)).await;
    let used = match inventory {
        Ok(Ok(references)) => references.iter().fold(0u64, |total, reference| total.saturating_add(reference.size)),
        Ok(Err(failure)) => return Some(error(StatusCode::INTERNAL_SERVER_ERROR, &failure.to_string())),
        Err(failure) => return Some(error(StatusCode::INTERNAL_SERVER_ERROR, &failure.to_string())),
    };
    if used.saturating_add(size) > state.files.limits.quota_bytes {
        return Some(error(StatusCode::PAYLOAD_TOO_LARGE, &state.files.limits.quota_reached()));
    }
    if files.len() >= MAX_FILES {
        return Some(error(StatusCode::PAYLOAD_TOO_LARGE, TOO_MANY_FILES));
    }
    None
}
