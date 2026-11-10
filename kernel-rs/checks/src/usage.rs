//! An action's `signal` key, copied from kernel-rs/src/usage.rs at 8ecb7e5.
//! Recording Signals and Usage stays in the Kernel.

use super::refusal::{refuse, Refusal};
use super::ActionDefinition;
use serde_json::Value;

pub(crate) const MAX_SIGNAL_PROPERTIES: usize = 8;
/// The property that records the caller's ID instead of an input.
const SIGNAL_ACTOR: &str = "actor";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Signal {
    pub(crate) name: String,
    pub(crate) properties: Vec<String>,
}

/// Checks the parts of an action that a `"kind": "signal"` action changes, and
/// its `signal` key. `validate_action` calls it after kind and access and
/// before the SQL checks, which a signal action skips.
pub(crate) fn check_signal_action(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if action.kind == "signal" && !action.statement.trim().is_empty() {
        return Err(refuse("signal.statement", format!("Action {name} is a signal action, so it runs no SQL and cannot declare a statement.")));
    }
    Ok(())
}

/// Parses and checks one action's `signal` key. `Ok(None)` when it has none.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Signal>, Refusal> {
    let Some(value) = action.signal.as_ref() else {
        if action.kind == "signal" {
            return Err(refuse("signal.missing", format!("Action {name} is a signal action, so it must declare a signal.")));
        }
        return Ok(None);
    };
    if action.kind == "query" {
        return Err(refuse("signal.on-query", format!(
            r#"Action {name} is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action."#
        )));
    }
    let invalid = || refuse("signal.invalid", format!("Action {name} has an invalid signal declaration."));
    let record = value.as_object().ok_or_else(invalid)?;
    if record.keys().any(|key| key != "name" && key != "properties") {
        return Err(invalid());
    }
    let signal_name = record.get("name").and_then(Value::as_str).ok_or_else(invalid)?;
    if !valid_name(signal_name) {
        return Err(refuse("signal.name-invalid", format!(
            r#"Action {name} names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters."#
        )));
    }
    let properties = match record.get("properties") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(invalid()),
    };
    if properties.len() > MAX_SIGNAL_PROPERTIES {
        // The number is written out so src/sdk/contract.test.ts finds the message.
        return Err(refuse("signal.properties-too-many", format!("Action {name} names more than 8 signal properties.")));
    }
    let inputs: Vec<&str> = action
        .bindings
        .iter()
        .filter(|binding| binding.source == "input")
        .filter_map(|binding| binding.key.as_deref())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for key in &properties {
        if !seen.insert(key.as_str()) {
            return Err(refuse("signal.property-twice", format!("Action {name} names signal property {key} twice.")));
        }
        if key == SIGNAL_ACTOR && inputs.contains(&SIGNAL_ACTOR) {
            return Err(refuse("signal.property-actor-input", format!(
                "Action {name} names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input."
            )));
        }
        if key != SIGNAL_ACTOR && !inputs.contains(&key.as_str()) {
            return Err(refuse("signal.property-unbound", format!("Action {name} names signal property {key}, which is not one of its input bindings.")));
        }
    }
    Ok(Some(Signal { name: signal_name.to_owned(), properties }))
}

/// Dot-separated lowercase words, at least two, as action names are, up to 64 characters.
fn valid_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    name.len() <= 64
        && parts.len() >= 2
        && parts.iter().all(|part| {
            let mut bytes = part.bytes();
            bytes.next().is_some_and(|c| c.is_ascii_lowercase())
                && bytes.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}
