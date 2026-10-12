//! Calls between apps, ADR-0019.
//!
//! An action of `"kind": "call"` names a URL, a method, a body built from its
//! inputs, an optional `auth` and an optional `then` mutation. `actions_invoke`
//! checks the caller and the inputs as for any action, then queues the call in
//! `calls.sqlite` and answers `{ callId, queued: true }`. The worker below
//! sends it from the Kernel's own process, never from the browser, with a
//! 10-second timeout, a 64 KiB cap on the answer and retries with backoff. When
//! the call ends, delivered or failed, the `then` mutation runs with inputs read
//! from the answer by JSON pointers. The owner reads the outbox with
//! `calls_read`.
//!
//! A call whose host is a Deployment of this installation goes through
//! `http::call_route`, the route code a request from outside meets, with the
//! same callers check. Every other host is resolved once, every address it
//! resolves to must be public, and the request is pinned to the address that
//! was checked, so a DNS answer cannot change between the check and the
//! connection. The rules are listed in docs/mcp.md, "Calls between apps".
//!
//! The checks that need no knowledge of the installation run at both doors:
//! `parseServerActionSource` in src/kernel/actions.ts makes them in the same
//! order with the same messages, and kernel-rs/src/call-cases.json holds the
//! cases both test suites run. The checks that do, http only to this
//! installation and never the editor's host, run at release and again before
//! each attempt, as unset secrets are refused only by the Kernel.

use super::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) const CALLS_DATABASE: &str = "calls.sqlite";
const MAX_URL_CHARS: usize = 2048;
const MAX_BODY_KEYS: usize = 32;
const MAX_POINTER_CHARS: usize = 200;
/// How long one attempt may take, from connecting to the last byte of the answer.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest answer a call reads. A larger one ends the call as failed.
pub(crate) const MAX_ANSWER_BYTES: usize = 64 * 1024;
/// Calls an app may queue in any 60 seconds, unless LITTLEBOX_CALLS_PER_MINUTE says otherwise.
const DEFAULT_PER_MINUTE: i64 = 20;
/// Attempts before a call that keeps meeting a retryable answer fails. The
/// waits double from 30 seconds, so the last attempt runs about 7.5 minutes
/// after the first.
const MAX_ATTEMPTS: i64 = 5;
const FIRST_BACKOFF_MS: i64 = 30_000;
/// A paused app's call waits this long before it is looked at again.
const PAUSE_WAIT_MS: i64 = 60_000;
/// How long a claimed call stays with one worker pass. Longer than an attempt.
const LEASE_MS: i64 = 60_000;
const BATCH: i64 = 20;
const WORKER_INTERVAL: Duration = Duration::from_secs(5);
/// Finished calls kept per Deployment, oldest dropped first.
const KEEP_PER_DEPLOYMENT: i64 = 1000;
const OUTBOX_PAGE: i64 = 50;
/// The part of an answer the outbox keeps for the owner.
const EXCERPT_CHARS: usize = 1024;
const RESERVED_SEGMENTS: [&str; 4] = ["_littlebox", "oauth", ".well-known", "mcp"];
const ANSWER_ROOTS: [&str; 5] = ["ok", "status", "error", "body", "input"];
const DEFAULT_SIGNATURE_HEADER: &str = "X-Littlebox-Signature";
pub(crate) const RATE_LIMITED: &str = "This app reached its limit of calls in the last minute. Try again in a minute.";

/// The keys an action of `"kind": "call"` adds to an action definition.
#[derive(Clone, Default, Deserialize)]
pub(crate) struct CallKeys {
    #[serde(default)]
    url: Option<Value>,
    #[serde(default)]
    method: Option<Value>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    headers: Option<Value>,
    #[serde(default)]
    auth: Option<Value>,
    #[serde(default)]
    then: Option<Value>,
}

impl CallKeys {
    /// The call keys the action declares, for `action_shape::check_keys`.
    pub(crate) fn present(&self) -> Vec<&'static str> {
        [
            ("url", &self.url),
            ("method", &self.method),
            ("body", &self.body),
            ("headers", &self.headers),
            ("auth", &self.auth),
            ("then", &self.then),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.is_some().then_some(key))
        .collect()
    }

    fn any(&self) -> bool {
        self.url.is_some()
            || self.method.is_some()
            || self.body.is_some()
            || self.headers.is_some()
            || self.auth.is_some()
            || self.then.is_some()
    }
}

/// A checked call declaration.
#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub(crate) url: url::Url,
    pub(crate) method: String,
    /// Body key to what it sends, sorted by body key. A key with `.` expands
    /// into nested JSON on POST, so a Checkout Session can send `line_items.0.price`.
    pub(crate) body: Vec<(String, BodyValue)>,
    /// Extra request headers, sorted by name. Only `Idempotency-Key` is allowed.
    pub(crate) headers: Vec<(String, BodyValue)>,
    pub(crate) auth: Option<Auth>,
    pub(crate) then: Option<Then>,
}

/// What one body key sends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BodyValue {
    /// The value of this input.
    Input(String),
    /// A value fixed in actions.json, which the caller cannot change.
    Constant(Value),
}

/// The longest constant string a body may carry.
const MAX_CONSTANT_CHARS: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    /// Sign with Littlebox's route scheme, in `header`, with the secret's value.
    Sign { secret: String, header: String },
    /// Send the secret's value, an API key, as `Authorization: Bearer`.
    Key { secret: String },
}

