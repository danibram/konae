//! An action's `http` key, copied from kernel-rs/src/http.rs at 8ecb7e5.
//! Routing, API keys, secrets and signature checks on a request stay in the
//! Kernel.

use super::refusal::{refuse, Refusal};
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HttpBinding {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) signature: Option<SignatureBinding>,
    /// The declared callers, each of "signature", "key" and "session".
    pub(super) callers: Option<Vec<String>>,
    /// Input keys and the JSON path each reads from a POST body.
    pub(super) fields: Option<Vec<(String, Vec<super::provider::Step>)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SignatureBinding {
    /// The header the MAC arrives in; `svix-signature` for the svix scheme.
    pub(super) header: String,
    pub(super) secret: String,
    pub(super) scheme: super::provider::Scheme,
}

const RESERVED_SEGMENTS: [&str; 4] = ["_konae", "oauth", ".well-known", "mcp"];
const ROUTE_CALLERS: [&str; 3] = ["signature", "key", "session"];

/// Parses and checks one action's `http` key. `Ok(None)` when it has none.
pub(super) fn binding(name: &str, action: &ActionDefinition) -> Result<Option<HttpBinding>, Refusal> {
    let Some(value) = action.http.as_ref() else { return Ok(None) };
    if action.kind == "signal" {
        return Err(refuse("signal.route", format!(
            "Action {name} is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route."
        )));
    }
    let invalid = || refuse("route.invalid", format!("Action {name} has an invalid http binding."));
    let record = value.as_object().ok_or_else(invalid)?;
    if record.keys().any(|key| !["method", "path", "signature", "callers", "fields"].contains(&key.as_str())) {
        return Err(invalid());
    }
    let path = record.get("path").and_then(Value::as_str).ok_or_else(invalid)?;
    let method = record.get("method").and_then(Value::as_str);
    let method = match (method, action.kind.as_str()) {
        (Some("GET"), "query") => "GET",
        (Some("POST"), "mutation") => "POST",
        _ => return Err(refuse("route.method-mismatch", format!("Action {name} must bind GET for a query and POST for a mutation."))),
    };
    if action.access != "anonymous" {
        return Err(refuse("route.authenticated", format!(
            "Action {name} is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route."
        )));
    }
    if !valid_path(path) {
        return Err(refuse("route.path-invalid", format!(
            r#"Action {name} has an invalid http path. Use a leading "/" and segments of letters, digits, ".", "_", "~" or "-", up to 200 characters."#
        )));
    }
    let first = path[1..].split('/').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&first.as_str()) {
        return Err(refuse("route.path-reserved", format!(
            "Action {name} binds the reserved path {path}. Paths under /_konae, /oauth, /.well-known and /mcp belong to the Kernel."
        )));
    }
    let signature = match record.get("signature") {
        None => None,
        Some(value) => {
            let (header, secret, scheme) = super::provider::signature(name, method, value)?;
            Some(SignatureBinding { header, secret, scheme })
        }
    };
    let callers = match record.get("callers") {
        None => None,
        Some(value) => {
            let invalid_callers = || {
                refuse("route.callers-invalid", format!(r#"Action {name} has an invalid callers list. List one or more of "signature", "key" and "session", each once."#))
            };
            let items = value.as_array().filter(|items| !items.is_empty()).ok_or_else(invalid_callers)?;
            let mut callers: Vec<String> = Vec::new();
            for item in items {
                let caller = item.as_str().filter(|caller| ROUTE_CALLERS.contains(caller)).ok_or_else(invalid_callers)?;
                if callers.iter().any(|seen| seen == caller) {
                    return Err(invalid_callers());
                }
                callers.push(caller.to_owned());
            }
            let signs = callers.iter().any(|caller| caller == "signature");
            if signs && signature.is_none() {
                return Err(refuse("route.callers-no-signature", format!(r#"Action {name} lists "signature" among its callers but declares no signature."#)));
            }
            if !signs && signature.is_some() {
                return Err(refuse("route.callers-without-signature", format!(r#"Action {name} declares a signature, so its callers must include "signature"."#)));
            }
            Some(callers)
        }
    };
    let fields = match record.get("fields") {
        None => None,
        Some(value) => Some(super::provider::fields(name, method, action, value)?),
    };
    Ok(Some(HttpBinding { method: method.to_owned(), path: path.to_owned(), signature, callers, fields }))
}

fn valid_path(path: &str) -> bool {
    path.len() <= 200
        && path.starts_with('/')
        && path[1..].split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        })
}

/// Refuses two actions bound to the same method and path. Names are visited
/// in sorted order, as the editor does, so both name the same pair.
pub(super) fn validate_routes(program: &ActionProgram) -> Result<(), Refusal> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    let mut seen: HashMap<String, &str> = HashMap::new();
    for name in names {
        let Some(http) = binding(name, &program.actions[name])? else { continue };
        let route = format!("{} {}", http.method, http.path);
        if let Some(earlier) = seen.get(&route) {
            return Err(refuse("route.duplicate", format!("Actions {earlier} and {name} both bind {route}.")));
        }
        seen.insert(route, name);
    }
    Ok(())
}
