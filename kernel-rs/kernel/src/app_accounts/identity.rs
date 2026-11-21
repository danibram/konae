use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;

use crate::auth::{AuthError, AuthService, AuthUser};
use crate::{AppState, Deployment, accounts, auth, error, http, layout, named_lock, path};

pub fn open_auth(
    state: &AppState,
    deployment: &Deployment,
) -> std::result::Result<AuthService, AuthError> {
    accounts::app_store(state, deployment)
}

pub async fn authenticate_deployment(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
) -> std::result::Result<Option<AuthUser>, Response> {
    let token = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(headers, &app_cookie_name(state, &deployment.id, headers)));
    let Some(token) = token.filter(|token| !token.is_empty() && token.len() <= 128) else {
        return Ok(None);
    };
    let path = layout::identity(&state.data_root, &deployment.id);
    let scope = auth::deployment_scope(&deployment.id);
    let verification = state.accounts.verification;
    runtime_identity(state, &deployment.id, move || auth::runtime_session(&path, &scope, verification, &token))
        .await?
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// The blocking task owns the lock until SQLite closes or returns its connection.
pub(crate) async fn runtime_identity<T: Send + 'static>(
    state: &AppState,
    deployment: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, Response> {
    let guard = named_lock(state, &format!("identity:{deployment}"), false).await;
    let permit = state.guards.statement_permit(deployment).await.map_err(http::rate_limited)?;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _permit = permit;
        work()
    })
    .await
    .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// The app session cookie's name for a request to a Deployment.
///
/// A generated hostname and a verified alias keep their own origin, so the
/// name is the one they always had, with `__Host-` under HTTPS. A path address
/// shares `<domain>` with every other app and the home, so the name carries
/// the Deployment id. The cookie is `Path=/` because one app answers on two
/// addresses, `/p/<id>` and `/p/<name>`, and a `Path=/p/<segment>/` cookie set
/// on one would not reach the other. A `__Host-` name also needs `Path=/`. The
/// per-app name keeps two apps' sessions from overwriting each other; it does
/// not keep a hostile app out, since every path shares one origin (ADR-0014).
pub fn app_cookie_name(state: &AppState, deployment_id: &str, headers: &HeaderMap) -> String {
    let https = state.public_origin.starts_with("https://");
    if path::on_path_host(state, headers) {
        return format!("{}konae_app_session_{deployment_id}", if https { "__Secure-" } else { "" });
    }
    if https { "__Host-konae_app_session" } else { "konae_app_session" }.to_owned()
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_owned))
}