impl Auth {
    pub(crate) fn secret(&self) -> &str {
        match self {
            Auth::Sign { secret, .. } | Auth::Key { secret } => secret,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Then {
    pub(crate) action: String,
    /// Input key of the then mutation to a JSON pointer into the answer, sorted by key.
    pub(crate) inputs: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Declarations. The same checks, order and messages as `parseCall` and
// `assertCallTargets` in src/kernel/actions.ts.

/// Refuses call keys on an action of another kind, and checks a call. Runs
/// after `action_shape::check_action`. `Ok(None)` for an action that is not a call.
pub(crate) fn check_kind(name: &str, action: &ActionDefinition) -> Result<Option<Call>, String> {
    if action.kind != "call" {
        if action.call.any() {
            let kind = match action.kind.as_str() {
                "query" => "query",
                "mutation" => "mutation",
                _ => "signal action",
            };
            return Err(format!(
                r#"Action {name} is a {kind}, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call"."#
            ));
        }
        return Ok(None);
    }
    declaration(name, action).map(Some)
}

/// Parses and checks one call.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Call, String> {
    if !action.statement.trim().is_empty() || action.signal.is_some() || action.guard.is_some() || action.http.is_some() {
        return Err(format!(
            "Action {name} is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route."
        ));
    }
    if action.bindings.iter().any(|binding| binding.source != "input") {
        return Err(format!("Action {name} is a call, so its bindings may only be inputs."));
    }
    let method = match action.call.method.as_ref().and_then(Value::as_str) {
        Some("GET") => "GET",
        Some("POST") => "POST",
        _ => return Err(format!(r#"Action {name} must call with "method": "GET" or "POST"."#)),
    };
    let url = static_url(name, action.call.url.as_ref())?;
    let inputs: Vec<&str> = action.bindings.iter().filter_map(|binding| binding.key.as_deref()).collect();
    let body = body_map(name, method, action.call.body.as_ref(), &inputs)?;
    let headers = headers_map(name, action.call.headers.as_ref(), &inputs)?;
    let auth = auth(name, action.call.auth.as_ref())?;
    let then = then(name, action.call.then.as_ref(), &inputs)?;
    Ok(Call { url, method: method.to_owned(), body, headers, auth, then })
}

fn static_url(name: &str, value: Option<&Value>) -> Result<url::Url, String> {
    let invalid = || {
        format!("Action {name} has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters.")
    };
    let text = value.and_then(Value::as_str).ok_or_else(invalid)?;
    if text.chars().count() > MAX_URL_CHARS || text.contains('#') || text.chars().any(|c| (c as u32) <= 0x20 || c as u32 == 0x7f) {
        return Err(invalid());
    }
    let url = url::Url::parse(text).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    let host = url.host_str().ok_or_else(invalid)?.to_ascii_lowercase();
    if matches!(url.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))) {
        return Err(format!("Action {name} calls the IP address {host}. Name a host instead."));
    }
    let bare = host.trim_end_matches('.');
    if matches!(bare, "localhost" | "metadata" | "instance-data") || bare.ends_with(".internal") {
        return Err(format!("Action {name} calls {host}, which names this machine or a cloud metadata service."));
    }
    let first = url.path().split('/').nth(1).unwrap_or("").to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&first.as_str()) {
        return Err(format!(
            "Action {name} calls a path under /_littlebox, /oauth, /.well-known or /mcp, which belong to the Kernel."
        ));
    }
    Ok(url)
}

fn valid_body_key(key: &str) -> bool {
    if !(1..=64).contains(&key.len()) {
        return false;
    }
    let mut segments = key.split('.');
    let Some(first) = segments.next() else { return false };
    std::iter::once(first).chain(segments).all(valid_body_segment)
}

/// The highest array index a nested body path may name. Keeps `nest_body` from
/// allocating a huge sparse array from a key such as `items.999999.price`.
const MAX_BODY_INDEX: usize = 31;

/// One segment of a body path: an object key, or a non-negative array index.
fn valid_body_segment(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    if segment.bytes().all(|byte| byte.is_ascii_digit()) {
        if !(segment == "0" || !segment.starts_with('0')) {
            return false;
        }
        return segment.parse::<usize>().ok().is_some_and(|index| index <= MAX_BODY_INDEX);
    }
    let bytes = segment.as_bytes();
    (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(byte))
}

/// A constant body value: `{ "value": <string of at most 1000 characters, number, boolean or null> }`.
fn constant(value: &Value) -> Option<Value> {
    let record = value.as_object().filter(|record| record.len() == 1)?;
    let constant = record.get("value")?;
    let valid = match constant {
        Value::String(text) => text.chars().count() <= MAX_CONSTANT_CHARS,
        Value::Number(_) | Value::Bool(_) | Value::Null => true,
        _ => false,
    };
    valid.then(|| constant.clone())
}

fn body_map(name: &str, method: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Vec<(String, BodyValue)>, String> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let invalid = || {
        format!(r#"Action {name} has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32."#)
    };
    let record = value.as_object().ok_or_else(invalid)?;
    if record.len() > MAX_BODY_KEYS
        || record.iter().any(|(key, sent)| !valid_body_key(key) || !(sent.is_string() || constant(sent).is_some()))
    {
        return Err(invalid());
    }
    let mut keys: Vec<&String> = record.keys().collect();
    keys.sort();
    for (index, key) in keys.iter().enumerate() {
        if method == "GET" && key.contains('.') {
            return Err(format!(
                "Action {name} sends body key {key} with a path. Nested body keys belong on POST."
            ));
        }
        for other in keys.iter().skip(index + 1) {
            if other.starts_with(&format!("{key}.")) {
                return Err(format!(
                    "Action {name} has body keys {key} and {other}, which overlap. Use one path or the other."
                ));
            }
        }
    }
    let mut body = Vec::new();
    for key in keys {
        let sent = &record[key];
        let Some(input) = sent.as_str() else {
            body.push((key.clone(), BodyValue::Constant(constant(sent).unwrap_or(Value::Null))));
            continue;
        };
        if !inputs.contains(&input) {
            return Err(format!("Action {name} sends body key {key} from {input}, which is not one of its input bindings."));
        }
        body.push((key.clone(), BodyValue::Input(input.to_owned())));
    }
    Ok(body)
}

/// The only request header a call may set besides auth and content-type.
const ALLOWED_CALL_HEADER: &str = "Idempotency-Key";
const MAX_HEADERS: usize = 8;

fn headers_map(name: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Vec<(String, BodyValue)>, String> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let invalid = || {
        format!(r#"Action {name} has an invalid headers. Map Idempotency-Key to an input or a constant, at most 8."#)
    };
    let record = value.as_object().ok_or_else(invalid)?;
    if record.is_empty() || record.len() > MAX_HEADERS {
        return Err(invalid());
    }
    let mut keys: Vec<&String> = record.keys().collect();
    keys.sort();
    let mut headers = Vec::new();
    for key in keys {
        if key.as_str() != ALLOWED_CALL_HEADER {
            return Err(format!(
                "Action {name} sets header {key}, which is not allowed. Only Idempotency-Key is allowed."
            ));
        }
        let sent = &record[key];
        if !(sent.is_string() || constant(sent).is_some()) {
            return Err(invalid());
        }
        let Some(input) = sent.as_str() else {
            let value = constant(sent).unwrap_or(Value::Null);
            if !value.is_string() && !value.is_number() && !value.is_boolean() {
                return Err(invalid());
            }
            headers.push((key.clone(), BodyValue::Constant(value)));
            continue;
        };
        if !inputs.contains(&input) {
            return Err(format!("Action {name} sends header {key} from {input}, which is not one of its input bindings."));
        }
        headers.push((key.clone(), BodyValue::Input(input.to_owned())));
    }
    Ok(headers)
}

fn auth(name: &str, value: Option<&Value>) -> Result<Option<Auth>, String> {
    let Some(value) = value else { return Ok(None) };
    let invalid = || {
        format!(r#"Action {name} has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds."#)
    };
    let record = value.as_object().ok_or_else(invalid)?;
    let signs = record.contains_key("secret");
    let keyed = record.contains_key("key");
    let allowed: &[&str] = if signs { &["secret", "header"] } else { &["key"] };
    if signs == keyed || record.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid());
    }
    let secret = record.get(if signs { "secret" } else { "key" }).and_then(Value::as_str).unwrap_or("");
    if !valid_secret_name(secret) {
        return Err(format!(
            r#"Action {name} names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter."#
        ));
    }
    if !signs {
        return Ok(Some(Auth::Key { secret: secret.to_owned() }));
    }
    let header = match record.get("header") {
        None => DEFAULT_SIGNATURE_HEADER.to_owned(),
        Some(header) => {
            let header = header.as_str().unwrap_or("");
            if header.is_empty()
                || header.len() > 64
                || !header.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || ["authorization", "cookie", "host", "content-type", "content-length"].contains(&header.to_ascii_lowercase().as_str())
            {
                return Err(format!("Action {name} has an invalid signature header."));
            }
            header.to_owned()
        }
    };
    Ok(Some(Auth::Sign { secret: secret.to_owned(), header }))
}

fn valid_secret_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn then(name: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Option<Then>, String> {
    let Some(value) = value else { return Ok(None) };
    let invalid = || format!("Action {name} has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer.");
    let record = value.as_object().ok_or_else(invalid)?;
    if record.keys().any(|key| key != "action" && key != "inputs") {
        return Err(invalid());
    }
    let action = record.get("action").and_then(Value::as_str).filter(|action| !action.is_empty()).ok_or_else(invalid)?;
    let mapped = match record.get("inputs") {
        None => serde_json::Map::new(),
        Some(Value::Object(mapped)) => mapped.clone(),
        Some(_) => return Err(invalid()),
    };
    if mapped.len() > super::action_shape::MAX_ACTIONS || mapped.iter().any(|(key, pointer)| key.is_empty() || !pointer.is_string()) {
        return Err(invalid());
    }
    let mut pairs: Vec<(String, String)> =
        mapped.iter().map(|(key, pointer)| (key.clone(), pointer.as_str().unwrap_or_default().to_owned())).collect();
    pairs.sort();
    for (_, pointer) in &pairs {
        let segments: Vec<&str> = pointer.split('/').skip(1).collect();
        let escapes_valid = pointer.split('~').skip(1).all(|rest| rest.starts_with('0') || rest.starts_with('1'));
        if pointer.chars().count() > MAX_POINTER_CHARS
            || !pointer.starts_with('/')
            || !escapes_valid
            || !ANSWER_ROOTS.contains(&segments[0])
        {
            return Err(format!(
                "Action {name} reads {pointer} for then, which is not a JSON pointer into /ok, /status, /error, /body or /input."
            ));
        }
        if segments[0] == "input" && segments.len() > 1 {
            let key = segments[1].replace("~1", "/").replace("~0", "~");
            if !inputs.contains(&key.as_str()) {
                return Err(format!("Action {name} reads {pointer} for then, but {key} is not one of its input bindings."));
            }
        }
    }
    Ok(Some(Then { action: action.to_owned(), inputs: pairs }))
}

/// Checks each call's `then` against the program, in sorted name order. Runs
/// after every action passed its own checks and no two routes collide.
pub(crate) fn check_program(program: &ActionProgram) -> Result<(), String> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Ok(Some(call)) = check_kind(name, &program.actions[name]) else { continue };
        let Some(then) = call.then else { continue };
        let target = &then.action;
        let Some(mutation) = program.actions.get(target) else {
            return Err(format!("Action {name} names {target} as its then, which is not declared."));
        };
        if mutation.kind != "mutation" {
            return Err(format!("Action {name} names {target} as its then, which is not a mutation."));
        }
        if mutation.http.is_some() {
            return Err(format!(
                "Action {name} names {target} as its then, which answers on an HTTP route. A then mutation runs only after its call."
            ));
        }
        if mutation.bindings.iter().any(|binding| binding.source == "actor") {
            return Err(format!(
                "Action {name} names {target} as its then, which binds the actor. A then mutation runs with no caller."
            ));
        }
        let inputs: Vec<&ActionBinding> = mutation.bindings.iter().filter(|binding| binding.source == "input").collect();
        for (key, _) in &then.inputs {
            if !inputs.iter().any(|binding| binding.key.as_deref() == Some(key.as_str())) {
                return Err(format!("Action {name} fills {key}, which is not an input of {target}."));
            }
        }
        let mut required: Vec<&str> =
            inputs.iter().filter(|binding| !binding.nullable).filter_map(|binding| binding.key.as_deref()).collect();
        required.sort();
        for key in required {
            if !then.inputs.iter().any(|(filled, _)| filled == key) {
                return Err(format!(
                    "Action {name} leaves {key} of {target} unfilled. Fill it from the answer or make the input nullable."
                ));
            }
        }
    }
    Ok(())
}

/// The call that names `action` as its `then`, if any. Such a mutation runs
/// only after its call, so `actions_invoke` refuses it.
pub(crate) fn then_of(program: &ActionProgram, action: &str) -> Option<String> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    names.into_iter().find_map(|name| {
        let call = check_kind(name, &program.actions[name]).ok().flatten()?;
        (call.then?.action == action).then(|| name.clone())
    })
}

/// The secrets a program's calls use, by action, in sorted name order.
pub(crate) fn secrets(program: &ActionProgram) -> Vec<(String, String)> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let auth = check_kind(name, &program.actions[name]).ok().flatten()?.auth?;
            Some((name.clone(), auth.secret().to_owned()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The manifest's `connects`: origins the page may reach with fetch, added to
// its connect-src. The same checks and messages as `checkConnects` in
// src/kernel/calls.ts.

const MAX_CONNECTS: usize = 10;

pub(crate) fn check_connects(manifest: &Value) -> Result<(), String> {
    let Some(value) = manifest.get("connects").filter(|value| !value.is_null()) else { return Ok(()) };
    let invalid = || r#"connects must be a list of at most 10 origins, such as ["https://api.example.com"]."#.to_owned();
    let entries = value.as_array().filter(|entries| entries.len() <= MAX_CONNECTS).ok_or_else(invalid)?;
    let mut seen: Vec<&str> = Vec::new();
    for entry in entries {
        let entry = entry.as_str().ok_or_else(invalid)?;
        // A browser treats localhost and its subdomains as secure, and a
        // local installation serves its apps there over http.
        let local = |url: &url::Url| url.host_str().is_some_and(|host| host == "localhost" || host.ends_with(".localhost"));
        let origin = url::Url::parse(entry)
            .ok()
            .filter(|url| (url.scheme() == "https" || (url.scheme() == "http" && local(url))) && !entry.contains('*'))
            .map(|url| url.origin().ascii_serialization());
        if origin.as_deref() != Some(entry) {
            return Err(format!(
                "connects lists {entry}, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost."
            ));
        }
        if seen.contains(&entry) {
            return Err(format!("connects lists {entry} twice."));
        }
        seen.push(entry);
    }
    Ok(())
}

/// The origins a published manifest adds to the page's connect-src.
pub(crate) fn connects(manifest: &Value) -> Vec<String> {
    if check_connects(manifest).is_err() {
        return Vec::new();
    }
    manifest
        .get("connects")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The installation's rules, at release and before each attempt.

fn host_of(url: &url::Url) -> String {
    url.host_str().unwrap_or("").trim_end_matches('.').to_ascii_lowercase()
}

/// The editor's hosts: the public origin, where owners sign in and MCP
/// answers, and the separate MCP origin when there is one.
fn editor_host(rules: &super::domain::Rules, host: &str) -> bool {
    rules.installation_hosts.iter().any(|editor| editor == host)
}

/// A host under the installation's suffix: one of its apps, or a name it may give one.
fn installation_host(rules: &super::domain::Rules, host: &str) -> bool {
    !rules.suffix.is_empty() && (host == rules.suffix || host.ends_with(&format!(".{}", rules.suffix)))
}

/// Every check release_plan runs on a parsed program before it builds
/// anything, in order: route secrets (http.rs `require_secrets`), then
/// calls. The cases are in release-cases.json, which the Validation Gate
/// runs too.
pub(crate) fn check_release(rules: &super::domain::Rules, db: &Connection, project: &str, program: &ActionProgram) -> Result<()> {
    super::http::require_secrets(db, project, program)?;
    check_calls(rules, db, project, program)
}

/// Refuses a release whose calls reach the editor, use http outside this
/// installation, or use a secret the project does not have.
fn check_calls(rules: &super::domain::Rules, db: &Connection, project: &str, program: &ActionProgram) -> Result<()> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Ok(Some(call)) = check_kind(name, &program.actions[name]) else { continue };
        let host = host_of(&call.url);
        anyhow::ensure!(
            !editor_host(rules, &host),
            "Action {name} calls {host}, the editor's host. A call reaches an app or an outside service, never the editor."
        );
        anyhow::ensure!(
            call.url.scheme() == "https" || installation_host(rules, &host),
            "Action {name} calls http://{host}, which is not an app of this installation. Use https."
        );
        if let Some(auth) = &call.auth {
            let secret = auth.secret();
            anyhow::ensure!(
                super::http::project_secret(db, project, secret)?.is_some(),
                "Action {name} calls with the secret {secret}, which is not set for this project. Set it with secret_set before publishing."
            );
        }
    }
    Ok(())
}

/// Whether an address may be reached by a call that leaves the installation:
/// not loopback, private, link-local (where cloud metadata answers), shared,
/// documentation, benchmarking, multicast, reserved or unspecified, and for
/// IPv6 not unique-local, link-local, or an IPv4 address in disguise that is
/// any of those.
pub(crate) fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_multicast()
        || address.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19)))
}

