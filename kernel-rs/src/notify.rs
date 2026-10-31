//! A mutation's notice: an email the Kernel queues when the statement
//! changes a row (ADR-0016).
//!
//! `"notify": { "to": "owner" | "actor", "subject", "body" }` sends to the
//! Project Owner's account email or to the calling account's verified email,
//! never to an address from the input, so an app cannot relay mail. The
//! subject and body are templates whose only placeholders are
//! `{input.<key>}`, for one of the action's input bindings, and `{actor.id}`.
//! A value loses its line breaks and is cut to 200 characters, and the HTML
//! part escapes it. The notice goes through the mail queue (`mail.rs`) under
//! the installation's total and its own creator, app and recipient budgets.
//! The mutation's answer never depends on it: a notice that cannot be sent
//! is counted in Usage under `notify` with the reason.
//!
//! `parseNotify` in src/kernel/actions.ts makes the same checks, and
//! kernel-rs/src/notify-cases.json holds the cases both test suites run.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};

use super::mail::{self, Notice, NoticeOutcome};
use super::refusal::{refuse, Refusal};
use super::{ActionDefinition, AppState, Deployment};

pub(crate) const MAX_SUBJECT_CHARS: usize = 120;
pub(crate) const MAX_BODY_CHARS: usize = 2000;
/// Characters one placeholder's value keeps.
const MAX_VALUE_CHARS: usize = 200;
/// Characters a rendered subject and body keep.
const MAX_RENDERED_SUBJECT_CHARS: usize = 200;
const MAX_RENDERED_BODY_CHARS: usize = 5000;

/// Columns a `changes` list may name.
const MAX_CHANGES_COLUMNS: usize = 8;
/// Changed rows the Kernel reads back from one run; the rest are counted
/// in `{changes.count}` but not listed or addressed.
pub(crate) const MAX_RETURNED_ROWS: usize = 1000;
/// Rows `{changes}` lists in one notice before "and N more".
const MAX_LISTED_ROWS: usize = 50;
/// Distinct accounts one run notifies through a column (ADR-0021).
pub(crate) const MAX_RECIPIENTS_PER_RUN: usize = 50;

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

/// Refuses a `changes` or `to` column the statement cannot return, against
/// the schema `connection` holds. The Validation Gate makes the same check
/// in src/agent/gate.ts, and kernel-rs/src/notify-column-cases.json holds the
/// cases both run.
pub(crate) fn check_columns(connection: &Connection, name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    let columns = returning(name, action);
    if columns.is_empty() {
        return Ok(());
    }
    let returns = connection
        .prepare(&with_returning(&action.statement, &columns))
        .is_ok_and(|statement| statement.column_count() == columns.len());
    if returns {
        Ok(())
    } else {
        Err(refuse("notify.column-unknown", format!(
            "Action {name} names a notify column its statement cannot return. Name columns of the table the statement changes."
        )))
    }
}

/// `{kernel.now}`: the run's time as `YYYY-MM-DD HH:MM UTC`.
pub(crate) fn format_now(now_ms: u64) -> String {
    let minutes = now_ms / 60_000;
    let (year, month, day) = super::copies::civil_from_days((minutes / 1440) as i64);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", minutes % 1440 / 60, minutes % 60)
}

/// What the statement changed, as a notice reads it.
pub(crate) struct Changed<'a> {
    /// Rows the statement changed, as SQLite counts them.
    pub(crate) count: u64,
    /// The first rows the Kernel read back, with the RETURNING columns.
    pub(crate) rows: &'a [Map<String, Value>],
    /// The time `{kernel.now}` shows, in milliseconds.
    pub(crate) now: u64,
}

/// What one notice fills its placeholders with.
struct Fill<'a> {
    input: &'a Value,
    actor: Option<&'a str>,
    now: u64,
    columns: &'a [String],
    rows: &'a [&'a Map<String, Value>],
    count: u64,
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => mail::one_line(value, MAX_VALUE_CHARS),
        Some(Value::Null) | None => String::new(),
        Some(value) => mail::one_line(&value.to_string(), MAX_VALUE_CHARS),
    }
}

