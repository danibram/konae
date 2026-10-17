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
use serde_json::Value;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recipient {
    /// The Project Owner's account email.
    Owner,
    /// The calling account's verified email.
    Actor,
}

#[derive(Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Input(String),
    ActorId,
}

#[derive(Debug)]
pub(crate) struct Declaration {
    pub(crate) to: Recipient,
    subject: Vec<Part>,
    body: Vec<Part>,
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
        .filter(|record| record.keys().all(|key| matches!(key.as_str(), "to" | "subject" | "body")))
        .filter(|record| record.get("subject").is_some_and(Value::is_string) && record.get("body").is_some_and(Value::is_string))
        .ok_or_else(|| refuse("notify.invalid", format!(r#"Action {name} has an invalid notify declaration. Give "to", "subject" and "body" and nothing else."#)))?;
    let to = match record.get("to").and_then(Value::as_str) {
        Some("owner") => Recipient::Owner,
        Some("actor") => Recipient::Actor,
        _ => return Err(refuse("notify.recipient-invalid", format!(r#"Action {name} has an invalid notify recipient. Use "owner" or "actor"."#))),
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
    Ok(Some(Declaration { to, subject, body }))
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
            _ => {
                return Err(refuse("notify.placeholder-unknown", format!(
                    "Action {name} names an unknown placeholder {{{token}}} in its notify {field}. Use {{input.<key>}} for one of its input bindings, or {{actor.id}}."
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

/// Fills a template. A value loses its line breaks and keeps at most 200
/// characters; a missing or null input and a caller without an ID fill
/// nothing.
fn fill(parts: &[Part], input: &Value, actor: Option<&str>) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            Part::Text(value) => text.push_str(value),
            Part::Input(key) => match input.get(key) {
                Some(Value::String(value)) => text.push_str(&mail::one_line(value, MAX_VALUE_CHARS)),
                Some(Value::Null) | None => {}
                Some(value) => text.push_str(&mail::one_line(&value.to_string(), MAX_VALUE_CHARS)),
            },
            Part::ActorId => text.push_str(actor.unwrap_or_default()),
        }
    }
    text
}

/// Subject, plain text and HTML of one notice. The template's own line
/// breaks stay in the body; the HTML part escapes everything and turns each
/// line break into `<br>`. A fixed last line says which app sent it and why
/// the recipient gets it.
pub(crate) fn render(declared: &Declaration, app_name: &str, input: &Value, actor: Option<&str>) -> (String, String, String) {
    let app = mail::one_line(app_name, 80);
    let app = if app.is_empty() { "Littlebox".to_owned() } else { app };
    let subject = mail::one_line(&fill(&declared.subject, input, actor), MAX_RENDERED_SUBJECT_CHARS);
    let subject = if subject.is_empty() { format!("Notice from {app}") } else { subject };
    let body: String = fill(&declared.body, input, actor).trim().chars().take(MAX_RENDERED_BODY_CHARS).collect();
    let why = match declared.to {
        Recipient::Owner => format!("{app} sent this notice through Littlebox because you own the app."),
        Recipient::Actor => format!("{app} sent this notice through Littlebox after something you did in the app."),
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

/// Records the notice of a mutation that changed a row, and counts the
/// outcome in Usage under `notify`: `<action>: queued`, or `<action>:
/// <reason>` when nothing will be sent. Call it only after the statement
/// changed a row. It never fails the mutation.
pub(crate) async fn record(state: &AppState, deployment: &Deployment, name: &str, action: &ActionDefinition, input: &Value, caller: Caller<'_>) {
    let Ok(Some(declared)) = declaration(name, action) else { return };
    let state = state.clone();
    let deployment = deployment.clone();
    let name = name.to_owned();
    let input = input.clone();
    let account = caller.account.map(str::to_owned);
    let actor_id = caller.actor_id.map(str::to_owned);
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = queue_notice(&state, &deployment, &name, &declared, &input, account.as_deref(), actor_id.as_deref())
            .unwrap_or_else(|error_value| {
                eprintln!("Notice of {name} on {} could not be recorded: {error_value}", deployment.id);
                NoticeOutcome::Refused("storage-error")
            });
        let reason = match outcome {
            NoticeOutcome::Queued => "queued",
            NoticeOutcome::Refused(kind) => kind,
        };
        super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Notify(format!("{name}: {reason}")));
    })
    .await;
    if let Err(error_value) = outcome {
        eprintln!("Notice task failed: {error_value}");
    }
}

fn queue_notice(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
    declared: &Declaration,
    input: &Value,
    account: Option<&str>,
    actor_id: Option<&str>,
) -> anyhow::Result<NoticeOutcome> {
    let Some(queue) = state.accounts.queue.as_ref() else {
        return Ok(NoticeOutcome::Refused("no-transport"));
    };
    let creator = super::accounts::project_creator(state, &deployment.project_id).unwrap_or_else(|| "operator".into());
    let recipient = match declared.to {
        Recipient::Owner => match owner_email(state, &creator)? {
            Some(email) => email,
            None => return Ok(NoticeOutcome::Refused("owner-has-no-email")),
        },
        Recipient::Actor => {
            let Some(account) = account else {
                return Ok(NoticeOutcome::Refused("no-account"));
            };
            match verified_email(state, &deployment.id, account)? {
                Some(email) => email,
                None => return Ok(NoticeOutcome::Refused("unverified-email")),
            }
        }
    };
    let (subject, text, html) = render(declared, &deployment.project_name, input, actor_id);
    let scope = super::auth::deployment_scope(&deployment.id);
    queue.record_notice(&Notice {
        recipient: &recipient,
        scope: &scope,
        creator: &creator,
        app_name: &deployment.project_name,
        action: name,
        subject: &subject,
        text: &text,
        html: &html,
    })
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
        let (subject, text, html) = render(&declared, "Room\nboard", &input, Some("user-1"));
        assert!(!subject.contains(['\r', '\n']));
        assert!(subject.starts_with("Booked Oak Bcc: x@example.test <b>"));
        assert_eq!(subject.chars().count(), MAX_RENDERED_SUBJECT_CHARS);
        assert!(text.starts_with("Room Oak Bcc: x@example.test <b> for 2 h.\nBooked by user-1. Braces } stay.\n\n-- \n"));
        assert!(text.contains("Room board sent this notice through Littlebox after something you did in the app."));
        assert!(html.contains("Room Oak Bcc: x@example.test &lt;b&gt; for 2 h.<br>Booked by user-1.") && !html.contains("<b>"));
        // A null input and a caller without an ID fill nothing.
        let (subject, text, _) = render(&declared, "Rooms", &json!({"room": "Oak", "title": null}), None);
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
        let (subject, text, _) = render(&declared, "Notes", &json!({}), None);
        assert_eq!(subject, "Notice from Notes");
        assert!(text.ends_with("Notes sent this notice through Littlebox because you own the app.\n"));
    }
}
