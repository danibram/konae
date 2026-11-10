//! The declaration of an action of `"kind": "call"`, copied from
//! kernel-rs/src/calls.rs at 8ecb7e5. The outbox, the worker, DNS and the
//! installation's host rules stay in the Kernel.

use super::refusal::{refuse, Refusal};
use super::*;
const MAX_URL_CHARS: usize = 2048;
const MAX_BODY_KEYS: usize = 32;
const MAX_POINTER_CHARS: usize = 200;
const RESERVED_SEGMENTS: [&str; 4] = ["_konae", "oauth", ".well-known", "mcp"];
const ANSWER_ROOTS: [&str; 5] = ["ok", "status", "error", "body", "input"];
const DEFAULT_SIGNATURE_HEADER: &str = "X-Konae-Signature";

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
    /// Sign with Konae's route scheme, in `header`, with the secret's value.
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
pub(crate) fn check_kind(name: &str, action: &ActionDefinition) -> Result<Option<Call>, Refusal> {
    if action.kind != "call" {
        if action.call.any() {
            let kind = match action.kind.as_str() {
                "query" => "query",
                "mutation" => "mutation",
                _ => "signal action",
            };
            return Err(refuse("call.keys-on-other-kind", format!(
                r#"Action {name} is a {kind}, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call"."#
            )));
        }
        return Ok(None);
    }
    declaration(name, action).map(Some)
}

