//! Bounded, portable schema transitions for an app's SQLite database.
//!
//! Planning reads schema and transformation metadata only. Callers apply a
//! plan to a throwaway database to collect affected-row counts, then apply it
//! again inside the caller's live transaction after confirmation. This module
//! never commits that transaction.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use rusqlite::{
    Connection, OptionalExtension, Transaction,
    hooks::{AuthAction, AuthContext, Authorization},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    denied_function,
    refusal::{Refusal, refuse},
    schema_change,
};

const FORMAT_VERSION: u32 = 1;
const MAX_TRANSITIONS: usize = 256;
const MAX_STATEMENTS: usize = 100;
const MAX_STATEMENT_BYTES: usize = 20_000;
const MAX_ROWS: u64 = 100_000;
const MAX_SQLITE_BYTES: i32 = 2 * 1024 * 1024;
const STATE_TABLE: &str = "__konae_schema_state";
const HISTORY_TABLE: &str = "__konae_schema_history";
pub const INTERNAL_TABLE_NAMES: [&str; 2] = [STATE_TABLE, HISTORY_TABLE];

const DOCUMENT_INVALID: &str = "schema-transform.document-invalid";
const STATEMENT_REFUSED: &str = "schema-transform.statement-refused";
const HISTORY_MISMATCH: &str = "schema-transform.history-mismatch";
const BEFORE_MISMATCH: &str = "schema-transform.before-mismatch";
const AFTER_MISMATCH: &str = "schema-transform.after-mismatch";
const ROW_LIMIT: &str = "schema-transform.row-limit";
const FINAL_SCHEMA_MISMATCH: &str = "schema-transform.final-schema-mismatch";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub version: u32,
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub predecessor: Option<String>,
    pub before: String,
    pub after: String,
    #[serde(rename = "maxRows")]
    pub max_rows: u64,
    pub statements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    pub predecessor: Option<String>,
    pub definition_sha256: String,
    pub before: String,
    pub after: String,
    pub affected_rows: u64,
    pub baseline_skipped: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryState {
    pub head_id: Option<String>,
    pub fingerprint: Option<String>,
    pub baseline_kind: Option<String>,
    pub entries: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTransition {
    pub transition: Transition,
    pub definition_sha256: String,
    pub writes_existing_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Digests for every definition in file order, including applied entries.
    pub ordered_definition_digests: Vec<String>,
    /// Entries that still need to run, in predecessor order.
    pub transitions: Vec<PlannedTransition>,
    pub current_fingerprint: String,
    pub final_fingerprint: String,
    pub requires_confirmation: bool,
    pub definition: Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTransition {
    pub id: String,
    pub definition_sha256: String,
    pub affected_rows: u64,
    pub baseline_skipped: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub transitions: Vec<AppliedTransition>,
}

fn invalid(message: impl Into<String>) -> Refusal {
    refuse(DOCUMENT_INVALID, message)
}
fn refused(message: impl Into<String>) -> Refusal {
    refuse(STATEMENT_REFUSED, message)
}
fn history_error(message: impl Into<String>) -> Refusal {
    refuse(HISTORY_MISMATCH, message)
}
fn before_error(message: impl Into<String>) -> Refusal {
    refuse(BEFORE_MISMATCH, message)
}
fn after_error(message: impl Into<String>) -> Refusal {
    refuse(AFTER_MISMATCH, message)
}

/// Parses and statically checks the bounded v1 file.
pub fn parse(source: &str) -> Result<Definition, Refusal> {
    let definition: Definition = serde_json::from_str(source)
        .map_err(|_| invalid("schema-transforms.json must be valid JSON."))?;
    validate_definition(&definition)?;
    Ok(definition)
}

fn validate_definition(definition: &Definition) -> Result<(), Refusal> {
    if definition.version != FORMAT_VERSION {
        return Err(invalid(format!(
            "schema-transforms.json version must be {FORMAT_VERSION}."
        )));
    }
    if definition.transitions.len() > MAX_TRANSITIONS {
        return Err(invalid(format!(
            "schema-transforms.json may contain at most {MAX_TRANSITIONS} transitions."
        )));
    }
    let mut ids = HashSet::new();
    let mut previous: Option<&Transition> = None;
    for transition in &definition.transitions {
        if transition.id.is_empty()
            || transition.id.len() > 100
            || !transition.id.bytes().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'.')
            })
        {
            return Err(invalid(
                "A transition ID must use lowercase letters, digits, dots, underscores or hyphens, up to 100 bytes.",
            ));
        }
        if !ids.insert(transition.id.as_str()) {
            return Err(invalid(format!(
                "Transition {} appears more than once.",
                transition.id
            )));
        }
        let expected_predecessor = previous.map(|item| item.id.as_str());
        if transition.predecessor.as_deref() != expected_predecessor {
            return Err(invalid(format!(
                "Transition {} must name its immediately preceding transition as predecessor.",
                transition.id
            )));
        }
        if previous.is_some_and(|item| item.after != transition.before) {
            return Err(invalid(format!(
                "Transition {} does not start at the previous transition's after fingerprint.",
                transition.id
            )));
        }
        if !valid_fingerprint(&transition.before) || !valid_fingerprint(&transition.after) {
            return Err(invalid(format!(
                "Transition {} must use lowercase sha256 fingerprints.",
                transition.id
            )));
        }
        if transition.max_rows == 0 || transition.max_rows > MAX_ROWS {
            return Err(invalid(format!(
                "Transition {} maxRows must be from 1 to {MAX_ROWS}.",
                transition.id
            )));
        }
        if transition.statements.is_empty() || transition.statements.len() > MAX_STATEMENTS {
            return Err(invalid(format!(
                "Transition {} must contain from 1 to {MAX_STATEMENTS} statements.",
                transition.id
            )));
        }
        for statement in &transition.statements {
            if statement.len() > MAX_STATEMENT_BYTES {
                return Err(refused(format!(
                    "Transition {} has a statement over {MAX_STATEMENT_BYTES} bytes.",
                    transition.id
                )));
            }
            validate_statement(statement)
                .map_err(|message| refused(format!("Transition {}: {message}", transition.id)))?;
        }
        previous = Some(transition);
    }
    Ok(())
}

