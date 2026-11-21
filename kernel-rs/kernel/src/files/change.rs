use super::*;

pub const NOT_UPLOADER: &str = "Only the person who uploaded this file may change or delete it.";
pub const FIXED_VISIBILITY: &str = "File visibility is fixed to app-users by this app.";
pub(super) const BAD_VISIBILITY: &str = "File visibility must be private or public.";

enum Change<'a> {
    Visibility(&'a str),
    Delete,
}

/// Sets a file's visibility to `private` or `public` and returns the file as
/// `files_list` shows it to the uploader.
pub async fn update_file(state: &AppState, headers: &HeaderMap, id: &str, visibility: Option<&str>) -> Response {
    match visibility {
        Some(value @ ("private" | "public" | "app-users")) => change(state, headers, id, Change::Visibility(value)).await,
        _ => error(StatusCode::BAD_REQUEST, BAD_VISIBILITY),
    }
}

/// Removes a file's entry and its bytes. Its size stops counting against the
/// quota, and a later read of its ID answers as for a file that never existed.
pub async fn delete_file(state: &AppState, headers: &HeaderMap, id: &str) -> Response {
    change(state, headers, id, Change::Delete).await
}

async fn change(state: &AppState, headers: &HeaderMap, id: &str, change: Change<'_>) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "files") { return refusal_error(&refused); }
    let actor = match authenticate_deployment(state, &deployment, headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "authentication-required"),
        Err(response) => return response,
    };
    if !valid_file_id(id) {
        return not_found();
    }
    let _guard = database_lock(state, &deployment.id).await;
    let directory = crate::layout::files(&state.data_root, &deployment.id);
    let mut files = match read_file_index(&directory).await {
        Ok(files) => files,
        Err(response) => return response,
    };
    let actor_id = actor.user_id.as_str();
    let Some(position) = files.iter().position(|file| file["id"].as_str() == Some(id)) else {
        return not_found();
    };
    if !file_readable(&deployment, &files[position], Some(actor_id)) {
        return not_found();
    }
    if file_owner(&files[position]) != Some(actor_id) {
        return error(StatusCode::FORBIDDEN, NOT_UPLOADER);
    }
    if let Change::Visibility(visibility) = change {
        if app_user_files(&deployment) {
            if visibility != "app-users" {
                return error(StatusCode::BAD_REQUEST, FIXED_VISIBILITY);
            }
            return Json(json!({"result": file_view(&deployment, &files[position], Some(actor_id))})).into_response();
        }
        if visibility == "app-users" {
            return error(StatusCode::BAD_REQUEST, BAD_VISIBILITY);
        }
    }
    let result = match change {
        Change::Visibility(visibility) => {
            files[position]["visibility"] = json!(visibility);
            file_view(&deployment, &files[position], Some(actor_id))
        }
        Change::Delete => {
            files.remove(position);
            json!({"id": id, "deleted": true})
        }
    };
    let index = match serde_json::to_vec_pretty(&files) {
        Ok(bytes) => bytes,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = write_atomic(&directory.join("index.json"), &index).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(_guard);
    // The index no longer names the file, so no reader can reach the bytes
    // even if removing them fails; `files-reconcile` removes what is left.
    if matches!(change, Change::Delete) {
        if let Err(e) = state.files.store.delete(&deployment.id, id).await {
            eprintln!("konae: could not remove the bytes of deleted file {id}: {e:#}");
        }
    }
    Json(json!({"result": result})).into_response()
}
