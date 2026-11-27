
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

mod conversations;
mod editor;
mod guide;
mod mcp;
mod owners;
mod projects;
mod state;
use state::Konae;

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(message) = renamed_command(std::env::args().nth(1).as_deref()) {
        eprintln!("{message}");
        std::process::exit(1);
    }
    if owners::clerk::operator_command()? { return Ok(()); }
    let installation = mcp::installation();
    if kernel::inventory::command("konae", &installation)? { return Ok(()); }
    if kernel::operator_command(&installation).await? { return Ok(()); }
    if std::env::args().nth(1).as_deref() == Some("queues") && kernel::operator::command(&installation).await? { return Ok(()); }
    if owners::operator_command()? { return Ok(()); }
    if owners::admission::operator_command()? { return Ok(()); }
    refuse_old_setup_key(
        std::env::var("KONAE_ADMIN_KEY").ok().as_deref(),
        std::env::var("KONAE_SETUP_KEY").ok().as_deref(),
    )?;
    let startup = Startup::from_env()?;
    let mcp_origin = mcp_origin(|name| std::env::var(name).ok(), &startup.hosts)?;
    let setup_key = setup_key_for(
        &startup.bind,
        &startup.hosts.public_origin,
        mcp_origin.as_deref(),
        std::env::var("KONAE_SETUP_KEY").ok().as_deref(),
        std::env::var("KONAE_ALLOW_DEFAULT_KEY").as_deref() == Ok("1"),
    )?;
    let lock = kernel::file_store::hold_kernel_lock(&startup.data_root)?;
    owners::recover_suspensions(&startup.data_root)?;
    let audit = mcp::runtime_audit(startup.data_root.clone());
    let mut running = kernel::start_locked(&startup, lock, installation, &setup_key, mcp_origin.iter().cloned().collect(), Some(audit)).await?;
    let konae = Konae {
        kernel: running.state.clone(),
        setup_key,
        mcp_origin,
        owner_verification: auth::Verification::from_env().map_err(anyhow::Error::msg)?,
    };
    owners::creator_store(&konae)?;
    running.start_copies();
    let editor = editor::Editor::from_env(&running.state.public_origin, security::https(&running.state))?;
    let listener = kernel::bind(&startup).await?;
    println!("Konae listening on {}", startup.port);
    if editor.is_some() {
        println!("Serving the editor at {}", running.state.public_origin);
    }
    let service = with_editor(kernel::stack(running.state.clone(), owner_routes(konae)), editor);
    axum::serve(listener, axum::ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(service)).await?;
    Ok(())
}

fn owner_routes(konae: Konae) -> Router<AppState> {
    Router::<Konae>::new()
        .merge(conversations::routes())
        .merge(mcp::routes())
        .merge(owners::oauth::routes())
        .merge(owners::routes())
        .merge(owners::owner_page::routes())
        .with_state(konae)
}

type Stack = tower::util::BoxCloneSyncService<Request, Response, std::convert::Infallible>;

fn with_editor(stack: Stack, editor: Option<editor::Editor>) -> Stack {
    match editor {
        Some(editor) => Stack::new(
            tower::ServiceBuilder::new()
                .layer(axum::middleware::from_fn_with_state(Arc::new(editor), editor::serve))
                .service(stack),
        ),
        None => stack,
    }
}

fn mcp_origin(get: impl Fn(&str) -> Option<String>, hosts: &hosts::Hosts) -> Result<Option<String>> {
    let Some(domain) = hosts.domain.as_deref() else {
        return Ok(get("KONAE_MCP_ORIGIN"));
    };
    let origin = hosts::non_empty(&get, "KONAE_MCP_ORIGIN").unwrap_or_else(|| hosts.public_origin.clone());
    hosts::refuse_on_domain("KONAE_MCP_ORIGIN", &origin, domain)?;
    Ok(Some(origin))
}

const RENAMED_COMMANDS: [(&str, &str); 6] = [
    ("creator-code-create", "owner-code-create"),
    ("creator-code-list", "owner-code-list"),
    ("creator-code-revoke", "owner-code-revoke"),
    ("creator-account-suspend", "owner-account-suspend"),
    ("creator-account-resume", "owner-account-resume"),
    ("creator-suspend", "owner-suspend"),
];

fn renamed_command(name: Option<&str>) -> Option<String> {
    let (old, new) = RENAMED_COMMANDS.iter().find(|(old, _)| Some(*old) == name)?;
    Some(format!("{old} is now {new}."))
}

fn refuse_old_setup_key(old: Option<&str>, new: Option<&str>) -> Result<()> {
    let set = |value: Option<&str>| value.is_some_and(|value| !value.is_empty());
    if set(old) && !set(new) {
        anyhow::bail!("KONAE_ADMIN_KEY is now KONAE_SETUP_KEY. Set KONAE_SETUP_KEY to the same value and remove KONAE_ADMIN_KEY.");
    }
    Ok(())
}