fn valid_fingerprint(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

fn validate_statement(statement: &str) -> Result<(), String> {
    let mut words = schema_change::canonical_tokens(statement);
    if words.last().is_some_and(|word| word == ";") {
        words.pop();
    }
    if words.is_empty() || words.contains(&";".to_owned()) {
        return Err("Write one supported SQL statement without a second statement.".into());
    }
    let first = words[0].as_str();
    let table_at = match first {
        "create" => validate_create(&words)?,
        "alter" => validate_alter(&words)?,
        "insert" => validate_insert(&words)?,
        "update" => validate_update(&words)?,
        _ => {
            return Err(
                "Use CREATE TABLE/INDEX, ALTER TABLE RENAME/ADD COLUMN, INSERT or UPDATE.".into(),
            );
        }
    };
    if table_at.is_some_and(|at| words.get(at).is_some_and(|name| is_forbidden_object(name))) {
        return Err(
            "A transition cannot access SQLite schema tables or Konae's reserved metadata tables."
                .into(),
        );
    }
    if words.iter().any(|word| is_forbidden_object(word)) {
        return Err(
            "A transition cannot access SQLite schema tables or Konae's reserved metadata tables."
                .into(),
        );
    }
    if words
        .windows(2)
        .any(|pair| pair[1] == "(" && (denied_function(&pair[0]) || external_function(&pair[0])))
    {
        return Err(
            "Filesystem, network and extension functions are refused in transitions.".into(),
        );
    }
    Ok(())
}

fn validate_create(words: &[String]) -> Result<Option<usize>, String> {
    let (object, name_at) = if words.get(1).is_some_and(|word| word == "table") {
        ("table", 2)
    } else if words.get(1).is_some_and(|word| word == "index") {
        ("index", 2)
    } else if words.get(1).is_some_and(|word| word == "unique")
        && words.get(2).is_some_and(|word| word == "index")
    {
        ("index", 3)
    } else {
        return Err("CREATE is limited to TABLE and INDEX.".into());
    };
    let name_at = optional_if_not_exists(words, name_at);
    let Some(name) = words.get(name_at) else {
        return Err(format!("CREATE {object} needs a name."));
    };
    if is_qualified(name) || name == "temp" || name == "temporary" {
        return Err("Use an unqualified app table or index name.".into());
    }
    if object == "index" && !words.iter().any(|word| word == "on") {
        return Err("CREATE INDEX must name its table with ON.".into());
    }
    Ok(Some(name_at))
}

fn optional_if_not_exists(words: &[String], at: usize) -> usize {
    if words.get(at).is_some_and(|word| word == "if")
        && words.get(at + 1).is_some_and(|word| word == "not")
        && words.get(at + 2).is_some_and(|word| word == "exists")
    {
        at + 3
    } else {
        at
    }
}

fn validate_alter(words: &[String]) -> Result<Option<usize>, String> {
    if words.get(1).is_none_or(|word| word != "table") || words.len() < 6 {
        return Err("ALTER is limited to ALTER TABLE RENAME or ADD COLUMN.".into());
    }
    let table_at = 2;
    if is_qualified(&words[table_at]) {
        return Err("ALTER TABLE must name an unqualified app table.".into());
    }
    match (
        words.get(3).map(String::as_str),
        words.get(4).map(String::as_str),
    ) {
        (Some("rename"), Some("to")) if words.len() == 6 => Ok(Some(table_at)),
        (Some("rename"), Some("column")) if words.len() == 8 && words[6] == "to" => {
            Ok(Some(table_at))
        }
        (Some("add"), Some("column")) if words.len() >= 7 => Ok(Some(table_at)),
        _ => Err("ALTER TABLE supports only RENAME TO, RENAME COLUMN, or ADD COLUMN.".into()),
    }
}

fn validate_insert(words: &[String]) -> Result<Option<usize>, String> {
    if words.iter().any(|word| word == "replace") {
        return Err("INSERT OR REPLACE and REPLACE are refused.".into());
    }
    let into = words
        .iter()
        .position(|word| word == "into")
        .ok_or("INSERT must use INTO.")?;
    let table_at = into + 1;
    let Some(table) = words.get(table_at) else {
        return Err("INSERT INTO needs an app table.".into());
    };
    if is_qualified(table) {
        return Err("INSERT INTO must name an unqualified app table.".into());
    }
    if !words
        .iter()
        .any(|word| matches!(word.as_str(), "values" | "select" | "default"))
    {
        return Err("INSERT must provide VALUES, SELECT, or DEFAULT VALUES.".into());
    }
    Ok(Some(table_at))
}

fn validate_update(words: &[String]) -> Result<Option<usize>, String> {
    let mut table_at = 1;
    if words.get(table_at).is_some_and(|word| word == "or") {
        if !words
            .get(table_at + 1)
            .is_some_and(|word| matches!(word.as_str(), "abort" | "fail" | "ignore" | "rollback"))
        {
            return Err("UPDATE OR supports ABORT, FAIL, IGNORE or ROLLBACK.".into());
        }
        table_at += 2;
    }
    let Some(table) = words.get(table_at) else {
        return Err("UPDATE needs an app table.".into());
    };
    if is_qualified(table) || !words.iter().skip(table_at + 1).any(|word| word == "set") {
        return Err("UPDATE must name an unqualified app table and SET columns.".into());
    }
    Ok(Some(table_at))
}

fn is_qualified(name: &str) -> bool {
    name.contains('.')
}

fn is_forbidden_object(name: &str) -> bool {
    let name = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "sqlite_master"
            | "sqlite_schema"
            | "sqlite_temp_master"
            | "sqlite_temp_schema"
            | STATE_TABLE
            | HISTORY_TABLE
    )
}

