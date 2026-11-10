//! An action's `notify` key, copied from kernel-rs/src/notify.rs at 8ecb7e5.
//! `check_columns` prepares the statement against the schema and needs
//! SQLite; queueing and sending notices stay in the Kernel.

use super::refusal::{refuse, Refusal};
use super::ActionDefinition;
use serde_json::Value;

pub(crate) const MAX_SUBJECT_CHARS: usize = 120;
pub(crate) const MAX_BODY_CHARS: usize = 2000;
/// Columns a `changes` list may name.
const MAX_CHANGES_COLUMNS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Recipient {
    /// The Project Owner's account email.
    Owner,
    /// The calling account's verified email.
    Actor,
    /// The verified email of each account whose ID a changed row carries in
    /// this column.
    Column(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Input(String),
    ActorId,
    KernelNow,
    Changes,
    ChangesCount,
}

#[derive(Debug)]
pub(crate) struct Declaration {
    pub(crate) to: Recipient,
    /// Columns `{changes}` lists, in order.
    changes: Vec<String>,
    subject: Vec<Part>,
    body: Vec<Part>,
}

impl Declaration {
    /// The columns the Kernel appends as RETURNING: the `changes` list, then
    /// the recipient column when the list does not hold it. Empty when the
    /// notice needs no row.
    pub(crate) fn returning(&self) -> Vec<String> {
        let mut columns = self.changes.clone();
        if let Recipient::Column(column) = &self.to {
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        }
        columns
    }
}

/// A SQL name the Kernel can quote: letters, digits and underscores, not
/// starting with a digit, at most 64 characters.
fn is_column_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value.len() <= 64
}

/// Parses and checks one action's `notify` key. `Ok(None)` when it has none.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Declaration>, Refusal> {
    let Some(value) = action.notify.as_ref() else { return Ok(None) };
    // Each message is written out so src/sdk/contract.test.ts finds it.
    if action.kind == "query" {
        return Err(refuse("notify.not-mutation", format!("Action {name} is a query, so it cannot declare notify. Declare it on a mutation.")));
    }
    if action.kind != "mutation" {
        return Err(refuse("notify.not-mutation", format!("Action {name} is a signal action, so it cannot declare notify. Declare it on a mutation.")));
    }
    let record = value
        .as_object()
        .filter(|record| record.keys().all(|key| matches!(key.as_str(), "to" | "subject" | "body" | "changes")))
        .filter(|record| record.get("subject").is_some_and(Value::is_string) && record.get("body").is_some_and(Value::is_string))
        .ok_or_else(|| refuse("notify.invalid", format!(
            r#"Action {name} has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else."#
        )))?;
    let column = record
        .get("to")
        .and_then(Value::as_object)
        .filter(|to| to.len() == 1)
        .and_then(|to| to.get("column"))
        .and_then(Value::as_str)
        .filter(|column| is_column_name(column));
    let to = match (record.get("to").and_then(Value::as_str), column) {
        (Some("owner"), _) => Recipient::Owner,
        (Some("actor"), _) => Recipient::Actor,
        (None, Some(column)) => Recipient::Column(column.to_owned()),
        _ => return Err(refuse("notify.recipient-invalid", format!(
            r#"Action {name} has an invalid notify recipient. Use "owner", "actor" or {{"column": "<name>"}} for a column of account IDs."#
        ))),
    };
    let changes = match record.get("changes") {
        None => Vec::new(),
        Some(value) => {
            let columns: Option<Vec<String>> = value
                .as_array()
                .filter(|list| (1..=MAX_CHANGES_COLUMNS).contains(&list.len()))
                .and_then(|list| list.iter().map(|item| item.as_str().filter(|c| is_column_name(c)).map(str::to_owned)).collect());
            match columns {
                Some(columns) if columns.iter().enumerate().all(|(index, column)| !columns[..index].contains(column)) => columns,
                _ => return Err(refuse("notify.changes-invalid", format!(
                    "Action {name} has an invalid notify changes list. Name 1 to 8 columns of the changed table, each once, in letters, digits and underscores."
                ))),
            }
        }
    };
    let subject = record["subject"].as_str().unwrap_or_default();
    if subject.trim().is_empty() || subject.chars().count() > MAX_SUBJECT_CHARS || subject.chars().any(is_control) {
        return Err(refuse("notify.subject-invalid", format!("Action {name} has an invalid notify subject. Use 1 to 120 characters on one line.")));
    }
    let body = record["body"].as_str().unwrap_or_default();
    if body.trim().is_empty() || body.chars().count() > MAX_BODY_CHARS || body.chars().any(|c| c != '\n' && is_control(c)) {
        return Err(refuse("notify.body-invalid", format!(
            "Action {name} has an invalid notify body. Use 1 to 2000 characters, with line breaks and no other control characters."
        )));
    }
    let inputs: Vec<&str> = action
        .bindings
        .iter()
        .filter(|binding| binding.source == "input")
        .filter_map(|binding| binding.key.as_deref())
        .collect();
    let subject = parts(name, "subject", subject, &inputs)?;
    let body = parts(name, "body", body, &inputs)?;
    if subject.contains(&Part::Changes) {
        return Err(refuse("notify.changes-in-subject", format!(
            "Action {name} puts {{changes}} in its notify subject. Put {{changes}} in the body, and {{changes.count}} in the subject."
        )));
    }
    if body.contains(&Part::Changes) && changes.is_empty() {
        return Err(refuse("notify.changes-undeclared", format!(
            r#"Action {name} uses {{changes}} in its notify body without a changes list. Name the columns to show in notify "changes"."#
        )));
    }
    Ok(Some(Declaration { to, changes, subject, body }))
}

