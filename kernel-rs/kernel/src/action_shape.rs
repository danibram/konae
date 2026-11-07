//! The shape of a Server Action program, checked at every publish.
//!
//! An action's name, access, kind, bindings and size, and the number of
//! actions a project declares. `parseServerActionSource` in
//! src/kernel/actions.ts makes the same checks in the same order with the same
//! messages, and kernel-rs/kernel/src/action-cases.json holds the cases both test
//! suites run. Actions are checked in sorted name order, so with two bad
//! actions both doors name the same one.

use serde_json::Value;

use super::refusal::{refuse, Refusal};
use super::{ActionDefinition, ActionProgram};

pub const MAX_ACTIONS: usize = 100;
const MAX_NAME_CHARS: usize = 100;
const MAX_STATEMENT_BYTES: usize = 20_000;
const MAX_BINDINGS: usize = 100;
const MAX_STRING_LENGTH: usize = 100_000;

/// Reads `actions.json` and checks it, as `parseServerActionSource` does. A
/// field of the wrong type, such as `"nullable": "yes"`, is refused with the
/// Gate's code before the Kernel's types read the file, and an action earlier
/// in name order that breaks a rule is named first, as the Gate names it. A
/// file that is not JSON, or not version 1, is refused without a code on
/// both doors.
pub fn read(source: &str) -> anyhow::Result<ActionProgram> {
    let value: Value = serde_json::from_str(source).map_err(|error_value| anyhow::anyhow!("Invalid actions.json: {error_value}"))?;
    if value["version"] == 1 {
        if let Some(actions) = value["actions"].as_object().filter(|actions| actions.len() <= MAX_ACTIONS) {
            let mut names = actions.keys().collect::<Vec<_>>();
            names.sort();
            for (at, name) in names.iter().enumerate() {
                if let Err(refused) = check_types(name, &actions[*name]) {
                    for earlier in &names[..at] {
                        let action: ActionDefinition = serde_json::from_value(actions[*earlier].clone())?;
                        super::validate_action(earlier, &action)?;
                    }
                    return Err(refused.into());
                }
            }
        }
    }
    let program: ActionProgram = serde_json::from_value(value).map_err(|error_value| anyhow::anyhow!("Invalid actions.json: {error_value}"))?;
    anyhow::ensure!(program.version == 1, "Unsupported actions.json version.");
    check_program(&program)?;
    Ok(program)
}