/// `{changes}`: one line per row, its listed columns joined by " · ", at
/// most 50 rows, then a line that counts the rest.
fn listed(fill: &Fill<'_>) -> String {
    let mut lines: Vec<String> = fill
        .rows
        .iter()
        .take(MAX_LISTED_ROWS)
        .map(|row| {
            let values: Vec<String> = fill.columns.iter().map(|column| value_text(row.get(column))).collect();
            format!("- {}", values.join(" · "))
        })
        .collect();
    let more = fill.count.saturating_sub(lines.len() as u64);
    if more > 0 {
        lines.push(format!("- and {more} more"));
    }
    lines.join("\n")
}

/// Fills a template. A value loses its line breaks and keeps at most 200
/// characters; a missing or null input and a caller without an ID fill
/// nothing.
fn fill(parts: &[Part], values: &Fill<'_>) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            Part::Text(value) => text.push_str(value),
            Part::Input(key) => text.push_str(&value_text(values.input.get(key))),
            Part::ActorId => text.push_str(values.actor.unwrap_or_default()),
            Part::KernelNow => text.push_str(&format_now(values.now)),
            Part::Changes => text.push_str(&listed(values)),
            Part::ChangesCount => text.push_str(&values.count.to_string()),
        }
    }
    text
}

/// Subject, plain text and HTML of one notice. The template's own line
/// breaks stay in the body; the HTML part escapes everything and turns each
/// line break into `<br>`. A fixed last line says which app sent it and why
/// the recipient gets it.
fn render(declared: &Declaration, app_name: &str, values: &Fill<'_>) -> (String, String, String) {
    let app = mail::one_line(app_name, 80);
    let app = if app.is_empty() { "Konae".to_owned() } else { app };
    let subject = mail::one_line(&fill(&declared.subject, values), MAX_RENDERED_SUBJECT_CHARS);
    let subject = if subject.is_empty() { format!("Notice from {app}") } else { subject };
    let body: String = fill(&declared.body, values).trim().chars().take(MAX_RENDERED_BODY_CHARS).collect();
    let why = match declared.to {
        Recipient::Owner => format!("{app} sent this notice through Konae because you own the app."),
        Recipient::Actor => format!("{app} sent this notice through Konae after something you did in the app."),
        Recipient::Column(_) => format!("{app} sent this notice through Konae because a change in the app names your account."),
    };
    let text = format!("{body}\n\n-- \n{why}\n");
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;line-height:1.5;color:#20232a\"><p>{}</p><p style=\"color:#586174\">{}</p></body></html>",
        mail::escape_html(&body).replace('\n', "<br>"),
        mail::escape_html(&why),
    );
    (subject, text, html)
}

/// Who called the mutation, as the notice needs it.
pub(crate) struct Caller<'a> {
    /// The app account behind a session. A visitor, a signed request and an
    /// API key have none.
    pub(crate) account: Option<&'a str>,
    /// The ID `{actor.id}` fills: the account's, or an API key's.
    pub(crate) actor_id: Option<&'a str>,
}

/// Records the notices of a mutation that changed a row, and counts each
/// outcome in Usage under `notify`: `<action>: queued`, or `<action>:
/// <reason>` when nothing will be sent. Call it only after the statement
/// changed a row. It never fails the mutation.
pub(crate) async fn record(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    action: &ActionDefinition,
    input: &Value,
    caller: Caller<'_>,
    changed: Changed<'_>,
) {
    let Ok(Some(declared)) = declaration(name, action) else { return };
    let state = state.clone();
    let deployment = deployment.clone();
    let name = name.to_owned();
    let input = input.clone();
    let account = caller.account.map(str::to_owned);
    let actor_id = caller.actor_id.map(str::to_owned);
    let (count, rows, now) = (changed.count, changed.rows.to_vec(), changed.now);
    let outcome = tokio::task::spawn_blocking(move || {
        let run = Changed { count, rows: &rows, now };
        let outcomes = queue_notices(&state, &deployment, &name, &declared, &input, account.as_deref(), actor_id.as_deref(), &run)
            .unwrap_or_else(|error_value| {
                eprintln!("Notice of {name} on {} could not be recorded: {error_value}", deployment.id);
                vec![NoticeOutcome::Refused("storage-error")]
            });
        for outcome in outcomes {
            let reason = match outcome {
                NoticeOutcome::Queued => "queued",
                NoticeOutcome::Refused(kind) => kind,
            };
            super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Notify(format!("{name}: {reason}")));
        }
    })
    .await;
    if let Err(error_value) = outcome {
        eprintln!("Notice task failed: {error_value}");
    }
}