/// Parses and checks one call.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Call, Refusal> {
    if !action.statement.trim().is_empty() || action.signal.is_some() || action.guard.is_some() || action.http.is_some() {
        return Err(refuse("call.keys-refused", format!(
            "Action {name} is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route."
        )));
    }
    if action.bindings.iter().any(|binding| binding.source != "input") {
        return Err(refuse("call.bindings-input-only", format!("Action {name} is a call, so its bindings may only be inputs.")));
    }
    let method = match action.call.method.as_ref().and_then(Value::as_str) {
        Some("GET") => "GET",
        Some("POST") => "POST",
        _ => return Err(refuse("call.method-invalid", format!(r#"Action {name} must call with "method": "GET" or "POST"."#))),
    };
    let url = static_url(name, action.call.url.as_ref())?;
    let inputs: Vec<&str> = action.bindings.iter().filter_map(|binding| binding.key.as_deref()).collect();
    let body = body_map(name, method, action.call.body.as_ref(), &inputs)?;
    let headers = headers_map(name, action.call.headers.as_ref(), &inputs)?;
    let auth = auth(name, action.call.auth.as_ref())?;
    let then = then(name, action.call.then.as_ref(), &inputs)?;
    Ok(Call { url, method: method.to_owned(), body, headers, auth, then })
}

fn static_url(name: &str, value: Option<&Value>) -> Result<url::Url, Refusal> {
    let invalid = || {
        refuse("call.url-invalid", format!("Action {name} has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters."))
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
        return Err(refuse("call.host-ip", format!("Action {name} calls the IP address {host}. Name a host instead.")));
    }
    let bare = host.trim_end_matches('.');
    if matches!(bare, "localhost" | "metadata" | "instance-data") || bare.ends_with(".internal") {
        return Err(refuse("call.host-local", format!("Action {name} calls {host}, which names this machine or a cloud metadata service.")));
    }
    let first = url.path().split('/').nth(1).unwrap_or("").to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&first.as_str()) {
        return Err(refuse("call.path-reserved", format!(
            "Action {name} calls a path under /_konae, /oauth, /.well-known or /mcp, which belong to the Kernel."
        )));
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

fn body_map(name: &str, method: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Vec<(String, BodyValue)>, Refusal> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let invalid = || {
        refuse("call.body-invalid", format!(r#"Action {name} has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32."#))
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
            return Err(refuse("call.body-path-on-get", format!(
                "Action {name} sends body key {key} with a path. Nested body keys belong on POST."
            )));
        }
        for other in keys.iter().skip(index + 1) {
            if other.starts_with(&format!("{key}.")) {
                return Err(refuse("call.body-overlap", format!(
                    "Action {name} has body keys {key} and {other}, which overlap. Use one path or the other."
                )));
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
            return Err(refuse("call.body-unbound", format!("Action {name} sends body key {key} from {input}, which is not one of its input bindings.")));
        }
        body.push((key.clone(), BodyValue::Input(input.to_owned())));
    }
    Ok(body)
}

/// The only request header a call may set besides auth and content-type.
const ALLOWED_CALL_HEADER: &str = "Idempotency-Key";
const MAX_HEADERS: usize = 8;

fn headers_map(name: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Vec<(String, BodyValue)>, Refusal> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let invalid = || {
        refuse("call.headers-invalid", format!(r#"Action {name} has an invalid headers. Map Idempotency-Key to an input or a constant, at most 8."#))
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
            return Err(refuse("call.header-refused", format!(
                "Action {name} sets header {key}, which is not allowed. Only Idempotency-Key is allowed."
            )));
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
            return Err(refuse("call.header-unbound", format!("Action {name} sends header {key} from {input}, which is not one of its input bindings.")));
        }
        headers.push((key.clone(), BodyValue::Input(input.to_owned())));
    }
    Ok(headers)
}

fn auth(name: &str, value: Option<&Value>) -> Result<Option<Auth>, Refusal> {
    let Some(value) = value else { return Ok(None) };
    let invalid = || {
        refuse("call.auth-invalid", format!(r#"Action {name} has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds."#))
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
        return Err(refuse("call.secret-invalid", format!(
            r#"Action {name} names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter."#
        )));
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
                return Err(refuse("call.signature-header-invalid", format!("Action {name} has an invalid signature header.")));
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

fn then(name: &str, value: Option<&Value>, inputs: &[&str]) -> Result<Option<Then>, Refusal> {
    let Some(value) = value else { return Ok(None) };
    let invalid = || refuse("call.then-invalid", format!("Action {name} has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer."));
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
            return Err(refuse("call.then-pointer-invalid", format!(
                "Action {name} reads {pointer} for then, which is not a JSON pointer into /ok, /status, /error, /body or /input."
            )));
        }
        if segments[0] == "input" && segments.len() > 1 {
            let key = segments[1].replace("~1", "/").replace("~0", "~");
            if !inputs.contains(&key.as_str()) {
                return Err(refuse("call.then-input-unbound", format!("Action {name} reads {pointer} for then, but {key} is not one of its input bindings.")));
            }
        }
    }
    Ok(Some(Then { action: action.to_owned(), inputs: pairs }))
}

/// Checks each call's `then` against the program, in sorted name order. Runs
/// after every action passed its own checks and no two routes collide.
pub(crate) fn check_program(program: &ActionProgram) -> Result<(), Refusal> {
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Ok(Some(call)) = check_kind(name, &program.actions[name]) else { continue };
        let Some(then) = call.then else { continue };
        let target = &then.action;
        let Some(mutation) = program.actions.get(target) else {
            return Err(refuse("call.then-undeclared", format!("Action {name} names {target} as its then, which is not declared.")));
        };
        if mutation.kind != "mutation" {
            return Err(refuse("call.then-not-mutation", format!("Action {name} names {target} as its then, which is not a mutation.")));
        }
        if mutation.http.is_some() {
            return Err(refuse("call.then-routed", format!(
                "Action {name} names {target} as its then, which answers on an HTTP route. A then mutation runs only after its call."
            )));
        }
        if mutation.bindings.iter().any(|binding| binding.source == "actor") {
            return Err(refuse("call.then-binds-actor", format!(
                "Action {name} names {target} as its then, which binds the actor. A then mutation runs with no caller."
            )));
        }
        let inputs: Vec<&ActionBinding> = mutation.bindings.iter().filter(|binding| binding.source == "input").collect();
        for (key, _) in &then.inputs {
            if !inputs.iter().any(|binding| binding.key.as_deref() == Some(key.as_str())) {
                return Err(refuse("call.then-fills-unknown", format!("Action {name} fills {key}, which is not an input of {target}.")));
            }
        }
        let mut required: Vec<&str> =
            inputs.iter().filter(|binding| !binding.nullable).filter_map(|binding| binding.key.as_deref()).collect();
        required.sort();
        for key in required {
            if !then.inputs.iter().any(|(filled, _)| filled == key) {
                return Err(refuse("call.then-unfilled", format!(
                    "Action {name} leaves {key} of {target} unfilled. Fill it from the answer or make the input nullable."
                )));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The manifest's `connects`: origins the page may reach with fetch, added to
// its connect-src. The same checks and messages as `checkConnects` in
// src/kernel/calls.ts.

const MAX_CONNECTS: usize = 10;

pub(crate) fn check_connects(manifest: &Value) -> Result<(), Refusal> {
    let Some(value) = manifest.get("connects").filter(|value| !value.is_null()) else { return Ok(()) };
    let invalid = || refuse("connects.invalid", r#"connects must be a list of at most 10 origins, such as ["https://api.example.com"]."#);
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
            return Err(refuse("connects.origin-invalid", format!(
                "connects lists {entry}, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost."
            )));
        }
        if seen.contains(&entry) {
            return Err(refuse("connects.origin-twice", format!("connects lists {entry} twice.")));
        }
        seen.push(entry);
    }
    Ok(())
}