/// The checks `parseActionBody` and `parseBinding` in src/kernel/actions.ts
/// make before an action's SQL, on the JSON as written, so a value of the
/// wrong type gets the rule's code instead of a parse error.
fn check_types(name: &str, action: &Value) -> Result<(), Refusal> {
    if !valid_name(name) {
        return Err(refuse("action.name-invalid", format!("Invalid server action name: {name}")));
    }
    let Some(action) = action.as_object() else {
        return Err(refuse("action.invalid", format!("Invalid server action: {name}")));
    };
    let text = |key: &str| action.get(key).and_then(Value::as_str);
    if text("access") == Some("granted") {
        return Err(refuse(
            "action.access-unsupported",
            format!(r#"Action {name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated"."#),
        ));
    }
    if !matches!(text("access"), Some("anonymous" | "authenticated")) {
        return Err(refuse("action.access-invalid", format!(r#"Action {name} has an invalid access. Use "anonymous" or "authenticated"."#)));
    }
    let kind = text("kind").unwrap_or_default();
    if !matches!(kind, "query" | "mutation" | "signal" | "call") {
        return Err(refuse("action.kind-invalid", format!(r#"Action {name} has an invalid kind. Use "query", "mutation", "signal" or "call"."#)));
    }
    let Some(bindings) = action.get("bindings").and_then(Value::as_array) else {
        return Err(refuse("action.bindings-missing", format!("Action {name} requires a bindings array.")));
    };
    for (index, binding) in bindings.iter().enumerate() {
        check_binding_types(name, index, binding)?;
    }
    if matches!(kind, "query" | "mutation") && !action.get("statement").is_some_and(Value::is_string) {
        return Err(refuse("action.invalid", format!("Invalid server action: {name}")));
    }
    Ok(())
}

fn check_binding_types(name: &str, index: usize, binding: &Value) -> Result<(), Refusal> {
    let invalid = || refuse("action.binding-invalid", format!("Invalid binding {index} in {name}."));
    let binding = binding.as_object().ok_or_else(invalid)?;
    let text = |key: &str| binding.get(key).and_then(Value::as_str);
    let nullable = binding.get("nullable");
    if nullable.is_some_and(|value| !value.is_boolean()) || (nullable == Some(&Value::Bool(true)) && text("source") != Some("input")) {
        return Err(refuse("action.binding-nullable-invalid", format!("Action {name} has an invalid nullable input binding.")));
    }
    let valid = match text("source") {
        Some("input") if text("key").is_some() => match text("type") {
            Some("string") => {
                let length = binding.get("maxLength");
                // 100.0 is the integer 100 to the editor's JSON reader, so it is here too.
                let whole = |length: &Value| length.as_f64().is_some_and(|length| length.fract() == 0.0 && (1.0..=MAX_STRING_LENGTH as f64).contains(&length));
                if length.is_some_and(|length| !whole(length)) {
                    return Err(refuse("action.max-length-invalid", format!("Invalid maxLength in {name}.")));
                }
                true
            }
            Some("number" | "boolean") => true,
            _ => false,
        },
        Some("actor") => text("field") == Some("id"),
        Some("kernel") => text("field") == Some("now"),
        // An absent value and null both bind NULL.
        Some("literal") => binding.get("value").is_none_or(|value| value.is_null() || value.is_string() || value.is_number()),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid()) }
}

/// Checks the program: its size, then each action in sorted name order, then
/// that no two actions bind one route.
pub fn check_program(program: &ActionProgram) -> Result<(), Refusal> {
    if program.actions.len() > MAX_ACTIONS {
        // Each message is written out so src/sdk/contract.test.ts finds it.
        return Err(refuse("action.count-over", "A project may declare at most 100 actions."));
    }
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        super::validate_action(name, &program.actions[name])?;
    }
    super::http::validate_routes(program)?;
    super::calls::check_program(program)
}

/// The checks that come before an action's SQL: its name, access, kind,
/// bindings and size.
pub fn check_action(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if !valid_name(name) {
        return Err(refuse("action.name-invalid", format!("Invalid server action name: {name}")));
    }
    if action.access == "granted" {
        // Reserved for per-grant access; the Kernel refuses it until grants exist.
        return Err(refuse(
            "action.access-unsupported",
            format!(r#"Action {name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated"."#),
        ));
    }
    if action.access != "anonymous" && action.access != "authenticated" {
        return Err(refuse("action.access-invalid", format!(r#"Action {name} has an invalid access. Use "anonymous" or "authenticated"."#)));
    }
    if !matches!(action.kind.as_str(), "query" | "mutation" | "signal" | "call") {
        return Err(refuse("action.kind-invalid", format!(r#"Action {name} has an invalid kind. Use "query", "mutation", "signal" or "call"."#)));
    }
    for (index, binding) in action.bindings.iter().enumerate() {
        if binding.nullable && binding.source != "input" {
            return Err(refuse("action.binding-nullable-invalid", format!("Action {name} has an invalid nullable input binding.")));
        }
        let valid = match binding.source.as_str() {
            "input" => binding.key.is_some() && matches!(binding.value_type.as_deref(), Some("string" | "number" | "boolean")),
            "actor" => binding.field.as_deref() == Some("id"),
            "kernel" => binding.field.as_deref() == Some("now"),
            // An absent value and null both bind NULL.
            "literal" => binding.value.as_ref().is_none_or(|value| value.is_string() || value.is_number()),
            _ => false,
        };
        if !valid {
            return Err(refuse("action.binding-invalid", format!("Invalid binding {index} in {name}.")));
        }
        if binding.value_type.as_deref() == Some("string")
            && binding.max_length.is_some_and(|length| !(1..=MAX_STRING_LENGTH).contains(&length))
        {
            return Err(refuse("action.max-length-invalid", format!("Invalid maxLength in {name}.")));
        }
    }
    if action.statement.len() > MAX_STATEMENT_BYTES || action.bindings.len() > MAX_BINDINGS {
        return Err(refuse("action.limits-exceeded", format!("Action {name} exceeds statement or binding limits.")));
    }
    Ok(())
}

/// The top-level keys each kind takes. `parseAction` in src/kernel/actions.ts
/// holds the same lists. `statement` on a signal action or a call may only be
/// empty, which `usage` and `calls` check.
fn allowed_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "query" => &["kind", "access", "statement", "bindings", "http"],
        "mutation" => &["kind", "access", "statement", "bindings", "http", "signal", "guard", "notify", "schedule"],
        "signal" => &["kind", "access", "statement", "bindings", "signal"],
        _ => &["kind", "access", "statement", "bindings", "url", "method", "body", "headers", "auth", "then"],
    }
}

/// Refuses a key the action's kind does not take. It runs after the action's
/// other checks, so a key with its own refusal, such as a guard on a query,
/// keeps it. A key set to null counts as absent, on both doors. Keys are named
/// in sorted order, as the editor names them.
pub fn check_keys(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    let declared = [
        ("http", action.http.is_some()),
        ("signal", action.signal.is_some()),
        ("guard", action.guard.is_some()),
        ("notify", action.notify.is_some()),
        ("schedule", action.schedule.is_some()),
        ("subscribe", action.subscribe.is_some()),
    ];
    let mut keys: Vec<&str> = declared.iter().filter(|(_, present)| *present).map(|(key, _)| *key).collect();
    keys.extend(action.call.present());
    keys.extend(action.extra.iter().filter(|(_, value)| !value.is_null()).map(|(key, _)| key.as_str()));
    keys.sort_unstable();
    let allowed = allowed_keys(&action.kind);
    match keys.into_iter().find(|key| !allowed.contains(key)) {
        Some(key) => {
            let kind = &action.kind;
            Err(refuse("action.key-unknown", format!("Action {name} has a key {key} that a {kind} action does not take.")))
        }
        None => Ok(()),
    }
}

/// Lowercase words of letters and digits, each starting with a letter, joined
/// by dots, at least two of them and at most 100 characters in all.
fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME_CHARS
        && name.split('.').count() >= 2
        && name.split('.').all(|word| {
            word.chars().next().is_some_and(|first| first.is_ascii_lowercase())
                && word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// The editor runs the same file in src/kernel/actions.test.ts and through
    /// the Validation Gate in src/agent/gate.test.ts.
    #[test]
    fn the_kernel_refuses_the_action_shapes_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("action-cases.json")).unwrap();
        assert!(cases.len() >= 25);
        for case in cases {
            let source = serde_json::json!({"version": 1, "actions": case["actions"]}).to_string();
            let outcome = super::read(&source).map(|_| ()).map_err(|error_value| {
                crate::refusal::of(&error_value).cloned().unwrap_or_else(|| panic!("{}: {error_value} has no code", case["case"]))
            });
            crate::refusal::assert_case(&case, outcome);
        }
    }
}
