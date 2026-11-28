use super::*;

pub async fn serve_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "files") { return refusal_error(&refused); }
    {
        let _guard = database_lock(&state, &deployment.id).await;
        if let Err(failure) = crate::files::reconcile_ftps_uploads(&state, &deployment).await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &failure.to_string());
        }
    }
    let actor = match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let private_deployment = is_private(&deployment);
    if private_deployment && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let actor_id = actor.as_ref().map(|user| user.user_id.as_str());
    let Some(metadata) = find_file_metadata(&state, &deployment.id, &id).await else {
        return error(StatusCode::NOT_FOUND, "File not found.");
    };
    if !file_readable(&deployment, &metadata, actor_id) {
        return error(StatusCode::NOT_FOUND, "File not found.");
    }
    file_response(&state, &deployment.id, &metadata, &headers).await
}

/// The one byte range a `Range` header asks for, as `start..end`. `None`
/// for no header or one this path does not serve, which then gets the whole
/// file; `Err` for a range that starts past the end.
pub fn requested_range(headers: &HeaderMap, size: u64) -> Result<Option<std::ops::Range<u64>>, ()> {
    let Some(value) = headers.get(header::RANGE).and_then(|value| value.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((first, last)) = spec.split_once('-') else { return Ok(None) };
    let (first, last) = (first.trim(), last.trim());
    let range = match (first.parse::<u64>().ok(), last.parse::<u64>().ok()) {
        (Some(start), _) if start >= size => return Err(()),
        (Some(start), Some(end)) if end >= start => start..end.saturating_add(1).min(size),
        (Some(start), None) if last.is_empty() => start..size,
        (None, Some(suffix)) if first.is_empty() && suffix > 0 => size.saturating_sub(suffix)..size,
        _ => return Ok(None),
    };
    Ok(Some(range))
}

/// A stored file's bytes with its own type, streamed from the store. Raster
/// images display inline; everything else downloads, and nothing in it may
/// run. A single `Range` gets 206 with that part.
pub async fn file_response(state: &AppState, deployment: &str, metadata: &Value, headers: &HeaderMap) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    let Some(id) = metadata["id"].as_str() else { return not_found() };
    let size = metadata["size"].as_u64().unwrap_or(0);


    let Ok(etag) = HeaderValue::from_str(&format!("\"{id}-{size}\"")) else { return not_found() };
    if if_none_match(headers, &etag) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(header::ETAG, etag);
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(FILE_CACHE_CONTROL));
        return response;
    }
    let range = match requested_range(headers, size) {
        Ok(range) => range,
        Err(()) => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return response;
        }
    };
    let blob = match state.files.store.get(deployment, id, range.clone()).await {
        Ok(Some(blob)) => blob,
        Ok(None) => return not_found(),
        Err(e) => {
            eprintln!("konae: could not read file {id}: {e:#}");
            return not_found();
        }
    };
    let content_type = metadata["type"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let inline = [
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/avif",
    ]
    .contains(&content_type);
    let length = blob.length;
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::with_capacity(blob.reader, 64 * 1024)));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(range) = range {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        if let Ok(value) = HeaderValue::from_str(&format!("bytes {}-{}/{size}", range.start, range.end.saturating_sub(1))) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(files::served_type(content_type))
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if inline { "inline" } else { "attachment" }),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    headers.insert(header::ETAG, etag);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(FILE_CACHE_CONTROL));
    response
}

/// A browser may keep a file's bytes but asks the Kernel before each use,
/// so a read still passes the caller's check and a deleted file or a lost
/// right answers 404 at once. No shared cache keeps them.
pub const FILE_CACHE_CONTROL: &str = "private, no-cache";


fn if_none_match(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(value) = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok()) else { return false };
    let Ok(etag) = etag.to_str() else { return false };
    value.split(',').map(str::trim).any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag)
}


/// The `Content-Type` the file read path sends for a stored type.
///
/// The uploader chooses the stored type. `Content-Disposition: attachment` and
/// the file's own `sandbox` policy stop a file that is opened as a page, but a
/// page on the app's origin may still load the file as a `<script>`, a module
/// or a worker, and the app policy's `script-src 'self'` allows the path. A
/// browser runs that only when the type is a JavaScript type, so a JavaScript
/// type, and the HTML, XML and SVG types that can carry script, are sent as
/// `application/octet-stream`. The stored type is kept, and `files_list` still
/// reports it.
pub fn served_type(stored: &str) -> &str {
    let essence = stored.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let scriptable = ["javascript", "ecmascript", "jscript", "livescript", "html", "xml", "svg"]
        .iter()
        .any(|word| essence.contains(word));
    if scriptable || essence.is_empty() { "application/octet-stream" } else { stored }
}
