//! The change from an app's live database to the one its `schema.sql`
//! states (ADR-0005).
//!
//! The Kernel runs `schema.sql` only in a scratch database in memory, then
//! compares that database's tables, columns and indexes with the live one,
//! reading both through SQLite itself. Only statements the Kernel writes from
//! that comparison touch the live file:
//!
//! - A new table, a new column and a new index apply.
//! - A table whose definition changed is rebuilt the way the SQLite page on
//!   ALTER TABLE says (section "Making Other Kinds Of Table Schema Changes"):
//!   foreign keys off, then in one transaction create the new table under
//!   another name, copy the rows, drop the old table, rename the new one,
//!   recreate the indexes and check the foreign keys.
//! - A dropped table, a dropped column and a column whose type changed lose
//!   something, so the change applies only with the confirmation that the
//!   plan of those same statements carries.
//!
//! An index loses no row, so a dropped or changed index applies without one.

use super::{denied_function, limit_database_size, notify, refusal::refuse, ActionProgram, MAX_APP_DATABASE_BYTES};
use anyhow::Result;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

/// What the live database loses under a change, in a sentence for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    pub table: String,
    pub column: Option<String>,
    pub sentence: String,
}

/// The statements the Kernel ran, or would run, and what they lose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub statements: Vec<String>,
    pub losses: Vec<Loss>,
}

impl Change {
    /// The confirmation a change that loses something needs. It is a digest
    /// of the statements, so a confirmation fits only the plan it was given for.
    pub fn confirmation(&self) -> Option<String> {
        (!self.losses.is_empty()).then(|| hex::encode(Sha256::digest(self.statements.join(";\n").as_bytes())))
    }

    /// `{ statements, losses, confirmation }` as `release_plan` answers it.
    /// `confirmation` is null when the change loses nothing.
    pub fn to_json(&self) -> Value {
        json!({
            "statements": self.statements,
            "losses": self.losses.iter().map(|loss| json!({"table": loss.table, "column": loss.column, "sentence": loss.sentence})).collect::<Vec<_>>(),
            "confirmation": self.confirmation(),
        })
    }
}

/// Whether the change may apply.
pub enum Mode<'a> {
    /// Apply what loses nothing, and what loses something only when the
    /// confirmation matches.
    Apply(Option<&'a str>),
    /// Apply everything. The caller holds a copy it will throw away and wants
    /// the plan.
    Plan,
}

const NEEDS_CONFIRMATION: &str = "schema.confirmation-required";

/// Changes the database at `path` to the one `schema` states, in one
/// transaction, and checks that every action of `program` prepares against
/// the result. Nothing is kept when any step fails.
pub fn apply(path: &Path, schema: &str, program: &ActionProgram, mode: Mode<'_>) -> Result<Change> {
    if let Some(refusal) = super::sql_rules::schema_refusal(schema) {
        anyhow::bail!(refusal);
    }
    let scratch = Scratch::load(schema)?;
    let mut connection = Connection::open(path)?;
    // Runs before the authorizer below.
    limit_database_size(&connection, MAX_APP_DATABASE_BYTES)?;
    // An expression index, a CHECK or a copied row runs SQL once per stored
    // row, so a value has the runtime's size limit here too, and the whole
    // change a deadline long enough to index or copy a full database.
    connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    connection.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline));
    // The SQLite procedure turns foreign keys off before the transaction,
    // since the pragma does nothing inside one.
    connection.execute_batch("PRAGMA foreign_keys=OFF")?;
    connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
        AuthAction::CreateTrigger { .. } | AuthAction::CreateTempTrigger { .. } => Authorization::Deny,
        AuthAction::CreateView { .. } | AuthAction::CreateTempView { .. } => Authorization::Deny,
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    let transaction = connection.transaction()?;
    let violations_before = foreign_key_violations(&transaction)?;
    let mut change = Change::default();
    derive(&transaction, &scratch, &mut change)?;
    if foreign_key_violations(&transaction)?.iter().any(|(key, count)| *count > violations_before.get(key).copied().unwrap_or(0)) {
        anyhow::bail!("schema.sql changes a table so that rows no longer match a foreign key. Keep the referenced columns, or change the rows first.");
    }
    // Actions prepare without the authorizer, as they always did. The
    // runtime's own authorizer refuses a denied function when the action runs.
    transaction.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
    for (name, action) in program.actions.iter().filter(|(_, action)| action.kind != "signal") {
        let statement = transaction.prepare(&action.statement).map_err(|error| {
            anyhow::anyhow!("action {name} does not match the deployed schema: {error}")
        })?;
        if action.kind == "mutation" && statement.column_count() > 0 {
            anyhow::bail!("Action {name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.");
        }
        drop(statement);
        notify::check_columns(&transaction, name, action)?;
    }
    if let Mode::Apply(confirmation) = mode {
        if let Some(expected) = change.confirmation() {
            if confirmation != Some(expected.as_str()) {
                let sentences = change.losses.iter().map(|loss| loss.sentence.as_str()).collect::<Vec<_>>().join(" ");
                let lead = if confirmation.is_some() { "The schema change differs from the plan that was confirmed." } else { "This schema change loses data." };
                anyhow::bail!(refuse(NEEDS_CONFIRMATION, format!("{lead} {sentences} Confirm the plan release_plan returns to apply it.")));
            }
        }
    }
    transaction.commit()?;
    connection.execute_batch("PRAGMA foreign_keys=ON")?;
    Ok(change)
}

