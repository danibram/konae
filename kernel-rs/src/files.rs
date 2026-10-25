//! What happens to an app file after its upload: the uploader changes its
//! visibility or deletes it. The runtime tools `files_update` and
//! `files_delete` in mcp.rs call these handlers; uploads, listing and reads
//! stay in main.rs and http.rs.
//!
//! Who may do what:
//!
//! | Caller                                   | Answer                                      |
//! | ---------------------------------------- | ------------------------------------------- |
//! | no session                               | 401 `authentication-required`               |
//! | a user who may not read the file         | 404 `File not found.`, as for a missing one |
//! | a user who reads it but did not upload it| 403 [`NOT_UPLOADER`]                        |
//! | the uploader                             | the change                                  |
//!
//! A file stored before files had owners has no uploader, so nobody may change
//! or delete it from the app; the owner's package export still carries it.
//! Both handlers hold the Deployment's database lock, the same one
//! `upload_file` takes, so a delete and an upload never race over the index or
//! the file quota, which is the sum of the sizes the index lists. The bytes
//! themselves go through the installation's `FileStore`, after the index
//! change commits.

use super::*;

/// Files one Deployment may hold. A name keeps at most 180 characters and no
/// control characters, so an index entry stays near 1 KiB and the whole
/// index stays under the 16 MiB a package's file index may take.
pub(super) const MAX_FILES: usize = 10_000;
pub(super) const TOO_MANY_FILES: &str = "This deployment has reached its limit of 10,000 files. Delete files before uploading more.";
pub(super) const NOT_UPLOADER: &str = "Only the person who uploaded this file may change or delete it.";
const BAD_VISIBILITY: &str = "File visibility must be private or public.";

enum Change<'a> {
    Visibility(&'a str),
    Delete,
}

/// Sets a file's visibility to `private` or `public` and returns the file as
/// `files_list` shows it to the uploader.
pub(super) async fn update_file(state: &AppState, headers: &HeaderMap, id: &str, visibility: Option<&str>) -> Response {
    match visibility {
        Some(value @ ("private" | "public")) => change(state, headers, id, Change::Visibility(value)).await,
        _ => error(StatusCode::BAD_REQUEST, BAD_VISIBILITY),
    }
}

/// Removes a file's entry and its bytes. Its size stops counting against the
/// quota, and a later read of its ID answers as for a file that never existed.
pub(super) async fn delete_file(state: &AppState, headers: &HeaderMap, id: &str) -> Response {
    change(state, headers, id, Change::Delete).await
}