/// The account IDs a column names, in ID order, each with the rows that
/// name it. A row whose column is not text names nobody.
fn recipients<'a>(column: &str, rows: &'a [Map<String, Value>]) -> std::collections::BTreeMap<&'a str, Vec<&'a Map<String, Value>>> {
    let mut named: std::collections::BTreeMap<&str, Vec<&Map<String, Value>>> = std::collections::BTreeMap::new();
    for row in rows {
        if let Some(id) = row.get(column).and_then(Value::as_str).filter(|id| !id.is_empty()) {
            named.entry(id).or_default().push(row);
        }
    }
    named
}

#[allow(clippy::too_many_arguments)]
fn queue_notices(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    declared: &Declaration,
    input: &Value,
    account: Option<&str>,
    actor_id: Option<&str>,
    changed: &Changed<'_>,
) -> anyhow::Result<Vec<NoticeOutcome>> {
    let Some(queue) = state.accounts.queue.as_ref() else {
        return Ok(vec![NoticeOutcome::Refused("no-transport")]);
    };
    let creator = super::accounts::project_creator(state, &deployment.project_id).unwrap_or_else(|| "operator".into());
    let send = |recipient: &str, rows: &[&Map<String, Value>], count: u64| -> anyhow::Result<NoticeOutcome> {
        let values = Fill { input, actor: actor_id, now: changed.now, columns: &declared.changes, rows, count };
        let (subject, text, html) = render(declared, &deployment.project_name, &values);
        let scope = super::auth::deployment_scope(&deployment.id);
        queue.record_notice(&Notice {
            recipient,
            scope: &scope,
            creator: &creator,
            app_name: &deployment.project_name,
            action: name,
            subject: &subject,
            text: &text,
            html: &html,
        })
    };
    let all: Vec<&Map<String, Value>> = changed.rows.iter().collect();
    match &declared.to {
        Recipient::Owner => match owner_email(state, &creator)? {
            Some(email) => Ok(vec![send(&email, &all, changed.count)?]),
            None => Ok(vec![NoticeOutcome::Refused("owner-has-no-email")]),
        },
        Recipient::Actor => {
            let Some(account) = account else {
                return Ok(vec![NoticeOutcome::Refused("no-account")]);
            };
            match verified_email(state, &deployment.id, account)? {
                Some(email) => Ok(vec![send(&email, &all, changed.count)?]),
                None => Ok(vec![NoticeOutcome::Refused("unverified-email")]),
            }
        }
        // Each account gets only the rows that name it, so one member
        // never reads another's rows. Past the cap, nobody else is written to.
        Recipient::Column(column) => {
            let mut outcomes = Vec::new();
            for (index, (id, rows)) in recipients(column, changed.rows).into_iter().enumerate() {
                if index >= MAX_RECIPIENTS_PER_RUN {
                    outcomes.push(NoticeOutcome::Refused("recipient-cap"));
                    continue;
                }
                outcomes.push(match verified_email(state, &deployment.id, id)? {
                    Some(email) => send(&email, &rows, rows.len() as u64)?,
                    None => NoticeOutcome::Refused("not-a-verified-account"),
                });
            }
            Ok(outcomes)
        }
    }
}

fn read_only(path: &std::path::Path) -> anyhow::Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(connection))
}

/// The email of the owner's active account in `owners.sqlite`. An owner
/// who signs in through Clerk keeps no email in the Kernel.
fn owner_email(state: &AppState, owner: &str) -> anyhow::Result<Option<String>> {
    let Some(db) = read_only(&state.data_root.join("owners.sqlite"))? else { return Ok(None) };
    Ok(db
        .query_row("SELECT email FROM app_users WHERE id = ?1 AND state = 'active'", [owner], |row| row.get(0))
        .optional()?)
}