/// The change from the live database in `transaction` to `scratch`, run
/// statement by statement into `change`.
fn derive(transaction: &Connection, scratch: &Scratch, change: &mut Change) -> Result<()> {
    let live = Schema::read(transaction)?;
    for wanted in &scratch.schema.tables {
        match live.table(&wanted.name) {
            None => run(transaction, change, wanted.sql.clone())?,
            Some(current) if same_sql(&current.sql, &wanted.sql) => {}
            Some(current) => change_table(transaction, scratch, current, wanted, change)?,
        }
    }
    for current in &live.tables {
        if scratch.schema.table(&current.name).is_none() {
            let rows: i64 = transaction.query_row(&format!("SELECT count(*) FROM {}", quote(&current.name)), [], |row| row.get(0))?;
            change.losses.push(Loss {
                table: current.name.clone(),
                column: None,
                sentence: format!("Drops table {} and its {}.", current.name, count(rows, "row")),
            });
            run(transaction, change, format!("DROP TABLE {}", quote(&current.name)))?;
        }
    }
    // A rebuilt or dropped table took its indexes with it, so read them again.
    let indexes = Schema::read(transaction)?.indexes;
    for current in &indexes {
        let wanted = scratch.schema.index(&current.name);
        if !wanted.is_some_and(|wanted| same_sql(&wanted.sql, &current.sql)) {
            run(transaction, change, format!("DROP INDEX {}", quote(&current.name)))?;
        }
    }
    for wanted in &scratch.schema.indexes {
        let kept = indexes.iter().any(|current| current.name.eq_ignore_ascii_case(&wanted.name) && same_sql(&wanted.sql, &current.sql));
        if !kept {
            run(transaction, change, wanted.sql.clone())?;
        }
    }
    Ok(())
}

/// Changes a table whose definition changed. New columns at the end go in
/// with ADD COLUMN when SQLite takes them and the result matches, and
/// anything else is a rebuild.
fn change_table(transaction: &Connection, scratch: &Scratch, current: &Table, wanted: &Table, change: &mut Change) -> Result<()> {
    let stored = |table: &Table| table.columns.iter().filter(|column| column.stored).map(|column| column.name.to_lowercase()).collect::<Vec<_>>();
    let wanted_stored = stored(wanted);
    let mut losses = Vec::new();
    for column in current.columns.iter().filter(|column| column.stored) {
        let target = wanted.columns.iter().find(|other| other.name.eq_ignore_ascii_case(&column.name));
        let filled: i64 = transaction.query_row(
            &format!("SELECT count(*) FROM {} WHERE {} IS NOT NULL", quote(&current.name), quote(&column.name)),
            [],
            |row| row.get(0),
        )?;
        if !wanted_stored.contains(&column.name.to_lowercase()) {
            losses.push(Loss {
                table: current.name.clone(),
                column: Some(column.name.clone()),
                sentence: format!("Drops column {}.{} and the {} it holds.", current.name, column.name, count(filled, "value")),
            });
        } else if let Some(target) = target.filter(|target| !target.declared.eq_ignore_ascii_case(&column.declared)) {
            losses.push(Loss {
                table: current.name.clone(),
                column: Some(column.name.clone()),
                sentence: format!(
                    "Changes column {}.{} from {} to {}, and SQLite converts its {} to the new type where it can.",
                    current.name, column.name, type_name(&column.declared), type_name(&target.declared), count(filled, "value")
                ),
            });
        }
    }
    if losses.is_empty() && add_columns(transaction, current, wanted, change)? {
        return Ok(());
    }
    change.losses.extend(losses);
    rebuild(transaction, scratch, current, wanted, change)
}