fn setup_key_for(
    bind: &str,
    public_origin: &str,
    mcp_origin: Option<&str>,
    configured: Option<&str>,
    allow_default: bool,
) -> Result<String> {
    let key = configured
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SETUP_KEY);
    if key != DEFAULT_SETUP_KEY || allow_default {
        return Ok(key.to_owned());
    }
    if !is_local_host(bind) {
        anyhow::bail!(
            "KONAE_BIND={bind} is reachable from other machines. Set KONAE_SETUP_KEY to a secret value before starting the Kernel."
        );
    }
    let origin_host = url::Url::parse(public_origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    if !origin_host.as_deref().is_some_and(is_local_host) {
        anyhow::bail!(
            "KONAE_PUBLIC_ORIGIN={public_origin} is not a local address. Set KONAE_SETUP_KEY to a secret value before starting the Kernel."
        );
    }
    if let Some(mcp_origin) = mcp_origin {
        let mcp_host = url::Url::parse(mcp_origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        if !mcp_host.as_deref().is_some_and(is_local_host) {
            anyhow::bail!(
                "KONAE_MCP_ORIGIN={mcp_origin} is not a local address. Set KONAE_SETUP_KEY to a secret value before starting the Kernel."
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

    #[tokio::test]
    async fn konae_names_a_taken_port_in_its_bind_error() {
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = held.local_addr().unwrap().port();
        let startup = kernel::Startup { port, bind: "127.0.0.1".into(), hosts: kernel::hosts::resolve(|_| None).unwrap(), data_root: std::path::PathBuf::new(), execution_mode: kernel::execution::ExecutionMode::Live };
        let message = kernel::bind(&startup).await.unwrap_err().to_string();
        assert!(message.contains(&format!("Port {port} on 127.0.0.1 is already in use")), "{message}");
        assert!(message.contains("KONAE_KERNEL_PORT"), "{message}");
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
    fn a_renamed_command_names_its_new_name() {
        assert_eq!(renamed_command(Some("creator-code-create")).as_deref(), Some("creator-code-create is now owner-code-create."));
        for (old, new) in RENAMED_COMMANDS {
            assert_eq!(renamed_command(Some(old)), Some(format!("{old} is now {new}.")));
        }
        assert_eq!(renamed_command(Some("owner-code-create")), None);
        assert_eq!(renamed_command(None), None);
    }

    #[test]
    fn the_old_setup_key_name_refuses_to_start_unless_the_new_one_is_set() {
        let message = refuse_old_setup_key(Some("s3cret"), None).unwrap_err().to_string();
        assert_eq!(message, "KONAE_ADMIN_KEY is now KONAE_SETUP_KEY. Set KONAE_SETUP_KEY to the same value and remove KONAE_ADMIN_KEY.");
        assert!(refuse_old_setup_key(Some("s3cret"), Some("")).is_err());
        assert!(refuse_old_setup_key(Some("s3cret"), Some("s3cret")).is_ok());
        assert!(refuse_old_setup_key(Some(""), None).is_ok());
        assert!(refuse_old_setup_key(None, None).is_ok());
    }

    #[test]
    fn default_setup_key_is_refused_off_loopback() {
        let local = "http://localhost:8080";
        for bind in ["127.0.0.1", "::1", "[::1]", "localhost"] {
            assert_eq!(setup_key_for(bind, local, None, None, false).unwrap(), DEFAULT_SETUP_KEY, "bind={bind}");
        }
        assert!(setup_key_for("0.0.0.0", local, None, None, false).is_err());
        assert!(setup_key_for("0.0.0.0", local, None, Some(""), false).is_err());
        assert!(setup_key_for("0.0.0.0", local, None, Some(DEFAULT_SETUP_KEY), false).is_err());
        assert!(setup_key_for("192.168.1.4", local, None, Some(DEFAULT_SETUP_KEY), false).is_err());
        assert_eq!(setup_key_for("0.0.0.0", local, None, Some("s3cret"), false).unwrap(), "s3cret");
        assert_eq!(
            setup_key_for("0.0.0.0", local, None, Some(DEFAULT_SETUP_KEY), true).unwrap(),
            DEFAULT_SETUP_KEY
        );
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_origin() {
        for origin in ["http://localhost:8790", "http://127.0.0.1:18900", "http://[::1]:8080", "http://app.localhost"] {
            assert!(setup_key_for("127.0.0.1", origin, None, None, false).is_ok(), "origin={origin}");
        }
        for origin in ["https://apps.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(setup_key_for("127.0.0.1", origin, None, None, false).is_err(), "origin={origin}");
            assert!(setup_key_for("127.0.0.1", origin, None, None, true).is_ok(), "origin={origin}");
            assert!(setup_key_for("127.0.0.1", origin, None, Some("s3cret"), false).is_ok(), "origin={origin}");
        }
    }

    #[test]
    fn default_setup_key_is_refused_behind_a_public_mcp_origin() {
        let local = "http://localhost:8080";
        for mcp in ["http://localhost:8790", "http://127.0.0.1:9000", "http://app.localhost"] {
            assert!(setup_key_for("127.0.0.1", local, Some(mcp), None, false).is_ok(), "mcp={mcp}");
        }
        for mcp in ["https://mcp.example.com", "http://192.168.1.4:8080", "not a url", ""] {
            assert!(setup_key_for("127.0.0.1", local, Some(mcp), None, false).is_err(), "mcp={mcp}");
            assert!(setup_key_for("127.0.0.1", local, Some(mcp), None, true).is_ok(), "mcp={mcp}");
            assert!(setup_key_for("127.0.0.1", local, Some(mcp), Some("s3cret"), false).is_ok(), "mcp={mcp}");
        }
    }
}
