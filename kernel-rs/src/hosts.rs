//! The hosts an installation answers on (ADR-0024).
//!
//! `LITTLEBOX_DOMAIN` derives the public origin, the MCP origin and the app
//! suffix. Each older variable still wins when it is set. Without the domain
//! the defaults are the ones the Kernel used before the domain existed.

use anyhow::{bail, Result};

const LEGACY_PUBLIC_ORIGIN: &str = "http://localhost:8080";
const LEGACY_HOST_SUFFIX: &str = "localhost";

/// The origins and suffix one installation serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hosts {
    /// `LITTLEBOX_PUBLIC_ORIGIN`, or its derived value.
    pub(crate) public_origin: String,
    /// `LITTLEBOX_MCP_ORIGIN`. `None` keeps the request host for MCP when no
    /// domain is set, as before.
    pub(crate) mcp_origin: Option<String>,
    /// `LITTLEBOX_HOST_SUFFIX`, or its derived value.
    pub(crate) host_suffix: String,
    /// `<scheme>://<domain>` with the port, where a path address answers.
    /// `None` without `LITTLEBOX_DOMAIN`.
    pub(crate) path_origin: Option<String>,
}

/// The hosts from the process environment.
pub(crate) fn from_env() -> Result<Hosts> {
    resolve(|name| std::env::var(name).ok())
}

/// The hosts from any variable lookup. Tests pass a map.
///
/// Derivation with `LITTLEBOX_DOMAIN=<domain>`:
/// - scheme: `LITTLEBOX_SCHEME` when set, else `http` for `localhost`, else `https`
/// - derived origin: `<scheme>://app.<domain>`, with `:<LITTLEBOX_PUBLIC_PORT>` when set
/// - public origin: `LITTLEBOX_PUBLIC_ORIGIN` when set, else the derived origin
/// - MCP origin: `LITTLEBOX_MCP_ORIGIN` when set, else the public origin
/// - host suffix: `LITTLEBOX_HOST_SUFFIX` when set, else `<domain>`
pub(crate) fn resolve(get: impl Fn(&str) -> Option<String>) -> Result<Hosts> {
    let Some(raw_domain) = non_empty(&get, "LITTLEBOX_DOMAIN") else {
        return Ok(Hosts {
            public_origin: get("LITTLEBOX_PUBLIC_ORIGIN").unwrap_or_else(|| LEGACY_PUBLIC_ORIGIN.into()),
            mcp_origin: get("LITTLEBOX_MCP_ORIGIN"),
            host_suffix: get("LITTLEBOX_HOST_SUFFIX").unwrap_or_else(|| LEGACY_HOST_SUFFIX.into()),
            path_origin: None,
        });
    };
    let domain = domain_name(&raw_domain)?;
    let scheme = match non_empty(&get, "LITTLEBOX_SCHEME") {
        Some(scheme) if scheme == "http" || scheme == "https" => scheme,
        Some(scheme) => bail!("LITTLEBOX_SCHEME={scheme} must be http or https."),
        None if domain == "localhost" => "http".to_owned(),
        None => "https".to_owned(),
    };
    let port = match non_empty(&get, "LITTLEBOX_PUBLIC_PORT") {
        Some(port) if port.bytes().all(|byte| byte.is_ascii_digit()) && port.parse::<u16>().is_ok() => format!(":{port}"),
        Some(port) => bail!("LITTLEBOX_PUBLIC_PORT={port} must be a port number."),
        None => String::new(),
    };
    let derived = format!("{scheme}://app.{domain}{port}");
    let public_origin = non_empty(&get, "LITTLEBOX_PUBLIC_ORIGIN").unwrap_or(derived);
    let mcp_origin = non_empty(&get, "LITTLEBOX_MCP_ORIGIN").unwrap_or_else(|| public_origin.clone());
    // The home and every path address share <domain>. The editor or MCP on that host would let an
    // app's script reach the owner's session, so the Kernel refuses to start that way (ADR-0024).
    let host_of = |origin: &str| url::Url::parse(origin).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    for (name, origin) in [("LITTLEBOX_PUBLIC_ORIGIN", &public_origin), ("LITTLEBOX_MCP_ORIGIN", &mcp_origin)] {
        if host_of(origin).as_deref() == Some(domain.as_str()) {
            bail!("{name}={origin} is on {domain}, where the home and path addresses answer. Put the editor and MCP on another host, such as app.{domain}.");
        }
    }
    Ok(Hosts {
        mcp_origin: Some(mcp_origin),
        host_suffix: non_empty(&get, "LITTLEBOX_HOST_SUFFIX").unwrap_or_else(|| domain.clone()),
        path_origin: Some(format!("{scheme}://{domain}{port}")),
        public_origin,
    })
}