/// Adds the columns `wanted` has after all of `current`'s, when that is the
/// only difference. False, with nothing changed, when it is not.
fn add_columns(transaction: &Connection, current: &Table, wanted: &Table, change: &mut Change) -> Result<bool> {
    let same_start = wanted.columns.len() > current.columns.len()
        && current.columns.iter().zip(&wanted.columns).all(|(a, b)| a.name.eq_ignore_ascii_case(&b.name));
    if !same_start {
        return Ok(false);
    }
    let mut statements = Vec::new();
    for column in &wanted.columns[current.columns.len()..] {
        let Some(definition) = column_definition(&wanted.sql, &column.name) else { return Ok(false) };
        statements.push(format!("ALTER TABLE {} ADD COLUMN {definition}", quote(&wanted.name)));
    }
    transaction.execute_batch("SAVEPOINT konae_add_columns")?;
    let added = statements.iter().try_for_each(|statement| transaction.execute_batch(statement)).is_ok()
        && Schema::read(transaction)?.table(&wanted.name).is_some_and(|table| same_sql(&table.sql, &wanted.sql));
    if added {
        transaction.execute_batch("RELEASE konae_add_columns")?;
        change.statements.extend(statements);
    } else {
        transaction.execute_batch("ROLLBACK TO konae_add_columns; RELEASE konae_add_columns")?;
    }
    Ok(added)
}

