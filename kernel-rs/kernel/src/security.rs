//! Response headers that keep hosted code on its own origin.
//!
//! Every app is served from `<slug>-<id>.<suffix>`, so a creator's JavaScript
//! runs next to other apps and next to the editor host. One layer adds the
//! headers below to every response the Kernel sends.
//!
//! - App hosts get a Content Security Policy. A Build Artifact inlines the
//!   Konae runtime and the compiled module in `index.html`, so the policy
//!   allows those exact scripts by their SHA-256 hashes instead of allowing
//!   inline script in general. Inline event handler attributes, `eval` and
//!   scripts from other origins stay blocked.
//! - Every host gets `frame-ancestors 'none'` and `X-Frame-Options: DENY`, so
//!   an app cannot frame the editor, another app or itself in another origin.
//! - Every host gets HSTS when the installation is served over HTTPS, and a
//!   custom domain gets it without `includeSubDomains`.
//! - A path address on `<domain>` (ADR-0014) is an app host for these headers.
//!   The layer runs outside the router, so the home page and the redirects of
//!   the path router get the same headers as an app response.

use super::*;
use axum::middleware::Next;
use sha2::{Digest, Sha256};

/// HTML larger than this is served without script hashes, so its inline
/// scripts do not run. Deployments are capped below this size.
const MAX_HASHED_HTML: usize = 32 * 1024 * 1024;

/// The policy for a response from an app host, given the hash sources of its
/// inline scripts.
///
/// Styles allow `'unsafe-inline'` because apps write `style="..."` attributes
/// through `innerHTML` and inline a `<style>` block; a hash cannot cover a
/// generated attribute. The starter templates load their fonts from Google
/// Fonts, so those two hosts are the only foreign origins. Images and media
/// accept `blob:` and `data:` so a page can show bytes it built itself, such
/// as a preview before upload, and apps keep small inline copies of images. `connects` are the
/// origins the manifest's `connects` adds to connect-src (ADR-0013).
pub fn app_policy(script_hashes: &[String], connects: &[String]) -> String {
    let mut script = String::from("'self'");
    for hash in script_hashes {
        script.push(' ');
        script.push_str(hash);
    }
    let mut connect = String::from("'self'");
    for origin in connects {
        connect.push(' ');
        connect.push_str(origin);
    }
    format!(
        "default-src 'self'; script-src {script}; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
         font-src 'self' data: https://fonts.gstatic.com; \
         img-src 'self' blob: data:; media-src 'self' blob: data:; \
         connect-src {connect}; form-action 'self'; frame-ancestors 'none'; \
         object-src 'none'; base-uri 'none'"
    )
}

/// The policy for an HTML page the Kernel serves on the editor host, such as
/// owner sign-in.
pub fn platform_page_policy(script_hashes: &[String]) -> String {
    let mut script = String::from("'self'");
    for hash in script_hashes {
        script.push(' ');
        script.push_str(hash);
    }
    format!(
        "default-src 'self'; script-src {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'self'; form-action 'self'; \
         frame-ancestors 'none'; object-src 'none'; base-uri 'none'"
    )
}

/// The policy for anything else on the editor host: JSON, errors and
/// discovery documents load nothing and may not be framed.
pub const PLATFORM_POLICY: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

/// The HSTS value. `includeSubDomains` makes a browser that visited the
/// installation's own host use HTTPS for app hosts under it before their
/// first visit. There is no `preload`, which is the operator's decision.
pub const HSTS: &str = "max-age=31536000; includeSubDomains";

/// The HSTS value on a custom domain. The owner's other names under it are
/// not the installation's to put on HTTPS, so it has no `includeSubDomains`.
pub const HSTS_ALIAS: &str = "max-age=31536000";

/// Whether the installation is served over HTTPS, by the same rule that picks
/// the `__Host-` cookie names.
pub fn https(state: &AppState) -> bool {
    state.public_origin.starts_with("https://")
        || state.own_origins.iter().any(|origin| origin.starts_with("https://"))
}

/// The HSTS value for a request, or none over plain HTTP.
pub fn hsts(state: &AppState, headers: &HeaderMap) -> Option<&'static str> {
    if !https(state) {
        return None;
    }
    Some(if super::domain::is_alias_request(state, headers) { HSTS_ALIAS } else { HSTS })
}