async fn change(state: &AppState, headers: &HeaderMap, id: &str, change: Change<'_>) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "File not found.");
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let actor = match authenticate_deployment(state, &deployment, headers) {
        Ok(Some(actor)) => actor,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "authentication-required"),
        Err(response) => return response,
    };
    if !valid_file_id(id) {
        return not_found();
    }
    let _guard = database_lock(state, &deployment.id).await;
    let directory = deployment_dir(state, &deployment.id).join("files");
    let mut files = match tokio::fs::read(directory.join("index.json")).await {
        Ok(bytes) => match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(files) => files,
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "File index is invalid."),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return not_found(),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let actor_id = actor.user_id.as_str();
    let Some(position) = files.iter().position(|file| file["id"].as_str() == Some(id)) else {
        return not_found();
    };
    if !file_readable(&files[position], Some(actor_id), is_private(&deployment)) {
        return not_found();
    }
    if file_owner(&files[position]) != Some(actor_id) {
        return error(StatusCode::FORBIDDEN, NOT_UPLOADER);
    }
    let result = match change {
        Change::Visibility(visibility) => {
            files[position]["visibility"] = json!(visibility);
            file_view(&files[position], Some(actor_id))
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
            eprintln!("littlebox: could not remove the bytes of deleted file {id}: {e:#}");
        }
    }
    Json(json!({"result": result})).into_response()
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
pub(super) fn served_type(stored: &str) -> &str {
    let essence = stored.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let scriptable = ["javascript", "ecmascript", "jscript", "livescript", "html", "xml", "svg"]
        .iter()
        .any(|word| essence.contains(word));
    if scriptable || essence.is_empty() { "application/octet-stream" } else { stored }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{login_test_state, response_json, signed_in};

    #[test]
    fn a_type_that_can_run_script_is_served_as_bytes() {
        for stored in ["text/javascript", "TEXT/JavaScript", "application/javascript", "application/x-javascript",
            "text/ecmascript", "text/jscript", "text/livescript", "text/html", "application/xhtml+xml",
            "image/svg+xml", "text/xml", "application/xml", ""] {
            assert_eq!(served_type(stored), "application/octet-stream", "{stored}");
        }
        for stored in ["image/png", "image/jpeg", "application/pdf", "text/plain", "text/csv", "application/json", "audio/mpeg"] {
            assert_eq!(served_type(stored), stored);
        }
    }

    fn upload_body(size: usize, visibility: Option<&str>) -> Request {
        let mut value = json!({"name": "a.bin", "type": "application/octet-stream", "dataUrl": encode_base64(&vec![7u8; size])});
        if let Some(visibility) = visibility {
            value["visibility"] = json!(visibility);
        }
        Request::builder().body(Body::from(value.to_string())).unwrap()
    }

    async fn upload(state: &AppState, headers: &HeaderMap, size: usize, visibility: Option<&str>) -> String {
        let response = response_json(upload_file(State(state.clone()), headers.clone(), upload_body(size, visibility)).await).await;
        response["result"]["id"].as_str().unwrap().to_owned()
    }

    async fn status_and_error(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        (status, response_json(response).await["error"].clone())
    }

    #[tokio::test]
    async fn the_uploader_publishes_a_private_file_and_takes_it_back() {
        let root = std::env::temp_dir().join(format!("littlebox-file-update-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, alice_id) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let (bob, _) = signed_in(&state, &deployment.id, &visitor, "bob@example.test");
        let draft = upload(&state, &alice, 2, None).await;
        assert_eq!(serve_file(State(state.clone()), AxumPath(draft.clone()), visitor.clone()).await.status(), StatusCode::NOT_FOUND);

        let published = response_json(update_file(&state, &alice, &draft, Some("public")).await).await;
        assert_eq!(published["result"]["visibility"], "public");
        assert_eq!(published["result"]["ownerId"], alice_id);
        assert_eq!(serve_file(State(state.clone()), AxumPath(draft.clone()), visitor.clone()).await.status(), StatusCode::OK);
        assert_eq!(serve_file(State(state.clone()), AxumPath(draft.clone()), bob.clone()).await.status(), StatusCode::OK);

        // Bob reads the public file but did not upload it.
        assert_eq!(
            status_and_error(update_file(&state, &bob, &draft, Some("private")).await).await,
            (StatusCode::FORBIDDEN, json!(NOT_UPLOADER))
        );
        assert_eq!(
            status_and_error(delete_file(&state, &bob, &draft).await).await,
            (StatusCode::FORBIDDEN, json!(NOT_UPLOADER))
        );

        let hidden = response_json(update_file(&state, &alice, &draft, Some("private")).await).await;
        assert_eq!(hidden["result"]["visibility"], "private");
        assert_eq!(serve_file(State(state.clone()), AxumPath(draft.clone()), visitor.clone()).await.status(), StatusCode::NOT_FOUND);
        // Bob may no longer read it, so to him it does not exist.
        assert_eq!(
            status_and_error(update_file(&state, &bob, &draft, Some("public")).await).await,
            (StatusCode::NOT_FOUND, json!("File not found."))
        );
        assert_eq!(
            status_and_error(delete_file(&state, &bob, &draft).await).await,
            (StatusCode::NOT_FOUND, json!("File not found."))
        );

        for value in [None, Some("organizers"), Some("Public")] {
            assert_eq!(
                status_and_error(update_file(&state, &alice, &draft, value).await).await,
                (StatusCode::BAD_REQUEST, json!(BAD_VISIBILITY))
            );
        }
        assert_eq!(
            status_and_error(update_file(&state, &visitor, &draft, Some("public")).await).await,
            (StatusCode::UNAUTHORIZED, json!("authentication-required"))
        );
        assert_eq!(
            status_and_error(delete_file(&state, &visitor, &draft).await).await,
            (StatusCode::UNAUTHORIZED, json!("authentication-required"))
        );
        for missing in [Uuid::new_v4().to_string(), "../index.json".to_owned()] {
            assert_eq!(
                status_and_error(update_file(&state, &alice, &missing, Some("public")).await).await,
                (StatusCode::NOT_FOUND, json!("File not found."))
            );
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_deployment_holds_a_bounded_number_of_files_with_bounded_names() {
        let root = std::env::temp_dir().join(format!("littlebox-file-count-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        // Control characters, which JSON writes six bytes each, are dropped.
        let named = json!({"name": format!("{}a{}", "\u{1}".repeat(500), "\u{7f}"), "type": "text/plain", "dataUrl": encode_base64(b"x")});
        let stored = response_json(upload_file(State(state.clone()), alice.clone(), Request::builder().body(Body::from(named.to_string())).unwrap()).await).await;
        assert_eq!(stored["result"]["name"], "a");
        let directory = root.join("deployments").join(&deployment.id).join("files");
        let full: Vec<Value> = (0..MAX_FILES).map(|index| json!({"id": format!("{index:036}"), "name": "f", "type": "text/plain", "size": 1, "visibility": "private", "ownerId": "x"})).collect();
        std::fs::write(directory.join("index.json"), serde_json::to_vec(&full).unwrap()).unwrap();
        let refused = upload_file(State(state.clone()), alice.clone(), upload_body(1, None)).await;
        assert_eq!(status_and_error(refused).await, (StatusCode::PAYLOAD_TOO_LARGE, json!(TOO_MANY_FILES)));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_deleted_file_frees_its_quota_and_reads_as_missing() {
        let root = std::env::temp_dir().join(format!("littlebox-file-delete-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let directory = root.join("deployments").join(&deployment.id).join("files");
        let mut ids = Vec::new();
        for _ in 0..10 {
            ids.push(upload(&state, &alice, 2 * 1024 * 1024, Some("public")).await);
        }
        let full = upload_file(State(state.clone()), alice.clone(), upload_body(1, None)).await;
        assert_eq!(
            status_and_error(full).await,
            (StatusCode::PAYLOAD_TOO_LARGE, json!(state.files.limits.quota_reached()))
        );

        let gone = ids.pop().unwrap();
        let deleted = response_json(delete_file(&state, &alice, &gone).await).await;
        assert_eq!(deleted["result"], json!({"id": gone, "deleted": true}));
        assert!(!directory.join(format!("{gone}.blob")).exists());
        assert!(!state.files.store.list(&deployment.id).await.unwrap().contains(&gone));
        assert_eq!(serve_file(State(state.clone()), AxumPath(gone.clone()), alice.clone()).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(serve_file(State(state.clone()), AxumPath(gone.clone()), visitor.clone()).await.status(), StatusCode::NOT_FOUND);
        let listed = response_json(list_files(State(state.clone()), alice.clone()).await).await;
        assert!(listed["result"].as_array().unwrap().iter().all(|file| file["id"] != gone.as_str()));
        assert_eq!(
            status_and_error(delete_file(&state, &alice, &gone).await).await,
            (StatusCode::NOT_FOUND, json!("File not found."))
        );

        let again = upload_file(State(state.clone()), alice.clone(), upload_body(2 * 1024 * 1024, None)).await;
        assert_eq!(again.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The operator's limits replace the defaults: a file the default per-file
    /// cap refuses is stored, and every refusal names the configured numbers.
    #[tokio::test]
    async fn raised_limits_admit_a_file_the_defaults_refuse() {
        let root = std::env::temp_dir().join(format!("littlebox-file-limits-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let three_mib = 3 * 1024 * 1024;
        let refused = upload_file(State(state.clone()), alice.clone(), upload_body(three_mib, None)).await;
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let mut raised = state.clone();
        raised.files = raised.files.clone().with_limits(file_store::FileLimits::parse(Some("7340032"), Some("4194304")).unwrap());
        let id = upload(&raised, &alice, three_mib, Some("public")).await;
        let read = serve_file(State(raised.clone()), AxumPath(id.clone()), visitor.clone()).await;
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(axum::body::to_bytes(read.into_body(), usize::MAX).await.unwrap().len(), three_mib);

        let too_big = upload_file(State(raised.clone()), alice.clone(), upload_body(4 * 1024 * 1024 + 1, None)).await;
        assert_eq!(status_and_error(too_big).await, (StatusCode::PAYLOAD_TOO_LARGE, json!("Files must be between 1 byte and 4 MiB.")));
        let second = upload(&raised, &alice, three_mib, None).await;
        assert!(!second.is_empty());
        let full = upload_file(State(raised.clone()), alice.clone(), upload_body(1024 * 1024 + 1, None)).await;
        assert_eq!(status_and_error(full).await, (StatusCode::PAYLOAD_TOO_LARGE, json!("This deployment has reached its 7 MiB file quota.")));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A range of a file answers 206 with that part, and a range past the end 416.
    #[tokio::test]
    async fn the_file_path_serves_one_byte_range() {
        let root = std::env::temp_dir().join(format!("littlebox-file-range-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let id = upload(&state, &alice, 1000, Some("public")).await;
        let ranged = |range: &'static str| {
            let mut headers = visitor.clone();
            headers.insert(header::RANGE, HeaderValue::from_static(range));
            serve_file(State(state.clone()), AxumPath(id.clone()), headers)
        };
        let part = ranged("bytes=10-19").await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 10-19/1000");
        assert_eq!(axum::body::to_bytes(part.into_body(), usize::MAX).await.unwrap().len(), 10);
        let tail = ranged("bytes=-100").await;
        assert_eq!(tail.headers()[header::CONTENT_RANGE], "bytes 900-999/1000");
        assert_eq!(ranged("bytes=1000-").await.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        let whole = ranged("items=0-1").await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.headers()[header::CONTENT_LENGTH], "1000");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_file_without_an_uploader_cannot_be_changed_from_the_app() {
        let root = std::env::temp_dir().join(format!("littlebox-file-legacy-{}", Uuid::new_v4()));
        let (state, visitor) = login_test_state(&root, "public").await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let directory = root.join("deployments").join(&deployment.id).join("files");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let private_legacy = Uuid::new_v4().to_string();
        let public_legacy = Uuid::new_v4().to_string();
        let index = json!([
            {"id": private_legacy, "name": "old.txt", "type": "text/plain", "size": 2, "createdAt": 1},
            {"id": public_legacy, "name": "old.png", "type": "image/png", "size": 2, "visibility": "public", "createdAt": 1}
        ]);
        tokio::fs::write(directory.join("index.json"), index.to_string()).await.unwrap();
        assert_eq!(
            status_and_error(delete_file(&state, &alice, &private_legacy).await).await,
            (StatusCode::NOT_FOUND, json!("File not found."))
        );
        assert_eq!(
            status_and_error(update_file(&state, &alice, &public_legacy, Some("private")).await).await,
            (StatusCode::FORBIDDEN, json!(NOT_UPLOADER))
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A visitor of a private Deployment is asked to sign in before any file is
    /// looked up; on a public Deployment a file the visitor may not read
    /// answers as a missing one. The same public file shows both.
    #[tokio::test]
    async fn a_visitor_is_asked_to_sign_in_on_a_private_app_and_told_not_found_on_a_public_one() {
        let root = std::env::temp_dir().join(format!("littlebox-file-visitor-{}", Uuid::new_v4()));
        for (visibility, public_status) in [("private", StatusCode::UNAUTHORIZED), ("public", StatusCode::OK)] {
            let (state, visitor) = login_test_state(&root.join(visibility), visibility).await;
            let deployment = state.deployments.read().await.values().next().cloned().unwrap();
            let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
            let public_file = upload(&state, &alice, 2, Some("public")).await;
            let private_file = upload(&state, &alice, 2, None).await;
            let read = |id: String| serve_file(State(state.clone()), AxumPath(id), visitor.clone());
            assert_eq!(read(public_file).await.status(), public_status, "{visibility}");
            let (status, message) = status_and_error(read(private_file).await).await;
            if visibility == "private" {
                assert_eq!((status, message), (StatusCode::UNAUTHORIZED, json!("authentication-required")));
            } else {
                assert_eq!((status, message), (StatusCode::NOT_FOUND, json!("File not found.")));
            }
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
