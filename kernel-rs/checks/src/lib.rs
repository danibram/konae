//! The Kernel's checks of `actions.json` and `schema.sql`, with no I/O, for
//! the editor to call as WebAssembly (ADR-0027, decision 7). This crate is a
//! spike, and neither the Kernel nor the Validation Gate uses it yet.
//!
//! At 8ecb7e5 every check that `validate_action` (src/main.rs:2808) and
//! `action_shape::check_program` (src/action_shape.rs:21) run is pure. It
//! reads the parsed program and nothing else. Two files hold only such
//! checks, src/action_shape.rs and src/guard.rs, and this crate compiles
//! them from the Kernel's own source. The other files keep pure checks
//! beside I/O, so this crate copies the pure lines verbatim:
//!
//! | Kernel source | Copied here | Stays in the Kernel, and what it needs |
//! | --- | --- | --- |
//! | src/main.rs:360-414, 2808-3032 | action types, `validate_action`, the LIMIT rule, the SQL tokenizer | `apply_additive_schema` (3066), SQLite: the authorizer, two applies, `prepare` of every statement, `column_count` |
//! | src/sql_rules.rs:9-149 | statement and `schema.sql` rules | `stored_trigger_or_view` (154), SQLite on a database file |
//! | src/calls.rs:34-36, 59-61, 64-485, 511-543 | call declarations, `then`, `connects` | `check_release` (579), mcp.sqlite and the installation's hosts; the outbox and DNS (668 on) |
//! | src/http.rs:53-148, 173-182, 190-205 | `http` binding, duplicate routes | secrets, API keys and request signatures (207 on) |
//! | src/provider.rs:30-329 | `signature` and `fields` | `verify` (385), ring and the clock |
//! | src/usage.rs:51-53, 473-558 | `signal` | the Usage and Signal stores, files under `AppState` |
//! | src/notify.rs:26-27, 34-35, 44-235 | `notify` | `check_columns` (241), SQLite prepares against the schema; sending (368 on) |
//! | src/schedule.rs:44-47, 55-59, 125-189 | `schedule`, `subscribe` | the timer and `Clock` (83), the environment and the clock |
//! | src/refusal.rs:10-64, 79-95 | `Refusal`, the code table | `of` and `from_body`, `anyhow` errors and HTTP bodies |
//!
//! The checks that need SQLite compare the program with the schema. The
//! schema applies twice under an authorizer, every statement prepares
//! against it, a mutation returns no column and a notify column exists. The
//! editor runs these on sql.js, which has no authorizer hook, so
//! `refusedStatements` in src/agent/gate.ts restates by hand what the
//! Kernel's authorizer refuses: TEMP objects, CREATE TABLE ... AS and four
//! functions. Those rules exist in TypeScript and in the authorizer, and not
//! in any pure Rust check here.
//!
//! The WebAssembly boundary is four exports and no imports: `alloc`,
//! `dealloc`, `check_actions` and `check_schema`. Each check takes UTF-8
//! bytes the caller wrote into memory from `alloc`, and answers a pointer and
//! length packed into one u64 (pointer high) of a JSON answer the caller
//! frees with `dealloc`: `null`, or `{ code, message, hint }`.

// The copies keep the parsed declarations the Kernel reads when it runs an
// action; the checks only need to know whether they parse.
#![allow(dead_code)]

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[path = "../../src/action_shape.rs"]
mod action_shape;
mod calls;
#[path = "../../src/guard.rs"]
mod guard;
mod http;
mod notify;
mod provider;
mod refusal;
mod schedule;
mod sql_rules;
mod usage;

#[derive(Deserialize)]
struct ActionProgram {
    version: u8,
    actions: HashMap<String, ActionDefinition>,
}

