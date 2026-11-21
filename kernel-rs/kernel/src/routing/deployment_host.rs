use axum::http::{HeaderMap, header};

use crate::{AppState, Deployment, deployment_by_id, domain, path};

/// The Deployment a request is for. A generated hostname and a verified
/// alias answer first, as before. Otherwise the request is on the path host,
/// and `path::route` has put the Deployment it resolved in `RESOLVED_HEADER`,
/// which the client cannot set because the route removes it first.
pub async fn deployment_for(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
    if let Some(deployment) = deployment_for_host(state, headers).await {
        return Some(deployment);
    }
    let id = headers.get(path::RESOLVED_HEADER)?.to_str().ok()?;
    deployment_by_id(state, id).await
}

/// The Deployment for a generated hostname or a verified alias. The
/// installation's own hosts answer as no app, whatever the map holds, so a
/// stored record on one of them cannot take the owner door.
pub async fn deployment_for_host(state: &AppState, headers: &HeaderMap) -> Option<Deployment> {
    let host = headers
        .get(header::HOST)?
        .to_str()
        .ok()?
        .split(':')
        .next()?;
    if domain::Rules::of(state).installation_hosts.iter().any(|own| own.eq_ignore_ascii_case(host.trim_end_matches('.'))) {
        return None;
    }
    let deployments = state.deployments.read().await;
    if let Some(deployment) = deployments.get(host) {
        return Some(deployment.clone());
    }

    let id = domain::serving(state, host)?;
    deployments.values().find(|deployment| deployment.id == id).cloned()
}

/// The Deployment a request answers, before `path::route` strips its prefix.
/// The security headers layer runs outside the router and reads the path itself.
pub async fn deployment_for_request(state: &AppState, headers: &HeaderMap, path: &str) -> Option<Deployment> {
    if let Some(deployment) = deployment_for_host(state, headers).await {
        return Some(deployment);
    }
    let id = path::resolve(state, headers, path).await?.deployment_id?;
    deployment_by_id(state, &id).await
}