fn public_v6(address: Ipv6Addr) -> bool {
    if let Some(v4) = address.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let segments = address.segments();
    // NAT64 (64:ff9b::/96) carries an IPv4 address in its last 32 bits.
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return public_v4(Ipv4Addr::new((segments[6] >> 8) as u8, segments[6] as u8, (segments[7] >> 8) as u8, segments[7] as u8));
    }
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || segments[..6] == [0, 0, 0, 0, 0, 0])
}

enum Target {
    /// A Deployment of this installation, reached in process.
    App(Deployment),
    /// A host outside, reached at this address after it was checked.
    Outside(SocketAddr),
}

/// Where a call goes, or the error kind that ends it. `Err((kind, retry))`.
async fn target(state: &AppState, url: &url::Url) -> Result<Target, (&'static str, bool)> {
    let host = host_of(url);
    let rules = super::domain::Rules::of(state);
    if editor_host(&rules, &host) {
        return Err(("editor-host", false));
    }
    if let Some(deployment) = state.deployments.read().await.get(&host).cloned() {
        return Ok(Target::App(deployment));
    }
    if installation_host(&rules, &host) {
        return Err(("app-not-found", false));
    }
    if url.scheme() != "https" {
        return Err(("http-outside", false));
    }
    if !matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(("address-refused", false));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<SocketAddr> = match tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host.as_str(), port))).await {
        Ok(Ok(addresses)) => addresses.collect(),
        _ => return Err(("dns-failed", true)),
    };
    if addresses.is_empty() {
        return Err(("dns-failed", true));
    }
    // Every address must be public, so a name that resolves to one public and
    // one private address cannot be steered to the private one.
    if addresses.iter().any(|address| !public_address(address.ip())) {
        return Err(("address-refused", false));
    }
    Ok(Target::Outside(addresses[0]))
}

