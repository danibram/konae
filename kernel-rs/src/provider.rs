//! Webhooks from outside services: the signature schemes a route may declare
//! besides Littlebox's own, and the `fields` map that reads a provider's
//! nested payload into the action's inputs (ADR-0018).
//!
//! A route's `signature` names a `scheme`:
//!
//! | Scheme        | Header the sender sets                  | MAC                                   | Replay refused by          |
//! | ------------- | --------------------------------------- | ------------------------------------- | -------------------------- |
//! | `littlebox`   | `header`: `t=..,n=..,v1=..`             | http.rs, over t, nonce, route, body   | the MAC, for 600 s         |
//! | `hmac-body`   | `header`: `[prefix]<hex or base64>`     | HMAC-SHA256 or SHA-1 of the raw body  | the MAC, for 72 hours      |
//! | `timestamped` | `header`: `<prefix><t>,<tag><hex>`      | HMAC-SHA256 of `<t>.<body>`           | `t` outside `window`, then the MAC until `t` leaves it |
//! | `svix`        | `svix-id`, `svix-timestamp`, `svix-signature` | HMAC-SHA256 of `<id>.<t>.<body>` with the `whsec_` key | `t` outside 300 s, then the id for 72 hours |
//!
//! The parse makes the same checks, in the same order and with the same
//! messages, as src/kernel/route-declaration.ts; kernel-rs/src/http-binding-cases.json
//! holds the cases and kernel-rs/src/provider-signature-cases.json the
//! vectors both sides check. A provider's MAC is remembered in the
//! Deployment's identity.sqlite, next to `route_signatures`, in its own table
//! because each entry has its own expiry. A request whose action then fails
//! gives its entry back, so the provider's retry of the same delivery runs.

use super::*;
use base64::Engine;
use ring::hmac;

/// How long a delivery that carries no timestamp, or a Svix id, is remembered.
/// Providers that retry with the same body stop within this time.
pub(super) const DELIVERY_MEMORY_SECONDS: u64 = 72 * 60 * 60;
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
    Littlebox,
    HmacBody { algorithm: Algorithm, encoding: Encoding, prefix: Option<String> },
    Timestamped { prefix: String, tag: String, window: u64 },
    Svix,
}

// ---------------------------------------------------------------------------
// Declarations.