#[derive(Clone, Deserialize)]
struct ActionDefinition {
    kind: String,
    access: String,
    /// Empty for a `"kind": "signal"` action, which runs no SQL.
    #[serde(default)]
    statement: String,
    bindings: Vec<ActionBinding>,
    /// The action's HTTP route, checked by `http::binding`.
    #[serde(default)]
    http: Option<Value>,
    /// The Signal the action records, checked by `usage::declaration`.
    #[serde(default)]
    signal: Option<Value>,
    /// A mutation's refusal when it changes no row, checked by `guard::declaration`.
    #[serde(default)]
    guard: Option<Value>,
    /// The email a mutation queues when it changes a row, checked by `notify::declaration`.
    #[serde(default)]
    notify: Option<Value>,
    /// When the Kernel's timer runs a mutation, checked by `schedule::declaration`.
    #[serde(default)]
    schedule: Option<Value>,
    /// Refused by `schedule::check_subscribe`: the Kernel pushes nothing.
    #[serde(default)]
    subscribe: Option<Value>,
    /// An action of `"kind": "call"`: its url, method, body, auth and then,
    /// checked by `calls::check_kind`.
    #[serde(flatten)]
    call: calls::CallKeys,
    /// Every other key, refused by `action_shape::check_keys`. It comes after
    /// `call` so serde hands it only the keys no field took.
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionBinding {
    source: String,
    key: Option<String>,
    #[serde(rename = "type")]
    value_type: Option<String>,
    max_length: Option<usize>,
    #[serde(default)]
    nullable: bool,
    field: Option<String>,
    value: Option<Value>,
}

fn validate_action(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    validate_declarations(name, action)?;
    action_shape::check_keys(name, action)
}

fn validate_declarations(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    action_shape::check_action(name, action)?;
    if calls::check_kind(name, action)?.is_some() {
        return Ok(());
    }
    usage::check_signal_action(name, action)?;
    if action.kind == "signal" {
        usage::declaration(name, action)?;
        http::binding(name, action)?;
        guard::declaration(name, action)?;
        notify::declaration(name, action)?;
        schedule::check_subscribe(name, action)?;
        schedule::declaration(name, action)?;
        return Ok(());
    }
    sql_rules::check_statement(name, action)?;
    if action.kind == "query" {
        if let Some(reason) = unbounded_query_reason(&action.statement) {
            return Err(refusal::refuse(
                "query.limit-missing",
                format!("Action {name} {reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one."),
            ));
        }
    }
    usage::declaration(name, action)?;
    http::binding(name, action)?;
    guard::declaration(name, action)?;
    notify::declaration(name, action)?;
    schedule::check_subscribe(name, action)?;
    schedule::declaration(name, action)?;
    Ok(())
}

/// Same rule as `src/lib/query-bound.ts`. A LIMIT counts only outside
/// parentheses, strings and comments. The editor and an exported Kernel refuse
/// the same query.
fn unbounded_query_reason(sql: &str) -> Option<&'static str> {
    let tokens = sql_tokens(sql);
    let top: Vec<&SqlTok> = tokens.iter().filter(|token| token.depth == 0).collect();
    if limit_index(&top).is_some() {
        return None;
    }
    if top.iter().any(|token| token.is_any(&["EXCEPT", "INTERSECT", "UNION"])) {
        return Some("combines more than one query and has no LIMIT on the whole result.");
    }
    if tokens.iter().any(|token| token.is_word("OVER")) {
        return Some("uses OVER, which keeps one result row per stored row, and has no LIMIT.");
    }
    let Some(from) = top.iter().position(|token| token.is_word("FROM")) else {
        return None;
    };
    for pair in top.windows(2) {
        if pair[0].is_word("GROUP") && pair[1].is_word("BY") {
            return Some("groups rows and has no LIMIT, so the result can grow with the data.");
        }
    }
    let Some(select) = top.iter().position(|token| token.is_word("SELECT")) else {
        return Some("reads rows and has no LIMIT.");
    };
    if select >= from || !aggregate_select_list(&top[select + 1..from]) {
        return Some("reads rows and has no LIMIT.");
    }
    None
}

/// Where the LIMIT clause starts among the top-level tokens. `:limit`,
/// `@limit` and `$limit` are named parameters, not the clause, as
/// `limitIndex` in src/lib/query-bound.ts reads them.
fn limit_index(top: &[&SqlTok]) -> Option<usize> {
    top.iter().enumerate().position(|(index, token)| {
        let parameter = index > 0 && [":", "@", "$"].iter().any(|symbol| top[index - 1].is_symbol(symbol));
        token.is_word("LIMIT") && !parameter
    })
}

