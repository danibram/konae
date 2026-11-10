//! The refusal type and the code table, copied from kernel-rs/src/refusal.rs
//! at 8ecb7e5 without `from_body` and `of`, which read HTTP bodies and
//! `anyhow` errors.

use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// A sentence the Kernel refuses a Capsule with, and the code of the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// A refusal under `code`. The message is the sentence both doors share.
pub(crate) fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into() }
}

impl Refusal {
    /// Several refusals named at once, such as every wrong key of a route's
    /// signature, are one refusal: their sentences joined by a space, under
    /// the first one's code. src/kernel/refusal.ts `joinRefusals` does the same.
    pub(crate) fn join(refusals: Vec<Refusal>) -> Refusal {
        let code = refusals.first().map_or("", |first| first.code);
        let message = refusals.into_iter().map(|refusal| refusal.message).collect::<Vec<_>>().join(" ");
        Refusal { code, message }
    }

    /// Who fixes it and how, from the table: `{ fix, remedy }`, and `tool`
    /// when one MCP tool clears it.
    pub(crate) fn hint(&self) -> Value {
        hint(self.code)
    }

    /// `{ code, message, hint }`, the shape every door answers a refusal with.
    pub(crate) fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message, "hint": self.hint()})
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

/// The table in refusal-codes.json, code to `{ fix, remedy, tool? }`.
pub(crate) fn table() -> &'static Map<String, Value> {
    static TABLE: OnceLock<Map<String, Value>> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(include_str!("../../src/refusal-codes.json")).expect("refusal-codes.json is a JSON object"))
}

/// The hint for a code, or null for a code the table does not hold.
pub(crate) fn hint(code: &str) -> Value {
    table().get(code).cloned().unwrap_or(Value::Null)
}

/// Checks one case of a shape case file against what a door answered: a
/// refusal case must name its code, and the door must give that code and the
/// case's sentence. src/kernel/refusal.ts `expectCase` is the editor's twin.
#[cfg(test)]
pub(crate) fn assert_case<T>(case: &Value, outcome: Result<T, Refusal>) {
    let name = &case["case"];
    match (case["error"].as_str(), outcome) {
        (None, Ok(_)) => {}
        (None, Err(refusal)) => panic!("{name}: refused with {refusal:?}"),
        (Some(message), Ok(_)) => panic!("{name}: passed, but the case expects {message}"),
        (Some(message), Err(refusal)) => {
            let code = case["code"].as_str().unwrap_or_else(|| panic!("{name}: a refusal case names no code"));
            assert_eq!(refusal.message, message, "{name}");
            assert_eq!(refusal.code, code, "{name}");
        }
    }
}