fn is_control(c: char) -> bool {
    (c as u32) < 0x20 || c as u32 == 0x7f
}

/// Splits a template into text and placeholders. Every `{` opens a
/// placeholder that the next `}` closes; a `}` on its own is text.
fn parts(name: &str, field: &str, template: &str, inputs: &[&str]) -> Result<Vec<Part>, Refusal> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(Part::Text(rest[..open].to_owned()));
        }
        let Some(close) = rest[open..].find('}') else {
            return Err(refuse("notify.placeholder-unclosed", format!(r#"Action {name} has an unclosed placeholder in its notify {field}. Close it with "}}"."#)));
        };
        let token = &rest[open + 1..open + close];
        let part = match token.strip_prefix("input.") {
            Some(key) if inputs.contains(&key) => Part::Input(key.to_owned()),
            _ if token == "actor.id" => Part::ActorId,
            _ if token == "kernel.now" => Part::KernelNow,
            _ if token == "changes" => Part::Changes,
            _ if token == "changes.count" => Part::ChangesCount,
            _ => {
                return Err(refuse("notify.placeholder-unknown", format!(
                    "Action {name} names an unknown placeholder {{{token}}} in its notify {field}. Use {{input.<key>}} for one of its input bindings, {{actor.id}}, {{kernel.now}}, {{changes}} or {{changes.count}}."
                )));
            }
        };
        parts.push(part);
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_owned()));
    }
    Ok(parts)
}

/// The statement with the Kernel's own RETURNING clause for these columns.
/// A trailing `;` and what follows it are cut; a trailing line comment ends
/// at the line break before the clause. The author still may not write
/// RETURNING (`mutation.returning`).
pub(crate) fn with_returning(statement: &str, columns: &[String]) -> String {
    let tokens = super::sql_tokens(statement);
    let end = tokens.iter().find(|token| token.is_symbol(";")).map_or(statement.chars().count(), |token| token.start);
    let kept: String = statement.chars().take(end).collect();
    // Brackets, not double quotes: SQLite reads an unknown "name" as a
    // string, so a misspelled column would pass the check and list itself.
    let list = columns.iter().map(|column| format!("[{column}]")).collect::<Vec<_>>().join(", ");
    format!("{}\nRETURNING {list}", kept.trim_end())
}

/// The columns a mutation's notice reads back, or none when it lists no
/// rows and addresses no column.
pub(crate) fn returning(name: &str, action: &ActionDefinition) -> Vec<String> {
    match declaration(name, action) {
        Ok(Some(declared)) => declared.returning(),
        _ => Vec::new(),
    }
}