fn aggregate_select_list(list: &[&SqlTok]) -> bool {
    let mut index = 0;
    if list.first().is_some_and(|token| token.is_any(&["ALL", "DISTINCT"])) {
        index = 1;
    }
    if index >= list.len() {
        return false;
    }
    let mut expressions = 0;
    while index < list.len() {
        if list[index].is_symbol(",") {
            index += 1;
            continue;
        }
        let Some(name) = list.get(index) else { return false; };
        let Some(open) = list.get(index + 1) else { return false; };
        if !name.is_any(&["AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL"]) || !open.is_symbol("(") {
            return false;
        }
        index += 2;
        let Some(close) = list.iter().skip(index).position(|token| token.is_symbol(")")) else {
            return false;
        };
        index += close + 1;
        if list.get(index).is_some_and(|token| token.is_word("AS")) {
            if !list.get(index + 1).is_some_and(|token| token.kind == SqlTokKind::Word || token.kind == SqlTokKind::Ident) {
                return false;
            }
            index += 2;
        } else if list.get(index).is_some_and(|token| token.kind == SqlTokKind::Word || token.kind == SqlTokKind::Ident) {
            index += 1;
        }
        expressions += 1;
        if index < list.len() && !list[index].is_symbol(",") {
            return false;
        }
    }
    expressions > 0
}

#[derive(Clone, Copy, PartialEq)]
enum SqlTokKind { Word, Ident, String, Symbol }

struct SqlTok {
    kind: SqlTokKind,
    text: String,
    depth: i32,
    /// Offset in chars into the statement, so a reader can quote the token as written.
    start: usize,
}

impl SqlTok {
    fn is_word(&self, word: &str) -> bool {
        self.kind == SqlTokKind::Word && self.text.eq_ignore_ascii_case(word)
    }

    fn is_any(&self, words: &[&str]) -> bool {
        self.kind == SqlTokKind::Word && words.iter().any(|word| self.text.eq_ignore_ascii_case(word))
    }

    fn is_symbol(&self, text: &str) -> bool {
        self.kind == SqlTokKind::Symbol && self.text == text
    }
}

fn sql_tokens(sql: &str) -> Vec<SqlTok> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut depth = 0i32;
    let mut index = 0;
    while index < chars.len() {
        let char = chars[index];
        let next = chars.get(index + 1).copied();
        if char == '-' && next == Some('-') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' { index += 1; }
            if index < chars.len() { index += 1; }
            continue;
        }
        if char == '/' && next == Some('*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') { index += 1; }
            index = (index + 2).min(chars.len());
            continue;
        }
        if char == '\'' || char == '"' || char == '`' || char == '[' {
            let close = if char == '[' { ']' } else { char };
            let mut end = index + 1;
            let mut text = String::new();
            while end < chars.len() {
                if chars[end] == close {
                    if close != ']' && chars.get(end + 1) == Some(&close) {
                        text.push(close);
                        end += 2;
                        continue;
                    }
                    break;
                }
                text.push(chars[end]);
                end += 1;
            }
            let span_end = (end + 1).min(chars.len());
            tokens.push(if char == '\'' {
                SqlTok { kind: SqlTokKind::String, text: String::new(), depth, start: index }
            } else {
                SqlTok { kind: SqlTokKind::Ident, text, depth, start: index }
            });
            index = span_end;
            continue;
        }
        if is_sql_word_start(char) {
            let mut end = index + 1;
            while end < chars.len() && is_sql_word_cont(chars[end]) { end += 1; }
            tokens.push(SqlTok { kind: SqlTokKind::Word, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '.' || chars[end] == '_') {
                end += 1;
            }
            tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: chars[index..end].iter().collect(), depth, start: index });
            index = end;
            continue;
        }
        if char.is_whitespace() {
            index += 1;
            continue;
        }
        if char == ')' { depth = (depth - 1).max(0); }
        tokens.push(SqlTok { kind: SqlTokKind::Symbol, text: char.to_string(), depth, start: index });
        if char == '(' { depth += 1; }
        index += 1;
    }
    tokens
}

fn is_sql_word_start(char: char) -> bool {
    char.is_ascii_alphabetic() || char == '_' || (char as u32) >= 0x80
}

fn is_sql_word_cont(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_' || char == '$' || (char as u32) >= 0x80
}

/// What the Kernel answers a publish whose actions.json it refuses, in the
/// order of `deploy` in src/main.rs: a file that does not parse and a wrong
/// version carry no code, a broken rule carries its code.
pub fn check_actions(source: &str) -> Value {
    let program: ActionProgram = match serde_json::from_str(source) {
        Ok(program) => program,
        Err(error) => return serde_json::json!({"code": null, "message": format!("Invalid actions.json: {error}")}),
    };
    if program.version != 1 {
        return serde_json::json!({"code": null, "message": "Unsupported actions.json version."});
    }
    action_shape::check_program(&program).err().map_or(Value::Null, |refused| refused.to_json())
}

