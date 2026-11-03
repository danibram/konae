//! The maintenance state of one Deployment, and the guards that keep a paused
//! app from taking writes. ADR-0014 introduced it and ADR-0027 keeps it.
//!
//! The state lives in `deployments/<id>/maintenance.json`, inside the
//! Deployment's folder, so a restart, a scheduled copy and a restore all keep
//! it. While it pauses an app:
//!
//! - the app host serves a maintenance page for the published shell and
//!   refuses every other request with one message;
//! - administrative tools that would change the app are refused;
//! - the mail worker holds the app's queued email, and the call outbox and
//!   the schedule wait.
//!
//! An installation that a restore could not reconcile is paused the same way
//! for every app (`recovery::hold`).

use std::sync::{LazyLock, RwLock as SyncRwLock};

use axum::{http::Method, middleware::Next};

use super::*;

pub(crate) const MARKER_FILE: &str = "maintenance.json";
/// How long the mail worker puts back a held job before looking again.
pub(crate) const MAIL_HOLD_MS: i64 = 60_000;

/// The answer to every refused request while an app is in maintenance.
const MESSAGE: &str = "This app is in maintenance. It accepts no writes, signups, recovery, administrative changes or email until it comes out of maintenance.";
const UNREADABLE: &str = "This app is in maintenance because its maintenance state (maintenance.json) cannot be read. The operator must repair or remove that file.";

/// The maintenance state of one Deployment. A file that exists but cannot be
/// read keeps the app paused, because the Kernel cannot tell why it was paused.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Marker {
    #[serde(default)]
    since: u64,
    #[serde(skip)]
    unreadable: bool,
}

impl Marker {
    fn message(&self) -> &'static str {
        if self.unreadable { UNREADABLE } else { MESSAGE }
    }
}

/// Markers by file path. Only this process writes them while it runs, and a
/// restore runs before it starts, so the cache stays true.
static MARKERS: LazyLock<SyncRwLock<HashMap<PathBuf, Option<Arc<Marker>>>>> = LazyLock::new(Default::default);

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\', '.'])
}

fn marker_path(data_root: &Path, id: &str) -> PathBuf {
    crate::layout::private(data_root, id).join(MARKER_FILE)
}

fn marker(data_root: &Path, id: &str) -> Option<Arc<Marker>> {
    if !valid_id(id) {
        return None;
    }
    let path = marker_path(data_root, id);
    if let Some(found) = MARKERS.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&path) {
        return found.clone();
    }
    let unreadable = || Arc::new(Marker { unreadable: true, ..Marker::default() });
    let loaded = match std::fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice::<Marker>(&bytes).map(Arc::new).unwrap_or_else(|_| unreadable())),
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(unreadable()),
    };
    MARKERS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path, loaded.clone());
    loaded
}

/// Puts a Deployment in maintenance or takes it out, for tests of the modules
/// the pause reaches.
#[cfg(test)]
pub(crate) fn pause_for_test(data_root: &Path, id: &str, paused: bool) {
    let path = marker_path(data_root, id);
    if paused {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&Marker { since: now_ms(), unreadable: false }).unwrap()).unwrap();
    } else {
        let _ = std::fs::remove_file(&path);
    }
    MARKERS.write().unwrap().remove(&path);
}

/// Forgets cached markers under `data_root`, for tests that edit files.
#[cfg(test)]
pub(crate) fn forget(data_root: &Path) {
    MARKERS.write().unwrap().retain(|path, _| !path.starts_with(data_root));
}

/// Why a Deployment is paused: its own maintenance state, or the
/// installation's when a restore has not reconciled its revocation record.
pub(crate) fn pause(data_root: &Path, id: &str) -> Option<String> {
    if let Some(message) = super::recovery::hold(data_root) {
        return Some(message);
    }
    marker(data_root, id).map(|marker| marker.message().to_owned())
}

/// Whether the mail worker must hold a job of this scope.
pub(crate) fn holds_mail(data_root: &Path, scope: &str) -> bool {
    super::recovery::hold(data_root).is_some()
        || scope.strip_prefix("deployment:").and_then(|id| marker(data_root, id)).is_some()
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn maintenance_page(name: &str, message: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} is in maintenance</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:34rem;margin:15vh auto;padding:0 1.5rem;color:#1f2328}}\
h1{{font-size:1.4rem}}\
@media (prefers-color-scheme: dark){{body{{background:#0d1117;color:#e6edf3}}}}</style></head>\
<body><main><h1>{title} is in maintenance</h1><p>{message}</p></main></body></html>",
        title = escape(name),
        message = escape(message),
    );
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, html).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("300"));
    response
}

fn refusal(message: &str) -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": message, "maintenance": true}))).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("300"));
    response
}

/// Middleware on every route. A request to a paused app's host gets the
/// maintenance page when it asks for a page, and a 503 with the message for
/// anything else: actions, uploads, signups, sign-ins, recovery, invitations
/// and HTTP routes. Nothing is redirected.
pub(super) async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path == "/_konae/health" {
        return next.run(request).await;
    }
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return next.run(request).await;
    };
    let Some(message) = pause(&state.data_root, &deployment.id) else {
        return next.run(request).await;
    };
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    if matches!(*request.method(), Method::GET | Method::HEAD) && (!path.starts_with("/_konae/") || wants_page) {
        maintenance_page(&deployment.project_name, &message)
    } else {
        refusal(&message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!("konae-maintenance-{}", Uuid::new_v4()));
        std::fs::create_dir_all(crate::layout::data(&root, "app-1")).unwrap();
        std::fs::create_dir_all(crate::layout::data(&root, "app-2")).unwrap();
        root
    }

    #[test]
    fn the_state_survives_a_restart_and_an_unreadable_one_pauses() {
        let root = scratch();
        assert!(pause(&root, "app-1").is_none());
        pause_for_test(&root, "app-1", true);
        forget(&root);
        assert_eq!(pause(&root, "app-1").as_deref(), Some(MESSAGE));
        std::fs::write(marker_path(&root, "app-1"), "{not json").unwrap();
        forget(&root);
        assert_eq!(pause(&root, "app-1").as_deref(), Some(UNREADABLE));
        pause_for_test(&root, "app-1", false);
        assert!(pause(&root, "app-1").is_none());
        assert!(marker(&root, "../escape").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_mail_worker_holds_a_paused_apps_email_only() {
        let root = scratch();
        pause_for_test(&root, "app-1", true);
        assert!(holds_mail(&root, "deployment:app-1"));
        assert!(!holds_mail(&root, "deployment:app-2"));
        assert!(!holds_mail(&root, "creator"));
        pause_for_test(&root, "app-1", false);
        assert!(!holds_mail(&root, "deployment:app-1"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_maintenance_page_escapes_what_it_shows() {
        let response = maintenance_page("<b>Notes</b>", "Paused & waiting");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let bytes = runtime.block_on(axum::body::to_bytes(response.into_body(), 1 << 20)).unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("&lt;b&gt;Notes&lt;/b&gt;"));
        assert!(body.contains("Paused &amp; waiting"));
        assert!(!body.contains("<script"));
    }
}