/// The rebuild of the SQLite page on ALTER TABLE. Foreign keys are already
/// off and the transaction open; the caller checks the keys before commit,
/// and the index pass recreates the table's indexes.
fn rebuild(transaction: &Connection, scratch: &Scratch, current: &Table, wanted: &Table, change: &mut Change) -> Result<()> {
    let mut temporary = format!("konae_new_{}", wanted.name);
    while Schema::read(transaction)?.table(&temporary).is_some() || scratch.schema.table(&temporary).is_some() {
        temporary.push('_');
    }
    let create = scratch.renamed(&wanted.name, &temporary)?;
    let copied = wanted
        .columns
        .iter()
        .filter(|column| column.stored)
        .filter(|column| current.columns.iter().any(|other| other.stored && other.name.eq_ignore_ascii_case(&column.name)))
        .map(|column| quote(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    run(transaction, change, create)?;
    if !copied.is_empty() {
        let copy = format!("INSERT INTO {} ({copied}) SELECT {copied} FROM {}", quote(&temporary), quote(&current.name));
        transaction
            .execute_batch(&copy)
            .map_err(|error| anyhow::anyhow!("schema.sql cannot change table {} and keep its rows: {error}", wanted.name))?;
        change.statements.push(copy);
    }
    // The copy sets an AUTOINCREMENT counter from the rows it copied, which
    // would hand out the ID of a deleted row again. The old counter carries
    // over. It is the Kernel's bookkeeping, so it is not among the statements
    // a confirmation covers.
    let counter: Option<i64> = transaction
        .query_row("SELECT seq FROM sqlite_sequence WHERE name = ?1", [&current.name], |row| row.get(0))
        .optional()
        .unwrap_or(None);
    run(transaction, change, format!("DROP TABLE {}", quote(&current.name)))?;
    run(transaction, change, format!("ALTER TABLE {} RENAME TO {}", quote(&temporary), quote(&wanted.name)))?;
    if let Some(counter) = counter {
        transaction.execute("UPDATE sqlite_sequence SET seq = ?1 WHERE name = ?2 AND seq < ?1", rusqlite::params![counter, wanted.name])?;
    }
    Ok(())
}

fn run(transaction: &Connection, change: &mut Change, statement: String) -> Result<()> {
    transaction
        .execute_batch(&statement)
        .map_err(|error| anyhow::anyhow!("schema change failed at {statement}: {error}"))?;
    change.statements.push(statement);
    Ok(())
}

/// Foreign key violations by child and parent table, so a change is refused
/// only for the ones it adds.
fn foreign_key_violations(connection: &Connection) -> Result<HashMap<(String, String), i64>> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    let mut counts = HashMap::new();
    while let Some(row) = rows.next()? {
        *counts.entry((row.get::<_, String>(0)?, row.get::<_, String>(2)?)).or_insert(0) += 1;
    }
    Ok(counts)
}

/// `schema.sql` run into an empty database in memory, under the rules the
/// Kernel always held it to, so it holds tables and indexes and calls no
/// denied function.
struct Scratch {
    connection: Connection,
    schema: Schema,
}

impl Scratch {
    fn load(schema: &str) -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2 * 1024 * 1024)?;
        connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::CreateTable { .. } | AuthAction::CreateIndex { .. } => Authorization::Allow,
            // CHECK constraints, expression and partial indexes, and generated
            // columns call SQL functions. The runtime denylist still applies.
            AuthAction::Function { function_name } if denied_function(function_name) => Authorization::Deny,
            AuthAction::Function { .. } => Authorization::Allow,
            AuthAction::Insert { table_name: "sqlite_master" } => Authorization::Allow,
            AuthAction::Update { table_name: "sqlite_master", .. } => Authorization::Allow,
            AuthAction::Read { .. } => Authorization::Allow,
            AuthAction::Reindex { .. } => Authorization::Allow,
            _ => Authorization::Deny,
        }));
        connection
            .execute_batch(schema)
            .map_err(|error| anyhow::anyhow!("schema may only create tables and indexes: {error}"))?;
        // The editor preview runs schema.sql again on every open, so the
        // Validation Gate runs it twice and the Kernel does too.
        connection
            .execute_batch(schema)
            .map_err(|error| anyhow::anyhow!("schema.sql is not idempotent: it fails when it is run a second time. {error}"))?;
        connection.authorizer::<fn(AuthContext<'_>) -> Authorization>(None);
        let schema = Schema::read(&connection)?;
        Ok(Self { connection, schema })
    }

    /// The CREATE TABLE of `name` under the name `to`. SQLite renames it,
    /// with every reference the table makes to itself, and the rename is
    /// rolled back.
    fn renamed(&self, name: &str, to: &str) -> Result<String> {
        self.connection.execute_batch("SAVEPOINT konae_rename")?;
        let renamed = self
            .connection
            .execute_batch(&format!("ALTER TABLE {} RENAME TO {}", quote(name), quote(to)))
            .map_err(anyhow::Error::from)
            .and_then(|()| {
                self.connection
                    .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name=?1", [to], |row| row.get::<_, String>(0))
                    .optional()?
                    .ok_or_else(|| anyhow::anyhow!("SQLite did not rename table {name}."))
            });
        self.connection.execute_batch("ROLLBACK TO konae_rename; RELEASE konae_rename")?;
        renamed
    }
}

#[derive(Debug)]
struct Column {
    name: String,
    declared: String,
    /// False for a generated column, which holds no data of its own.
    stored: bool,
}

#[derive(Debug)]
struct Table {
    name: String,
    sql: String,
    columns: Vec<Column>,
}

#[derive(Debug)]
struct Index {
    name: String,
    sql: String,
}

/// The tables and indexes of a database, as SQLite reads them. SQLite's own
/// tables and the indexes it makes for a UNIQUE or PRIMARY KEY are left out.
#[derive(Debug, Default)]
struct Schema {
    tables: Vec<Table>,
    indexes: Vec<Index>,
}

impl Schema {
    fn read(connection: &Connection) -> Result<Self> {
        let mut schema = Schema::default();
        let mut statement = connection.prepare(
            "SELECT type, name, sql FROM sqlite_master WHERE type IN ('table','index') AND sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY rowid",
        )?;
        let entries = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (kind, name, sql) in entries {
            if kind == "index" {
                schema.indexes.push(Index { name, sql });
                continue;
            }
            let mut columns = Vec::new();
            let mut info = connection.prepare(&format!("PRAGMA table_xinfo({})", quote(&name)))?;
            let mut rows = info.query([])?;
            while let Some(row) = rows.next()? {
                let hidden: i64 = row.get(6)?;
                columns.push(Column { name: row.get(1)?, declared: row.get(2)?, stored: !matches!(hidden, 2 | 3) });
            }
            schema.tables.push(Table { name, sql, columns });
        }
        Ok(schema)
    }

    fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|table| table.name.eq_ignore_ascii_case(name))
    }

    fn index(&self, name: &str) -> Option<&Index> {
        self.indexes.iter().find(|index| index.name.eq_ignore_ascii_case(name))
    }
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn count(n: i64, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

fn type_name(declared: &str) -> &str {
    if declared.is_empty() { "no type" } else { declared }
}

/// A token of SQL text and where it starts and ends in that text.
#[derive(Debug)]
struct Token {
    text: String,
    start: usize,
    end: usize,
}

/// Words, quoted names and strings, numbers and single symbols. Whitespace
/// and comments are dropped. A word or a quoted name comes back lowercased
/// and unquoted, so two spellings of one definition compare equal.
fn tokens(sql: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let end_of = |index: usize| chars.get(index).map_or(sql.len(), |(offset, _)| *offset);
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (start, char) = chars[index];
        let next = chars.get(index + 1).map(|(_, char)| *char);
        if char.is_whitespace() {
            index += 1;
        } else if char == '-' && next == Some('-') {
            while index < chars.len() && chars[index].1 != '\n' { index += 1; }
        } else if char == '/' && next == Some('*') {
            index += 2;
            while index < chars.len() && !(chars[index].1 == '*' && chars.get(index + 1).map(|(_, c)| *c) == Some('/')) { index += 1; }
            index = (index + 2).min(chars.len());
        } else if matches!(char, '\'' | '"' | '`' | '[') {
            let close = if char == '[' { ']' } else { char };
            let mut text = String::new();
            index += 1;
            while index < chars.len() {
                if chars[index].1 == close {
                    if close != ']' && chars.get(index + 1).map(|(_, c)| *c) == Some(close) {
                        text.push(close);
                        index += 2;
                        continue;
                    }
                    break;
                }
                text.push(chars[index].1);
                index += 1;
            }
            index = (index + 1).min(chars.len());
            let text = if char == '\'' { format!("'{text}'") } else { text.to_lowercase() };
            tokens.push(Token { text, start, end: end_of(index) });
        } else if char.is_alphanumeric() || char == '_' || char == '$' {
            let begin = index;
            while index < chars.len() && (chars[index].1.is_alphanumeric() || matches!(chars[index].1, '_' | '$' | '.') && index > begin) { index += 1; }
            let text: String = chars[begin..index].iter().map(|(_, c)| *c).collect();
            tokens.push(Token { text: text.to_lowercase(), start, end: end_of(index) });
        } else {
            index += 1;
            tokens.push(Token { text: char.to_string(), start, end: end_of(index) });
        }
    }
    tokens
}

/// Whether two CREATE statements define the same thing. They do when their
/// tokens match once whitespace, comments, quoting, letter case and IF NOT
/// EXISTS are set aside.
fn same_sql(a: &str, b: &str) -> bool {
    fn normal(sql: &str) -> Vec<String> {
        let mut words: Vec<String> = tokens(sql).into_iter().map(|token| token.text).collect();
        if let Some(at) = words.windows(3).position(|window| window == ["if", "not", "exists"]) {
            if at <= 3 {
                words.drain(at..at + 3);
            }
        }
        words
    }
    normal(a) == normal(b)
}