/// The CSP hash source of every inline `<script>` in an HTML document.
///
/// The scan follows the HTML tokenizer closely enough for documents the
/// Kernel serves: a script element starts at `<script` followed by a space,
/// `/` or `>`, its start tag ends at the first `>` outside a quoted attribute
/// value, and its text ends at the next `</script`. The browser hashes the
/// text after it turns CRLF and lone CR into LF, so the scan does the same.
/// A script with `src` has no text and adds nothing.
pub fn inline_script_hashes(html: &[u8]) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut hashes = Vec::new();
    let mut cursor = 0;
    while let Some(found) = find(&lower[cursor..], b"<script") {
        let name_end = cursor + found + b"<script".len();
        let Some(&next) = lower.get(name_end) else { break };
        if !matches!(next, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>') {
            cursor = name_end;
            continue;
        }
        let Some(tag_end) = start_tag_end(&lower, name_end) else { break };
        let text_start = tag_end + 1;
        let text_end = find(&lower[text_start..], b"</script").map_or(lower.len(), |at| text_start + at);
        let text = &html[text_start..text_end];
        if !text.is_empty() {
            let hash = format!("'sha256-{}'", encode_base64(&Sha256::digest(normalize_newlines(text))));
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
        cursor = text_end;
    }
    hashes
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn start_tag_end(lower: &[u8], from: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, &byte) in lower[from..].iter().enumerate() {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(from + offset),
            None => {}
        }
    }
    None
}

fn normalize_newlines(text: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(text.len());
    let mut bytes = text.iter().peekable();
    while let Some(&byte) = bytes.next() {
        if byte == b'\r' {
            output.push(b'\n');
            if bytes.peek() == Some(&&b'\n') {
                bytes.next();
            }
        } else {
            output.push(byte);
        }
    }
    output
}

/// Whether a request's `Origin` belongs to an app rather than to the editor
/// host. A deployment hostname always counts. Any other host under the app
/// suffix counts too unless it is the installation's public or MCP host, or a
/// local editor name on a local installation, so a deleted app's cached page
/// gains nothing either.
pub async fn from_app_origin(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(host) = url::Url::parse(origin).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)) else {
        return false;
    };
    let configured = |value: &str| url::Url::parse(value).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    let public = configured(&state.public_origin);
    let own = state.own_origins.iter().filter_map(|origin| configured(origin)).any(|own| own == host);
    // A stored record on an installation host does not make that host an app.
    let installation = public.as_deref() == Some(host.as_str()) || own;
    if installation {
        return false;
    }
    if state.deployments.read().await.contains_key(&host) || super::domain::claimed(state, &host) {
        return true;
    }
    // The path host is `<domain>`, which holds the path addresses and the home.
    // With `KONAE_DOMAIN=localhost` it is the bare `localhost`, so it is
    // an app origin before the local-editor names below are checked.
    if super::path::path_host(state).as_deref() == Some(host.as_str()) {
        return true;
    }
    let local = public.as_deref().is_some_and(is_local_host);
    if local && ["localhost", "127.0.0.1", "[::1]", "app.localhost"].contains(&host.as_str()) {
        return false;
    }
    let suffix = state.host_suffix.trim_start_matches('.').to_ascii_lowercase();
    !suffix.is_empty() && host.ends_with(&format!(".{suffix}"))
}