// ---------------------------------------------------------------------------
// The queue.

fn connection(data_root: &Path) -> Result<Connection> {
    let connection = Connection::open(data_root.join(CALLS_DATABASE))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS calls (
           id TEXT PRIMARY KEY,
           deployment TEXT NOT NULL,
           action TEXT NOT NULL,
           input TEXT NOT NULL,
           state TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'failed')),
           attempts INTEGER NOT NULL DEFAULT 0,
           next_attempt_at INTEGER NOT NULL,
           leased_until INTEGER,
           method TEXT,
           url TEXT,
           status INTEGER,
           error TEXT,
           answer TEXT,
           then_action TEXT,
           then_state TEXT,
           then_message TEXT,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS calls_due ON calls(state, next_attempt_at);
         CREATE INDEX IF NOT EXISTS calls_deployment ON calls(deployment, created_at);",
    )?;
    Ok(connection)
}

fn per_minute() -> i64 {
    std::env::var("LITTLEBOX_CALLS_PER_MINUTE")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=10_000).contains(value))
        .unwrap_or(DEFAULT_PER_MINUTE)
}

fn wake() -> &'static tokio::sync::Notify {
    static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
    WAKE.get_or_init(tokio::sync::Notify::new)
}

fn now() -> i64 {
    now_ms() as i64
}

/// Queues a call whose caller and inputs `invoke_action` has checked, and
/// answers `{ callId, queued: true }`.
pub(crate) async fn enqueue(state: &AppState, deployment: &Deployment, name: &str, input: &Value) -> Response {
    let data_root = state.data_root.clone();
    let deployment_id = deployment.id.clone();
    let action = name.to_owned();
    let input = input.clone();
    let limit = per_minute();
    let queued = tokio::task::spawn_blocking(move || -> Result<Option<String>> {
        let db = connection(&data_root)?;
        let at = now();
        let recent: i64 = db.query_row(
            "SELECT COUNT(*) FROM calls WHERE deployment = ?1 AND created_at > ?2",
            rusqlite::params![deployment_id, at - 60_000],
            |row| row.get(0),
        )?;
        if recent >= limit {
            return Ok(None);
        }
        let id = format!("call_{}", Uuid::new_v4().simple());
        db.execute(
            "INSERT INTO calls (id, deployment, action, input, state, attempts, next_attempt_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?5, ?5)",
            rusqlite::params![id, deployment_id, action, input.to_string(), at],
        )?;
        db.execute(
            "DELETE FROM calls WHERE deployment = ?1 AND state <> 'pending' AND id NOT IN
               (SELECT id FROM calls WHERE deployment = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
            rusqlite::params![deployment_id, KEEP_PER_DEPLOYMENT],
        )?;
        Ok(Some(id))
    })
    .await;
    match queued {
        Ok(Ok(Some(id))) => {
            super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Call(name.to_owned()));
            wake().notify_one();
            Json(json!({"result": {"callId": id, "queued": true}})).into_response()
        }
        Ok(Ok(None)) => error(StatusCode::TOO_MANY_REQUESTS, RATE_LIMITED),
        Ok(Err(error_value)) => error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
        Err(error_value) => error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string()),
    }
}

struct Claimed {
    id: String,
    deployment: String,
    action: String,
    input: Value,
    attempts: i64,
}

fn claim(data_root: &Path) -> Result<Vec<Claimed>> {
    let db = connection(data_root)?;
    let at = now();
    let mut statement = db.prepare(
        "UPDATE calls SET leased_until = ?1, attempts = attempts + 1, updated_at = ?2
         WHERE id IN (
           SELECT id FROM calls WHERE state = 'pending' AND next_attempt_at <= ?2
             AND (leased_until IS NULL OR leased_until < ?2)
           ORDER BY next_attempt_at LIMIT ?3)
         RETURNING id, deployment, action, input, attempts",
    )?;
    let rows = statement
        .query_map(rusqlite::params![at + LEASE_MS, at, BATCH], |row| {
            Ok(Claimed {
                id: row.get(0)?,
                deployment: row.get(1)?,
                action: row.get(2)?,
                input: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(json!({})),
                attempts: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// What one attempt produced.
#[derive(Default)]
struct Attempt {
    method: Option<String>,
    url: Option<String>,
    status: Option<u16>,
    error: Option<&'static str>,
    answer: Vec<u8>,
}

impl Attempt {
    fn ok(&self) -> bool {
        self.error.is_none() && self.status.is_some_and(|status| (200..300).contains(&status))
    }

    /// Whether a later attempt might get another answer: a network failure,
    /// a timeout, a DNS failure, 408, 425, 429 or a 5xx.
    fn retryable(&self, retry: bool) -> bool {
        match self.status {
            Some(status) if self.error.is_none() => matches!(status, 408 | 425 | 429) || status >= 500,
            _ => retry,
        }
    }

    fn excerpt(&self) -> Option<String> {
        (!self.answer.is_empty()).then(|| String::from_utf8_lossy(&self.answer).chars().take(EXCERPT_CHARS).collect())
    }
}

/// Puts a claimed call back for a later attempt. `counted` keeps the attempt.
fn release(data_root: &Path, id: &str, delay_ms: i64, counted: bool, attempt: &Attempt) -> Result<()> {
    let at = now();
    connection(data_root)?.execute(
        "UPDATE calls SET leased_until = NULL, next_attempt_at = ?1, updated_at = ?2, attempts = attempts - ?3,
           method = COALESCE(?4, method), url = COALESCE(?5, url), status = ?6, error = ?7, answer = ?8
         WHERE id = ?9 AND state = 'pending'",
        rusqlite::params![
            at + delay_ms,
            at,
            if counted { 0 } else { 1 },
            attempt.method,
            attempt.url,
            attempt.status,
            attempt.error,
            attempt.excerpt(),
            id
        ],
    )?;
    Ok(())
}

fn finish(data_root: &Path, id: &str, attempt: &Attempt, then: Option<(&str, &str, Option<&str>)>) -> Result<()> {
    let state = if attempt.ok() { "delivered" } else { "failed" };
    let (then_action, then_state, then_message) = match then {
        Some((action, state, message)) => (Some(action), Some(state), message),
        None => (None, None, None),
    };
    connection(data_root)?.execute(
        "UPDATE calls SET state = ?1, leased_until = NULL, updated_at = ?2, method = COALESCE(?3, method),
           url = COALESCE(?4, url), status = ?5, error = ?6, answer = ?7, then_action = ?8, then_state = ?9, then_message = ?10
         WHERE id = ?11 AND state = 'pending'",
        rusqlite::params![
            state,
            now(),
            attempt.method,
            attempt.url,
            attempt.status,
            attempt.error,
            attempt.excerpt(),
            then_action,
            then_state,
            then_message,
            id
        ],
    )?;
    Ok(())
}

/// Sends the calls that are due. Returns how many it looked at.
pub(crate) async fn run_once(state: &AppState) -> Result<usize> {
    let data_root = state.data_root.clone();
    let claimed = tokio::task::spawn_blocking(move || claim(&data_root)).await??;
    let count = claimed.len();
    let mut tasks = tokio::task::JoinSet::new();
    for job in claimed {
        let state = state.clone();
        tasks.spawn(async move {
            if let Err(error_value) = process(&state, job).await {
                eprintln!("Call worker pass failed: {error_value}");
            }
        });
    }
    while tasks.join_next().await.is_some() {}
    Ok(count)
}

/// Runs the worker on its interval and whenever a call is queued. Sending
/// happens on this task, never in a request handler.
pub(crate) fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(error_value) = run_once(&state).await {
                eprintln!("Call worker pass failed: {error_value}");
            }
            tokio::select! {
                _ = wake().notified() => {}
                _ = tokio::time::sleep(WORKER_INTERVAL) => {}
            }
        }
    });
}

/// Resolves one declared body or header value from the call's inputs.
fn resolve_sent(sent: &BodyValue, input: &Value) -> Option<Value> {
    match sent {
        BodyValue::Constant(value) => Some(value.clone()),
        BodyValue::Input(input_key) => input.get(input_key).filter(|value| !value.is_null()).cloned(),
    }
}

/// Expands flat body keys whose names contain `.` into nested JSON objects and
/// arrays. `line_items.0.price` becomes `{ "line_items": [{ "price": ... }] }`.
fn nest_body(flat: Vec<(String, Value)>) -> Value {
    let mut root = Value::Object(serde_json::Map::new());
    for (path, value) in flat {
        if !path.contains('.') {
            root.as_object_mut().expect("object").insert(path, value);
            continue;
        }
        let segments: Vec<&str> = path.split('.').collect();
        let mut cursor = &mut root;
        for (index, segment) in segments.iter().enumerate() {
            let last = index + 1 == segments.len();
            let array_index = segment.bytes().all(|byte| byte.is_ascii_digit()).then(|| segment.parse::<usize>().expect("digit segment"));
            if let Some(at) = array_index {
                if !cursor.is_array() {
                    *cursor = Value::Array(Vec::new());
                }
                let array = cursor.as_array_mut().expect("array");
                while array.len() <= at {
                    array.push(Value::Null);
                }
                if last {
                    array[at] = value;
                    break;
                }
                if array[at].is_null() {
                    let next = segments[index + 1];
                    array[at] = if next.bytes().all(|byte| byte.is_ascii_digit()) {
                        Value::Array(Vec::new())
                    } else {
                        Value::Object(serde_json::Map::new())
                    };
                }
                cursor = &mut array[at];
            } else {
                if !cursor.is_object() {
                    *cursor = Value::Object(serde_json::Map::new());
                }
                let object = cursor.as_object_mut().expect("object");
                if last {
                    object.insert(segment.to_string(), value);
                    break;
                }
                if !object.contains_key(*segment) {
                    let next = segments[index + 1];
                    object.insert(
                        segment.to_string(),
                        if next.bytes().all(|byte| byte.is_ascii_digit()) {
                            Value::Array(Vec::new())
                        } else {
                            Value::Object(serde_json::Map::new())
                        },
                    );
                }
                cursor = object.get_mut(*segment).expect("just inserted");
            }
        }
    }
    root
}

/// The URL, query and body one call sends, from its declaration and inputs.
/// A GET carries the body keys as query parameters after the URL's own. A POST
/// expands dotted keys into nested JSON.
fn request_parts(call: &Call, input: &Value) -> (url::Url, Vec<u8>) {
    let mut url = call.url.clone();
    let mut flat = Vec::new();
    for (key, sent) in &call.body {
        if let Some(value) = resolve_sent(sent, input) {
            flat.push((key.clone(), value));
        }
    }
    if call.method == "GET" {
        if !flat.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in &flat {
                let text = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                pairs.append_pair(key, &text);
            }
        }
        (url, Vec::new())
    } else {
        (url, nest_body(flat).to_string().into_bytes())
    }
}

