
use super::refusal::{refuse, Refusal};
use super::ActionDefinition;
use serde_json::Value;

pub const MAX_GUARD_CHARS: usize = 200;

/// Parses and checks one action's `guard` key. `Ok(None)` when it has none.
pub fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<String>, Refusal> {
    let Some(value) = action.guard.as_ref() else { return Ok(None) };
    if action.kind == "query" {
        return Err(refuse("guard.not-mutation", format!("Action {name} is a query, so it cannot declare a guard. Declare it on a mutation.")));
    }
    if action.kind != "mutation" {
        return Err(refuse("guard.not-mutation", format!("Action {name} is a signal action, so it cannot declare a guard. Declare it on a mutation.")));
    }
    let valid = value.as_str().filter(|text| {
        !text.trim().is_empty()
            && text.chars().count() <= MAX_GUARD_CHARS
            && !text.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f)
    });
    match valid {
        Some(text) => Ok(Some(text.to_owned())),
        None => Err(refuse("guard.invalid", format!("Action {name} has an invalid guard. Use a message of 1 to 200 characters on one line."))),
    }
}

/// What a finished action means for its caller, Usage and its Signal.
pub enum Outcome {
    Changed,
    Unchanged,
    Refused(String),
}

/// Reads a successful result: a query or a signal action always changed what
/// it was asked to; a mutation changed something when `changes > 0`.
pub fn outcome(name: &str, action: &ActionDefinition, result: &Value) -> Outcome {
    if action.kind != "mutation" || result["changes"].as_u64().unwrap_or(0) > 0 {
        return Outcome::Changed;
    }
    match declaration(name, action) {
        Ok(Some(message)) => Outcome::Refused(message),
        _ => Outcome::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(actions: &Value) -> super::super::ActionProgram {
        serde_json::from_value(serde_json::json!({"version": 1, "actions": actions})).unwrap()
    }

    #[test]
    fn guards_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("guard-cases.json")).unwrap();
        assert!(cases.len() >= 8);
        for case in cases {
            let program = program(&case["actions"]);
            let mut names = program.actions.keys().collect::<Vec<_>>();
            names.sort();
            let outcome = names.into_iter().try_for_each(|name| super::super::validate_action(name, &program.actions[name]));
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn a_guarded_mutation_that_changes_nothing_is_refused_and_one_without_a_guard_is_not() {
        let actions = program(&serde_json::json!({
            "club.promote": {"kind": "mutation", "access": "authenticated", "guard": "Only an organizer can promote a member.",
                "statement": "UPDATE members SET role = 'organizer' WHERE id = ?1", "bindings": [{"source": "actor", "field": "id"}]},
            "club.touch": {"kind": "mutation", "access": "authenticated",
                "statement": "UPDATE members SET seen = 1 WHERE id = ?1", "bindings": [{"source": "actor", "field": "id"}]},
            "club.list": {"kind": "query", "access": "authenticated",
                "statement": "SELECT id FROM members LIMIT 5", "bindings": []}
        }));
        let none = serde_json::json!({"changes": 0, "lastInsertId": 0});
        let one = serde_json::json!({"changes": 1, "lastInsertId": 0});
        assert!(matches!(outcome("club.promote", &actions.actions["club.promote"], &none), Outcome::Refused(ref m) if m == "Only an organizer can promote a member."));
        assert!(matches!(outcome("club.promote", &actions.actions["club.promote"], &one), Outcome::Changed));
        assert!(matches!(outcome("club.touch", &actions.actions["club.touch"], &none), Outcome::Unchanged));
        assert!(matches!(outcome("club.list", &actions.actions["club.list"], &serde_json::json!([])), Outcome::Changed));
    }
}