/// Adds the isolation headers to every response.
pub async fn headers(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let deployment = deployment_for_request(&state, request.headers(), request.uri().path()).await;
    let app = deployment.is_some();
    let connects = deployment.map(|deployment| super::calls::connects(&deployment.manifest)).unwrap_or_default();
    let hsts = hsts(&state, request.headers());
    let response = next.run(request).await;
    let (mut parts, body) = response.into_parts();
    let html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"));
    // A 304 has no body to hash, and a browser replaces the stored headers
    // with the ones a 304 sends, so a policy here would drop the page's
    // inline-script hashes. The page keeps the policy it arrived with.
    let body = if parts.headers.contains_key(header::CONTENT_SECURITY_POLICY) || parts.status == StatusCode::NOT_MODIFIED {
        body
    } else if html {
        let (hashes, body) = match axum::body::to_bytes(body, MAX_HASHED_HTML).await {
            Ok(bytes) => (inline_script_hashes(&bytes), Body::from(bytes)),
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Page is too large to serve."),
        };
        let policy = if app { app_policy(&hashes, &connects) } else { platform_page_policy(&hashes) };
        parts.headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&policy).unwrap());
        body
    } else {
        let policy = if app { app_policy(&[], &connects) } else { PLATFORM_POLICY.to_owned() };
        parts.headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&policy).unwrap());
        body
    };
    parts.headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    parts.headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Some(hsts) = hsts {
        parts.headers.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(hsts));
    }
    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(text: &str) -> String {
        format!("'sha256-{}'", encode_base64(&Sha256::digest(text.as_bytes())))
    }

    /// A browser keeps the headers a 304 sends in place of the stored ones,
    /// so a 304 that carried a policy without the page's inline-script hashes
    /// broke every published page on its second visit.
    #[tokio::test]
    async fn a_not_modified_answer_carries_no_policy() {
        let root = std::env::temp_dir().join(format!("konae-security-304-{}", uuid::Uuid::new_v4()));
        let (state, visitor) = crate::tests::login_test_state(&root, "public").await;
        let app = axum::Router::new()
            .route("/fresh", axum::routing::get(|| async { ([(header::CONTENT_TYPE, "text/html")], "<script>window.a=1</script>") }))
            .route("/same", axum::routing::get(|| async { StatusCode::NOT_MODIFIED }))
            .layer(axum::middleware::from_fn_with_state(state.clone(), headers))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let host = visitor[header::HOST].to_str().unwrap().to_owned();
        let client = reqwest::Client::new();
        let fresh = client.get(format!("http://{address}/fresh")).header("host", &host).send().await.unwrap();
        let policy = fresh.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_owned();
        assert!(policy.contains(&sha("window.a=1")), "{policy}");
        let same = client.get(format!("http://{address}/same")).header("host", &host).send().await.unwrap();
        assert_eq!(same.status(), StatusCode::NOT_MODIFIED);
        assert!(same.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_stored_alias_on_an_installation_host_is_not_an_app_origin() {
        let root = std::env::temp_dir().join(format!(
            "konae-security-installation-alias-{}",
            uuid::Uuid::new_v4()
        ));
        let (base, _) = crate::tests::login_test_state(&root, "public").await;
        let state = AppState {
            public_origin: "https://editor.apps.example.com".into(),
            own_origins: vec!["https://mcp.apps.example.com".into()],
            host_suffix: "apps.example.com".into(),
            ..base
        };
        let id = "legacy";
        tokio::fs::create_dir_all(crate::layout::private(&root, id))
            .await
            .unwrap();
        tokio::fs::write(
            crate::layout::private(&root, id).join(super::super::domain::RECORD_FILE),
            serde_json::to_vec(&super::super::domain::Record {
                hostname: "editor.apps.example.com".into(),
                token: "legacy-token".into(),
                verified: true,
                verified_at: Some(1),
                created_at: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://editor.apps.example.com"),
        );
        assert!(!from_app_origin(&state, &headers).await);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn hashes_each_inline_script_and_skips_external_ones() {
        let html = br#"<!doctype html><head><style>p{}</style></head><body><p>hi</p><script data-konae-runtime>window.a=1</script><script type="module">import x from './x.js'</script><script src="/runtime.js"></script></body>"#;
        assert_eq!(
            inline_script_hashes(html),
            vec![sha("window.a=1"), sha("import x from './x.js'")]
        );
    }

    #[test]
    fn a_document_without_inline_scripts_gets_no_hash() {
        let html = br#"<!doctype html><script src="/runtime.js"></script><script type="module" src="shell.js"></script><main>Docs</main>"#;
        assert!(inline_script_hashes(html).is_empty());
        assert!(app_policy(&[], &[]).contains("script-src 'self';"));
    }

    #[test]
    fn the_scan_matches_the_browser_on_case_quotes_and_newlines() {
        let html = b"<SCRIPT title=\"a>b\">one\r\ntwo\rthree</SCRIPT><scripts>no</scripts><script\n>four</script >";
        assert_eq!(inline_script_hashes(html), vec![sha("one\ntwo\nthree"), sha("four")]);
    }

    #[test]
    fn a_script_string_inside_a_script_does_not_open_a_second_script() {
        let html = br#"<script>const tag = "<script>"; run(tag)</script>"#;
        assert_eq!(inline_script_hashes(html), vec![sha(r#"const tag = "<script>"; run(tag)"#)]);
    }

    #[test]
    fn an_unterminated_script_hashes_to_the_end_of_the_document() {
        assert_eq!(inline_script_hashes(b"<script>tail"), vec![sha("tail")]);
        assert!(inline_script_hashes(b"<script").is_empty());
        assert!(inline_script_hashes(b"<script type='x").is_empty());
    }

    #[test]
    fn app_policy_blocks_inline_handlers_framing_and_foreign_connections() {
        let policy = app_policy(&[sha("x")], &[]);
        assert!(policy.starts_with("default-src 'self'; script-src 'self' 'sha256-"));
        assert!(!policy.contains("'unsafe-inline' 'sha256"), "script-src must not allow inline script");
        assert!(!policy.contains("unsafe-eval"));
        for directive in ["connect-src 'self';", "frame-ancestors 'none';", "object-src 'none';", "base-uri 'none'", "img-src 'self' blob: data:;"] {
            assert!(policy.contains(directive), "{directive} missing from {policy}");
        }
        assert!(platform_page_policy(&[]).contains("frame-ancestors 'none'"));
        assert!(PLATFORM_POLICY.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn connects_adds_its_origins_to_connect_src_and_nothing_else() {
        let policy = app_policy(&[], &["https://api.example.com".into(), "https://inbox-1.apps.example.org".into()]);
        assert!(policy.contains("connect-src 'self' https://api.example.com https://inbox-1.apps.example.org;"), "{policy}");
        assert!(policy.contains("default-src 'self';") && policy.contains("form-action 'self';"));
    }
}