/// The Idempotency-Key and other declared headers for one attempt.
fn request_headers(call: &Call, input: &Value) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, sent) in &call.headers {
        let Some(value) = resolve_sent(sent, input) else { continue };
        let text = match value {
            Value::String(text) => text,
            other => other.to_string(),
        };
        headers.push((name.clone(), text));
    }
    headers
}

/// A nonce for one signed attempt.
fn nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

/// A synthetic client address for in-process calls from one app, so the
/// target's per-address limit counts each calling app on its own. It is a
/// unique-local IPv6 address whose /64 is derived from the caller's ID; no
/// packet is ever sent to it.
fn caller_address(deployment_id: &str) -> IpAddr {
    let digest = super::mcp::digest(deployment_id.as_bytes());
    let bytes = hex::decode(&digest[..14]).unwrap_or_default();
    let mut octets = [0u8; 16];
    octets[0] = 0xfd;
    octets[1..8].copy_from_slice(&bytes[..7]);
    octets[15] = 1;
    IpAddr::V6(Ipv6Addr::from(octets))
}

async fn process(state: &AppState, job: Claimed) -> Result<()> {
    let data_root = state.data_root.clone();
    let deployment = state.deployments.read().await.values().find(|deployment| deployment.id == job.deployment).cloned();
    let Some(deployment) = deployment else {
        let attempt = Attempt { error: Some("app-gone"), ..Default::default() };
        return finish(&data_root, &job.id, &attempt, None);
    };
    // A retired origin never resumes, so its calls end; the app's new home
    // does not have them.
    if super::migration::marker(&data_root, &deployment.id).is_some_and(|marker| marker.retired()) {
        let attempt = Attempt { error: Some("app-moved"), ..Default::default() };
        return finish(&data_root, &job.id, &attempt, None);
    }
    // A paused app sends nothing; the call waits uncounted.
    if super::migration::pause(&data_root, &deployment.id).is_some() {
        return release(&data_root, &job.id, PAUSE_WAIT_MS, false, &Attempt::default());
    }
    let program: ActionProgram = serde_json::from_str(&deployment.actions)?;
    let call = program.actions.get(&job.action).and_then(|action| check_kind(&job.action, action).ok().flatten());
    let Some(call) = call else {
        let attempt = Attempt { error: Some("action-gone"), ..Default::default() };
        return finish(&data_root, &job.id, &attempt, None);
    };
    let (url, body) = request_parts(&call, &job.input);
    let extra_headers = request_headers(&call, &job.input);
    let mut attempt = Attempt { method: Some(call.method.clone()), url: Some(url.to_string()), ..Default::default() };
    let mut retry = false;
    match send(state, &deployment, &call, &url, &body, &extra_headers).await {
        Ok((status, answer)) => {
            attempt.status = Some(status);
            attempt.answer = answer;
        }
        Err((kind, retryable)) => {
            attempt.error = Some(kind);
            retry = retryable;
        }
    }
    if !attempt.ok() && attempt.retryable(retry) && job.attempts < MAX_ATTEMPTS {
        let backoff = FIRST_BACKOFF_MS << (job.attempts - 1).clamp(0, 10);
        return release(&data_root, &job.id, backoff, true, &attempt);
    }
    let then = match &call.then {
        Some(then) => Some((then.action.clone(), run_then(state, &deployment, &program, then, &job.input, &attempt).await)),
        None => None,
    };
    let then = then.as_ref().map(|(action, (state, message))| (action.as_str(), *state, message.as_deref()));
    finish(&data_root, &job.id, &attempt, then)
}