/// SHA-256 of sorted SQLite definitions. Names are sorted case-insensitively;
/// each definition uses the existing SQL tokenizer, which drops whitespace
/// and comments, normalizes identifiers and keywords, and preserves literal
/// string case. It does not claim equivalence beyond those token rules.
pub fn fingerprint(connection: &Connection) -> Result<String, Refusal> {
    let mut statement = connection.prepare("SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'")
        .map_err(|error| invalid(format!("Cannot read SQLite schema: {error}")))?;
    let mut objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| invalid(format!("Cannot read SQLite schema: {error}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| invalid(format!("Cannot read SQLite schema: {error}")))?;
    objects.retain(|(_, name, _, _)| !is_internal_name(name));
    objects.sort_by(|a, b| {
        (
            a.0.to_ascii_lowercase(),
            a.1.to_ascii_lowercase(),
            a.2.to_ascii_lowercase(),
        )
            .cmp(&(
                b.0.to_ascii_lowercase(),
                b.1.to_ascii_lowercase(),
                b.2.to_ascii_lowercase(),
            ))
    });
    let mut hash = Sha256::new();
    for (kind, name, table, sql) in objects {
        let fields = [
            kind.to_ascii_lowercase(),
            name.to_ascii_lowercase(),
            table.to_ascii_lowercase(),
        ];
        for field in fields {
            hash_field(&mut hash, field.as_bytes());
        }
        let tokens = schema_change::canonical_tokens(&sql);
        hash_field(&mut hash, &(tokens.len() as u64).to_be_bytes());
        for token in tokens {
            hash_field(&mut hash, token.as_bytes());
        }
    }
    Ok(format!("sha256:{}", hex::encode(hash.finalize())))
}

/// Fingerprints the schema produced by the same safe loader used for
/// `schema.sql`, without touching an app database.
pub fn fingerprint_schema(schema: &str) -> Result<String, Refusal> {
    let connection = schema_change::Scratch::connection(schema)
        .map_err(|error| invalid(format!("schema.sql is invalid: {error}")))?;
    fingerprint(&connection)
}

fn hash_field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

pub fn is_internal_table(name: &str) -> bool {
    schema_change::is_internal_table(name)
}

fn is_internal_name(name: &str) -> bool {
    is_internal_table(name)
}

/// Reads the app database's portable transition journal. No tables means an
/// untracked legacy app; exactly one of the pair is a damaged journal.
pub fn load_history(connection: &Connection) -> Result<HistoryState, Refusal> {
    let state_exists = object_exists(connection, STATE_TABLE)?;
    let history_exists = object_exists(connection, HISTORY_TABLE)?;
    if !state_exists && !history_exists {
        return Ok(HistoryState::default());
    }
    if !state_exists || !history_exists {
        return Err(history_error(
            "The schema transformation journal is incomplete.",
        ));
    }
    let state = connection
        .query_row(
            &format!(
                "SELECT head_id, fingerprint, baseline_kind FROM {STATE_TABLE} WHERE singleton=1"
            ),
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|error| {
            history_error(format!(
                "The schema transformation state is unreadable: {error}"
            ))
        })?;
    let mut statement = connection.prepare(&format!("SELECT id, predecessor, definition_sha256, before_fingerprint, after_fingerprint, affected_rows, baseline_skipped FROM {HISTORY_TABLE} ORDER BY ordinal"))
        .map_err(|error| history_error(format!("The schema transformation history is unreadable: {error}")))?;
    let entries = statement
        .query_map([], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                predecessor: row.get(1)?,
                definition_sha256: row.get(2)?,
                before: row.get(3)?,
                after: row.get(4)?,
                affected_rows: row.get(5)?,
                baseline_skipped: row.get::<_, i64>(6)? != 0,
            })
        })
        .map_err(|error| {
            history_error(format!(
                "The schema transformation history is unreadable: {error}"
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| {
            history_error(format!(
                "The schema transformation history is unreadable: {error}"
            ))
        })?;
    if entries.last().map(|entry| entry.id.as_str()) != state.0.as_deref() {
        return Err(history_error(
            "The schema transformation state does not match its history.",
        ));
    }
    Ok(HistoryState {
        head_id: state.0,
        fingerprint: Some(state.1),
        baseline_kind: Some(state.2),
        entries,
    })
}

fn object_exists(connection: &Connection, name: &str) -> Result<bool, Refusal> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 COLLATE NOCASE)",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| {
            history_error(format!(
                "Cannot inspect the schema transformation journal: {error}"
            ))
        })
}

/// Refuses an app-folder load that omits a document after a transition journal
/// has been created. Folder/import wiring calls this when the file is absent.
pub fn check_document_presence(
    connection: &Connection,
    document_present: bool,
) -> Result<(), Refusal> {
    let history = load_history(connection)?;
    if !document_present && !history.entries.is_empty() {
        return Err(history_error(
            "This app has applied schema transformations. Restore app/schema-transforms.json before opening it.",
        ));
    }
    Ok(())
}

