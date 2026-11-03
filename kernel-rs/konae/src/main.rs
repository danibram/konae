//! Konae: the Kernel with the owner's side. It links the `kernel` library
//! and adds the owner's MCP on `/mcp`, OAuth, Clerk, owner accounts and
//! admission, the guide, and the project list in `mcp.sqlite`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
#[cfg(test)]
use axum::body::Body;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use uuid::Uuid;

pub use kernel::*;
use kernel::auth::{AuthError, AuthService};
#[cfg(test)]
use kernel::limits::Guards;

mod admission;
mod clerk;
mod guide;
mod mcp;
mod oauth;
mod owners;
mod state;
use state::Konae;

#[tokio::main]
async fn main() -> Result<()> {
    if clerk::operator_command()? { return Ok(()); }
    let installation = mcp::installation();
    if kernel::operator_command(&installation).await? { return Ok(()); }
    if owners::operator_command()? { return Ok(()); }
    if admission::operator_command()? { return Ok(()); }
    let startup = Startup::from_env()?;
    let mcp_origin = mcp_origin(|name| std::env::var(name).ok(), &startup.hosts)?;
    let admin_key = admin_key_for(
        &startup.bind,
        &startup.hosts.public_origin,
        mcp_origin.as_deref(),
        std::env::var("KONAE_ADMIN_KEY").ok().as_deref(),
        std::env::var("KONAE_ALLOW_DEFAULT_KEY").as_deref() == Ok("1"),
    )?;
    let audit = mcp::runtime_audit(startup.data_root.clone());
    let running = kernel::start(&startup, installation, &admin_key, mcp_origin.iter().cloned().collect(), Some(audit)).await?;
    let konae = Konae {
        kernel: running.state.clone(),
        admin_key,
        mcp_origin,
        owner_verification: auth::Verification::from_env().map_err(anyhow::Error::msg)?,
    };
    // Apply the verification mode to creator accounts before serving, so a
    // switch to required revokes unverified sessions at startup.
    owners::creator_store(&konae)?;
    kernel::listen(&startup, running.state.clone(), owner_routes(konae), "Konae Rust Kernel").await
}

/// Konae's routes: the owner's MCP, OAuth and owner sign-in.
fn owner_routes(konae: Konae) -> Router<AppState> {
    Router::<Konae>::new()
        .merge(mcp::routes())
        .merge(oauth::routes())
        .merge(owners::routes())
        .with_state(konae)
}

/// `KONAE_MCP_ORIGIN`, or with `KONAE_DOMAIN` the public origin. `None` keeps
/// the request host for MCP when no domain is set. MCP may not answer on
/// `<domain>`, where the home and path addresses answer.
fn mcp_origin(get: impl Fn(&str) -> Option<String>, hosts: &hosts::Hosts) -> Result<Option<String>> {
    let Some(domain) = hosts.domain.as_deref() else {
        return Ok(get("KONAE_MCP_ORIGIN"));
    };
    let origin = hosts::non_empty(&get, "KONAE_MCP_ORIGIN").unwrap_or_else(|| hosts.public_origin.clone());
    hosts::refuse_on_domain("KONAE_MCP_ORIGIN", &origin, domain)?;
    Ok(Some(origin))
}

