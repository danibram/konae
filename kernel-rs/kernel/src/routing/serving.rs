use std::sync::Arc;

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{
    AppState, Deployment, SignupMode, SignupPolicy, artifact, auth, authenticate_deployment,
    content_type, deployment_for, enter, error, google, http, is_private, layout, modules,
    own_page, safe_relative_path, seo, usage,
};

pub async fn serve_root(State(state): State<AppState>, headers: HeaderMap) -> Response {
    serve_key(state, headers, "index.html".into()).await
}

pub async fn login_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Konae deployment not found.");
    };
    if !modules::is_on(&deployment.modules, "auth") {
        return error(StatusCode::NOT_FOUND, "Authentication is not enabled for this app.");
    }
    match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(Some(_)) => (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response(),
        Ok(None) => private_login_page(&deployment.project_name, &deployment.signup_policy, state.accounts.verification, google::offered_here(&state, &deployment, &headers)),
        Err(response) => response,
    }
}

pub async fn serve_artifact(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    serve_key(
        state,
        headers,
        if path.is_empty() {
            "index.html".into()
        } else {
            path
        },
    )
    .await
}

pub async fn serve_key(state: AppState, headers: HeaderMap, path: String) -> Response {
    serve_key_with_routes(state, headers, path, None).await
}

pub(crate) async fn serve_key_with_routes(
    state: AppState,
    headers: HeaderMap,
    path: String,
    routes: Option<Arc<http::RouteTable>>,
) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Konae deployment not found."),
    };
    if is_private(&deployment) && path == "robots.txt" {
        return ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "User-agent: *\nAllow: /\n").into_response();
    }
    if is_private(&deployment) {
        match authenticate_deployment(&state, &deployment, &headers).await {
            Ok(Some(_)) => {}
            Ok(None) => return private_login_page(&deployment.project_name, &deployment.signup_policy, state.accounts.verification, google::offered_here(&state, &deployment, &headers)),
            Err(response) => return response,
        }
    }
    if artifact::kernel_path(&path) {
        return artifact::not_found_page();
    }
    let seo_on = modules::is_on(&deployment.modules, "seo");
    let address = seo_on.then(|| seo::address(&state, &deployment));
    let cache = ArtifactCache::of(&deployment);




    let navigation = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("document"));
    let routing = artifact::routing(&deployment.manifest);
    let html_candidate = artifact::candidates(&path, routing).iter().any(|candidate| {
        candidate.to_ascii_lowercase().ends_with(".html") || candidate.to_ascii_lowercase().ends_with(".htm")
    });
    let spa_fallback = seo_on && routing == artifact::Routing::Spa
        && (!safe_relative_path(&path) || tokio::fs::metadata(crate::layout::dist(&state.data_root, &deployment.id).join(&path)).await.is_err());
    let may_be_page = seo_on && (html_candidate || spa_fallback || path == "robots.txt" || path == "sitemap.xml");
    if !navigation && etag_matches(&headers, &cache.version_tag()) && (!seo_on || !may_be_page) {
        return cache.not_modified(cache.version_tag());
    }
    if seo_on && path == "robots.txt" {
        return seo::robots_response(&deployment, &path, read_artifact(&state, &deployment, &path).await.ok(), address.as_ref().unwrap(), &headers, &cache, navigation).unwrap();
    }
    if seo_on && path == "sitemap.xml" {
        return seo::sitemap_response(&state, &deployment, &path, read_artifact(&state, &deployment, &path).await.ok(), address.as_ref().unwrap(), &headers, &cache).await.unwrap();
    }
    for candidate in artifact::candidates(&path, routing) {
        if let Ok(bytes) = read_artifact(&state, &deployment, &candidate).await {
            count_page_view(&state, &deployment, &headers, &candidate).await;
            if seo_on {
                if let Some(response) = seo::html_response(&deployment, &candidate, &bytes, address.as_ref().unwrap(), &headers, &cache) {
                    return response;
                }
            }
            return cache.respond(&headers, bytes, &candidate);
        }
    }

    if let Some(routes) = routes.as_ref() {
        let page_path = format!("/{}", path.trim_start_matches('/'));
        if let Some(response) = http::serve_page_path(&state, &deployment, routes, &page_path, &headers).await {
            return response;
        }
    }


    if path == "favicon.ico" {
        return StatusCode::NO_CONTENT.into_response();
    }
    match routing {
        artifact::Routing::Spa => match read_artifact(&state, &deployment, "index.html").await {
            Ok(bytes) => {
                count_page_view(&state, &deployment, &headers, "index.html").await;
                let mut response = cache.respond(&headers, bytes, "index.html");
                if seo_on { response.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex")); }
                response
            }
            Err(_) => error(StatusCode::NOT_FOUND, "Artifact not found."),
        },
        artifact::Routing::Pages => match read_artifact(&state, &deployment, "404.html").await {
            Ok(bytes) => {
                let mut response = artifact_response(bytes, "404.html");
                *response.status_mut() = StatusCode::NOT_FOUND;
                response
            }
            Err(_) => artifact::not_found_page(),
        },
    }
}