/// Sends one attempt. `Ok((status, answer))` once an answer arrived;
/// `Err((kind, retry))` when none did.
async fn send(
    state: &AppState,
    deployment: &Deployment,
    call: &Call,
    url: &url::Url,
    body: &[u8],
    extra_headers: &[(String, String)],
) -> Result<(u16, Vec<u8>), (&'static str, bool)> {
    let target = target(state, url).await?;
    let secret = match &call.auth {
        None => None,
        Some(auth) => {
            let found = super::mcp::store(state).and_then(|db| super::http::project_secret(&db, &deployment.project_id, auth.secret()));
            match found {
                Ok(Some(value)) => Some(value),
                Ok(None) => return Err(("secret-unset", false)),
                Err(_) => return Err(("secret-unavailable", true)),
            }
        }
    };
    let path = url.path().to_owned();
    let query = url.query().unwrap_or("").to_owned();
    let mut headers: Vec<(String, String)> = vec![("content-type".into(), "application/json".into())];
    headers.extend(extra_headers.iter().cloned());
    match (&call.auth, secret) {
        (Some(Auth::Sign { header, .. }), Some(secret)) => {
            let signed = super::http::SignedRequest { method: &call.method, path: &path, query: &query, body };
            headers.push((header.clone(), super::http::sign(&secret, now_ms() / 1000, &nonce(), &signed)));
        }
        (Some(Auth::Key { .. }), Some(secret)) => headers.push(("authorization".into(), format!("Bearer {secret}"))),
        _ => {}
    }
    match target {
        Target::App(app) => Ok(send_in_process(state, deployment, &app, &call.method, &path, &query, &headers, body).await?),
        Target::Outside(address) => send_outside(url, address, &call.method, &headers, body).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_in_process(
    state: &AppState,
    caller: &Deployment,
    app: &Deployment,
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), (&'static str, bool)> {
    // A paused app answers every request from outside with its maintenance
    // refusal; a call from inside meets the same.
    if super::migration::pause(&state.data_root, &app.id).is_some() {
        return Ok((503, br#"{"error":"This app is paused."}"#.to_vec()));
    }
    let mut map = HeaderMap::new();
    map.insert(header::HOST, HeaderValue::from_str(&app.hostname).map_err(|_| ("app-not-found", false))?);
    for (name, value) in headers {
        let name = axum::http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| ("invalid-header", false))?;
        map.insert(name, HeaderValue::from_str(value).map_err(|_| ("invalid-header", false))?);
    }
    let response = super::http::call_route(state, app, method, path, query, body, &map, caller_address(&caller.id)).await;
    let status = response.status().as_u16();
    match axum::body::to_bytes(response.into_body(), MAX_ANSWER_BYTES).await {
        Ok(bytes) => Ok((status, bytes.to_vec())),
        Err(_) => Err(("answer-too-large", false)),
    }
}

/// Sends one attempt to a host outside the installation, connected to the
/// address `target` checked. Redirects are not followed: a 3xx is the answer.
pub(crate) async fn send_outside(
    url: &url::Url,
    address: SocketAddr,
    method: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), (&'static str, bool)> {
    let host = url.host_str().unwrap_or("");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(TIMEOUT)
        .user_agent("Littlebox-Kernel")
        .resolve(host, address)
        .build()
        .map_err(|_| ("network", true))?;
    let mut request = if method == "GET" { client.get(url.clone()) } else { client.post(url.clone()).body(body.to_vec()) };
    for (name, value) in headers {
        if method == "GET" && name == "content-type" {
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    let failure = |error_value: reqwest::Error| if error_value.is_timeout() { ("timeout", true) } else { ("network", true) };
    let mut response = request.send().await.map_err(failure)?;
    let status = response.status().as_u16();
    if response.content_length().is_some_and(|length| length as usize > MAX_ANSWER_BYTES) {
        return Err(("answer-too-large", false));
    }
    let mut answer = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failure)? {
        if answer.len() + chunk.len() > MAX_ANSWER_BYTES {
            return Err(("answer-too-large", false));
        }
        answer.extend_from_slice(&chunk);
    }
    Ok((status, answer))
}

/// The document a `then` mutation's pointers read.
fn answer_document(input: &Value, attempt: &Attempt) -> Value {
    let body = serde_json::from_slice::<Value>(&attempt.answer).ok().filter(|_| !attempt.answer.is_empty());
    json!({
        "ok": attempt.ok(),
        "status": attempt.status,
        "error": attempt.error,
        "body": body,
        "input": input,
    })
}

/// Runs the call's `then` mutation once, with no actor, under the
/// Deployment's database lock. Returns the outcome the outbox keeps: done,
/// refused with the guard, or failed with the reason.
async fn run_then(
    state: &AppState,
    deployment: &Deployment,
    program: &ActionProgram,
    then: &Then,
    input: &Value,
    attempt: &Attempt,
) -> (&'static str, Option<String>) {
    let Some(mutation) = program.actions.get(&then.action) else {
        return ("failed", Some(format!("Action {} is not declared.", then.action)));
    };
    let document = answer_document(input, attempt);
    let mut values = serde_json::Map::new();
    for (key, pointer) in &then.inputs {
        let Some(value) = document.pointer(pointer).filter(|value| !value.is_null()) else { continue };
        let declared = mutation
            .bindings
            .iter()
            .find(|binding| binding.key.as_deref() == Some(key.as_str()))
            .and_then(|binding| binding.value_type.as_deref());
        // A string input takes a number, a boolean or a structure as its JSON text.
        let value = match (declared, value) {
            (Some("string"), Value::String(_)) => value.clone(),
            (Some("string"), other) => Value::String(other.to_string()),
            _ => value.clone(),
        };
        values.insert(key.clone(), value);
    }
    let values = Value::Object(values);
    let params = match resolve_bindings(mutation, &values, None, None) {
        Ok(params) => params,
        Err(message) => return ("failed", Some(message)),
    };
    let result = {
        let _guard = database_lock(state, &deployment.id).await;
        run_action(database_path(state, &deployment.id), mutation, params).await
    };
    match result {
        Ok(value) => match super::guard::outcome(&then.action, mutation, &value) {
            super::guard::Outcome::Refused(message) => {
                super::usage::execution(&state.data_root, &deployment.id, &then.action, "continuation", "refused");
                super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Refused(then.action.clone()));
                ("refused", Some(message))
            }
            outcome => {
                super::usage::execution(&state.data_root, &deployment.id, &then.action, "continuation", "ok");
                super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Action(then.action.clone()));
                if matches!(outcome, super::guard::Outcome::Changed) {
                    super::usage::signal(&state.data_root, &deployment.id, &then.action, mutation, &values, None);
                    super::notify::record(
                        state, deployment, &then.action, mutation, &values,
                        super::notify::Caller { account: None, actor_id: None },
                    ).await;
                }
                ("done", None)
            }
        },
        Err(error_value) => {
            super::usage::execution(&state.data_root, &deployment.id, &then.action, "continuation", "failed");
            ("failed", Some(error_value.to_string()))
        },
    }
}

// ---------------------------------------------------------------------------
// The outbox. `mcp.rs` lists `calls_read` for every owner credential, since
// it needs only `read`, and checks the Deployment's project grant.

pub(crate) fn admin_tools() -> Vec<Value> {
    let text = json!({"type": "string", "minLength": 1});
    vec![super::mcp::tool(
        "calls_read",
        "Read a deployment's outbox: the calls its call actions queued, newest first, 50 at a time, with state (pending, delivered or failed), attempts, method, URL, HTTP status, error kind, the first 1 KiB of the answer and what the then mutation did. Pass nextCursor back as cursor for the next page.",
        json!({"deploymentId": text, "cursor": {"type": "string", "minLength": 1, "maxLength": 20}}),
        &["deploymentId"],
        true,
    )]
}

pub(crate) async fn admin_call(state: &AppState, deployment: &Deployment, a: &Value) -> Result<Value> {
    let before = match a.get("cursor").and_then(Value::as_str) {
        None => i64::MAX,
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid cursor. Pass back the nextCursor of an earlier page."))?,
    };
    let data_root = state.data_root.clone();
    let id = deployment.id.clone();
    tokio::task::spawn_blocking(move || read_outbox(&data_root, &id, before)).await?
}

fn read_outbox(data_root: &Path, deployment: &str, before: i64) -> Result<Value> {
    let db = connection(data_root)?;
    let mut query = db.prepare(
        "SELECT rowid, id, action, state, attempts, method, url, status, error, answer, then_action, then_state, then_message,
                created_at, updated_at, next_attempt_at
         FROM calls WHERE deployment = ?1 AND rowid < ?2 ORDER BY rowid DESC LIMIT ?3",
    )?;
    let rows = query
        .query_map(rusqlite::params![deployment, before, OUTBOX_PAGE + 1], |row| {
            let state: String = row.get(3)?;
            let then_action: Option<String> = row.get(10)?;
            Ok((
                row.get::<_, i64>(0)?,
                json!({
                    "callId": row.get::<_, String>(1)?,
                    "action": row.get::<_, String>(2)?,
                    "state": state,
                    "attempts": row.get::<_, i64>(4)?,
                    "method": row.get::<_, Option<String>>(5)?,
                    "url": row.get::<_, Option<String>>(6)?,
                    "status": row.get::<_, Option<i64>>(7)?,
                    "error": row.get::<_, Option<String>>(8)?,
                    "answer": row.get::<_, Option<String>>(9)?,
                    "then": then_action.map(|action| json!({"action": action, "state": row.get::<_, Option<String>>(11).ok().flatten(), "message": row.get::<_, Option<String>>(12).ok().flatten()})),
                    "createdAt": row.get::<_, i64>(13)?,
                    "updatedAt": row.get::<_, i64>(14)?,
                    "nextAttemptAt": if state == "pending" { Some(row.get::<_, i64>(15)?) } else { None },
                }),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() as i64 > OUTBOX_PAGE;
    let page: Vec<_> = rows.into_iter().take(OUTBOX_PAGE as usize).collect();
    let next = if more { page.last().map(|(rowid, _)| rowid.to_string()) } else { None };
    Ok(json!({"calls": page.into_iter().map(|(_, call)| call).collect::<Vec<_>>(), "nextCursor": next}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(actions: &Value) -> ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    /// The Validation Gate runs the same file in src/agent/gate-release.test.ts.
    /// The rules come from the file, not from LITTLEBOX_MCP_ORIGIN, so the
    /// test does not race other tests that set the environment.
    #[test]
    fn release_checks_refuse_what_the_gate_refuses_with_the_same_message() {
        use base64::Engine;
        let file: Value = serde_json::from_str(include_str!("release-cases.json")).unwrap();
        let hosts = file["rules"]["installationHosts"].as_array().unwrap();
        let rules = super::super::domain::Rules {
            suffix: file["rules"]["suffix"].as_str().unwrap().to_owned(),
            installation_hosts: hosts.iter().map(|host| host.as_str().unwrap().to_owned()).collect(),
        };
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for case in cases {
            let name = case["case"].as_str().unwrap();
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL);
                INSERT INTO projects VALUES ('project_a', 'ana', '{}');").unwrap();
            for (index, secret) in case["secrets"].as_array().unwrap().iter().enumerate() {
                // A value that is not base64 when the case says it is no svix key.
                let value = if secret["svixKey"] == json!(false) {
                    format!("whsec_not*base64*{index}")
                } else {
                    format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode(format!("release-case-key-{index}")))
                };
                let set = json!({"name": secret["name"], "value": value, "use": secret["use"]});
                super::super::http::secrets_call(&db, "project_a", None, "secret_set", &set).unwrap();
            }
            let listed = super::super::http::secrets_call(&db, "project_a", None, "secrets_list", &json!({})).unwrap();
            for secret in case["secrets"].as_array().unwrap() {
                let row = listed["secrets"].as_array().unwrap().iter().find(|row| row["name"] == secret["name"]).unwrap();
                assert_eq!(row["use"], secret["use"], "{name}: secrets_list answers the use the case set");
                if let Some(flag) = secret.get("svixKey") {
                    assert_eq!(&row["svixKey"], flag, "{name}: secrets_list answers whether the value is a svix key");
                }
            }
            let outcome = check_release(&rules, &db, "project_a", &program(&case["actions"])).err().map(|error| error.to_string());
            assert_eq!(outcome.as_deref(), case["error"].as_str(), "{name}");
        }
    }

    /// The editor runs the same file in src/kernel/calls.test.ts.
    #[test]
    fn declarations_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("call-cases.json")).unwrap();
        assert!(cases.len() >= 40);
        for case in cases {
            let outcome = if let Some(connects) = case.get("connects") {
                check_connects(&json!({"connects": connects}))
            } else {
                super::super::action_shape::check_program(&program(&case["actions"]))
            };
            match case["error"].as_str() {
                None => assert_eq!(outcome, Ok(()), "{}", case["case"]),
                Some(message) => assert_eq!(outcome, Err(message.to_owned()), "{}", case["case"]),
            }
        }
    }

    #[test]
    fn private_loopback_link_local_and_disguised_addresses_are_not_public() {
        let refused = [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "0.1.2.3",
            "255.255.255.255", "224.0.0.1", "240.0.0.1", "192.0.0.8", "198.18.0.1", "192.0.2.1", "::1", "::", "fc00::1",
            "fd00:ec2::254", "fe80::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "64:ff9b::a9fe:a9fe",
            "2001:db8::1", "::127.0.0.1",
        ];
        for address in refused {
            assert!(!public_address(address.parse().unwrap()), "{address}");
        }
        for address in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111", "::ffff:93.184.216.34", "64:ff9b::5db8:d822"] {
            assert!(public_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn the_answer_document_carries_status_body_error_and_input() {
        let attempt = Attempt { status: Some(403), answer: br#"{"error":"No."}"#.to_vec(), ..Default::default() };
        let document = answer_document(&json!({"id": 7}), &attempt);
        assert_eq!(document, json!({"ok": false, "status": 403, "error": null, "body": {"error": "No."}, "input": {"id": 7}}));
        let attempt = Attempt { error: Some("timeout"), ..Default::default() };
        assert_eq!(answer_document(&json!({}), &attempt)["error"], "timeout");
        assert!(attempt.retryable(true));
        assert!(!Attempt { status: Some(404), ..Default::default() }.retryable(false));
        assert!(Attempt { status: Some(503), ..Default::default() }.retryable(false));
        assert!(Attempt { status: Some(429), ..Default::default() }.retryable(false));
    }

    #[test]
    fn a_get_carries_its_body_keys_as_query_parameters_and_a_post_as_json() {
        let action: ActionDefinition = serde_json::from_value(json!({
            "kind": "call", "access": "anonymous", "method": "GET", "url": "https://api.example.com/v1/items?fixed=1",
            "bindings": [{"source": "input", "key": "q", "type": "string"}, {"source": "input", "key": "n", "type": "number", "nullable": true}],
            "body": {"query": "q", "limit": "n", "source": {"value": "tasks"}}
        }))
        .unwrap();
        let call = declaration("items.find", &action).unwrap();
        let (url, body) = request_parts(&call, &json!({"q": "a b", "n": null}));
        assert_eq!(url.as_str(), "https://api.example.com/v1/items?fixed=1&query=a+b&source=tasks");
        assert!(body.is_empty());
        let post = Call { method: "POST".into(), ..call };
        let (_, body) = request_parts(&post, &json!({"q": "x", "n": 3}));
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"query": "x", "limit": 3, "source": "tasks"}));
    }

    #[test]
    fn a_post_expands_dotted_body_keys_and_sets_idempotency_key() {
        let action: ActionDefinition = serde_json::from_value(json!({
            "kind": "call", "access": "authenticated", "method": "POST", "url": "https://api.stripe.com/v1/checkout/sessions",
            "bindings": [
                {"source": "input", "key": "orderId", "type": "string"},
                {"source": "input", "key": "qty", "type": "number"},
                {"source": "input", "key": "price", "type": "string"},
                {"source": "input", "key": "idempotency", "type": "string"}
            ],
            "body": {
                "mode": {"value": "payment"},
                "client_reference_id": "orderId",
                "line_items.0.price": "price",
                "line_items.0.quantity": "qty"
            },
            "headers": {"Idempotency-Key": "idempotency"},
            "auth": {"key": "stripe_secret"}
        }))
        .unwrap();
        let call = declaration("orders.checkout", &action).unwrap();
        let (_, body) = request_parts(&call, &json!({"orderId": "42", "qty": 2, "price": "price_print", "idempotency": "order-42"}));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({
                "mode": "payment",
                "client_reference_id": "42",
                "line_items": [{"price": "price_print", "quantity": 2}]
            })
        );
        assert_eq!(request_headers(&call, &json!({"idempotency": "order-42"})), vec![("Idempotency-Key".into(), "order-42".into())]);
    }

    #[test]
    fn each_calling_app_gets_its_own_synthetic_client_address() {
        let first = caller_address("dep_one");
        assert_eq!(first, caller_address("dep_one"));
        assert_ne!(super::super::limits::client_key(first), super::super::limits::client_key(caller_address("dep_two")));
        assert!(!public_address(first));
    }

    /// A one-request HTTP server on loopback. `send_outside` takes the address
    /// it connects to, so it can be pointed here without the address check.
    async fn server(answer: Vec<u8>, delay: Duration) -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 8192];
            let read = socket.read(&mut request).await.unwrap();
            tokio::time::sleep(delay).await;
            let _ = socket.write_all(&answer).await;
            request.truncate(read);
            request
        });
        (address, handle)
    }

    #[tokio::test]
    async fn an_outside_call_reads_the_answer_follows_no_redirect_and_caps_the_body() {
        let url = url::Url::parse("https://api.example.com/hook").unwrap();
        let plain = url::Url::parse("http://api.example.com/hook").unwrap();
        let (address, request) = server(b"HTTP/1.1 201 Created\r\ncontent-length: 11\r\n\r\n{\"ok\":true}".to_vec(), Duration::ZERO).await;
        let answer = send_outside(&plain, address, "POST", &[("x-sig".into(), "t=1".into())], b"{}").await.unwrap();
        assert_eq!(answer, (201, br#"{"ok":true}"#.to_vec()));
        let sent = String::from_utf8(request.await.unwrap()).unwrap();
        assert!(sent.starts_with("POST /hook HTTP/1.1"), "{sent}");
        assert!(sent.to_ascii_lowercase().contains("x-sig: t=1"), "{sent}");

        let (address, _) = server(b"HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/\r\ncontent-length: 0\r\n\r\n".to_vec(), Duration::ZERO).await;
        assert_eq!(send_outside(&plain, address, "GET", &[], b"").await.unwrap().0, 302);

        let big = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}", MAX_ANSWER_BYTES + 1, "x".repeat(MAX_ANSWER_BYTES + 1));
        let (address, _) = server(big.into_bytes(), Duration::ZERO).await;
        assert_eq!(send_outside(&plain, address, "GET", &[], b"").await, Err(("answer-too-large", false)));

        // https to a server that speaks plain HTTP fails as a network error, which retries.
        let (address, _) = server(b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), Duration::ZERO).await;
        assert_eq!(send_outside(&url, address, "GET", &[], b"").await.unwrap_err().1, true);
    }

    fn test_state(root: &Path, public_origin: &str) -> AppState {
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(super::super::copies::CopyStatus::default()),
            admin_key: "test-admin".into(),
            accounts: super::super::accounts::AccountSettings::local(),
            public_origin: public_origin.into(),
            host_suffix: "apps.example.org".into(),
            guards: Arc::new(super::super::limits::Guards::new(Vec::new())),
        }
    }

    async fn publish(state: &AppState, actions: Value, schema: &str) -> Result<Deployment, String> {
        let body = json!({
            "project": {"id": "caller", "name": "Caller", "slug": "caller", "manifest": {},
                "files": {"actions.json": actions.to_string(), "schema.sql": schema}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>caller</p>"}}
        });
        let request = Request::builder().header("x-littlebox-admin-key", "test-admin").body(Body::from(body.to_string())).unwrap();
        let response = deploy(State(state.clone()), request).await;
        if response.status() != StatusCode::OK {
            let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
            return Err(serde_json::from_slice::<Value>(&bytes).unwrap()["error"].as_str().unwrap_or("").to_owned());
        }
        Ok(state.deployments.read().await.values().next().cloned().unwrap())
    }

    fn call_action(url: &str) -> Value {
        json!({"version": 1, "actions": {
            "hook.send": {"kind": "call", "access": "anonymous", "method": "POST", "url": url, "bindings": [],
                "then": {"action": "hook.logged", "inputs": {"status": "/status", "error": "/error"}}},
            "hook.logged": {"kind": "mutation", "access": "anonymous", "statement": "INSERT INTO log (status, error) VALUES (?1, ?2)",
                "bindings": [{"source": "input", "key": "status", "type": "number", "nullable": true},
                             {"source": "input", "key": "error", "type": "string", "nullable": true}]}
        }})
    }

    #[tokio::test]
    async fn a_call_that_keeps_failing_retries_with_backoff_then_fails_and_runs_its_then_once() {
        let root = std::env::temp_dir().join(format!("littlebox-calls-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
        let state = test_state(&root, "https://editor.example.org");
        // .invalid never resolves (RFC 2606), so every attempt fails DNS, which retries.
        let app = publish(&state, call_action("https://hooks.invalid/send"), "CREATE TABLE IF NOT EXISTS log (status INTEGER, error TEXT);").await.unwrap();
        let queued = enqueue(&state, &app, "hook.send", &json!({})).await;
        assert_eq!(queued.status(), StatusCode::OK);
        let row = |root: &Path| {
            connection(root).unwrap().query_row("SELECT state, attempts, error, next_attempt_at - updated_at FROM calls", [], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, i64>(3)?))
            }).unwrap()
        };
        let logged = |root: &Path| -> Vec<(Option<i64>, Option<String>)> {
            let db = Connection::open(root.join("deployments").join(&app.id).join("database.sqlite")).unwrap();
            let mut query = db.prepare("SELECT status, error FROM log").unwrap();
            query.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        for attempt in 1..MAX_ATTEMPTS {
            assert_eq!(run_once(&state).await.unwrap(), 1);
            let (state_name, attempts, error, wait) = row(&root);
            assert_eq!((state_name.as_str(), attempts, error.as_deref()), ("pending", attempt, Some("dns-failed")));
            assert_eq!(wait, FIRST_BACKOFF_MS << (attempt - 1), "attempt {attempt} waits twice as long as the one before");
            assert!(logged(&root).is_empty(), "the then mutation waits for the last attempt");
            // Not due yet: a pass leaves it alone.
            assert_eq!(run_once(&state).await.unwrap(), 0);
            connection(&root).unwrap().execute("UPDATE calls SET next_attempt_at = 0", []).unwrap();
        }
        assert_eq!(run_once(&state).await.unwrap(), 1);
        assert_eq!(row(&root).0, "failed");
        assert_eq!(row(&root).1, MAX_ATTEMPTS);
        assert_eq!(logged(&root), vec![(None, Some("dns-failed".to_owned()))]);
        assert_eq!(run_once(&state).await.unwrap(), 0, "a finished call is never sent again");
        let outbox = read_outbox(&root, &app.id, i64::MAX).unwrap();
        assert_eq!(outbox["calls"][0]["then"], json!({"action": "hook.logged", "state": "done", "message": null}));

        // A paused app keeps its call waiting, uncounted; a retired origin ends it.
        super::super::migration::pause_for_test(&root, &app.id, true);
        assert_eq!(enqueue(&state, &app, "hook.send", &json!({})).await.status(), StatusCode::OK);
        assert_eq!(run_once(&state).await.unwrap(), 1);
        let waiting = connection(&root).unwrap().query_row(
            "SELECT state, attempts FROM calls ORDER BY rowid DESC LIMIT 1", [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).unwrap();
        assert_eq!(waiting, ("pending".to_owned(), 0));
        super::super::migration::retire_for_test(&root, &app.id);
        connection(&root).unwrap().execute("UPDATE calls SET next_attempt_at = 0", []).unwrap();
        assert_eq!(run_once(&state).await.unwrap(), 1);
        let moved = read_outbox(&root, &app.id, i64::MAX).unwrap();
        assert_eq!((moved["calls"][0]["state"].as_str(), moved["calls"][0]["error"].as_str()), (Some("failed"), Some("app-moved")));
        assert_eq!(logged(&root).len(), 1, "a moved app runs no then mutation");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_installation_rules_refuse_at_release_and_a_private_answer_refuses_at_run() {
        let root = std::env::temp_dir().join(format!("littlebox-calls-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("deployments")).await.unwrap();
        let state = test_state(&root, "https://editor.example.org");
        let schema = "CREATE TABLE IF NOT EXISTS log (status INTEGER, error TEXT);";
        assert_eq!(
            publish(&state, call_action("https://editor.example.org/oauth2/x"), schema).await.err().unwrap(),
            "Action hook.send calls editor.example.org, the editor's host. A call reaches an app or an outside service, never the editor."
        );
        assert_eq!(
            publish(&state, call_action("http://api.example.com/send"), schema).await.err().unwrap(),
            "Action hook.send calls http://api.example.com, which is not an app of this installation. Use https."
        );
        let mut keyed = call_action("https://api.example.com/send");
        keyed["actions"]["hook.send"]["auth"] = json!({"key": "api_key"});
        assert_eq!(
            publish(&state, keyed, schema).await.err().unwrap(),
            "Action hook.send calls with the secret api_key, which is not set for this project. Set it with secret_set before publishing."
        );
        // http to a name under the installation's suffix passes; no app holds it, so the call fails at once.
        assert!(publish(&state, call_action("http://gone-1.apps.example.org/send"), schema).await.is_ok());
        let url = |text: &str| url::Url::parse(text).unwrap();
        assert!(matches!(target(&state, &url("http://gone-1.apps.example.org/send")).await, Err(("app-not-found", false))));
        assert!(matches!(target(&state, &url("https://editor.example.org/x")).await, Err(("editor-host", false))));
        // localhost resolves on every machine without a network: to loopback, which no call may reach.
        assert!(matches!(target(&state, &url("https://localhost/x")).await, Err(("address-refused", false))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_outside_call_that_answers_too_late_times_out() {
        let plain = url::Url::parse("http://api.example.com/slow").unwrap();
        let (address, _) = server(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n".to_vec(), TIMEOUT + Duration::from_secs(2)).await;
        let started = std::time::Instant::now();
        assert_eq!(send_outside(&plain, address, "GET", &[], b"").await, Err(("timeout", true)));
        assert!(started.elapsed() < TIMEOUT + Duration::from_secs(2));
    }
}