/// What the Kernel answers a `schema.sql` holding a statement it will not
/// apply, before SQLite reads it.
pub fn check_schema(schema: &str) -> Value {
    sql_rules::schema_refusal(schema).map_or(Value::Null, |refused| refused.to_json())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    fn cases(file: &str) -> Vec<Value> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src").join(file);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn program(actions: &Value) -> super::ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    /// Each action by itself, in sorted name order, as the Kernel's tests
    /// for these files run them.
    fn each_action(actions: &Value) -> Result<(), super::refusal::Refusal> {
        let program = program(actions);
        let mut names = program.actions.keys().collect::<Vec<_>>();
        names.sort();
        names.into_iter().try_for_each(|name| super::validate_action(name, &program.actions[name]))
    }

    /// The case files the Kernel's own tests run through these checks give
    /// the same refusals here. action-cases.json, guard-cases.json and
    /// sql-rule-cases.json run in the modules they belong to.
    #[test]
    fn the_copied_checks_refuse_what_the_kernel_refuses() {
        for case in cases("call-cases.json") {
            let outcome = match case.get("connects") {
                Some(connects) => super::calls::check_connects(&json!({"connects": connects})),
                None => super::action_shape::check_program(&program(&case["actions"])),
            };
            super::refusal::assert_case(&case, outcome);
        }
        for case in cases("notify-cases.json") {
            super::refusal::assert_case(&case, super::action_shape::check_program(&program(&case["actions"])));
        }
        for file in ["signal-cases.json", "schedule-cases.json"] {
            for case in cases(file) {
                super::refusal::assert_case(&case, each_action(&case["actions"]));
            }
        }
        for case in cases("http-binding-cases.json") {
            let outcome = each_action(&case["actions"]).and_then(|_| super::http::validate_routes(&program(&case["actions"])));
            super::refusal::assert_case(&case, outcome);
        }
        for case in cases("signature-preset-cases.json") {
            let outcome = super::provider::signature("hooks.receive", case["method"].as_str().unwrap(), &case["signature"]);
            if case["error"].is_null() {
                assert_eq!(outcome, super::provider::signature("hooks.receive", case["method"].as_str().unwrap(), &case["expands"]));
            } else {
                super::refusal::assert_case(&case, outcome);
            }
        }
    }

    #[test]
    fn a_file_that_does_not_parse_or_has_another_version_is_refused_without_a_code() {
        assert_eq!(super::check_actions(r#"{"version": 2, "actions": {}}"#)["message"], "Unsupported actions.json version.");
        assert!(super::check_actions("{").get("code").is_some_and(Value::is_null));
        assert_eq!(super::check_actions(r#"{"version": 1, "actions": {}}"#), Value::Null);
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::alloc::{alloc as allocate, dealloc as deallocate, Layout};

    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        if len == 0 {
            return std::ptr::NonNull::dangling().as_ptr();
        }
        unsafe { allocate(Layout::from_size_align(len, 1).unwrap()) }
    }

    /// Frees what `alloc` gave or a check answered.
    ///
    /// # Safety
    /// `ptr` and `len` must come from one call of `alloc` or one answer.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
        if len != 0 {
            unsafe { deallocate(ptr, Layout::from_size_align(len, 1).unwrap()) }
        }
    }

    fn answer(value: serde_json::Value) -> u64 {
        let bytes = value.to_string().into_bytes().into_boxed_slice();
        let len = bytes.len();
        let ptr = Box::into_raw(bytes) as *mut u8;
        ((ptr as u64) << 32) | len as u64
    }

    /// # Safety
    /// `ptr` and `len` must name `len` bytes the caller wrote after `alloc`.
    unsafe fn text<'a>(ptr: *const u8, len: usize) -> Result<&'a str, std::str::Utf8Error> {
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// # Safety
    /// As `text`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn check_actions(ptr: *const u8, len: usize) -> u64 {
        answer(match unsafe { text(ptr, len) } {
            Ok(source) => super::check_actions(source),
            Err(_) => serde_json::json!({"code": null, "message": "actions.json is not UTF-8."}),
        })
    }

    /// # Safety
    /// As `text`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn check_schema(ptr: *const u8, len: usize) -> u64 {
        answer(match unsafe { text(ptr, len) } {
            Ok(schema) => super::check_schema(schema),
            Err(_) => serde_json::json!({"code": null, "message": "schema.sql is not UTF-8."}),
        })
    }
}