#[cfg(test)]
#[test]
fn the_editor_may_not_share_the_path_host() {
    let get = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string());
    let refused = resolve(get(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_PUBLIC_ORIGIN", "http://localhost:8790")])).unwrap_err();
    assert!(refused.to_string().contains("where the home and path addresses answer"), "{refused}");
    assert!(resolve(get(&[("LITTLEBOX_DOMAIN", "littlebox.com"), ("LITTLEBOX_MCP_ORIGIN", "https://littlebox.com")])).is_err());
    assert!(resolve(get(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_PUBLIC_ORIGIN", "http://app.localhost:8790")])).is_ok());
}

fn non_empty(get: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// A lowercase DNS name of letters, digits, hyphens and dots, without a
/// trailing dot. It has no scheme, port or path.
fn domain_name(raw: &str) -> Result<String> {
    let domain = raw.trim_end_matches('.').to_ascii_lowercase();
    let labels_ok = !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    if !labels_ok {
        bail!("LITTLEBOX_DOMAIN={raw} is not a domain name, such as littlebox.com or localhost.");
    }
    Ok(domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn unset_domain_keeps_the_old_defaults() {
        let hosts = resolve(env(&[])).unwrap();
        assert_eq!(hosts.public_origin, "http://localhost:8080");
        assert_eq!(hosts.mcp_origin, None);
        assert_eq!(hosts.host_suffix, "localhost");
    }

    #[test]
    fn unset_domain_passes_the_older_variables_through() {
        let hosts = resolve(env(&[
            ("LITTLEBOX_PUBLIC_ORIGIN", "https://apps.example.com"),
            ("LITTLEBOX_MCP_ORIGIN", "https://control.example.com"),
            ("LITTLEBOX_HOST_SUFFIX", "apps.example.com"),
            ("LITTLEBOX_PUBLIC_PORT", "9000"),
        ]))
        .unwrap();
        assert_eq!(hosts.public_origin, "https://apps.example.com");
        assert_eq!(hosts.mcp_origin.as_deref(), Some("https://control.example.com"));
        assert_eq!(hosts.host_suffix, "apps.example.com");
    }

    #[test]
    fn localhost_domain_is_http_with_the_port() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_PUBLIC_PORT", "8080")])).unwrap();
        assert_eq!(hosts.public_origin, "http://app.localhost:8080");
        assert_eq!(hosts.mcp_origin.as_deref(), Some("http://app.localhost:8080"));
        assert_eq!(hosts.host_suffix, "localhost");
        assert_eq!(hosts.path_origin.as_deref(), Some("http://localhost:8080"));
    }

    #[test]
    fn localhost_domain_without_a_port_has_no_port() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "localhost")])).unwrap();
        assert_eq!(hosts.public_origin, "http://app.localhost");
    }

    #[test]
    fn public_domain_is_https_without_a_port() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "littlebox.com")])).unwrap();
        assert_eq!(hosts.public_origin, "https://app.littlebox.com");
        assert_eq!(hosts.mcp_origin.as_deref(), Some("https://app.littlebox.com"));
        assert_eq!(hosts.host_suffix, "littlebox.com");
        assert_eq!(hosts.path_origin.as_deref(), Some("https://littlebox.com"));
    }

    #[test]
    fn domain_is_lowercased_and_loses_a_trailing_dot() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "  Littlebox.COM. ")])).unwrap();
        assert_eq!(hosts.public_origin, "https://app.littlebox.com");
        assert_eq!(hosts.host_suffix, "littlebox.com");
    }

    #[test]
    fn scheme_override_wins_over_the_domain_default() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_SCHEME", "https")])).unwrap();
        assert_eq!(hosts.public_origin, "https://app.localhost");
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "littlebox.com"), ("LITTLEBOX_SCHEME", "http")])).unwrap();
        assert_eq!(hosts.public_origin, "http://app.littlebox.com");
    }

    #[test]
    fn explicit_variables_win_over_the_derived_values() {
        let hosts = resolve(env(&[
            ("LITTLEBOX_DOMAIN", "littlebox.com"),
            ("LITTLEBOX_PUBLIC_ORIGIN", "https://home.example.org"),
            ("LITTLEBOX_MCP_ORIGIN", "https://mcp.example.org"),
            ("LITTLEBOX_HOST_SUFFIX", "apps.example.org"),
        ]))
        .unwrap();
        assert_eq!(hosts.public_origin, "https://home.example.org");
        assert_eq!(hosts.mcp_origin.as_deref(), Some("https://mcp.example.org"));
        assert_eq!(hosts.host_suffix, "apps.example.org");
    }

    #[test]
    fn mcp_defaults_to_the_public_origin_it_is_given() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "littlebox.com"), ("LITTLEBOX_PUBLIC_ORIGIN", "https://home.example.org")])).unwrap();
        assert_eq!(hosts.mcp_origin.as_deref(), Some("https://home.example.org"));
    }

    #[test]
    fn empty_variables_count_as_unset_when_the_domain_is_set() {
        let hosts = resolve(env(&[("LITTLEBOX_DOMAIN", "littlebox.com"), ("LITTLEBOX_PUBLIC_ORIGIN", ""), ("LITTLEBOX_HOST_SUFFIX", "")])).unwrap();
        assert_eq!(hosts.public_origin, "https://app.littlebox.com");
        assert_eq!(hosts.host_suffix, "littlebox.com");
    }

    #[test]
    fn invalid_domain_scheme_and_port_are_refused() {
        for domain in ["https://littlebox.com", "littlebox.com/p", "a b.com", "-x.com", "x..com", "localhost:8080", "."] {
            assert!(resolve(env(&[("LITTLEBOX_DOMAIN", domain)])).is_err(), "domain={domain}");
        }
        assert!(resolve(env(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_SCHEME", "ftp")])).is_err());
        assert!(resolve(env(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_PUBLIC_PORT", "http")])).is_err());
        assert!(resolve(env(&[("LITTLEBOX_DOMAIN", "localhost"), ("LITTLEBOX_PUBLIC_PORT", "70000")])).is_err());
    }
}
