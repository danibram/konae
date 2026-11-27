use super::*;

/// The JSON upload, `{ name, type, dataUrl, visibility }`, that tests use.
/// Runtime MCP uploads arrive as transfers and go to [`upload_from_path`].
#[cfg(any(test, feature = "testing"))]
pub async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "files") { return refusal_error(&refused); }
    if let Err(response) = upload_actor(&state, &deployment, &headers).await {
        return response;
    }
    let limits = state.files.limits;
    // Base64 takes four bytes for every three, and the JSON around it a little more.
    let body_limit = (limits.max_file_bytes / 3 * 4 + 64 * 1024).min(usize::MAX as u64) as usize;
    let bytes = match axum::body::to_bytes(request.into_body(), body_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("File upload exceeds the {} limit.", limits.max_file()),
            );
        }
    };
    let input: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "File upload must be JSON with name, type, and dataUrl.",
            );
        }
    };
    let data = match input
        .get("dataUrl")
        .and_then(Value::as_str)
        .map(decode_base64)
        .transpose()
    {
        Ok(Some(value)) => value,
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "File upload dataUrl must contain base64 data.",
            );
        }
    };
    let scratch = state.data_root.join("transfers");
    let source = scratch.join(format!(".upload-{}", Uuid::new_v4()));
    let written = async {
        tokio::fs::create_dir_all(&scratch).await?;
        tokio::fs::write(&source, &data).await
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&source).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let upload = Upload {
        name: input.get("name").and_then(Value::as_str),
        content_type: input.get("type").and_then(Value::as_str),
        visibility: input.get("visibility").and_then(Value::as_str),
    };
    let response = upload_from_path(&state, &headers, upload, &source).await;
    let _ = tokio::fs::remove_file(&source).await;
    response
}

/// What the uploader says about a file besides its bytes.
pub struct Upload<'a> {
    pub name: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub visibility: Option<&'a str>,
}

pub async fn upload_actor(state: &AppState, deployment: &Deployment, headers: &HeaderMap) -> Result<AuthUser, Response> {
    match authenticate_deployment(state, deployment, headers).await {
        Ok(Some(actor)) => Ok(actor),
        Ok(None) => Err(error(StatusCode::UNAUTHORIZED, "authentication-required")),
        Err(response) => Err(response),
    }
}

/// Stores the bytes of `source` as a new file of the signed-in user. The
/// bytes go to the store before the index names them, outside the
/// Deployment's lock, so a slow store holds up no action; the quota is
/// checked before and again when the index changes.
pub async fn upload_from_path(state: &AppState, headers: &HeaderMap, upload: Upload<'_>, source: &Path) -> Response {
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "files") { return refusal_error(&refused); }
    let actor = match upload_actor(state, &deployment, headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let visibility = match (app_user_files(&deployment), upload.visibility) {
        // The manifest grants shared reads. If it is removed, new files revert to uploader-only.
        (true, None | Some("app-users")) => "private",
        (true, Some("private" | "public")) => return error(StatusCode::BAD_REQUEST, files::FIXED_VISIBILITY),
        (false, None | Some("private")) => "private",
        (false, Some("public")) => "public",
        (_, Some(_)) => {
            return error(
                StatusCode::BAD_REQUEST,
                "File visibility must be private or public.",
            );
        }
    };
    let name = upload
        .name
        .unwrap_or("file")
        .replace(['/', '\\'], "-")
        .chars()
        .filter(|character| !character.is_control())
        .take(180)
        .collect::<String>();
    let content_type = upload
        .content_type
        .filter(|v| {
            v.len() < 128
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-/".contains(&c))
        })
        .unwrap_or("application/octet-stream")
        .to_owned();
    let size = match tokio::fs::metadata(source).await {
        Ok(metadata) => metadata.len(),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if size == 0 || size > state.files.limits.max_file_bytes {
        return error(StatusCode::PAYLOAD_TOO_LARGE, &state.files.limits.file_size_refused());
    }
    let directory = crate::layout::files(&state.data_root, &deployment.id);
    {
        let _guard = database_lock(state, &deployment.id).await;
        match read_file_index(&directory).await {
            Ok(files) => {
                if let Some(refused) = file_room(state, &deployment.id, &files, size).await {
                    return refused;
                }
            }
            Err(response) => return response,
        }
    }
    let path = source.to_owned();
    let sha256 = match tokio::task::spawn_blocking(move || copies::checksum_file(&path)).await {
        Ok(Ok((length, digest))) if length == size => digest,
        Ok(Ok(_)) => return error(StatusCode::INTERNAL_SERVER_ERROR, "The upload changed size before storage."),
        Ok(Err(e)) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let id = Uuid::new_v4().to_string();
    if let Err(e) = state.files.store.put(&deployment.id, &id, source).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("The file could not be stored: {e:#}"));
    }
    let metadata = json!({
        "id": id,
        "name": name,
        "type": content_type,
        "size": size,
        "sha256": sha256,
        "visibility": visibility,
        "ownerId": actor.user_id,
        "createdAt": now_ms(),
    });
    let indexed = async {
        let _guard = database_lock(state, &deployment.id).await;
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        let mut files = read_file_index(&directory).await?;
        if let Some(refused) = file_room(state, &deployment.id, &files, size).await {
            return Err(refused);
        }
        files.insert(0, metadata.clone());
        let index = serde_json::to_vec_pretty(&files)
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        write_atomic(&directory.join("index.json"), &index)
            .await
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
    }
    .await;
    match indexed {
        Ok(()) => Json(json!({"result": file_view(&deployment, &metadata, Some(&actor.user_id))})).into_response(),
        Err(response) => {
            // No index names the bytes, so nothing can read them.
            if let Err(e) = state.files.store.delete(&deployment.id, &id).await {
                eprintln!("konae: could not remove the bytes of refused file {id}: {e:#}");
            }
            response
        }
    }
}
