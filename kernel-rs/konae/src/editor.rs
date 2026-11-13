//! The editor, served by Konae on the editor host.
//!
//! `bun run build` writes the editor into `dist/`, and `KONAE_EDITOR_DIR`
//! names that folder. Konae then answers every GET on the host of
//! `KONAE_PUBLIC_ORIGIN` with it, except the paths Konae itself answers:
//! `/mcp`, `/_konae`, `/oauth` and `/.well-known`. A path that names no
//! file and has no extension, such as `/app/<id>`, answers `index.html`,
//! because the editor routes in the browser.
//!
//! The editor carries its own Content Security Policy, which stops at
//! framing. Its preview is a `srcdoc` iframe that inherits the editor's
//! policy, and esbuild, sql.js and the model providers the agent calls need
//! more than `'self'`, so Konae's own page policy would break it. The `kernel`
//! binary never serves the editor.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The paths Konae answers on the editor host. The editor never shadows them.
const KONAE_PATHS: [&str; 4] = ["/mcp", "/_konae", "/oauth", "/.well-known"];

/// The editor's policy. It is the one the Vite server sends in development.
pub const EDITOR_POLICY: &str = "frame-ancestors 'none'";

/// Vite names every file under `assets/` by its content hash.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub struct Editor {
    root: PathBuf,
    host: String,
    hsts: Option<&'static str>,
}

impl Editor {
    /// The editor `KONAE_EDITOR_DIR` names, or `None` when it is unset or
    /// empty. A folder without `index.html` stops Konae at startup, so an
    /// operator learns of a wrong path before an owner opens a blank page.
    pub fn from_env(public_origin: &str, https: bool) -> Result<Option<Self>> {
        let Some(dir) = std::env::var("KONAE_EDITOR_DIR").ok().filter(|value| !value.trim().is_empty()) else {
            return Ok(None);
        };
        Self::new(PathBuf::from(dir), public_origin, https).map(Some)
    }

    pub fn new(root: PathBuf, public_origin: &str, https: bool) -> Result<Self> {
        if !root.join("index.html").is_file() {
            anyhow::bail!(
                "KONAE_EDITOR_DIR={} has no index.html. Point it at the folder `bun run build` writes, or unset it to serve no editor.",
                root.display()
            );
        }
        let host = url::Url::parse(public_origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .ok_or_else(|| anyhow::anyhow!("KONAE_PUBLIC_ORIGIN={public_origin} names no host, so Konae cannot tell where the editor answers."))?;
        Ok(Self { root, host, hsts: https.then_some(kernel::security::HSTS) })
    }

    fn answers(&self, method: &Method, headers: &HeaderMap, path: &str) -> bool {
        if method != Method::GET && method != Method::HEAD {
            return false;
        }
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.rsplit_once(':').filter(|(_, port)| port.parse::<u16>().is_ok()).map_or(value, |(name, _)| name))
            .map(str::to_ascii_lowercase);
        if host.as_deref() != Some(self.host.as_str()) {
            return false;
        }
        !KONAE_PATHS
            .iter()
            .any(|prefix| path == *prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/')))
    }

    /// The file a path names under the editor's folder, or `None` for a path
    /// that leaves it.
    fn file(&self, path: &str) -> Option<PathBuf> {
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.as_os_str().is_empty() {
            return Some(self.root.join("index.html"));
        }
        if path.contains('\\') || !relative.components().all(|part| matches!(part, Component::Normal(_))) {
            return None;
        }
        Some(self.root.join(relative))
    }

    async fn respond(&self, path: &str) -> Response {
        let (file, cache) = match self.file(path) {
            Some(file) if file.is_file() => {
                let cache = if path.starts_with("/assets/") { IMMUTABLE } else { "no-cache" };
                (file, cache)
            }
            Some(_) if !path.rsplit('/').next().unwrap_or_default().contains('.') => (self.root.join("index.html"), "no-cache"),
            _ => return self.headers(kernel::error(StatusCode::NOT_FOUND, "Not found.")),
        };
        let bytes = match tokio::fs::read(&file).await {
            Ok(bytes) => bytes,
            Err(_) => return self.headers(kernel::error(StatusCode::NOT_FOUND, "Not found.")),
        };
        let name = file.to_string_lossy();
        let response = (
            [
                (header::CONTENT_TYPE, kernel::content_type(&name)),
                (header::CACHE_CONTROL, cache),
            ],
            Body::from(bytes),
        )
            .into_response();
        self.headers(response)
    }

    fn headers(&self, mut response: Response) -> Response {
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(EDITOR_POLICY));
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        if let Some(hsts) = self.hsts {
            headers.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(hsts));
        }
        response
    }
}

