//! The shape of a Server Action program, checked at every publish.
//!
//! An action's name, access, kind, bindings and size, and the number of
//! actions a project declares. `parseServerActionSource` in
//! src/kernel/actions.ts makes the same checks in the same order with the same
//! messages, and kernel-rs/src/action-cases.json holds the cases both test
//! suites run. Actions are checked in sorted name order, so with two bad
//! actions both doors name the same one.

use super::refusal::{refuse, Refusal};
use super::{ActionDefinition, ActionProgram};

pub(crate) const MAX_ACTIONS: usize = 100;
const MAX_NAME_CHARS: usize = 100;
const MAX_STATEMENT_BYTES: usize = 20_000;
const MAX_BINDINGS: usize = 100;
const MAX_STRING_LENGTH: usize = 100_000;

/// Checks the program: its size, then each action in sorted name order, then
/// that no two actions bind one route.
pub(crate) fn check_program(program: &ActionProgram) -> Result<(), Refusal> {
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
pub(crate) fn check_action(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
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
            "actor" => matches!(binding.field.as_deref(), Some("id" | "role")),
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
pub(crate) fn check_keys(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
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
            let program: super::ActionProgram =
                serde_json::from_value(serde_json::json!({"version": 1, "actions": case["actions"]})).unwrap();
            crate::refusal::assert_case(&case, super::check_program(&program));
        }
    }
}