/// Builds a static, read-only plan against the current DB fingerprint and
/// journal. It does not execute SQL or change either metadata table.
pub fn plan(connection: &Connection, definition: &Definition) -> Result<Plan, Refusal> {
    validate_definition(definition)?;
    let history = load_history(connection)?;
    let current_fingerprint = fingerprint(connection)?;
    validate_history_prefix(definition, &history)?;
    if let Some(stored) = history.fingerprint.as_deref() {
        if stored != current_fingerprint {
            return Err(history_error(
                "The current SQLite schema does not match its transformation journal.",
            ));
        }
    }
    let applied_count = history.entries.len();
    let mut transitions = Vec::new();
    let ordered_definition_digests = definition
        .transitions
        .iter()
        .map(definition_digest)
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected = history
        .fingerprint
        .as_deref()
        .unwrap_or(&current_fingerprint);
    for (index, transition) in definition
        .transitions
        .iter()
        .enumerate()
        .skip(applied_count)
    {
        if transition.before != expected {
            return Err(before_error(format!(
                "Transition {} expects {}, but the database is at {}.",
                transition.id, transition.before, expected
            )));
        }
        let digest = ordered_definition_digests[index].clone();
        let writes_existing_data = transition.statements.iter().any(|sql| {
            let tokens = schema_change::canonical_tokens(sql);
            matches!(
                tokens.first().map(String::as_str),
                Some("insert" | "update")
            )
        });
        transitions.push(PlannedTransition {
            transition: transition.clone(),
            definition_sha256: digest,
            writes_existing_data,
        });
        expected = &transition.after;
    }
    let final_fingerprint = definition
        .transitions
        .last()
        .map(|item| item.after.clone())
        .unwrap_or_else(|| current_fingerprint.clone());
    Ok(Plan {
        ordered_definition_digests,
        requires_confirmation: transitions.iter().any(|item| item.writes_existing_data),
        transitions,
        current_fingerprint,
        final_fingerprint,
        definition: definition.clone(),
    })
}

fn validate_history_prefix(definition: &Definition, history: &HistoryState) -> Result<(), Refusal> {
    if history.entries.len() > definition.transitions.len() {
        return Err(history_error(
            "schema-transforms.json removed an applied transition.",
        ));
    }
    for (index, entry) in history.entries.iter().enumerate() {
        let Some(transition) = definition.transitions.get(index) else {
            return Err(history_error(
                "schema-transforms.json removed an applied transition.",
            ));
        };
        let digest = definition_digest(transition)?;
        if entry.id != transition.id
            || entry.predecessor != transition.predecessor
            || entry.definition_sha256 != digest
            || entry.before != transition.before
            || entry.after != transition.after
        {
            return Err(history_error(format!(
                "Applied transition {} was changed or moved in schema-transforms.json.",
                entry.id
            )));
        }
    }
    if let Some(head) = history.head_id.as_deref() {
        if definition
            .transitions
            .get(history.entries.len().saturating_sub(1))
            .map(|item| item.id.as_str())
            != Some(head)
        {
            return Err(history_error(
                "The transformation history head is missing from schema-transforms.json.",
            ));
        }
    }
    Ok(())
}