/// The keys each scheme takes, in the order its refusal lists them.
fn scheme_keys(scheme: &str) -> &'static [&'static str] {
    match scheme {
        "littlebox" => &["scheme", "header", "secret"],
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
const PRESETS: &str = include_str!("signature-presets.json");

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
pub(super) fn signature(name: &str, method: &str, value: &Value) -> Result<(String, String, Scheme), String> {
    let record = value.as_object().ok_or_else(|| format!("Action {name} has an invalid http binding."))?;
    let Some(provider) = record.get("provider") else { return long_form(name, method, record) };
    let table = presets();
    let Some((provider, row)) = provider.as_str().and_then(|provider| table.get_key_value(provider)) else {
        let mut providers = table.keys().map(String::as_str).collect::<Vec<_>>();
        providers.sort();
        let choices = quoted_choices(&providers);
        return Err(format!("Action {name} names an unknown signature provider. Use {choices}."));
    };
    let mut extra = record.keys().map(String::as_str).filter(|key| !["provider", "secret"].contains(key)).collect::<Vec<_>>();
    if !extra.is_empty() {
        extra.sort();
        let remove = quoted_list(&extra);
        return Err(format!(
            r#"Action {name} names the signature provider "{provider}", so its signature takes only "provider" and "secret". Remove {remove}."#
        ));
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
fn long_form(name: &str, method: &str, record: &serde_json::Map<String, Value>) -> Result<(String, String, Scheme), String> {
    let scheme = match record.get("scheme") {
        None => "littlebox",
        Some(Value::String(scheme)) if ["littlebox", "hmac-body", "timestamped", "svix"].contains(&scheme.as_str()) => scheme.as_str(),
        Some(_) => {
            return Err(format!(
                r#"Action {name} names an unknown signature scheme. Use "littlebox", "hmac-body", "timestamped" or "svix"."#
            ));
        }
    };
    let mut problems: Vec<String> = Vec::new();
    let keys = scheme_keys(scheme);
    if record.keys().any(|key| !keys.contains(&key.as_str())) {
        let takes = quoted_list(keys);
        problems.push(format!("Action {name} has an invalid {scheme} signature. It takes {takes}."));
    }
    if method == "GET" && scheme != "littlebox" {
        problems.push(format!(
            "Action {name} binds GET, so its signature must use the littlebox scheme. A provider's scheme signs a POST body."
        ));
        return Err(problems.join(" "));
    }
    let header = if scheme == "svix" {
        Some("svix-signature")
    } else {
        record.get("header").and_then(Value::as_str).filter(|header| valid_header(header))
    };
    if header.is_none() {
        problems.push(format!("Action {name} has an invalid signature header."));
    }
    let secret = record.get("secret").and_then(Value::as_str).filter(|secret| valid_secret_name(secret));
    if secret.is_none() {
        problems.push(format!(
            r#"Action {name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter."#
        ));
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
                problems.push(format!(r#"Action {name} has an invalid signature algorithm. Use "sha256" or "sha1"."#));
            }
            let encoding = match text("encoding") {
                Some("hex") => Some(Encoding::Hex),
                Some("base64") => Some(Encoding::Base64),
                _ => None,
            };
            if encoding.is_none() {
                problems.push(format!(r#"Action {name} has an invalid signature encoding. Use "hex" or "base64"."#));
            }
            let prefix = match record.get("prefix") {
                None => Some(None),
                Some(Value::String(prefix)) if valid_body_prefix(prefix) => Some(Some(prefix.clone())),
                Some(_) => None,
            };
            if prefix.is_none() {
                problems.push(format!(
                    r#"Action {name} has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256="."#
                ));
            }
            match (algorithm, encoding, prefix) {
                (Some(algorithm), Some(encoding), Some(prefix)) => Some(Scheme::HmacBody { algorithm, encoding, prefix }),
                _ => None,
            }
        }
        "timestamped" => {
            let prefix = text("prefix").filter(|prefix| valid_field_prefix(prefix));
            if prefix.is_none() {
                problems.push(format!(r#"Action {name} has an invalid timestamp prefix. Use the field's name and "=", such as "t="."#));
            }
            let tag = text("tag").filter(|tag| valid_field_prefix(tag) && Some(*tag) != text("prefix"));
            if tag.is_none() {
                problems.push(format!(
                    r#"Action {name} has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's."#
                ));
            }
            // 300.0 is the integer 300 to the editor's JSON reader, so it is here too.
            let window = record
                .get("window")
                .and_then(Value::as_f64)
                .filter(|seconds| seconds.fract() == 0.0 && (1.0..=MAX_WINDOW_SECONDS as f64).contains(seconds));
            if window.is_none() {
                problems.push(format!("Action {name} has an invalid signature window. Use a whole number of seconds from 1 to 3600."));
            }
            match (prefix, tag, window) {
                (Some(prefix), Some(tag), Some(window)) => {
                    Some(Scheme::Timestamped { prefix: prefix.to_owned(), tag: tag.to_owned(), window: window as u64 })
                }
                _ => None,
            }
        }
        "svix" => Some(Scheme::Svix),
        _ => Some(Scheme::Littlebox),
    };
    match (header, secret, parsed) {
        (Some(header), Some(secret), Some(parsed)) if problems.is_empty() => Ok((header.to_owned(), secret.to_owned(), parsed)),
        _ => Err(problems.join(" ")),
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
pub(super) fn fields(name: &str, method: &str, action: &ActionDefinition, value: &Value) -> Result<Vec<(String, Vec<Step>)>, String> {
    if method == "GET" {
        return Err(format!("Action {name} binds GET, so it cannot declare fields. Fields read a POST body."));
    }
    let record = value
        .as_object()
        .filter(|record| (1..=MAX_FIELDS).contains(&record.len()))
        .ok_or_else(|| format!(r#"Action {name} has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id"."#))?;
    let mut keys = record.keys().collect::<Vec<_>>();
    keys.sort();
    let mut parsed = Vec::new();
    for key in keys {
        let declared = action.bindings.iter().any(|binding| binding.source == "input" && binding.key.as_deref() == Some(key.as_str()));
        if !declared {
            return Err(format!("Action {name} maps {key}, which is not one of its input bindings."));
        }
        let steps = record[key].as_str().and_then(json_path).ok_or_else(|| {
            format!(r#"Action {name} maps {key} to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id"."#)
        })?;
        parsed.push((key.clone(), steps));
    }
    Ok(parsed)
}

fn at<'a>(body: &'a Value, steps: &[Step]) -> Option<&'a Value> {
    steps.iter().try_fold(body, |value, step| match step {
        Step::Key(key) => value.as_object()?.get(key),
        Step::Index(index) => value.as_array()?.get(*index),
    })
}

/// The action input a route with `fields` reads from a POST body: each input
/// binding takes the value at its path when `fields` names it, else the body's
/// key of its name. Every other key of the body is left out. A body that is
/// not an object goes on as it is, for the binding check to refuse.
pub(super) fn project(fields: &[(String, Vec<Step>)], action: &ActionDefinition, body: &Value) -> Value {
    let Some(object) = body.as_object() else { return body.clone() };
    let mut input = serde_json::Map::new();
    for key in action.bindings.iter().filter(|binding| binding.source == "input").filter_map(|binding| binding.key.as_ref()) {
        let value = match fields.iter().find(|(field, _)| field == key) {
            Some((_, steps)) => at(body, steps),
            None => object.get(key),
        };
        if let Some(value) = value {
            input.insert(key.clone(), value.clone());
        }
    }
    Value::Object(input)
}

// ---------------------------------------------------------------------------
// Verification.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Delivery {
    /// The MAC matched. The key the replay memory records, and when it may forget it.
    Valid { key: String, expires: u64 },
    Invalid,
    /// The timestamp is outside the window. Checked before the MAC.
    Stale,
}

/// The HMAC key of a Svix signing secret: the base64 after `whsec_`.
pub(super) fn svix_key(secret: &str) -> Option<Vec<u8>> {
    let encoded = secret.strip_prefix("whsec_").unwrap_or(secret);
    base64::engine::general_purpose::STANDARD.decode(encoded).ok().filter(|key| !key.is_empty())
}

fn digits(text: &str) -> Option<u64> {
    (!text.is_empty() && text.len() <= 12 && text.bytes().all(|byte| byte.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

fn first_match(key: &hmac::Key, message: &[u8], tags: &[Vec<u8>]) -> Option<String> {
    tags.iter().find(|tag| hmac::verify(key, message, tag).is_ok()).map(hex::encode)
}

/// Checks a provider's signature on a POST body. `header` reads one request
/// header by its lowercase name. `now` is in Unix seconds.
pub(super) fn verify(scheme: &Scheme, signature_header: &str, secret: &str, header: &dyn Fn(&str) -> Option<String>, body: &[u8], now: u64) -> Delivery {
    match scheme {
        Scheme::Littlebox => Delivery::Invalid,
        Scheme::HmacBody { algorithm, encoding, prefix } => {
            let Some(value) = header(&signature_header.to_ascii_lowercase()) else { return Delivery::Invalid };
            let encoded = match prefix {
                Some(prefix) => match value.trim().strip_prefix(prefix.as_str()) {
                    Some(rest) => rest.to_owned(),
                    None => return Delivery::Invalid,
                },
                None => value.trim().to_owned(),
            };
            let (algorithm, length) = match algorithm {
                Algorithm::Sha256 => (hmac::HMAC_SHA256, 32),
                Algorithm::Sha1 => (hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, 20),
            };
            let tag = match encoding {
                Encoding::Hex => hex::decode(&encoded).ok(),
                Encoding::Base64 => base64::engine::general_purpose::STANDARD.decode(&encoded).ok(),
            };
            let Some(tag) = tag.filter(|tag| tag.len() == length) else { return Delivery::Invalid };
            let key = hmac::Key::new(algorithm, secret.as_bytes());
            match first_match(&key, body, &[tag]) {
                Some(mac) => Delivery::Valid { key: format!("mac:{mac}"), expires: now + DELIVERY_MEMORY_SECONDS },
                None => Delivery::Invalid,
            }
        }
        Scheme::Timestamped { prefix, tag, window } => {
            let Some(value) = header(&signature_header.to_ascii_lowercase()) else { return Delivery::Invalid };
            let mut timestamp = None;
            let mut tags = Vec::new();
            for part in value.split(',').map(str::trim) {
                if let Some(text) = part.strip_prefix(prefix.as_str()) {
                    timestamp = digits(text);
                } else if let Some(text) = part.strip_prefix(tag.as_str()) {
                    match hex::decode(text) {
                        Ok(bytes) if bytes.len() == 32 => tags.push(bytes),
                        _ => {}
                    }
                }
            }
            let Some(timestamp) = timestamp else { return Delivery::Invalid };
            if timestamp.abs_diff(now) > *window {
                return Delivery::Stale;
            }
            if tags.is_empty() || tags.len() > MAX_PROVIDER_TAGS {
                return Delivery::Invalid;
            }
            let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
            let message = [format!("{timestamp}.").as_bytes(), body].concat();
            match first_match(&key, &message, &tags) {
                // A request with this `t` is stale once `t` leaves the window,
                // so the memory may forget it then.
                Some(mac) => Delivery::Valid { key: format!("mac:{mac}"), expires: timestamp + window + 1 },
                None => Delivery::Invalid,
            }
        }
        Scheme::Svix => {
            let [id, sent, signatures] = SVIX_HEADERS.map(|name| header(name));
            let (Some(id), Some(sent), Some(signatures)) = (id, sent, signatures) else { return Delivery::Invalid };
            let Some(timestamp) = digits(&sent) else { return Delivery::Invalid };
            if id.is_empty() || id.len() > 128 || id.bytes().any(|byte| !byte.is_ascii_graphic()) {
                return Delivery::Invalid;
            }
            if timestamp.abs_diff(now) > SVIX_WINDOW_SECONDS {
                return Delivery::Stale;
            }
            let tags = signatures
                .split_whitespace()
                .filter_map(|entry| entry.strip_prefix("v1,"))
                .filter_map(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded).ok())
                .filter(|tag| tag.len() == 32)
                .collect::<Vec<_>>();
            if tags.is_empty() || tags.len() > MAX_PROVIDER_TAGS {
                return Delivery::Invalid;
            }
            let Some(secret) = svix_key(secret) else { return Delivery::Invalid };
            let key = hmac::Key::new(hmac::HMAC_SHA256, &secret);
            let message = [format!("{id}.{sent}.").as_bytes(), body].concat();
            // Svix sends a retry with the same id and a new signature, so the
            // id, which the MAC covers, is what a replay repeats.
            match first_match(&key, &message, &tags) {
                Some(_) => Delivery::Valid { key: format!("id:{id}"), expires: now + DELIVERY_MEMORY_SECONDS },
                None => Delivery::Invalid,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Replay memory. identity.sqlite, as `route_signatures`, so it survives a
// restart and travels with the accounts in an export.

fn deliveries_table(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS route_deliveries(
           path TEXT NOT NULL, key TEXT NOT NULL, expires_at INTEGER NOT NULL, PRIMARY KEY(path, key));
         CREATE INDEX IF NOT EXISTS route_deliveries_expiry ON route_deliveries(expires_at);",
    )
}

/// Records a delivery on `path`. `false` when it is already recorded and has
/// not expired, which is a replay. Expired entries are pruned on the way.
pub(super) fn claim(db: &Connection, path: &str, key: &str, expires: u64, now: u64) -> Result<bool> {
    deliveries_table(db)?;
    db.execute("DELETE FROM route_deliveries WHERE expires_at < ?1", [now as i64])?;
    let inserted = db.execute(
        "INSERT INTO route_deliveries(path, key, expires_at) VALUES(?1, ?2, ?3) ON CONFLICT(path, key) DO NOTHING",
        rusqlite::params![path, key, expires as i64],
    )?;
    Ok(inserted == 1)
}

/// Forgets a delivery whose action did not succeed, so the provider's retry
/// of the same delivery runs instead of answering `replayed-signature`.
pub(super) fn release(db: &Connection, path: &str, key: &str) -> Result<()> {
    deliveries_table(db)?;
    db.execute("DELETE FROM route_deliveries WHERE path=?1 AND key=?2", rusqlite::params![path, key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: Vec<(String, String)>) -> impl Fn(&str) -> Option<String> {
        move |name: &str| pairs.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.clone())
    }

    fn scheme_of(declaration: &Value) -> (String, Scheme) {
        let (header, _, scheme) = signature("hooks.receive", "POST", declaration).unwrap();
        (header, scheme)
    }

    /// src/lib/provider-signature.ts signs the same vectors; the provider
    /// examples are the ones their documentation publishes.
    #[test]
    fn the_vectors_verify_and_a_changed_byte_does_not() {
        let vectors: Vec<Value> = serde_json::from_str(include_str!("provider-signature-cases.json")).unwrap();
        assert!(vectors.len() >= 5);
        for vector in vectors {
            let case = vector["case"].as_str().unwrap();
            let (header_name, scheme) = scheme_of(&vector["signature"]);
            let pairs = vector["headers"].as_object().unwrap().iter().map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned())).collect::<Vec<_>>();
            let secret = vector["secret"].as_str().unwrap();
            let body = vector["body"].as_str().unwrap().as_bytes();
            let now = vector["now"].as_u64().unwrap();
            assert!(matches!(verify(&scheme, &header_name, secret, &headers(pairs.clone()), body, now), Delivery::Valid { .. }), "{case}");
            let mut tampered = body.to_vec();
            tampered.push(b' ');
            assert_eq!(verify(&scheme, &header_name, secret, &headers(pairs.clone()), &tampered, now), Delivery::Invalid, "{case}: tampered body");
            assert_eq!(verify(&scheme, &header_name, &format!("{secret}x"), &headers(pairs), body, now), Delivery::Invalid, "{case}: wrong secret");
        }
    }

    /// src/lib/provider-signature.test.ts runs the same file against the
    /// editor's parser.
    #[test]
    fn a_preset_expands_to_its_long_form_and_refusals_name_every_wrong_key() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("signature-preset-cases.json")).unwrap();
        assert!(cases.len() >= 15);
        for case in cases {
            let label = case["case"].as_str().unwrap();
            let method = case["method"].as_str().unwrap();
            let outcome = signature("hooks.receive", method, &case["signature"]);
            match case["error"].as_str() {
                Some(message) => assert_eq!(outcome, Err(message.to_owned()), "{label}"),
                None => assert_eq!(outcome, signature("hooks.receive", method, &case["expands"]), "{label}"),
            }
            assert!(case["error"].is_string() || outcome.is_ok(), "{label}");
        }
    }

    /// A provider joins the table only with a vector that signs as it does.
    #[test]
    fn every_preset_verifies_a_vector() {
        let vectors: Vec<Value> = serde_json::from_str(include_str!("provider-signature-cases.json")).unwrap();
        for provider in presets().keys() {
            assert!(vectors.iter().any(|vector| vector["signature"]["provider"] == provider.as_str()), "{provider} has no vector");
        }
    }

    #[test]
    fn hmac_body_reads_its_prefix_and_encoding_and_remembers_the_mac_for_72_hours() {
        let secret = "github-secret-0123456";
        let body = br#"{"action":"opened"}"#;
        let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()), body);
        let (header_name, github) = scheme_of(&json!({"scheme": "hmac-body", "header": "X-Hub-Signature-256", "secret": "gh", "algorithm": "sha256", "encoding": "hex", "prefix": "sha256="}));
        let sent = |value: String| headers(vec![("x-hub-signature-256".into(), value)]);
        let good = format!("sha256={}", hex::encode(tag.as_ref()));
        assert_eq!(
            verify(&github, &header_name, secret, &sent(good.clone()), body, 100),
            Delivery::Valid { key: format!("mac:{}", hex::encode(tag.as_ref())), expires: 100 + DELIVERY_MEMORY_SECONDS }
        );
        let upper = format!("sha256={}", hex::encode_upper(tag.as_ref()));
        assert!(matches!(verify(&github, &header_name, secret, &sent(upper), body, 100), Delivery::Valid { .. }), "hex in either case");
        assert_eq!(verify(&github, &header_name, secret, &sent(hex::encode(tag.as_ref())), body, 100), Delivery::Invalid, "the declared prefix is required");
        assert_eq!(verify(&github, &header_name, secret, &headers(vec![]), body, 100), Delivery::Invalid, "missing");
        let (_, shopify) = scheme_of(&json!({"scheme": "hmac-body", "header": "X-Shopify-Hmac-Sha256", "secret": "shop", "algorithm": "sha256", "encoding": "base64"}));
        let encoded = base64::engine::general_purpose::STANDARD.encode(tag.as_ref());
        assert!(matches!(verify(&shopify, "X-Shopify-Hmac-Sha256", secret, &headers(vec![("x-shopify-hmac-sha256".into(), encoded)]), body, 100), Delivery::Valid { .. }));
        let (_, sha1) = scheme_of(&json!({"scheme": "hmac-body", "header": "X-Hub-Signature", "secret": "gh", "algorithm": "sha1", "encoding": "hex", "prefix": "sha1="}));
        let short = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret.as_bytes()), body);
        assert!(matches!(verify(&sha1, "X-Hub-Signature", secret, &headers(vec![("x-hub-signature".into(), format!("sha1={}", hex::encode(short.as_ref())))]), body, 100), Delivery::Valid { .. }));
        assert_eq!(verify(&sha1, "X-Hub-Signature", secret, &headers(vec![("x-hub-signature".into(), good)]), body, 100), Delivery::Invalid, "a SHA-256 value under sha1");
    }

    #[test]
    fn timestamped_checks_the_window_before_the_mac_and_ignores_other_fields() {
        let secret = "whsec_stripe_like_0123";
        let body = br#"{"id":"evt_1"}"#;
        let (header_name, scheme) = scheme_of(&json!({"scheme": "timestamped", "header": "Stripe-Signature", "secret": "pay", "prefix": "t=", "tag": "v1=", "window": 300}));
        let mac = |t: u64, key: &str| hex::encode(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()), &[format!("{t}.").as_bytes(), body].concat()).as_ref());
        let sent = |value: String| headers(vec![("stripe-signature".into(), value)]);
        let t = 1_800_000_000;
        let good = format!("t={t},v1={},v0={}", mac(t, secret), "00".repeat(32));
        assert_eq!(verify(&scheme, &header_name, secret, &sent(good.clone()), body, t + 10), Delivery::Valid { key: format!("mac:{}", mac(t, secret)), expires: t + 301 });
        assert_eq!(verify(&scheme, &header_name, "another", &sent(good.clone()), body, t + 301), Delivery::Stale, "stale before the MAC");
        assert_eq!(verify(&scheme, &header_name, secret, &sent(good.clone()), body, t - 301), Delivery::Stale, "from the future");
        assert_eq!(verify(&scheme, &header_name, secret, &sent(good.replacen(&format!("t={t}"), &format!("t={}", t + 1), 1)), body, t), Delivery::Invalid, "t is signed");
        let rotated = format!("t={t},v1={},v1={}", "ab".repeat(32), mac(t, secret));
        assert!(matches!(verify(&scheme, &header_name, secret, &sent(rotated), body, t), Delivery::Valid { .. }), "two values while a secret rotates");
        let crowded = format!("t={t},{}v1={}", "v1=ab,".repeat(5000), mac(t, secret));
        assert!(matches!(verify(&scheme, &header_name, secret, &sent(crowded), body, t), Delivery::Valid { .. }), "values that are not 32 bytes cost no MAC");
        let many = format!("t={t},{}v1={}", format!("v1={},", "ab".repeat(32)).repeat(4), mac(t, secret));
        assert_eq!(verify(&scheme, &header_name, secret, &sent(many), body, t), Delivery::Invalid, "five values");
        assert_eq!(verify(&scheme, &header_name, secret, &sent(format!("v1={}", mac(t, secret))), body, t), Delivery::Invalid, "no timestamp");
    }

    #[test]
    fn svix_reads_three_headers_and_remembers_the_id() {
        let raw = b"svix-test-key-0123456789";
        let secret = format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode(raw));
        let body = br#"{"type":"email.delivered"}"#;
        let sign = |id: &str, t: u64| base64::engine::general_purpose::STANDARD.encode(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, raw), &[format!("{id}.{t}.").as_bytes(), body].concat()).as_ref());
        let sent = |id: &str, t: u64, signature: String| headers(vec![("svix-id".into(), id.into()), ("svix-timestamp".into(), t.to_string()), ("svix-signature".into(), signature)]);
        let t = 1_800_000_000;
        assert_eq!(
            verify(&Scheme::Svix, "svix-signature", &secret, &sent("msg_1", t, format!("v1,{}", sign("msg_1", t))), body, t),
            Delivery::Valid { key: "id:msg_1".into(), expires: t + DELIVERY_MEMORY_SECONDS }
        );
        assert!(matches!(verify(&Scheme::Svix, "svix-signature", &secret, &sent("msg_1", t, format!("v1,AAAA v1,{}", sign("msg_1", t))), body, t), Delivery::Valid { .. }));
        assert_eq!(verify(&Scheme::Svix, "svix-signature", &secret, &sent("msg_2", t, format!("v1,{}", sign("msg_1", t))), body, t), Delivery::Invalid, "the id is signed");
        assert_eq!(verify(&Scheme::Svix, "svix-signature", &secret, &sent("msg_1", t, format!("v1,{}", sign("msg_1", t))), body, t + 301), Delivery::Stale);
        assert_eq!(verify(&Scheme::Svix, "svix-signature", "whsec_%%%", &sent("msg_1", t, format!("v1,{}", sign("msg_1", t))), body, t), Delivery::Invalid, "a secret that is not base64");
        assert_eq!(verify(&Scheme::Svix, "svix-signature", &secret, &headers(vec![]), body, t), Delivery::Invalid);
    }

    #[test]
    fn a_claim_refuses_a_replay_until_it_expires_or_is_released() {
        let db = Connection::open_in_memory().unwrap();
        assert!(claim(&db, "/hooks/a", "mac:1", 200, 100).unwrap());
        assert!(!claim(&db, "/hooks/a", "mac:1", 200, 150).unwrap(), "replayed");
        assert!(claim(&db, "/hooks/b", "mac:1", 200, 150).unwrap(), "each path remembers its own");
        release(&db, "/hooks/a", "mac:1").unwrap();
        assert!(claim(&db, "/hooks/a", "mac:1", 200, 160).unwrap(), "a released delivery runs again");
        assert!(claim(&db, "/hooks/a", "mac:1", 400, 201).unwrap(), "an expired entry is pruned");
    }

    #[test]
    fn a_path_reads_keys_and_indexes_and_projection_leaves_other_keys_out() {
        assert_eq!(json_path("$.data.object.id"), Some(vec![Step::Key("data".into()), Step::Key("object".into()), Step::Key("id".into())]));
        assert_eq!(json_path("$.commits[0].id"), Some(vec![Step::Key("commits".into()), Step::Index(0), Step::Key("id".into())]));
        for bad in ["", "$", "data.id", "$..id", "$.a b", "$[01]", "$[1000]", "$.a[", "$.a]", "$['a']", "$.a.", &format!("${}", ".a".repeat(17))] {
            assert_eq!(json_path(bad), None, "{bad}");
        }
        let action: ActionDefinition = serde_json::from_value(json!({"kind": "mutation", "access": "anonymous", "statement": "", "bindings": [
            {"source": "input", "key": "id", "type": "string"}, {"source": "input", "key": "type", "type": "string"},
            {"source": "input", "key": "note", "type": "string", "nullable": true}, {"source": "kernel", "field": "now"}]})).unwrap();
        let fields = fields("pay.receive", "POST", &action, &json!({"id": "$.data.object.id", "note": "$.data.object.lines[1]"})).unwrap();
        let body = json!({"type": "charge.failed", "livemode": false, "data": {"object": {"id": "ch_1", "lines": ["a", "b"]}}});
        assert_eq!(project(&fields, &action, &body), json!({"id": "ch_1", "type": "charge.failed", "note": "b"}));
        assert_eq!(project(&fields, &action, &json!({"data": {}})), json!({}), "a missing path leaves the input out");
        assert_eq!(project(&fields, &action, &json!([1])), json!([1]));
    }
}
