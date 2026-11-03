//! A refusal of a Capsule carries a stable code beside its sentence
//! (docs/adr/0020-a-refusal-carries-a-stable-code.md).
//!
//! The site that refuses names the code, so a reworded sentence keeps its
//! code and a client branches on the code, never on the text. The codes live
//! in refusal-codes.json with who fixes each one and a one-line remedy; the
//! Validation Gate imports the same file (src/kernel/refusal.ts), and every
//! refusal case in the shape case files names the code both doors must give.

use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// A sentence the Kernel refuses a Capsule with, and the code of the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

/// A refusal under `code`. The message is the sentence both doors share.
pub fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into() }
}

impl Refusal {
    /// Several refusals named at once, such as every wrong key of a route's
    /// signature, are one refusal: their sentences joined by a space, under
    /// the first one's code. src/kernel/refusal.ts `joinRefusals` does the same.
    pub fn join(refusals: Vec<Refusal>) -> Refusal {
        let code = refusals.first().map_or("", |first| first.code);
        let message = refusals.into_iter().map(|refusal| refusal.message).collect::<Vec<_>>().join(" ");
        Refusal { code, message }
    }

    /// Who fixes it and how, from the table: `{ fix, remedy }`, and `tool`
    /// when one MCP tool clears it.
    pub fn hint(&self) -> Value {
        hint(self.code)
    }

    /// `{ code, message, hint }`, the shape every door answers a refusal with.
    pub fn to_json(&self) -> Value {
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
pub fn table() -> &'static Map<String, Value> {
    static TABLE: OnceLock<Map<String, Value>> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(include_str!("refusal-codes.json")).expect("refusal-codes.json is a JSON object"))
}

/// The hint for a code, or null for a code the table does not hold.
pub fn hint(code: &str) -> Value {
    table().get(code).cloned().unwrap_or(Value::Null)
}

/// The refusal an HTTP answer carries, `{ error, code }` as `refusal_error`
/// in main.rs writes it, when the code is in the table. A tool that calls a
/// Kernel route keeps the code this way.
pub fn from_body(body: &Value) -> Option<Refusal> {
    let (code, _) = table().get_key_value(body["code"].as_str()?)?;
    Some(Refusal { code: code.as_str(), message: body["error"].as_str()?.to_owned() })
}

/// The refusal an error carries, when a Capsule rule refused it.
pub fn of(error: &anyhow::Error) -> Option<&Refusal> {
    error.downcast_ref::<Refusal>()
}

/// Checks one case of a shape case file against what a door answered: a
/// refusal case must name its code, and the door must give that code and the
/// case's sentence. src/kernel/refusal.ts `expectCase` is the editor's twin.
#[cfg(any(test, feature = "testing"))]
pub fn assert_case<T>(case: &Value, outcome: Result<T, Refusal>) {
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

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// The parts of a Capsule a code may name, and the parts of the
    /// installation the owner names outside it.
    const GROUPS: &[&str] = &[
        "action", "call", "connects", "domain", "guard", "mutation", "notify", "path", "query", "release", "route", "schedule", "schema",
        "secret", "signal", "signature", "signin", "statement",
    ];

    /// Every refusal case in every shape case file, as (file, case, code).
    fn refusal_cases() -> Vec<(String, String, Option<String>)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with("-cases.json"))
            .collect();
        files.sort();
        let mut cases = Vec::new();
        for file in files {
            let value: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(&file)).unwrap()).unwrap();
            let list = value.get("cases").cloned().unwrap_or(value);
            for case in list.as_array().unwrap() {
                if case["error"].is_string() {
                    cases.push((file.clone(), case["case"].as_str().unwrap_or("").to_owned(), case["code"].as_str().map(str::to_owned)));
                }
            }
        }
        cases
    }

    #[test]
    fn every_code_is_a_dotted_name_in_a_known_group_with_a_fix_and_a_one_line_remedy() {
        for (code, row) in super::table() {
            let (group, rule) = code.split_once('.').unwrap_or_else(|| panic!("{code} has no group"));
            assert!(GROUPS.contains(&group), "{code} is not in a known group");
            assert!(!rule.is_empty() && rule.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'), "{code} is not lowercase dotted");
            assert!(matches!(row["fix"].as_str(), Some("capsule" | "owner" | "operator")), "{code} has no fix");
            let remedy = row["remedy"].as_str().unwrap_or("");
            assert!(!remedy.is_empty() && !remedy.contains('\n'), "{code} has no one-line remedy");
        }
    }

    #[test]
    fn every_refusal_case_names_a_code_from_the_table() {
        let cases = refusal_cases();
        assert!(cases.len() > 300);
        for (file, case, code) in cases {
            let code = code.unwrap_or_else(|| panic!("{file}: the refusal case \"{case}\" names no code"));
            assert!(super::table().contains_key(&code), "{file}: \"{case}\" names {code}, which refusal-codes.json does not hold");
        }
    }

    #[test]
    fn every_code_in_the_table_has_a_case() {
        let used: std::collections::HashSet<String> = refusal_cases().into_iter().filter_map(|(_, _, code)| code).collect();
        let unused: Vec<&String> = super::table().keys().filter(|code| !used.contains(*code)).collect();
        assert!(unused.is_empty(), "no refusal case uses {unused:?}");
    }

    /// A code the Kernel names in its source is a row of the table, so a typo
    /// cannot answer a code no client knows.
    #[test]
    fn every_code_the_kernel_names_is_in_the_table() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut named = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for part in source.split("refuse(\"").skip(1) {
                let code = &part[..part.find('"').unwrap()];
                named += 1;
                assert!(super::table().contains_key(code), "{} names {code}, which refusal-codes.json does not hold", path.display());
            }
        }
        assert!(named > 100);
    }

    #[test]
    fn a_joined_refusal_keeps_the_first_code_and_every_sentence() {
        let joined = super::Refusal::join(vec![
            super::refuse("signature.header-invalid", "Action a.b has an invalid signature header."),
            super::refuse("signature.secret-invalid", "Action a.b names an invalid signature secret."),
        ]);
        assert_eq!(joined.code, "signature.header-invalid");
        assert_eq!(joined.to_string(), "Action a.b has an invalid signature header. Action a.b names an invalid signature secret.");
        assert_eq!(joined.to_json()["hint"]["fix"], "capsule");
    }
}