/// The text of one column's definition in a CREATE TABLE, as written there.
fn column_definition(sql: &str, column: &str) -> Option<String> {
    let tokens = tokens(sql);
    let open = tokens.iter().position(|token| token.text == "(")?;
    let mut depth = 0;
    let mut start = open + 1;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.text.as_str() {
            "(" => depth += 1,
            ")" | "," if depth == 1 => {
                if let Some(first) = tokens.get(start).filter(|first| first.text == column.to_lowercase()) {
                    if index > start {
                        return Some(sql[first.start..tokens[index - 1].end].to_owned());
                    }
                }
                if token.text == ")" {
                    return None;
                }
                start = index + 1;
            }
            ")" => depth -= 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionDefinition, ActionProgram};

    fn program(statements: &[(&str, &str)]) -> ActionProgram {
        let actions = statements
            .iter()
            .map(|(name, statement)| {
                let definition: ActionDefinition = serde_json::from_value(json!({"kind": "query", "access": "anonymous", "statement": statement, "bindings": []})).unwrap();
                ((*name).to_owned(), definition)
            })
            .collect();
        ActionProgram { version: 1, actions }
    }

    fn database() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("konae-schema-change-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("database.sqlite");
        crate::initialize_database(&path).unwrap();
        (root, path)
    }

    fn rows(path: &Path, sql: &str) -> Vec<Vec<String>> {
        let connection = Connection::open(path).unwrap();
        let mut statement = connection.prepare(sql).unwrap();
        let width = statement.column_count();
        statement
            .query_map([], |row| (0..width).map(|i| row.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}"))).collect())
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    const TASKS: &str = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";

    fn seeded() -> (std::path::PathBuf, std::path::PathBuf) {
        let (root, path) = database();
        apply(&path, TASKS, &program(&[]), Mode::Apply(None)).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("INSERT INTO tasks(title, done) VALUES ('first', 0), ('second', 1), ('third', 0);")
            .unwrap();
        (root, path)
    }

    #[test]
    fn an_unchanged_schema_changes_nothing_and_a_second_pass_is_empty() {
        let (root, path) = seeded();
        assert_eq!(apply(&path, TASKS, &program(&[]), Mode::Apply(None)).unwrap(), Change::default());
        // Another spelling of the same definitions is the same schema.
        let respelled = "-- the tasks\ncreate table if not exists tasks (\n  \"id\" integer primary key,\n  title text not null,\n  done integer not null default 0\n);\ncreate index if not exists tasks_title on tasks (title);";
        assert_eq!(apply(&path, respelled, &program(&[]), Mode::Apply(None)).unwrap(), Change::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_table_column_and_index_apply_and_keep_every_row() {
        let (root, path) = seeded();
        let before = rows(&path, "SELECT id, title, done FROM tasks ORDER BY id");
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, note TEXT, priority INTEGER NOT NULL DEFAULT 2);
            CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);
            CREATE INDEX IF NOT EXISTS tasks_done ON tasks(done);
            CREATE TABLE IF NOT EXISTS tags (id INTEGER PRIMARY KEY, task_id INTEGER NOT NULL REFERENCES tasks(id), name TEXT NOT NULL);";
        let actions = program(&[("tasks.list", "SELECT id, title, note, priority FROM tasks LIMIT 50")]);
        let change = apply(&path, wanted, &actions, Mode::Apply(None)).unwrap();
        assert!(change.losses.is_empty(), "{change:?}");
        assert_eq!(change.confirmation(), None);
        assert_eq!(
            change.statements,
            vec![
                "ALTER TABLE \"tasks\" ADD COLUMN note TEXT".to_owned(),
                "ALTER TABLE \"tasks\" ADD COLUMN priority INTEGER NOT NULL DEFAULT 2".to_owned(),
                "CREATE TABLE tags (id INTEGER PRIMARY KEY, task_id INTEGER NOT NULL REFERENCES tasks(id), name TEXT NOT NULL)".to_owned(),
                "CREATE INDEX tasks_done ON tasks(done)".to_owned(),
            ]
        );
        assert_eq!(rows(&path, "SELECT id, title, done FROM tasks ORDER BY id"), before);
        assert_eq!(rows(&path, "SELECT DISTINCT priority FROM tasks"), vec![vec!["Integer(2)".to_owned()]]);
        assert_eq!(apply(&path, wanted, &actions, Mode::Apply(None)).unwrap(), Change::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_column_add_column_refuses_is_rebuilt_without_confirmation() {
        let (root, path) = seeded();
        // ADD COLUMN takes no UNIQUE column and no default that is an expression.
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, slug TEXT UNIQUE, created_at TEXT NOT NULL DEFAULT (datetime('now')));
            CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let change = apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap();
        assert!(change.losses.is_empty());
        assert!(change.statements.iter().any(|statement| statement.starts_with("INSERT INTO \"konae_new_tasks\" (\"id\", \"title\", \"done\")")), "{change:?}");
        assert_eq!(rows(&path, "SELECT count(*), count(created_at) FROM tasks"), vec![vec!["Integer(3)".to_owned(), "Integer(3)".to_owned()]]);
        // The rebuild took the index with the old table, and the index pass made it again.
        assert_eq!(rows(&path, "SELECT name FROM sqlite_master WHERE type='index' AND sql IS NOT NULL"), vec![vec!["Text(\"tasks_title\")".to_owned()]]);
        assert_eq!(apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap(), Change::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dropped_column_needs_the_confirmation_of_its_plan_and_keeps_every_other_column() {
        let (root, path) = seeded();
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let before = rows(&path, "SELECT id, title FROM tasks ORDER BY id");

        let refused = apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap_err();
        let refusal = crate::refusal::of(&refused).expect("a coded refusal");
        assert_eq!(refusal.code, NEEDS_CONFIRMATION);
        assert!(refusal.message.contains("Drops column tasks.done and the 3 values it holds."), "{refusal:?}");
        assert_eq!(rows(&path, "SELECT count(done) FROM tasks"), vec![vec!["Integer(3)".to_owned()]], "nothing changed");

        // The plan, made on a copy as release_plan makes it.
        let copy = root.join("copy.sqlite");
        Connection::open(&path).unwrap().execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()]).unwrap();
        let plan = apply(&copy, wanted, &program(&[]), Mode::Plan).unwrap();
        let confirmation = plan.confirmation().expect("a change that loses data asks");
        assert_eq!(plan.to_json()["losses"][0]["column"], "done");

        assert!(crate::refusal::of(&apply(&path, wanted, &program(&[]), Mode::Apply(Some(&"0".repeat(64)))).unwrap_err()).is_some(), "a stale confirmation is refused");
        let applied = apply(&path, wanted, &program(&[]), Mode::Apply(Some(&confirmation))).unwrap();
        assert_eq!(applied.statements, plan.statements);
        assert_eq!(rows(&path, "SELECT id, title FROM tasks ORDER BY id"), before);
        assert_eq!(rows(&path, "SELECT name FROM pragma_table_info('tasks')"), vec![vec!["Text(\"id\")".to_owned()], vec!["Text(\"title\")".to_owned()]]);
        assert_eq!(apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap(), Change::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dropped_table_and_a_retyped_column_say_what_they_lose() {
        let (root, path) = seeded();
        let with_notes = format!("{TASKS} CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT);");
        apply(&path, &with_notes, &program(&[]), Mode::Apply(None)).unwrap();
        Connection::open(&path).unwrap().execute_batch("INSERT INTO notes(body) VALUES ('a'), ('b');").unwrap();
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done TEXT NOT NULL DEFAULT '0'); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let error = apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap_err().to_string();
        assert!(error.contains("Changes column tasks.done from INTEGER to TEXT, and SQLite converts its 3 values to the new type where it can."), "{error}");
        assert!(error.contains("Drops table notes and its 2 rows."), "{error}");
        let plan = apply(&path, wanted, &program(&[]), Mode::Plan).unwrap();
        assert!(plan.statements.contains(&"DROP TABLE \"notes\"".to_owned()));
        assert_eq!(rows(&path, "SELECT typeof(done), count(*) FROM tasks GROUP BY 1"), vec![vec!["Text(\"text\")".to_owned(), "Integer(3)".to_owned()]]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rebuild_keeps_foreign_keys_and_refuses_to_break_them() {
        let (root, path) = database();
        let schema = "CREATE TABLE IF NOT EXISTS lists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, list_id INTEGER NOT NULL REFERENCES lists(id), label TEXT);";
        apply(&path, schema, &program(&[]), Mode::Apply(None)).unwrap();
        Connection::open(&path).unwrap().execute_batch("INSERT INTO lists VALUES (1, 'home'); INSERT INTO items VALUES (1, 1, 'milk');").unwrap();
        // The parent is rebuilt, and the child's reference still holds afterwards.
        let reordered = "CREATE TABLE IF NOT EXISTS lists (name TEXT NOT NULL, id INTEGER PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, list_id INTEGER NOT NULL REFERENCES lists(id), label TEXT);";
        apply(&path, reordered, &program(&[]), Mode::Apply(None)).unwrap();
        assert_eq!(rows(&path, "SELECT count(*) FROM pragma_foreign_key_check"), vec![vec!["Integer(0)".to_owned()]]);
        assert_eq!(rows(&path, "SELECT name FROM lists"), vec![vec!["Text(\"home\")".to_owned()]]);
        // A parent whose key column goes would leave the child pointing at nothing.
        let broken = "CREATE TABLE IF NOT EXISTS lists (name TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, list_id INTEGER NOT NULL REFERENCES lists(id), label TEXT);";
        let plan = apply(&path, broken, &program(&[]), Mode::Plan);
        assert!(plan.is_err(), "{plan:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rebuild_that_cannot_keep_its_rows_changes_nothing() {
        let (root, path) = seeded();
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, owner TEXT NOT NULL); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title);";
        let error = format!("{:#}", apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap_err());
        assert!(error.contains("cannot change table tasks and keep its rows"), "{error}");
        assert_eq!(rows(&path, "SELECT count(*) FROM tasks"), vec![vec!["Integer(3)".to_owned()]]);
        assert_eq!(rows(&path, "SELECT count(*) FROM pragma_table_info('tasks')"), vec![vec!["Integer(3)".to_owned()]]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dropped_or_changed_index_applies_without_confirmation() {
        let (root, path) = seeded();
        let wanted = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0); CREATE INDEX IF NOT EXISTS tasks_title ON tasks(title, done);";
        let change = apply(&path, wanted, &program(&[]), Mode::Apply(None)).unwrap();
        assert_eq!(change.statements, vec!["DROP INDEX \"tasks_title\"".to_owned(), "CREATE INDEX tasks_title ON tasks(title, done)".to_owned()]);
        let change = apply(&path, "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0);", &program(&[]), Mode::Apply(None)).unwrap();
        assert_eq!(change.statements, vec!["DROP INDEX \"tasks_title\"".to_owned()]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rebuilt_autoincrement_table_never_hands_out_a_deleted_id_again() {
        let (root, path) = database();
        apply(&path, "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT, extra TEXT);", &program(&[]), Mode::Apply(None)).unwrap();
        Connection::open(&path).unwrap().execute_batch("INSERT INTO notes(body) VALUES ('a'), ('b'), ('c'); DELETE FROM notes WHERE id = 3;").unwrap();
        let wanted = "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT);";
        let plan = apply(&path, wanted, &program(&[]), Mode::Plan).unwrap();
        assert_eq!(plan.losses.len(), 1);
        Connection::open(&path).unwrap().execute("INSERT INTO notes(body) VALUES ('d')", []).unwrap();
        assert_eq!(rows(&path, "SELECT max(id) FROM notes"), vec![vec!["Integer(4)".to_owned()]]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_schema_that_fails_its_second_run_is_refused_as_the_gate_refuses_it() {
        let (root, path) = database();
        let error = apply(&path, "CREATE TABLE notes (id INTEGER PRIMARY KEY);", &program(&[]), Mode::Apply(None)).unwrap_err().to_string();
        assert!(error.starts_with("schema.sql is not idempotent: it fails when it is run a second time."), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_schema_change_cases_answer_as_written() {
        let file: Value = serde_json::from_str(include_str!("schema-change-cases.json")).unwrap();
        for case in file["cases"].as_array().unwrap() {
            let (root, path) = database();
            apply(&path, case["live"].as_str().unwrap(), &program(&[]), Mode::Apply(None)).unwrap();
            Connection::open(&path).unwrap().execute_batch(case["rows"].as_str().unwrap()).unwrap();
            let schema = case["schema"].as_str().unwrap();
            let confirmation = match case["confirmation"].as_str() {
                Some("plan") => {
                    let copy = root.join("copy.sqlite");
                    Connection::open(&path).unwrap().execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()]).unwrap();
                    apply(&copy, schema, &program(&[]), Mode::Plan).unwrap().confirmation()
                }
                other => other.map(str::to_owned),
            };
            let outcome = apply(&path, schema, &program(&[]), Mode::Apply(confirmation.as_deref()))
                .map_err(|error| crate::refusal::of(&error).cloned().unwrap_or_else(|| panic!("{}: {error}", case["case"])));
            crate::refusal::assert_case(case, outcome);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn column_definitions_come_back_as_written() {
        let sql = "CREATE TABLE t (\n  a INTEGER PRIMARY KEY, -- key\n  \"b c\" TEXT CHECK (length(\"b c\") > 0),\n  d TEXT DEFAULT 'x,y',\n  UNIQUE (a, d)\n)";
        assert_eq!(column_definition(sql, "b c").as_deref(), Some("\"b c\" TEXT CHECK (length(\"b c\") > 0)"));
        assert_eq!(column_definition(sql, "d").as_deref(), Some("d TEXT DEFAULT 'x,y'"));
        assert!(same_sql("CREATE TABLE IF NOT EXISTS t(a)", "create table [T] ( \"a\" )"));
        assert!(!same_sql("CREATE TABLE t(a TEXT DEFAULT 'X')", "CREATE TABLE t(a TEXT DEFAULT 'x')"));
    }
}