/// Counts a page view when a browser navigates to one of the artifact's HTML
/// pages. Only a navigation carries `Sec-Fetch-Dest: document`, so assets, a
/// script's fetch, a HEAD request, curl and most crawlers count nothing, and a
/// prefetch is skipped. The name is the file served, never the address asked
/// for: the artifact bounds the names, so no visitor can grow the store by
/// inventing paths. A single-page app's client-side navigation is invisible
/// here, and a view is a load, not a person (ADR-0010, page views).
pub async fn count_page_view(state: &AppState, deployment: &Deployment, headers: &HeaderMap, file: &str) {
    let lower = file.to_ascii_lowercase();
    if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
        return;
    }
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or("");
    if !header("sec-fetch-dest").eq_ignore_ascii_case("document") {
        return;
    }
    if header("sec-purpose").contains("prefetch") || header("purpose").contains("prefetch") {
        return;
    }
    if headers.contains_key(header::COOKIE) && enter::entered_request(state, deployment, headers).await {
        return;
    }
    usage::count(&state.data_root, &deployment.id, usage::Operation::PageView(file.to_owned()));
}

pub async fn read_artifact(state: &AppState, deployment: &Deployment, path: &str) -> Result<Vec<u8>> {
    if !safe_relative_path(path) {
        anyhow::bail!("invalid artifact path");
    }
    let local = layout::dist(&state.data_root, &deployment.id).join(path);
    Ok(tokio::fs::read(local).await?)
}

pub fn private_login_page(app: &str, policy: &SignupPolicy, verification: auth::Verification, google: bool) -> Response {
    let verified = verification == auth::Verification::Required;
    let (note, create) = match policy.mode {
        SignupMode::Open if verified => (
            "New here? Enter your email and create an account. We send a link to choose your password.",
            true,
        ),
        SignupMode::Open => (
            "New here? Enter an email and password, then create an account.",
            true,
        ),
        SignupMode::Allowlist => (
            "Only invited email addresses can create an account.",
            true,
        ),
        SignupMode::Invitation => (
            "Only invited people can create an account. Open the link in your invitation email.",
            false,
        ),
        SignupMode::Closed => (
            "This app is not accepting new accounts. Sign in with an existing account.",
            false,
        ),
    };
    let app = own_page::escape(app);
    let body = [
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in · "#,
        &app,
        r#"</title><style>"#,
        own_page::CSS,
        r#"</style><link rel="icon" href="data:,"><main><p class="kicker">"#,
        &app,
        r#"</p><h1>Sign in</h1><p>"#,
        note,
        "</p>",
        if google { google::BUTTON } else { "" },
        r#"<form id="auth"><label>Email <input name="email" type="email" required autocomplete="email"></label><label>Password <input name="password" type="password" minlength="8" required autocomplete="current-password"></label><button formaction="/_konae/auth/login">Sign in</button>"#,
        if create && verified {
            r#"<button formaction="/_konae/auth/signup" formnovalidate>Create account</button>"#
        } else if create {
            r#"<button formaction="/_konae/auth/signup">Create account</button>"#
        } else {
            ""
        },
        r#"<p id="error" role="alert"></p><a id="forgot" href="/_konae/account">Forgot your password?</a></form></main><script>{const m=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);const prefix=m?'/p/'+m[1]:'';document.querySelector('#forgot').href=prefix+'/_konae/account';document.querySelector('#auth').addEventListener('submit',async e=>{e.preventDefault();const path=prefix+new URL(e.submitter?.formAction||'/_konae/auth/login',location.href).pathname;const form=new FormData(e.currentTarget);const response=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({email:form.get('email'),password:form.get('password')})});const body=await response.json().catch(()=>({}));if(response.status===202){document.querySelector('#error').textContent=body.result.message;return}if(response.ok){location.replace(prefix+'/');return}document.querySelector('#error').textContent=body.error||'Sign in failed.'})}</script></html>"#,
    ]
    .concat();
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

/// How a browser may keep a Deployment's files. Every file is stored but
/// checked on each use (`no-cache`), so a publish shows at once and an
/// unchanged file costs a 304. A private app's files are `private` so no shared
/// cache holds a page that sits behind its login. File names are not
/// guaranteed to carry a content hash, so nothing is marked `immutable`.
pub struct ArtifactCache {
    pub version: String,
    pub private: bool,
}

impl ArtifactCache {
    pub fn of(deployment: &Deployment) -> Self {
        Self { version: deployment.artifact_version.clone(), private: is_private(deployment) }
    }

    pub fn version_tag(&self) -> String {
        format!("\"{}\"", self.version)
    }

    pub fn control(&self) -> HeaderValue {
        HeaderValue::from_static(if self.private { "private, no-cache" } else { "public, no-cache" })
    }

    pub fn not_modified(&self, tag: String) -> Response {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&tag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response.headers_mut().insert(header::CACHE_CONTROL, self.control());
        response
    }

    pub fn respond(&self, headers: &HeaderMap, bytes: Vec<u8>, path: &str) -> Response {
        let tag = self.version_tag();
        if etag_matches(headers, &tag) {
            return self.not_modified(tag);
        }
        let mut response = artifact_response(bytes, path);
        if let Ok(value) = HeaderValue::from_str(&tag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response.headers_mut().insert(header::CACHE_CONTROL, self.control());
        response
    }
}

/// `If-None-Match` holds a list of tags, possibly weak. A browser never sends
/// `*` on a GET, so it matches nothing here.
pub fn etag_matches(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|candidate| candidate.trim())
        .any(|candidate| candidate.strip_prefix("W/").unwrap_or(candidate) == tag)
}

pub fn artifact_response(bytes: Vec<u8>, path: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
