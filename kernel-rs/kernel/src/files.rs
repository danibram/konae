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
pub const MAX_FILES: usize = 10_000;
pub const TOO_MANY_FILES: &str = "This deployment has reached its limit of 10,000 files. Delete files before uploading more.";
pub const NOT_UPLOADER: &str = "Only the person who uploaded this file may change or delete it.";
pub const FIXED_VISIBILITY: &str = "File visibility is fixed to app-users by this app.";
const BAD_VISIBILITY: &str = "File visibility must be private or public.";

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
    let actor = match authenticate_deployment(state, &deployment, headers) {
        Ok(Some(actor)) => actor,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "authentication-required"),
        Err(response) => return response,
    };
    if !valid_file_id(id) {
        return not_found();
    }
    let _guard = database_lock(state, &deployment.id).await;
    let directory = crate::layout::files(&state.data_root, &deployment.id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{login_test_state, response_json, signed_in};

    /// Runs one test over the local folder and over an S3 store, the
    /// in-process fake unless `KONAE_TEST_S3_ENDPOINT` names a server.
    macro_rules! on_both_stores {
        ($name:ident) => {
            mod $name {
                #[tokio::test]
                async fn local_folder() {
                    super::$name(false).await
                }
                #[tokio::test]
                async fn s3() {
                    super::$name(true).await
                }
            }
        };
    }

    async fn login_on(root: &std::path::Path, visibility: &str, s3: bool) -> (AppState, HeaderMap) {
        let (mut state, visitor) = login_test_state(root, visibility).await;
        if s3 {
            state.files.store = Arc::new(file_store::tests::test_s3_store().await);
        }
        (state, visitor)
    }

    #[tokio::test]
    async fn app_users_policy_covers_existing_and_new_files() {
        let root = std::env::temp_dir().join(format!("konae-file-policy-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", false).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let (bob, _) = signed_in(&state, &deployment.id, &visitor, "bob@example.test");
        let old_private = upload(&state, &alice, 2, None).await;
        let old_public = upload(&state, &alice, 2, Some("public")).await;
        let mut updated = deployment.clone();
        updated.manifest["fileAccess"] = json!("app-users");
        state.deployments.write().await.insert(updated.hostname.clone(), updated);
        for id in [&old_private, &old_public] {
            assert_eq!(serve_file(State(state.clone()), AxumPath(id.clone()), visitor.clone()).await.status(), StatusCode::NOT_FOUND);
            assert_eq!(serve_file(State(state.clone()), AxumPath(id.clone()), bob.clone()).await.status(), StatusCode::OK);
        }
        let listed = response_json(list_files(State(state.clone()), bob.clone()).await).await;
        assert_eq!(listed["result"].as_array().unwrap().len(), 2);
        assert!(listed["result"].as_array().unwrap().iter().all(|f| f["visibility"] == "app-users"));
        assert_eq!(response_json(list_files(State(state.clone()), visitor.clone()).await).await["result"], json!([]));
        assert_eq!(status_and_error(update_file(&state, &bob, &old_private, Some("public")).await).await, (StatusCode::FORBIDDEN, json!(NOT_UPLOADER)));
        assert_eq!(status_and_error(update_file(&state, &alice, &old_private, Some("public")).await).await, (StatusCode::BAD_REQUEST, json!(FIXED_VISIBILITY)));
        assert_eq!(response_json(update_file(&state, &alice, &old_private, Some("app-users")).await).await["result"]["visibility"], "app-users");
        let created = response_json(upload_file(State(state.clone()), alice.clone(), upload_body(2, None)).await).await;
        assert_eq!(created["result"]["visibility"], "app-users");
        assert_eq!(status_and_error(upload_file(State(state.clone()), alice.clone(), upload_body(2, Some("public"))).await).await, (StatusCode::BAD_REQUEST, json!(FIXED_VISIBILITY)));
        assert_eq!(response_json(upload_file(State(state.clone()), alice.clone(), upload_body(2, Some("app-users"))).await).await["result"]["visibility"], "app-users");
        assert_eq!(status_and_error(upload_file(State(state.clone()), visitor.clone(), upload_body(2, None)).await).await.0, StatusCode::UNAUTHORIZED);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn published_policy_protects_existing_file_routes() {
        use tower::ServiceExt;
        let root = std::env::temp_dir().join(format!("konae-file-publish-{}", Uuid::new_v4()));
        let state = AppState::for_test(&root);
        let release = |project: &str, shared: bool| json!({
            "project": {"id": project, "name": project, "slug": project, "manifest": if shared { json!({"fileAccess":"app-users"}) } else { json!({}) },
                "files": {"actions.json": json!({"version":1,"actions":{"items.count":{"kind":"query","access":"anonymous","statement":"SELECT 1 AS n LIMIT 1","bindings":[]}}}).to_string(), "schema.sql":""}},
            "visibility":"public", "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<main>attachments</main>"}}
        });
        assert_eq!(deploy(&state, release("attachments", false).to_string().as_bytes()).await.status(), StatusCode::OK);
        let app = state.deployments.read().await.values().find(|d| d.project_id == "attachments").unwrap().clone();
        let mut visitor = HeaderMap::new();
        visitor.insert(header::HOST, app.hostname.parse().unwrap());
        visitor.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        let (alice, alice_id) = signed_in(&state, &app.id, &visitor, "alice@example.test");
        let (bob, _) = signed_in(&state, &app.id, &visitor, "bob@example.test");
        let mut stored = Vec::new();
        for (name, ty, bytes, visibility) in [("photo.png", "image/png", &b"synthetic png"[..], "public"), ("document.pdf", "application/pdf", &b"%PDF-1.4 synthetic"[..], "private")] {
            let body = json!({"name":name,"type":ty,"dataUrl":encode_base64(bytes),"visibility":visibility});
            let request = Request::builder().body(Body::from(body.to_string())).unwrap();
            let saved = response_json(upload_file(State(state.clone()), alice.clone(), request).await).await;
            assert_eq!(saved["result"]["ownerId"], alice_id);
            stored.push((saved["result"]["id"].as_str().unwrap().to_owned(), ty, bytes));
        }
        let request = |headers: HeaderMap, id: &str| {
            let mut request = Request::builder().method("GET").uri(format!("/_konae/files/{id}")).body(Body::empty()).unwrap();
            *request.headers_mut() = headers;
            request
        };
        let read = |headers: HeaderMap, id: &str| crate::stack(state.clone(), Router::new()).oneshot(request(headers, id));
        assert_eq!(read(visitor.clone(), &stored[0].0).await.unwrap().status(), StatusCode::OK);
        assert_eq!(deploy(&state, release("attachments", true).to_string().as_bytes()).await.status(), StatusCode::OK);
        let updated = state.deployments.read().await.values().find(|d| d.project_id == "attachments").unwrap().clone();
        assert_eq!(updated.id, app.id);
        assert_eq!(updated.manifest["fileAccess"], "app-users");
        assert_eq!(deploy(&state, release("other", true).to_string().as_bytes()).await.status(), StatusCode::OK);
        let other = state.deployments.read().await.values().find(|d| d.project_id == "other").unwrap().clone();
        let (foreign, _) = signed_in(&state, &other.id, &visitor, "foreign@example.test");
        let listed = response_json(list_files(State(state.clone()), alice.clone()).await).await;
        assert_eq!(listed["result"].as_array().unwrap().len(), 2);
        for (id, ty, bytes) in &stored {
            let row = listed["result"].as_array().unwrap().iter().find(|f| f["id"] == *id).unwrap();
            assert_eq!(row["ownerId"], alice_id);
            assert_eq!(row["visibility"], "app-users");
            for headers in [&alice, &bob] {
                let response = read(headers.clone(), id).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[header::CONTENT_TYPE], *ty);
                assert_eq!(&axum::body::to_bytes(response.into_body(), 1024).await.unwrap()[..], *bytes);
            }
            assert_eq!(read(visitor.clone(), id).await.unwrap().status(), StatusCode::NOT_FOUND);
            assert_ne!(read(foreign.clone(), id).await.unwrap().status(), StatusCode::OK);
            assert_eq!(status_and_error(update_file(&state, &alice, id, Some("public")).await).await, (StatusCode::BAD_REQUEST, json!(FIXED_VISIBILITY)));
            assert_eq!(status_and_error(delete_file(&state, &bob, id).await).await, (StatusCode::FORBIDDEN, json!(NOT_UPLOADER)));
            assert_eq!(read(visitor.clone(), id).await.unwrap().status(), StatusCode::NOT_FOUND);
        }
        AuthService::open(crate::layout::identity(&root, &app.id)).unwrap().suspend("bob@example.test").unwrap();
        assert_ne!(read(bob, &stored[0].0).await.unwrap().status(), StatusCode::OK);
        assert_eq!(read(alice, &stored[0].0).await.unwrap().status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

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

    on_both_stores!(the_uploader_publishes_a_private_file_and_takes_it_back);
    async fn the_uploader_publishes_a_private_file_and_takes_it_back(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-update-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
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

    on_both_stores!(a_deployment_holds_a_bounded_number_of_files_with_bounded_names);
    async fn a_deployment_holds_a_bounded_number_of_files_with_bounded_names(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-count-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        // Control characters, which JSON writes six bytes each, are dropped.
        let named = json!({"name": format!("{}a{}", "\u{1}".repeat(500), "\u{7f}"), "type": "text/plain", "dataUrl": encode_base64(b"x")});
        let stored = response_json(upload_file(State(state.clone()), alice.clone(), Request::builder().body(Body::from(named.to_string())).unwrap()).await).await;
        assert_eq!(stored["result"]["name"], "a");
        let directory = crate::layout::files(&root, &deployment.id);
        let full: Vec<Value> = (0..MAX_FILES).map(|index| json!({"id": format!("{index:036}"), "name": "f", "type": "text/plain", "size": 1, "visibility": "private", "ownerId": "x"})).collect();
        std::fs::write(directory.join("index.json"), serde_json::to_vec(&full).unwrap()).unwrap();
        let refused = upload_file(State(state.clone()), alice.clone(), upload_body(1, None)).await;
        assert_eq!(status_and_error(refused).await, (StatusCode::PAYLOAD_TOO_LARGE, json!(TOO_MANY_FILES)));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    on_both_stores!(a_deleted_file_frees_its_quota_and_reads_as_missing);
    async fn a_deleted_file_frees_its_quota_and_reads_as_missing(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-delete-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let directory = crate::layout::files(&root, &deployment.id);
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
        assert!(!directory.join(format!("{gone}.blob")).exists() || s3);
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

    on_both_stores!(raised_limits_admit_a_file_the_defaults_refuse);
    /// The operator's limits replace the defaults: a file the default per-file
    /// cap refuses is stored, and every refusal names the configured numbers.
    async fn raised_limits_admit_a_file_the_defaults_refuse(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-limits-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
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

    on_both_stores!(the_file_path_serves_one_byte_range);
    /// A range of a file answers 206 with that part, and a range past the end 416.
    async fn the_file_path_serves_one_byte_range(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-range-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
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

    on_both_stores!(a_browser_that_holds_a_file_revalidates_it_without_the_store);
    /// A file answers with its tag and `no-cache`; the same tag gets 304 and
    /// no bytes, and only while the caller may still read the file.
    async fn a_browser_that_holds_a_file_revalidates_it_without_the_store(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-etag-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let id = upload(&state, &alice, 1000, Some("public")).await;
        let read = |headers: HeaderMap| serve_file(State(state.clone()), AxumPath(id.clone()), headers);
        let first = read(visitor.clone()).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CACHE_CONTROL], "private, no-cache");
        let etag = first.headers()[header::ETAG].clone();
        assert_eq!(etag, format!("\"{id}-1000\""));
        let asking = |tag: &str| {
            let mut headers = visitor.clone();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(tag).unwrap());
            headers
        };
        let tag = etag.to_str().unwrap().to_owned();
        let kept = read(asking(&tag)).await;
        assert_eq!(kept.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(kept.headers()[header::ETAG], etag);
        assert!(axum::body::to_bytes(kept.into_body(), usize::MAX).await.unwrap().is_empty());
        assert_eq!(read(asking(&format!("\"other\", W/{tag}"))).await.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(read(asking("\"other\"")).await.status(), StatusCode::OK);
        // The bytes are gone from the store, yet the tag still answers: the
        // index alone decides, and the store is never asked.
        state.files.store.delete(&deployment.id, &id).await.unwrap();
        assert_eq!(read(asking(&tag)).await.status(), StatusCode::NOT_MODIFIED);
        // A file the caller may no longer read answers 404 whatever the tag.
        update_file(&state, &alice, &id, Some("private")).await;
        assert_eq!(read(asking(&tag)).await.status(), StatusCode::NOT_FOUND);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    on_both_stores!(a_file_without_an_uploader_cannot_be_changed_from_the_app);
    async fn a_file_without_an_uploader_cannot_be_changed_from_the_app(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-legacy-{}", Uuid::new_v4()));
        let (state, visitor) = login_on(&root, "public", s3).await;
        let deployment = state.deployments.read().await.values().next().cloned().unwrap();
        let (alice, _) = signed_in(&state, &deployment.id, &visitor, "alice@example.test");
        let directory = crate::layout::files(&root, &deployment.id);
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

    on_both_stores!(a_visitor_is_asked_to_sign_in_on_a_private_app_and_told_not_found_on_a_public_one);
    /// A visitor of a private Deployment is asked to sign in before any file is
    /// looked up; on a public Deployment a file the visitor may not read
    /// answers as a missing one. The same public file shows both.
    async fn a_visitor_is_asked_to_sign_in_on_a_private_app_and_told_not_found_on_a_public_one(s3: bool) {
        let root = std::env::temp_dir().join(format!("konae-file-visitor-{}", Uuid::new_v4()));
        for (visibility, public_status) in [("private", StatusCode::UNAUTHORIZED), ("public", StatusCode::OK)] {
            let (state, visitor) = login_on(&root.join(visibility), visibility, s3).await;
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