fn definition_digest(transition: &Transition) -> Result<String, Refusal> {
    let bytes = serde_json::to_vec(transition).map_err(|error| {
        invalid(format!(
            "Cannot encode transition {}: {error}",
            transition.id
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Initializes a genuinely fresh database from final schema and records every
/// supplied transition as baseline-skipped. Fresh means no user schema; rows
/// in unrelated stores do not affect this decision.
pub fn initialize_fresh_baseline(
    transaction: &Transaction<'_>,
    definition: &Definition,
    final_schema: &str,
) -> Result<Applied, Refusal> {
    validate_definition(definition)?;
    if !load_history(transaction)?.entries.is_empty()
        || object_exists(transaction, STATE_TABLE)?
        || object_exists(transaction, HISTORY_TABLE)?
    {
        return Err(history_error(
            "A fresh schema baseline cannot replace an existing transformation journal.",
        ));
    }
    if !user_schema_is_empty(transaction)? {
        return Err(history_error(
            "A fresh schema baseline requires a database with no app tables or indexes.",
        ));
    }
    let final_fingerprint = fingerprint_schema(final_schema)?;
    if definition
        .transitions
        .last()
        .is_some_and(|item| item.after != final_fingerprint)
    {
        return Err(refuse(
            FINAL_SCHEMA_MISMATCH,
            "The final transition fingerprint does not match schema.sql.",
        ));
    }
    apply_initial_schema(transaction, final_schema)?;
    let actual = fingerprint(transaction)?;
    if actual != final_fingerprint {
        return Err(refuse(
            FINAL_SCHEMA_MISMATCH,
            "The initialized database does not match schema.sql.",
        ));
    }
    let applied = Applied {
        transitions: definition
            .transitions
            .iter()
            .map(|transition| {
                Ok(AppliedTransition {
                    id: transition.id.clone(),
                    definition_sha256: definition_digest(transition)?,
                    affected_rows: 0,
                    baseline_skipped: true,
                })
            })
            .collect::<Result<Vec<_>, Refusal>>()?,
    };
    write_history(transaction, &applied, definition, &actual, "fresh")?;
    Ok(applied)
}

fn user_schema_is_empty(connection: &Connection) -> Result<bool, Refusal> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND lower(name) NOT IN (lower(?1), lower(?2))",
        [STATE_TABLE, HISTORY_TABLE], |row| row.get(0),
    ).map_err(|error| history_error(format!("Cannot inspect the app schema: {error}")))?;
    Ok(count == 0)
}

fn apply_initial_schema(transaction: &Transaction<'_>, schema: &str) -> Result<(), Refusal> {
    schema_change::Scratch::connection(schema)
        .map_err(|error| invalid(format!("schema.sql is invalid: {error}")))?;
    transaction.authorizer(Some(transform_authorizer));
    let result = transaction.execute_batch(schema);
    transaction.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
    result.map_err(|error| {
        invalid(format!(
            "schema.sql could not initialize the fresh database: {error}"
        ))
    })
}

/// Applies a previously planned transition chain inside the caller's outer
/// transaction. On error, the caller must roll back that transaction. This
/// function clears its authorizer and deadline but never commits.
pub fn apply_in_transaction(
    transaction: &Transaction<'_>,
    plan: &Plan,
    final_schema: &str,
) -> Result<Applied, Refusal> {
    validate_definition(&plan.definition)?;
    let plan_digests = plan
        .definition
        .transitions
        .iter()
        .map(definition_digest)
        .collect::<Result<Vec<_>, _>>()?;
    if plan_digests != plan.ordered_definition_digests {
        return Err(history_error(
            "The transformation plan's ordered definition digests changed.",
        ));
    }
    let expected_final = fingerprint_schema(final_schema)?;
    if expected_final != plan.final_fingerprint {
        return Err(refuse(
            FINAL_SCHEMA_MISMATCH,
            "The planned final fingerprint does not match schema.sql.",
        ));
    }
    let start = fingerprint(transaction)?;
    if start != plan.current_fingerprint {
        return Err(before_error(
            "The database changed after the transformation plan was made.",
        ));
    }
    let history_before = load_history(transaction)?;
    validate_history_prefix(&plan.definition, &history_before)?;
    if history_before
        .fingerprint
        .as_deref()
        .is_some_and(|stored| stored != start)
    {
        return Err(history_error(
            "The current SQLite schema does not match its transformation journal.",
        ));
    }
    let expected_pending = plan
        .definition
        .transitions
        .iter()
        .skip(history_before.entries.len())
        .collect::<Vec<_>>();
    if expected_pending.len() != plan.transitions.len()
        || expected_pending
            .iter()
            .zip(&plan.transitions)
            .any(|(expected, actual)| {
                **expected != actual.transition
                    || definition_digest(expected).ok().as_deref()
                        != Some(actual.definition_sha256.as_str())
            })
    {
        return Err(history_error(
            "The transformation plan no longer matches the app's applied history.",
        ));
    }
    transaction
        .set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
            MAX_SQLITE_BYTES,
        )
        .map_err(|error| {
            refused(format!(
                "Cannot set the SQLite statement size limit: {error}"
            ))
        })?;
    let deadline = Instant::now() + Duration::from_secs(30);
    transaction.progress_handler(10_000, Some(move || Instant::now() > deadline));
    transaction.authorizer(Some(transform_authorizer));
    let mut applied = Vec::new();
    let result = (|| {
        for planned in &plan.transitions {
            let before = fingerprint(transaction)?;
            if before != planned.transition.before {
                return Err(before_error(format!(
                    "Transition {} expects {}, but the database is at {}.",
                    planned.transition.id, planned.transition.before, before
                )));
            }
            let mut affected_rows = 0u64;
            for sql in &planned.transition.statements {
                let before_changes = transaction.total_changes();
                transaction.execute_batch(sql).map_err(|error| {
                    refused(format!(
                        "Transition {} failed: {error}",
                        planned.transition.id
                    ))
                })?;
                // total_changes counts INSERT/UPDATE row writes, including
                // trigger effects. SQLite DDL statements count as zero.
                let changed = transaction.total_changes().saturating_sub(before_changes) as u64;
                affected_rows = affected_rows.saturating_add(changed);
                if affected_rows > planned.transition.max_rows {
                    return Err(refuse(
                        ROW_LIMIT,
                        format!(
                            "Transition {} changed more than its {} row limit.",
                            planned.transition.id, planned.transition.max_rows
                        ),
                    ));
                }
            }
            let after = fingerprint(transaction)?;
            if after != planned.transition.after {
                return Err(after_error(format!(
                    "Transition {} produced {}, but declares {}.",
                    planned.transition.id, after, planned.transition.after
                )));
            }
            applied.push(AppliedTransition {
                id: planned.transition.id.clone(),
                definition_sha256: planned.definition_sha256.clone(),
                affected_rows,
                baseline_skipped: false,
            });
        }
        let final_actual = fingerprint(transaction)?;
        if final_actual != expected_final {
            return Err(refuse(
                FINAL_SCHEMA_MISMATCH,
                "The transformed database does not match schema.sql.",
            ));
        }
        transaction.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
        let history = load_history(transaction)?;
        if !applied.is_empty() || !history.entries.is_empty() {
            let kind = history.baseline_kind.as_deref().unwrap_or("legacy");
            write_history(
                transaction,
                &Applied {
                    transitions: applied.clone(),
                },
                &plan.definition,
                &final_actual,
                kind,
            )?;
        }
        Ok(Applied {
            transitions: applied,
        })
    })();
    transaction.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
    transaction.progress_handler(0, None::<fn() -> bool>);
    result
}

fn write_history(
    transaction: &Transaction<'_>,
    applied: &Applied,
    definition: &Definition,
    fingerprint: &str,
    baseline_kind: &str,
) -> Result<(), Refusal> {
    transaction.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {STATE_TABLE}(singleton INTEGER PRIMARY KEY CHECK(singleton=1), head_id TEXT, fingerprint TEXT NOT NULL, baseline_kind TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS {HISTORY_TABLE}(ordinal INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, predecessor TEXT, definition_sha256 TEXT NOT NULL, before_fingerprint TEXT NOT NULL, after_fingerprint TEXT NOT NULL, affected_rows INTEGER NOT NULL, baseline_skipped INTEGER NOT NULL CHECK(baseline_skipped IN (0,1)))"
    )).map_err(|error| history_error(format!("Cannot create the schema transformation journal: {error}")))?;
    let mut ordinal: i64 = transaction
        .query_row(
            &format!("SELECT coalesce(max(ordinal),0) FROM {HISTORY_TABLE}"),
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            history_error(format!(
                "Cannot read the schema transformation journal: {error}"
            ))
        })?;
    for item in &applied.transitions {
        let transition = definition
            .transitions
            .iter()
            .find(|entry| entry.id == item.id)
            .ok_or_else(|| {
                history_error(format!(
                    "Transition {} is missing from its definition.",
                    item.id
                ))
            })?;
        ordinal += 1;
        transaction.execute(&format!("INSERT INTO {HISTORY_TABLE}(ordinal,id,predecessor,definition_sha256,before_fingerprint,after_fingerprint,affected_rows,baseline_skipped) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
            rusqlite::params![ordinal, item.id, transition.predecessor, item.definition_sha256, transition.before, transition.after, item.affected_rows, item.baseline_skipped as i64])
            .map_err(|error| history_error(format!("Cannot append transition {} to its journal: {error}", item.id)))?;
    }
    let previous_head: Option<String> = transaction
        .query_row(
            &format!("SELECT head_id FROM {STATE_TABLE} WHERE singleton=1"),
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            history_error(format!(
                "Cannot read the schema transformation state: {error}"
            ))
        })?;
    let head = applied
        .transitions
        .last()
        .map(|item| item.id.clone())
        .or(previous_head);
    transaction.execute(&format!("INSERT INTO {STATE_TABLE}(singleton,head_id,fingerprint,baseline_kind) VALUES (1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET head_id=excluded.head_id,fingerprint=excluded.fingerprint,baseline_kind=excluded.baseline_kind"),
        rusqlite::params![head, fingerprint, baseline_kind])
        .map_err(|error| history_error(format!("Cannot update the schema transformation state: {error}")))?;
    Ok(())
}

