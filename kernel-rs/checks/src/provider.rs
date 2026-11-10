//! A route's `signature` and `fields`, copied from kernel-rs/src/provider.rs
//! at 8ecb7e5. Verifying a request's MAC, which needs ring and the clock,
//! stays in the Kernel.

use super::refusal::{refuse, Refusal};
use super::*;

/// The Svix timestamp window, as Svix's own libraries use.
pub(super) const SVIX_WINDOW_SECONDS: u64 = 300;
/// The largest `window` a `timestamped` signature may declare.
const MAX_WINDOW_SECONDS: u64 = 3600;
/// Signature values one header may carry, so a header cannot ask for one MAC
/// per value. A rotation sends two.
const MAX_PROVIDER_TAGS: usize = 4;
const MAX_FIELDS: usize = 32;
const MAX_PATH_STEPS: usize = 16;
const MAX_PATH_LENGTH: usize = 200;
const SVIX_HEADERS: [&str; 3] = ["svix-id", "svix-timestamp", "svix-signature"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Algorithm {
    Sha256,
    Sha1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Encoding {
    Hex,
    Base64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Scheme {
    Konae,
    HmacBody { algorithm: Algorithm, encoding: Encoding, prefix: Option<String> },
    Timestamped { prefix: String, tag: String, window: u64 },
    Svix,
}

// ---------------------------------------------------------------------------
// Declarations.

/// The keys each scheme takes, in the order its refusal lists them.
fn scheme_keys(scheme: &str) -> &'static [&'static str] {
    match scheme {
        "konae" => &["scheme", "header", "secret"],
        "hmac-body" => &["scheme", "header", "secret", "algorithm", "encoding", "prefix"],
        "timestamped" => &["scheme", "header", "secret", "prefix", "tag", "window"],
        _ => &["scheme", "secret"],
    }
}

fn quoted_list(keys: &[&str]) -> String {
    let quoted = keys.iter().map(|key| format!("\"{key}\"")).collect::<Vec<_>>();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

const UNSIGNABLE_HEADERS: [&str; 5] = ["authorization", "cookie", "host", "content-type", "content-length"];

fn valid_header(header: &str) -> bool {
    !header.is_empty()
        && header.len() <= 64
        && header.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !UNSIGNABLE_HEADERS.contains(&header.to_ascii_lowercase().as_str())
}

fn valid_secret_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// `t=` or `v1=`: a field name of letters, digits, `_` and `-`, then `=`.
fn valid_field_prefix(prefix: &str) -> bool {
    (2..=16).contains(&prefix.len())
        && prefix.ends_with('=')
        && prefix[..prefix.len() - 1].bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_body_prefix(prefix: &str) -> bool {
    (1..=32).contains(&prefix.len()) && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"=-_.:".contains(&byte))
}

/// The providers a route may name instead of spelling out a scheme, each the
/// long form it stands for without its secret. src/kernel/route-declaration.ts
/// imports the same file, so both doors expand a preset alike.
const PRESETS: &str = include_str!("../../src/signature-presets.json");

fn presets() -> &'static serde_json::Map<String, Value> {
    static TABLE: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(PRESETS).expect("signature-presets.json is an object of long-form signatures"))
}

fn quoted_choices(keys: &[&str]) -> String {
    let quoted = keys.iter().map(|key| format!("\"{key}\"")).collect::<Vec<_>>();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Parses a route's `signature`. A `provider` names a row of the preset table
/// and takes only a `secret` beside it; it then parses as the long form that
/// row spells. Returns the header the scheme reads its MAC from, the secret's
/// name and the scheme.
pub(super) fn signature(name: &str, method: &str, value: &Value) -> Result<(String, String, Scheme), Refusal> {
    let record = value.as_object().ok_or_else(|| refuse("route.invalid", format!("Action {name} has an invalid http binding.")))?;
    let Some(provider) = record.get("provider") else { return long_form(name, method, record) };
    let table = presets();
    let Some((provider, row)) = provider.as_str().and_then(|provider| table.get_key_value(provider)) else {
        let mut providers = table.keys().map(String::as_str).collect::<Vec<_>>();
        providers.sort();
        let choices = quoted_choices(&providers);
        return Err(refuse("signature.provider-unknown", format!("Action {name} names an unknown signature provider. Use {choices}.")));
    };
    let mut extra = record.keys().map(String::as_str).filter(|key| !["provider", "secret"].contains(key)).collect::<Vec<_>>();
    if !extra.is_empty() {
        extra.sort();
        let remove = quoted_list(&extra);
        return Err(refuse("signature.provider-extra-keys", format!(
            r#"Action {name} names the signature provider "{provider}", so its signature takes only "provider" and "secret". Remove {remove}."#
        )));
    }
    let mut expanded = row.as_object().cloned().unwrap_or_default();
    if let Some(secret) = record.get("secret") {
        expanded.insert("secret".into(), secret.clone());
    }
    long_form(name, method, &expanded)
}

/// Parses a long-form `signature`: its scheme, then every key that scheme
/// takes. An unknown scheme, and a provider's scheme on GET, stop the parse;
/// every other wrong key adds its sentence, so one refusal names them all.
fn long_form(name: &str, method: &str, record: &serde_json::Map<String, Value>) -> Result<(String, String, Scheme), Refusal> {
    let scheme = match record.get("scheme") {
        None => "konae",
        Some(Value::String(scheme)) if ["konae", "hmac-body", "timestamped", "svix"].contains(&scheme.as_str()) => scheme.as_str(),
        Some(_) => {
            return Err(refuse("signature.scheme-unknown", format!(
                r#"Action {name} names an unknown signature scheme. Use "konae", "hmac-body", "timestamped" or "svix"."#
            )));
        }
    };
    let mut problems: Vec<Refusal> = Vec::new();
    let keys = scheme_keys(scheme);
    if record.keys().any(|key| !keys.contains(&key.as_str())) {
        let takes = quoted_list(keys);
        problems.push(refuse("signature.keys-invalid", format!("Action {name} has an invalid {scheme} signature. It takes {takes}.")));
    }
    if method == "GET" && scheme != "konae" {
        problems.push(refuse("signature.provider-on-get", format!(
            "Action {name} binds GET, so its signature must use the konae scheme. A provider's scheme signs a POST body."
        )));
        return Err(Refusal::join(problems));
    }
    let header = if scheme == "svix" {
        Some("svix-signature")
    } else {
        record.get("header").and_then(Value::as_str).filter(|header| valid_header(header))
    };
    if header.is_none() {
        problems.push(refuse("signature.header-invalid", format!("Action {name} has an invalid signature header.")));
    }
    let secret = record.get("secret").and_then(Value::as_str).filter(|secret| valid_secret_name(secret));
    if secret.is_none() {
        problems.push(refuse("signature.secret-invalid", format!(
            r#"Action {name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter."#
        )));
    }
    let text = |key: &str| record.get(key).and_then(Value::as_str);
    let parsed = match scheme {
        "hmac-body" => {
            let algorithm = match text("algorithm") {
                Some("sha256") => Some(Algorithm::Sha256),
                Some("sha1") => Some(Algorithm::Sha1),
                _ => None,
            };
            if algorithm.is_none() {
                problems.push(refuse("signature.algorithm-invalid", format!(r#"Action {name} has an invalid signature algorithm. Use "sha256" or "sha1"."#)));
            }
            let encoding = match text("encoding") {
                Some("hex") => Some(Encoding::Hex),
                Some("base64") => Some(Encoding::Base64),
                _ => None,
            };
            if encoding.is_none() {
                problems.push(refuse("signature.encoding-invalid", format!(r#"Action {name} has an invalid signature encoding. Use "hex" or "base64"."#)));
            }
            let prefix = match record.get("prefix") {
                None => Some(None),
                Some(Value::String(prefix)) if valid_body_prefix(prefix) => Some(Some(prefix.clone())),
                Some(_) => None,
            };
            if prefix.is_none() {
                problems.push(refuse("signature.prefix-invalid", format!(
                    r#"Action {name} has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256="."#
                )));
            }
            match (algorithm, encoding, prefix) {
                (Some(algorithm), Some(encoding), Some(prefix)) => Some(Scheme::HmacBody { algorithm, encoding, prefix }),
                _ => None,
            }
        }
        "timestamped" => {
            let prefix = text("prefix").filter(|prefix| valid_field_prefix(prefix));
            if prefix.is_none() {
                problems.push(refuse("signature.timestamp-prefix-invalid", format!(r#"Action {name} has an invalid timestamp prefix. Use the field's name and "=", such as "t="."#)));
            }
            let tag = text("tag").filter(|tag| valid_field_prefix(tag) && Some(*tag) != text("prefix"));
            if tag.is_none() {
                problems.push(refuse("signature.tag-invalid", format!(
                    r#"Action {name} has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's."#
                )));
            }
            // 300.0 is the integer 300 to the editor's JSON reader, so it is here too.
            let window = record
                .get("window")
                .and_then(Value::as_f64)
                .filter(|seconds| seconds.fract() == 0.0 && (1.0..=MAX_WINDOW_SECONDS as f64).contains(seconds));
            if window.is_none() {
                problems.push(refuse("signature.window-invalid", format!("Action {name} has an invalid signature window. Use a whole number of seconds from 1 to 3600.")));
            }
            match (prefix, tag, window) {
                (Some(prefix), Some(tag), Some(window)) => {
                    Some(Scheme::Timestamped { prefix: prefix.to_owned(), tag: tag.to_owned(), window: window as u64 })
                }
                _ => None,
            }
        }
        "svix" => Some(Scheme::Svix),
        _ => Some(Scheme::Konae),
    };
    match (header, secret, parsed) {
        (Some(header), Some(secret), Some(parsed)) if problems.is_empty() => Ok((header.to_owned(), secret.to_owned(), parsed)),
        _ => Err(Refusal::join(problems)),
    }
}

/// One step of a JSON path: `.name` or `[index]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Key(String),
    Index(usize),
}

/// `$` and then one to 16 steps, each `.name` with letters, digits, `_` and
/// `-`, or `[index]` with a number below 1000 written without leading zeros.
pub(super) fn json_path(path: &str) -> Option<Vec<Step>> {
    if path.len() > MAX_PATH_LENGTH {
        return None;
    }
    let mut rest = path.strip_prefix('$')?;
    let mut steps = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            let key = &after[..end];
            if key.is_empty() || key.len() > 64 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') {
                return None;
            }
            steps.push(Step::Key(key.to_owned()));
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']')?;
            let digits = &after[..end];
            if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|byte| byte.is_ascii_digit()) || (digits.len() > 1 && digits.starts_with('0')) {
                return None;
            }
            steps.push(Step::Index(digits.parse().ok()?));
            rest = &after[end + 1..];
        } else {
            return None;
        }
    }
    (!steps.is_empty() && steps.len() <= MAX_PATH_STEPS).then_some(steps)
}

/// Parses a route's `fields`: input keys in sorted order, each an input
/// binding of the action, each mapped to a JSON path.
pub(super) fn fields(name: &str, method: &str, action: &ActionDefinition, value: &Value) -> Result<Vec<(String, Vec<Step>)>, Refusal> {
    if method == "GET" {
        return Err(refuse("route.fields-on-get", format!("Action {name} binds GET, so it cannot declare fields. Fields read a POST body.")));
    }
    let record = value
        .as_object()
        .filter(|record| (1..=MAX_FIELDS).contains(&record.len()))
        .ok_or_else(|| refuse("route.fields-invalid", format!(r#"Action {name} has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id"."#)))?;
    let mut keys = record.keys().collect::<Vec<_>>();
    keys.sort();
    let mut parsed = Vec::new();
    for key in keys {
        let declared = action.bindings.iter().any(|binding| binding.source == "input" && binding.key.as_deref() == Some(key.as_str()));
        if !declared {
            return Err(refuse("route.field-unbound", format!("Action {name} maps {key}, which is not one of its input bindings.")));
        }
        let steps = record[key].as_str().and_then(json_path).ok_or_else(|| {
            refuse("route.field-path-invalid", format!(r#"Action {name} maps {key} to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id"."#))
        })?;
        parsed.push((key.clone(), steps));
    }
    Ok(parsed)
}