/// Returns the installation setup key. The well-known default only works on a
/// loopback bind with a local public origin, unless the operator opts in with
/// KONAE_ALLOW_DEFAULT_KEY=1. A proxy can expose a loopback bind, so the
/// public origin counts too.
fn admin_key_for(
    bind: &str,
    public_origin: &str,
    mcp_origin: Option<&str>,
    configured: Option<&str>,
    allow_default: bool,
) -> Result<String> {
    let key = configured
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ADMIN_KEY);
    if key != DEFAULT_ADMIN_KEY || allow_default {
        return Ok(key.to_owned());
    }
    if !is_local_host(bind) {
        anyhow::bail!(
            "KONAE_BIND={bind} is reachable from other machines. Set KONAE_ADMIN_KEY to a secret value before starting the Kernel."
        );
    }
    let origin_host = url::Url::parse(public_origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    if !origin_host.as_deref().is_some_and(is_local_host) {
        anyhow::bail!(
            "KONAE_PUBLIC_ORIGIN={public_origin} is not a local address. Set KONAE_ADMIN_KEY to a secret value before starting the Kernel."
        );
    }
    // MCP and owner sign-in also answer on this origin's host.
    if let Some(mcp_origin) = mcp_origin {
        let mcp_host = url::Url::parse(mcp_origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        if !mcp_host.as_deref().is_some_and(is_local_host) {
            anyhow::bail!(
                "KONAE_MCP_ORIGIN={mcp_origin} is not a local address. Set KONAE_ADMIN_KEY to a secret value before starting the Kernel."
            );
        }
    }
    Ok(key.to_owned())
}


#[cfg(test)]
mod startup_tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + Clone {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    fn origin_of(pairs: &[(&str, &str)]) -> Result<Option<String>> {
        let get = env(pairs);
        mcp_origin(get.clone(), &hosts::resolve(get)?)
    }

    #[test]
    fn the_mcp_origin_follows_the_domain_and_its_own_variable() {
        assert_eq!(origin_of(&[]).unwrap(), None);
        assert_eq!(origin_of(&[("KONAE_MCP_ORIGIN", "https://control.example.com")]).unwrap().as_deref(), Some("https://control.example.com"));
        assert_eq!(origin_of(&[("KONAE_DOMAIN", "localhost"), ("KONAE_PUBLIC_PORT", "8080")]).unwrap().as_deref(), Some("http://app.localhost:8080"));
        assert_eq!(origin_of(&[("KONAE_DOMAIN", "konae.es")]).unwrap().as_deref(), Some("https://app.konae.es"));
        assert_eq!(origin_of(&[("KONAE_DOMAIN", "konae.es"), ("KONAE_MCP_ORIGIN", "https://mcp.example.org")]).unwrap().as_deref(), Some("https://mcp.example.org"));
        assert_eq!(origin_of(&[("KONAE_DOMAIN", "konae.es"), ("KONAE_PUBLIC_ORIGIN", "https://home.example.org")]).unwrap().as_deref(), Some("https://home.example.org"));
        assert!(origin_of(&[("KONAE_DOMAIN", "konae.es"), ("KONAE_MCP_ORIGIN", "https://konae.es")]).is_err());
    }

    /// A code Konae names in its source is a row of the Kernel's table.
    #[test]
    fn every_code_konae_names_is_in_the_kernel_table() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for part in source.split("refuse(\"").skip(1) {
                let code = &part[..part.find('"').unwrap()];
                assert!(refusal::table().contains_key(code), "{} names {code}, which refusal-codes.json does not hold", path.display());
            }
        }
    }

    #[test]
    fn default_setup_key_is_refused_off_loopback() {
        let local = "http://localhost:8080";
        for bind in ["127.0.0.1", "::1", "[::1]", "localhost"] {
            assert_eq!(admin_key_for(bind, local, None, None, false).unwrap(), DEFAULT_ADMIN_KEY, "bind={bind}");
        }
        assert!(admin_key_for("0.0.0.0", local, None, None, false).is_err());
        assert!(admin_key_for("0.0.0.0", local, None, Some(""), false).is_err());
        assert!(admin_key_for("0.0.0.0", local, None, Some(DEFAULT_ADMIN_KEY), false).is_err());
        assert!(admin_key_for("192.168.1.4", local, None, Some(DEFAULT_ADMIN_KEY), false).is_err());
        assert_eq!(admin_key_for("0.0.0.0", local, None, Some("s3cret"), false).unwrap(), "s3cret");
        assert_eq!(
            admin_key_for("0.0.0.0", local, None, Some(DEFAULT_ADMIN_KEY), true).unwrap(),
            DEFAULT_ADMIN_KEY
        );
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_origin() {
        for origin in ["http://localhost:8790", "http://127.0.0.1:18900", "http://[::1]:8080", "http://app.localhost"] {
            assert!(admin_key_for("127.0.0.1", origin, None, None, false).is_ok(), "origin={origin}");
        }
        for origin in ["https://apps.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(admin_key_for("127.0.0.1", origin, None, None, false).is_err(), "origin={origin}");
            assert!(admin_key_for("127.0.0.1", origin, None, None, true).is_ok(), "origin={origin}");
            assert!(admin_key_for("127.0.0.1", origin, None, Some("s3cret"), false).is_ok(), "origin={origin}");
        }
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_mcp_origin() {
        let local = "http://localhost:8080";
        for mcp in ["http://localhost:8790", "http://127.0.0.1:9000", "http://app.localhost"] {
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, false).is_ok(), "mcp={mcp}");
        }
        for mcp in ["https://mcp.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, false).is_err(), "mcp={mcp}");
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), None, true).is_ok(), "mcp={mcp}");
            assert!(admin_key_for("127.0.0.1", local, Some(mcp), Some("s3cret"), false).is_ok(), "mcp={mcp}");
        }
    }
}
