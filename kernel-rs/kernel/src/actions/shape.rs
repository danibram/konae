
use serde_json::Value;

use crate::refusal::{refuse, Refusal};
use crate::{ActionDefinition, ActionProgram};

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
            check_names(&names)?;
            for (at, name) in names.iter().enumerate() {
                if let Err(refused) = check_types(name, &actions[*name]) {
                    for earlier in &names[..at] {
                        let action: ActionDefinition = serde_json::from_value(actions[*earlier].clone())?;
                        crate::validate_action(earlier, &action)?;
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

fn check_names(names: &[&String]) -> Result<(), Refusal> {
    let invalid = names.iter().filter(|name| !valid_name(name)).map(|name| name.as_str()).collect::<Vec<_>>();
    if invalid.len() > 1 { return Err(refuse("action.name-invalid", format!("Invalid server action names: {}", invalid.join(", ")))); }
    Ok(())
}

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
    if !matches!(text("access"), Some("anonymous" | "authenticated" | "scheduled")) {
        return Err(refuse("action.access-invalid", format!(r#"Action {name} has an invalid access. Use "anonymous", "authenticated" or "scheduled"."#)));
    }
    let kind = text("kind").unwrap_or_default();
    if !matches!(kind, "query" | "mutation" | "signal" | "call") {
        return Err(refuse("action.kind-invalid", format!(r#"Action {name} has an invalid kind. Use "query", "mutation", "signal" or "call"."#)));
    }
    if action.contains_key("page") && kind != "query" {
        return Err(refuse("page.query", format!("Action {name} declares a page path, so it must be an anonymous query.")));
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
        Some("literal") => binding.get("value").is_none_or(|value| value.is_null() || value.is_string() || value.is_number()),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid()) }
}

/// Checks the program: its size, then each action in sorted name order, then
/// that no two actions bind one route.
pub fn check_program(program: &ActionProgram) -> Result<(), Refusal> {
    if program.actions.len() > MAX_ACTIONS {
        return Err(refuse("action.count-over", "A project may declare at most 100 actions."));
    }
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    check_names(&names)?;
    for name in names {
        crate::validate_action(name, &program.actions[name])?;
    }
    for (name, action) in &program.actions {
        let Some(issue) = &action.issue else { continue; };
        let valid = program.actions.get(&issue.download).is_some_and(|rule| rule.kind == "query" && rule.access == "authenticated"
            && rule.bindings.iter().any(|binding| binding.source == "actor" && binding.field.as_deref() == Some("id"))
            && rule.bindings.iter().any(|binding| binding.source == "input" && binding.key.as_deref() == Some("issueKey") && binding.value_type.as_deref() == Some("string")));
        if !valid {
            return Err(refuse("documents.download-rule-invalid", format!("Action {name} issue.download must name an authenticated query with an actor binding and the issue key input.")));
        }
    }
    crate::http::validate_routes(program)?;
    crate::page::check_program(program)?;
    crate::calls::check_program(program)
}

/// The checks that come before an action's SQL: its name, access, kind,
/// bindings and size.
pub fn check_action(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if !valid_name(name) {
        return Err(refuse("action.name-invalid", format!("Invalid server action name: {name}")));
    }
    if action.access == "granted" {
        return Err(refuse(
            "action.access-unsupported",
            format!(r#"Action {name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated"."#),
        ));
    }
    if !matches!(action.access.as_str(), "anonymous" | "authenticated" | "scheduled") {
        return Err(refuse("action.access-invalid", format!(r#"Action {name} has an invalid access. Use "anonymous", "authenticated" or "scheduled"."#)));
    }
    if !matches!(action.kind.as_str(), "query" | "mutation" | "signal" | "call") {
        return Err(refuse("action.kind-invalid", format!(r#"Action {name} has an invalid kind. Use "query", "mutation", "signal" or "call"."#)));
    }
    if action.access == "scheduled" && (!matches!(action.kind.as_str(), "query" | "mutation")
        || action.http.is_some() || action.page.is_some() || action.notify.is_some() || action.issue.is_some()
        || action.signal.is_some() || action.schedule.is_some() || action.subscribe.is_some()
        || action.bindings.iter().any(|binding| binding.source == "actor"))
    {
        return Err(refuse("action.scheduled-shape", format!("Action {name} with access scheduled must be a query or mutation without an actor binding, route or external effect.")));
    }
    for (index, binding) in action.bindings.iter().enumerate() {
        if binding.nullable && binding.source != "input" {
            return Err(refuse("action.binding-nullable-invalid", format!("Action {name} has an invalid nullable input binding.")));
        }
        let valid = match binding.source.as_str() {
            "input" => binding.key.is_some() && matches!(binding.value_type.as_deref(), Some("string" | "number" | "boolean")),
            "actor" => binding.field.as_deref() == Some("id"),
            "kernel" => binding.field.as_deref() == Some("now"),
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
    if let Some(issue) = &action.issue {
        let input_key = action.bindings.iter().any(|binding| binding.source == "input" && binding.key.as_deref() == Some(issue.key.as_str()) && binding.value_type.as_deref() == Some("string"));
        if action.kind != "mutation" || action.access != "authenticated" || !input_key {
            return Err(refuse("documents.issue-key-invalid", format!("Action {name} issue.key must name a string input binding on an authenticated mutation.")));
        }
        if !valid_name(&issue.download) {
            return Err(refuse("documents.download-rule-invalid", format!("Action {name} issue.download must name an authenticated query with an actor binding and the issue key input.")));
        }
        if issue.snapshot.is_empty() || issue.snapshot.len() > 32 || issue.snapshot.iter().any(|column| !document_identifier(column)) {
            return Err(refuse("documents.snapshot-invalid", format!("Action {name} issue.snapshot must name 1 to 32 returned columns.")));
        }
        if issue.template.is_empty() || issue.template.len() > 64 * 1024 || issue.typst_version != crate::documents::DEFAULT_TYPST_VERSION {
            return Err(refuse("documents.template-invalid", format!("Action {name} issue must include a template of at most 64 KiB and Typst version {}.", crate::documents::DEFAULT_TYPST_VERSION)));
        }
        if issue.resources.iter().any(|(path, value)| !document_path(path) || match value.strip_prefix("base64:") {
            Some(encoded) => base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).map_or(true, |bytes| bytes.len() > 256 * 1024),
            None => value.len() > 256 * 1024,
        }) {
            return Err(refuse("documents.resources-invalid", format!("Action {name} issue.resources must use relative paths and values up to 256 KiB.")));
        }
    }
    Ok(())
}

fn document_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn document_path(value: &str) -> bool {
    let path = std::path::Path::new(value);
    !value.is_empty() && value.len() <= 200 && !path.is_absolute()
        && path.components().all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn allowed_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "query" => &["kind", "access", "statement", "bindings", "http", "page", "mcp"],
        "mutation" => &["kind", "access", "statement", "bindings", "http", "page", "signal", "guard", "notify", "schedule", "issue"],
        "signal" => &["kind", "access", "statement", "bindings", "signal", "page"],
        _ => &["kind", "access", "statement", "bindings", "url", "method", "body", "headers", "auth", "then", "page"],
    }
}

/// Refuses a key the action's kind does not take. It runs after the action's
/// other checks, so a key with its own refusal, such as a guard on a query,
/// keeps it. A key set to null counts as absent, on both doors. Keys are named
/// in sorted order, as the editor names them.
pub fn check_keys(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if let Some(mcp) = action.extra.get("mcp").filter(|value| !value.is_null()) {
        let valid = action.kind == "query"
            && action.access == "authenticated"
            && mcp.as_object().is_some_and(|object| {
                object.len() == 1
                    && object.get("description").and_then(Value::as_str).is_some_and(|description| !description.is_empty() && description.len() <= 1000)
            });
        if !valid {
            return Err(refuse("action.mcp-shape", format!("Action {name} must be an authenticated query with an MCP description.")));
        }
    }
    let declared = [
        ("http", action.http.is_some()),
        ("page", action.page.is_some()),
        ("signal", action.signal.is_some()),
        ("guard", action.guard.is_some()),
        ("notify", action.notify.is_some()),
        ("schedule", action.schedule.is_some()),
        ("issue", action.issue.is_some()),
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

    #[test]
    fn the_kernel_refuses_the_action_shapes_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../action-cases.json")).unwrap();
        assert!(cases.len() >= 25);
        for case in cases {
            let source = serde_json::json!({"version": 1, "actions": case["actions"]}).to_string();
            let outcome = super::read(&source).map(|_| ()).map_err(|error_value| {
                crate::refusal::of(&error_value).cloned().unwrap_or_else(|| panic!("{}: {error_value} has no code", case["case"]))
            });
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn page_declarations_match_the_editor_refusals() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../page-cases.json")).unwrap();
        for case in cases {
            let source = serde_json::json!({"version": 1, "actions": case["actions"]}).to_string();
            let outcome = super::read(&source).map(|_| ()).map_err(|error_value| {
                crate::refusal::of(&error_value).cloned().unwrap_or_else(|| panic!("{}: {error_value} has no code", case["case"]))
            });
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn documents_declarations_match_the_editor_refusals() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../documents-cases.json")).unwrap();
        for case in cases {
            let source = serde_json::json!({"version": 1, "actions": case["actions"]}).to_string();
            let database = std::env::temp_dir().join(format!("konae-documents-shape-{}.sqlite", uuid::Uuid::new_v4()));
            let outcome = super::read(&source).and_then(|program| {
                crate::schema_change::apply(&database, case["schema"].as_str().unwrap(), &program, crate::schema_change::Mode::Plan).map(|_| ())
            }).map_err(|error| {
                crate::refusal::of(&error).cloned().unwrap_or_else(|| panic!("{}: {error} has no code", case["case"]))
            });
            let _ = std::fs::remove_file(database);
            crate::refusal::assert_case(&case, outcome);
        }
    }
}