fn transform_authorizer(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Pragma { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::Delete { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. } => Authorization::Deny,
        AuthAction::Function { function_name }
            if denied_function(function_name) || external_function(function_name) =>
        {
            Authorization::Deny
        }
        AuthAction::Read { table_name, .. } if is_internal_name(table_name) => Authorization::Deny,
        AuthAction::Insert { table_name } if is_internal_name(table_name) => Authorization::Deny,
        AuthAction::Update { table_name, .. } if is_internal_name(table_name) => {
            Authorization::Deny
        }
        AuthAction::CreateTable { table_name, .. } | AuthAction::CreateIndex { table_name, .. }
            if is_internal_name(table_name) =>
        {
            Authorization::Deny
        }
        AuthAction::AlterTable { table_name, .. } if is_internal_name(table_name) => {
            Authorization::Deny
        }
        // SQLite reports sqlite_master inserts/updates while carrying out
        // approved CREATE/ALTER statements. Static parsing rejects direct
        // author SQL writes to those tables before the authorizer runs.
        AuthAction::CreateTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Read { .. }
        | AuthAction::Select
        | AuthAction::Function { .. }
        | AuthAction::Recursive
        | AuthAction::Reindex { .. } => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

fn external_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "http_get" | "http_post" | "http_request" | "curl" | "fetch"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fp(schema: &str) -> String {
        let sql = schema
            .replace("CREATE TABLE IF NOT EXISTS ", "CREATE TABLE ")
            .replace("CREATE TABLE ", "CREATE TABLE IF NOT EXISTS ");
        fingerprint_schema(&sql).unwrap()
    }
    fn transition(
        id: &str,
        predecessor: Option<&str>,
        before: &str,
        after: &str,
        sql: &str,
    ) -> Transition {
        Transition {
            id: id.into(),
            predecessor: predecessor.map(str::to_owned),
            before: before.into(),
            after: after.into(),
            max_rows: 10,
            statements: vec![sql.into()],
        }
    }
    fn db(schema: &str) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema).unwrap();
        connection
    }

    #[test]
    fn fingerprints_use_shared_token_normalization_and_preserve_literal_case() {
        let left = db("CREATE TABLE tasks(id INTEGER, label TEXT DEFAULT 'X'); /* comment */");
        let right =
            db("create table [tasks] (\"id\" integer, `label` text default 'X') -- comment\n;");
        let changed_literal = db("CREATE TABLE tasks(id INTEGER, label TEXT DEFAULT 'x');");
        assert_eq!(fingerprint(&left).unwrap(), fingerprint(&right).unwrap());
        assert_ne!(
            fingerprint(&left).unwrap(),
            fingerprint(&changed_literal).unwrap()
        );
    }

    #[test]
    fn document_uses_max_rows_on_the_wire() {
        let source = serde_json::json!({
            "version": 1,
            "transitions": [{
                "id": "v1",
                "predecessor": null,
                "before": fp("CREATE TABLE todos(id INTEGER PRIMARY KEY);"),
                "after": fp("CREATE TABLE tasks(id INTEGER PRIMARY KEY);"),
                "maxRows": 7,
                "statements": ["ALTER TABLE todos RENAME TO tasks"]
            }]
        })
        .to_string();
        let parsed = parse(&source).unwrap();
        assert_eq!(parsed.transitions[0].max_rows, 7);
        let serialized = serde_json::to_value(parsed).unwrap();
        assert_eq!(serialized["transitions"][0]["maxRows"], 7);
        assert!(serialized["transitions"][0].get("max_rows").is_none());
    }

    #[test]
    fn fingerprints_exclude_both_internal_journal_tables() {
        let plain = db("CREATE TABLE tasks(id INTEGER PRIMARY KEY);");
        let journaled = db("CREATE TABLE tasks(id INTEGER PRIMARY KEY);");
        journaled
            .execute_batch(
                "CREATE TABLE __konae_schema_state(singleton INTEGER PRIMARY KEY);\
             CREATE TABLE __konae_schema_history(ordinal INTEGER PRIMARY KEY);",
            )
            .unwrap();
        assert_eq!(
            fingerprint(&plain).unwrap(),
            fingerprint(&journaled).unwrap()
        );
        assert!(
            fingerprint_schema("CREATE TABLE IF NOT EXISTS __konae_schema_state(id INTEGER);")
                .is_err()
        );
    }

    #[test]
    fn rename_fingerprint_matches_the_final_schema() {
        let before = "CREATE TABLE todos(id INTEGER PRIMARY KEY, title TEXT);";
        let after = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, title TEXT);";
        let connection = db(before);
        let transition = transition(
            "rename",
            None,
            &fp(before),
            &fp(after),
            "ALTER TABLE todos RENAME TO tasks",
        );
        let definition = Definition {
            version: 1,
            transitions: vec![transition],
        };
        let plan = plan(&connection, &definition).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let applied = apply_in_transaction(&tx, &plan, after).unwrap();
        assert_eq!(applied.transitions[0].id, "rename");
        tx.commit().unwrap();
        assert_eq!(fingerprint(&connection).unwrap(), fp(after));
    }

    #[test]
    fn skipped_releases_run_each_transition_in_order_and_retry_is_empty() {
        let s0 = "CREATE TABLE todos(id INTEGER PRIMARY KEY, title TEXT);";
        let s1 = "CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT);";
        let s2 = "CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, done INTEGER NOT NULL DEFAULT 0);";
        let s3 = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, label TEXT, done INTEGER NOT NULL DEFAULT 0);";
        let definition = Definition {
            version: 1,
            transitions: vec![
                transition(
                    "v1-rename",
                    None,
                    &fp(s0),
                    &fp(s1),
                    "ALTER TABLE todos RENAME TO tasks",
                ),
                transition(
                    "v2-add",
                    Some("v1-rename"),
                    &fp(s1),
                    &fp(s2),
                    "ALTER TABLE tasks ADD COLUMN done INTEGER NOT NULL DEFAULT 0",
                ),
                Transition {
                    statements: vec![
                        "ALTER TABLE tasks RENAME COLUMN title TO label".into(),
                        "UPDATE tasks SET label=trim(label)".into(),
                    ],
                    ..transition(
                        "v3-convert",
                        Some("v2-add"),
                        &fp(s2),
                        &fp(s3),
                        "ALTER TABLE tasks RENAME COLUMN title TO label",
                    )
                },
            ],
        };
        let connection = db(
            "CREATE TABLE todos(id INTEGER PRIMARY KEY, title TEXT); INSERT INTO todos(title) VALUES (' first '), (' second ');",
        );
        let planned = plan(&connection, &definition).unwrap();
        assert_eq!(planned.transitions.len(), 3);
        let tx = connection.unchecked_transaction().unwrap();
        let applied = apply_in_transaction(&tx, &planned, s3).unwrap();
        assert_eq!(
            applied
                .transitions
                .iter()
                .map(|row| row.affected_rows)
                .collect::<Vec<_>>(),
            vec![0, 0, 2]
        );
        tx.commit().unwrap();
        let history = load_history(&connection).unwrap();
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.head_id.as_deref(), Some("v3-convert"));
        let second_plan = plan(&connection, &definition).unwrap();
        assert!(second_plan.transitions.is_empty());
        assert_eq!(fingerprint(&connection).unwrap(), fp(s3));
        let labels: Vec<String> = connection
            .prepare("SELECT label FROM tasks ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(labels, ["first", "second"]);
    }

    #[test]
    fn fresh_baseline_skips_and_locks_every_definition() {
        let final_schema = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY);";
        let final_fp = fp(final_schema);
        let first = transition(
            "v1",
            None,
            &fp("CREATE TABLE todos(id INTEGER PRIMARY KEY);"),
            &final_fp,
            "ALTER TABLE todos RENAME TO tasks",
        );
        let definition = Definition {
            version: 1,
            transitions: vec![first],
        };
        let connection = Connection::open_in_memory().unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let applied = initialize_fresh_baseline(&tx, &definition, final_schema).unwrap();
        assert!(applied.transitions[0].baseline_skipped);
        tx.commit().unwrap();
        let history = load_history(&connection).unwrap();
        assert_eq!(history.head_id.as_deref(), Some("v1"));
        let changed = Definition {
            transitions: vec![Transition {
                statements: vec!["ALTER TABLE todos RENAME TO items".into()],
                ..definition.transitions[0].clone()
            }],
            ..definition
        };
        assert_eq!(
            plan(&connection, &changed).unwrap_err().code,
            HISTORY_MISMATCH
        );
    }

    #[test]
    fn fresh_initialization_uses_final_schema_and_skips_all_history() {
        let final_schema = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, label TEXT);";
        let prior_schema = "CREATE TABLE todos(id INTEGER PRIMARY KEY, title TEXT);";
        let definition = Definition {
            version: 1,
            transitions: vec![transition(
                "v1",
                None,
                &fp(prior_schema),
                &fp(final_schema),
                "ALTER TABLE todos RENAME TO tasks",
            )],
        };
        let connection = Connection::open_in_memory().unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let applied = initialize_fresh_baseline(&tx, &definition, final_schema).unwrap();
        tx.commit().unwrap();
        assert_eq!(applied.transitions.len(), 1);
        assert!(applied.transitions[0].baseline_skipped);
        let history = load_history(&connection).unwrap();
        assert_eq!(
            history.entries[0].definition_sha256,
            definition_digest(&definition.transitions[0]).unwrap()
        );
        assert!(
            plan(&connection, &definition)
                .unwrap()
                .transitions
                .is_empty()
        );
    }

    #[test]
    fn legacy_plan_requires_the_observed_initial_fingerprint() {
        let schema = "CREATE TABLE todos(id INTEGER PRIMARY KEY);";
        let connection = db(schema);
        let after = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY);";
        let definition = Definition {
            version: 1,
            transitions: vec![transition(
                "rename",
                None,
                &fp("CREATE TABLE other(id INTEGER PRIMARY KEY);"),
                &fp(after),
                "ALTER TABLE todos RENAME TO tasks",
            )],
        };
        assert_eq!(
            plan(&connection, &definition).unwrap_err().code,
            BEFORE_MISMATCH
        );
    }

    #[test]
    fn insert_and_update_require_confirmation_and_report_changes() {
        let schema = "CREATE TABLE tasks(id INTEGER PRIMARY KEY, label TEXT);";
        let connection = db(
            "CREATE TABLE tasks(id INTEGER PRIMARY KEY, label TEXT); INSERT INTO tasks VALUES (1,' x ');",
        );
        let after = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, label TEXT, done INTEGER NOT NULL DEFAULT 0);";
        let supported = Definition {
            version: 1,
            transitions: vec![Transition {
                id: "clean".into(),
                predecessor: None,
                before: fp(schema),
                after: fp(after),
                max_rows: 5,
                statements: vec![
                    "UPDATE tasks SET label = trim(label)".into(),
                    "ALTER TABLE tasks ADD COLUMN done INTEGER NOT NULL DEFAULT 0".into(),
                ],
            }],
        };
        let plan = plan(&connection, &supported).unwrap();
        assert!(plan.requires_confirmation);
        let tx = connection.unchecked_transaction().unwrap();
        let applied = apply_in_transaction(&tx, &plan, after).unwrap();
        assert_eq!(applied.transitions[0].affected_rows, 1);
        tx.commit().unwrap();
        let label: String = connection
            .query_row("SELECT label FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(label, "x");
    }

    #[test]
    fn bounded_parser_refuses_destructive_and_external_sql() {
        for sql in [
            "DROP TABLE tasks",
            "DELETE FROM tasks",
            "REPLACE INTO tasks VALUES (1)",
            "ALTER TABLE tasks DROP COLUMN label",
            "CREATE TRIGGER x AFTER INSERT ON tasks BEGIN SELECT 1; END",
            "PRAGMA writable_schema=ON",
            "ATTACH DATABASE 'x' AS x",
            "UPDATE __konae_schema_state SET head_id='x'",
            "UPDATE tasks SET label=readfile('/etc/passwd')",
            "INSERT INTO sqlite_schema(name) VALUES ('x')",
            "UPDATE tasks SET label='x'; DELETE FROM tasks",
        ] {
            assert!(validate_statement(sql).is_err(), "accepted {sql}");
        }
    }

    #[test]
    fn authorizer_blocks_external_functions_even_inside_allowed_dml() {
        let schema = "CREATE TABLE IF NOT EXISTS tasks(label TEXT);";
        let connection = db(schema);
        let tx = connection.unchecked_transaction().unwrap();
        tx.authorizer(Some(transform_authorizer));
        let result = tx.query_row("SELECT readfile('/etc/passwd')", [], |_| Ok(()));
        assert!(result.is_err());
        tx.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
        tx.rollback().unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn row_limit_rolls_back_all_statements_in_the_outer_transaction() {
        let schema = "CREATE TABLE IF NOT EXISTS tasks(label TEXT);";
        let connection =
            db("CREATE TABLE tasks(label TEXT); INSERT INTO tasks VALUES ('a'),('b');");
        let definition = Definition {
            version: 1,
            transitions: vec![Transition {
                id: "backfill".into(),
                predecessor: None,
                before: fp(schema),
                after: fp(schema),
                max_rows: 1,
                statements: vec!["UPDATE tasks SET label='x'".into()],
            }],
        };
        let plan = plan(&connection, &definition).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        assert_eq!(
            apply_in_transaction(&tx, &plan, schema).unwrap_err().code,
            ROW_LIMIT
        );
        tx.rollback().unwrap();
        let values: Vec<String> = connection
            .prepare("SELECT label FROM tasks ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(values, ["a", "b"]);
    }

    #[test]
    fn missing_document_refuses_when_history_exists() {
        let schema = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY);";
        let prior = "CREATE TABLE todos(id INTEGER PRIMARY KEY);";
        let definition = Definition {
            version: 1,
            transitions: vec![transition(
                "rename",
                None,
                &fp(prior),
                &fp(schema),
                "ALTER TABLE todos RENAME TO tasks",
            )],
        };
        let connection = Connection::open_in_memory().unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        initialize_fresh_baseline(&tx, &definition, schema).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            check_document_presence(&connection, false)
                .unwrap_err()
                .code,
            HISTORY_MISMATCH
        );
    }
}