/// Answers an editor request, and passes every other request to the Kernel.
pub async fn serve(State(editor): State<Arc<Editor>>, request: Request, next: Next) -> Response {
    if !editor.answers(request.method(), request.headers(), request.uri().path()) {
        return next.run(request).await;
    }
    editor.respond(request.uri().path()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::any};
    use tower::ServiceExt;

    fn folder() -> PathBuf {
        let root = std::env::temp_dir().join(format!("konae-editor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("index.html"), "<!doctype html><div id=root></div>").unwrap();
        std::fs::write(root.join("favicon.svg"), "<svg/>").unwrap();
        std::fs::write(root.join("assets/index-abc.js"), "console.log(1)").unwrap();
        std::fs::write(root.parent().unwrap().join(format!("{}-secret", root.file_name().unwrap().to_string_lossy())), "secret").unwrap();
        root
    }

    fn app(root: PathBuf) -> Router {
        let editor = Arc::new(Editor::new(root, "https://app.example.com", true).unwrap());
        Router::new()
            .fallback(any(|| async { (StatusCode::IM_A_TEAPOT, "kernel") }))
            .layer(axum::middleware::from_fn_with_state(editor, serve))
    }

    async fn get(app: &Router, method: &str, host: &str, path: &str) -> (StatusCode, HeaderMap, String) {
        let request = Request::builder().method(method).uri(path).header(header::HOST, host).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (parts.status, parts.headers, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn the_editor_host_answers_its_pages_and_files() {
        let app = app(folder());
        for path in ["/", "/app/proj-1", "/platform-access", "/usage"] {
            let (status, headers, body) = get(&app, "GET", "app.example.com", path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("id=root"), "{path}");
            assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
            assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
            assert_eq!(headers[header::CONTENT_SECURITY_POLICY], EDITOR_POLICY);
            assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
            assert_eq!(headers[header::STRICT_TRANSPORT_SECURITY], kernel::security::HSTS);
        }
        let (status, headers, body) = get(&app, "GET", "app.example.com:443", "/assets/index-abc.js").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(headers[header::CACHE_CONTROL], IMMUTABLE);
        let (status, headers, _) = get(&app, "GET", "APP.example.com", "/favicon.svg").await;
        assert_eq!((status, &headers[header::CONTENT_TYPE]), (StatusCode::OK, &HeaderValue::from_static("image/svg+xml")));
        let (status, _, _) = get(&app, "GET", "app.example.com", "/assets/missing.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn konae_paths_other_hosts_and_writes_reach_the_kernel() {
        let app = app(folder());
        for (method, host, path) in [
            ("GET", "app.example.com", "/mcp"),
            ("GET", "app.example.com", "/_konae/health"),
            ("GET", "app.example.com", "/_konae/owner/login"),
            ("GET", "app.example.com", "/oauth/authorize"),
            ("GET", "app.example.com", "/.well-known/oauth-authorization-server"),
            ("POST", "app.example.com", "/"),
            ("GET", "notes.example.com", "/"),
            ("GET", "example.com", "/p/notes"),
        ] {
            let (status, _, body) = get(&app, method, host, path).await;
            assert_eq!((status, body.as_str()), (StatusCode::IM_A_TEAPOT, "kernel"), "{method} {host}{path}");
        }
        let (status, _, body) = get(&app, "GET", "app.example.com", "/mcpanel").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("id=root"));
    }

    #[tokio::test]
    async fn a_path_never_leaves_the_editor_folder() {
        let root = folder();
        let name = root.file_name().unwrap().to_string_lossy().into_owned();
        let app = app(root);
        for path in [format!("/../{name}-secret"), format!("/assets/../../{name}-secret"), "/assets/..%2f..%2fx.js".to_owned()] {
            let (status, _, body) = get(&app, "GET", "app.example.com", &path).await;
            assert_ne!(body, "secret", "{path}");
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[test]
    fn a_folder_without_index_html_is_refused() {
        let root = std::env::temp_dir().join(format!("konae-editor-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let refused = Editor::new(root, "https://app.example.com", true).err().unwrap().to_string();
        assert!(refused.contains("has no index.html"), "{refused}");
    }
}