/// The email of an active app account that proved control of it by
/// answering a verification, invitation or recovery link.
fn verified_email(state: &AppState, deployment_id: &str, account: &str) -> anyhow::Result<Option<String>> {
    let path = state.data_root.join("deployments").join(deployment_id).join("identity.sqlite");
    let Some(db) = read_only(&path)? else { return Ok(None) };
    Ok(db
        .query_row(
            "SELECT email FROM app_users WHERE id = ?1 AND state = 'active' AND verified_at IS NOT NULL",
            [account],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program(actions: &Value) -> super::super::ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    /// The editor runs the same file in src/kernel/actions.test.ts and through
    /// the Validation Gate in src/agent/gate.test.ts.
    #[test]
    fn notify_refuses_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("notify-cases.json")).unwrap();
        assert!(cases.len() >= 15);
        for case in cases {
            let program = program(&case["actions"]);
            let outcome = super::super::action_shape::check_program(&program);
            crate::refusal::assert_case(&case, outcome);
        }
    }

    fn plain<'a>(input: &'a Value, actor: Option<&'a str>) -> Fill<'a> {
        Fill { input, actor, now: 0, columns: &[], rows: &[], count: 1 }
    }

    /// kernel-rs/src/notify-column-cases.json: a column the statement cannot
    /// return is refused against the schema. src/agent/gate.test.ts runs the
    /// same file through the Validation Gate.
    #[test]
    fn notify_columns_are_checked_against_the_schema_as_the_editor_checks_them() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("notify-column-cases.json")).unwrap();
        assert!(cases.len() >= 8);
        for case in cases {
            let program = program(&case["actions"]);
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(case["schema"].as_str().unwrap()).unwrap();
            let outcome = program.actions.iter().try_for_each(|(name, action)| {
                connection.prepare(&action.statement).expect("the statement prepares on its own");
                check_columns(&connection, name, action)
            });
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn the_kernel_appends_its_returning_clause_after_cutting_a_trailing_semicolon() {
        let columns = vec!["title".to_owned(), "assignee_id".to_owned()];
        assert_eq!(with_returning("UPDATE t SET a = 1 ; -- done", &columns), "UPDATE t SET a = 1\nRETURNING [title], [assignee_id]");
        assert_eq!(with_returning("UPDATE t SET a = ';' -- x", &columns), "UPDATE t SET a = ';' -- x\nRETURNING [title], [assignee_id]");
    }

    #[test]
    fn kernel_now_reads_as_a_utc_minute() {
        assert_eq!(format_now(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_now(1_793_948_400_000 + 59_999), "2026-11-06 07:00 UTC");
    }

    fn reminders() -> super::super::ActionProgram {
        program(&json!({
            "tasks.remind": {"kind": "mutation", "access": "authenticated", "schedule": {"daily": "07:00"},
                "statement": "UPDATE tasks SET reminded_at = ?1 WHERE reminded_at IS NULL",
                "bindings": [{"source": "kernel", "field": "now"}],
                "notify": {"to": {"column": "assignee_id"}, "changes": ["title", "due_day"],
                    "subject": "{changes.count} due", "body": "At {kernel.now}:\n{changes}"}}
        }))
    }

    fn row(title: &str, assignee: Value) -> Map<String, Value> {
        let value = json!({"title": title, "due_day": "2026-11-08", "assignee_id": assignee});
        value.as_object().unwrap().clone()
    }

    #[test]
    fn each_named_account_reads_only_the_rows_that_name_it() {
        let program = reminders();
        let declared = declaration("tasks.remind", &program.actions["tasks.remind"]).unwrap().unwrap();
        assert_eq!(declared.returning(), vec!["title", "due_day", "assignee_id"]);
        let rows = vec![row("Water\nplants", json!("ana")), row("File taxes", json!("ben")), row("Call mum", json!("ana")), row("Nobody", Value::Null), row("Number", json!(7))];
        let named = recipients("assignee_id", &rows);
        assert_eq!(named.keys().copied().collect::<Vec<_>>(), vec!["ana", "ben"]);
        let input = json!({});
        let ana = &named["ana"];
        let values = Fill { input: &input, actor: None, now: 1_793_948_400_000, columns: &declared.changes, rows: ana, count: ana.len() as u64 };
        let (subject, text, _) = render(&declared, "Chores", &values);
        assert_eq!(subject, "2 due");
        assert!(text.starts_with("At 2026-11-06 07:00 UTC:\n- Water plants · 2026-11-08\n- Call mum · 2026-11-08\n\n-- \n"), "{text}");
        assert!(!text.contains("File taxes"));
        assert!(text.contains("because a change in the app names your account."));
    }

    #[test]
    fn changes_lists_fifty_rows_and_counts_the_rest() {
        let program = reminders();
        let declared = declaration("tasks.remind", &program.actions["tasks.remind"]).unwrap().unwrap();
        let rows: Vec<Map<String, Value>> = (0..60).map(|index| row(&format!("t{index}"), json!("ana"))).collect();
        let listed: Vec<&Map<String, Value>> = rows.iter().collect();
        let input = json!({});
        let values = Fill { input: &input, actor: None, now: 0, columns: &declared.changes, rows: &listed, count: 1200 };
        let (_, text, _) = render(&declared, "Chores", &values);
        assert!(text.contains("- t49 · 2026-11-08\n- and 1150 more\n"), "{text}");
        assert!(!text.contains("t50 "));
    }

    fn booking() -> super::super::ActionProgram {
        program(&json!({
            "bookings.create": {"kind": "mutation", "access": "authenticated",
                "statement": "INSERT INTO bookings (user_id, room, title) VALUES (?1, ?2, ?3)",
                "bindings": [{"source": "actor", "field": "id"}, {"source": "input", "key": "room", "type": "string"},
                    {"source": "input", "key": "title", "type": "string", "nullable": true},
                    {"source": "input", "key": "hours", "type": "number", "nullable": true}],
                "notify": {"to": "actor", "subject": "Booked {input.room}{input.title}",
                    "body": "Room {input.room} for {input.hours} h.\nBooked by {actor.id}. Braces } stay."}}
        }))
    }

    #[test]
    fn a_notice_escapes_values_strips_their_line_breaks_and_caps_them() {
        let program = booking();
        let action = &program.actions["bookings.create"];
        let declared = declaration("bookings.create", action).unwrap().unwrap();
        assert_eq!(declared.to, Recipient::Actor);
        let input = json!({"room": "Oak\r\nBcc: x@example.test <b>", "title": "x".repeat(500), "hours": 2});
        let (subject, text, html) = render(&declared, "Room\nboard", &plain(&input, Some("user-1")));
        assert!(!subject.contains(['\r', '\n']));
        assert!(subject.starts_with("Booked Oak Bcc: x@example.test <b>"));
        assert_eq!(subject.chars().count(), MAX_RENDERED_SUBJECT_CHARS);
        assert!(text.starts_with("Room Oak Bcc: x@example.test <b> for 2 h.\nBooked by user-1. Braces } stay.\n\n-- \n"));
        assert!(text.contains("Room board sent this notice through Konae after something you did in the app."));
        assert!(html.contains("Room Oak Bcc: x@example.test &lt;b&gt; for 2 h.<br>Booked by user-1.") && !html.contains("<b>"));
        // A null input and a caller without an ID fill nothing.
        let (subject, text, _) = render(&declared, "Rooms", &plain(&json!({"room": "Oak", "title": null}), None));
        assert_eq!(subject, "Booked Oak");
        assert!(text.starts_with("Room Oak for  h.\nBooked by . Braces } stay."));
    }

    #[test]
    fn a_subject_that_renders_empty_names_the_app() {
        let program = program(&json!({
            "notes.add": {"kind": "mutation", "access": "authenticated", "statement": "INSERT INTO notes (body) VALUES (?1)",
                "bindings": [{"source": "input", "key": "body", "type": "string", "nullable": true}],
                "notify": {"to": "owner", "subject": "{input.body}", "body": "{input.body}"}}
        }));
        let declared = declaration("notes.add", &program.actions["notes.add"]).unwrap().unwrap();
        let (subject, text, _) = render(&declared, "Notes", &plain(&json!({}), None));
        assert_eq!(subject, "Notice from Notes");
        assert!(text.ends_with("Notes sent this notice through Konae because you own the app.\n"));
    }
}
