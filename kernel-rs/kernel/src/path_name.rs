//! The name an app may answer on under `<domain>/p/<name>`
//! (docs/adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md).
//!
//! The owner names the path with `path_set`. The Kernel checks the name here,
//! and the editor checks it in `src/lib/path-name.ts`. Both run the rules in
//! `path-cases.json` and give the same codes and words.

use super::refusal::{refuse, Refusal};

/// Names Konae serves on its own paths, or that would read as one of its
/// own addresses. A reserved name is refused before its length or characters
/// are checked, so `p` and `runtime.js` say why they are refused.
const RESERVED: &[&str] = &["_konae", "p", "app", "mcp", "www", "api", "admin", "runtime.js"];

const MIN_LENGTH: usize = 3;
const MAX_LENGTH: usize = 40;

/// The name in its stored form, or the refusal. Surrounding whitespace is
/// trimmed and the name is lowercased first.
pub fn check_name(raw: &str) -> Result<String, Refusal> {
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Err(refuse("path.missing", "Name the path, such as spring-menu."));
    }
    if RESERVED.contains(&name.as_str()) {
        return Err(refuse(
            "path.reserved",
            format!("{name} is reserved for Konae's own addresses. Choose another name, such as spring-menu."),
        ));
    }
    let length = name.chars().count();
    if length < MIN_LENGTH {
        return Err(refuse("path.length", format!("{name} is too short. Use {MIN_LENGTH} to {MAX_LENGTH} characters.")));
    }
    if length > MAX_LENGTH {
        return Err(refuse("path.length", format!("{name} is too long. Use {MIN_LENGTH} to {MAX_LENGTH} characters.")));
    }
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(refuse(
            "path.name-invalid",
            "Use only letters, digits and hyphens in the path name, such as spring-menu.",
        ));
    }
    if name.starts_with('-') {
        return Err(refuse("path.hyphen-edge", "Start the path name with a letter or digit, not a hyphen."));
    }
    if name.ends_with('-') {
        return Err(refuse("path.hyphen-edge", "End the path name with a letter or digit, not a hyphen."));
    }
    if is_uuid_shape(&name) {
        return Err(refuse(
            "path.uuid-shape",
            format!("{name} has the shape of an app id. Choose a name that does not look like one, such as spring-menu."),
        ));
    }
    Ok(name)
}

/// True for the 8-4-4-4-12 hex form with hyphens and for the 32 hex digits
/// without them. Both are the id the Kernel gives a Deployment.
fn is_uuid_shape(name: &str) -> bool {
    let bytes = name.as_bytes();
    match bytes.len() {
        32 => bytes.iter().all(u8::is_ascii_hexdigit),
        36 => bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| if matches!(index, 8 | 13 | 18 | 23) { *byte == b'-' } else { byte.is_ascii_hexdigit() }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// The CLI runs the same file in src/lib/path-name.test.ts.
    #[test]
    fn names_are_refused_with_the_words_the_cli_uses() {
        let file: Value = serde_json::from_str(include_str!("path-cases.json")).unwrap();
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for case in cases {
            let outcome = super::check_name(case["name"].as_str().unwrap());
            if case["error"].is_null() {
                assert_eq!(outcome, Ok(case["nameOut"].as_str().unwrap().to_owned()), "{}", case["case"]);
            } else {
                crate::refusal::assert_case(case, outcome);
            }
        }
    }
}
