//! The SQL rules an action statement and `schema.sql` must follow, with the
//! words both doors refuse them in. `parseAction` in src/kernel/actions.ts and
//! `refusedStatements` in src/agent/gate.ts give the same messages, and
//! kernel-rs/kernel/src/sql-rule-cases.json holds the cases both test suites run.
//!
//! A statement is read with the tokenizer in main.rs, so a keyword or a `;`
//! inside a string, a quoted name or a comment never counts.

use super::refusal::{refuse, Refusal};
use super::{sql_tokens, ActionDefinition, SqlTok, SqlTokKind};

const MUTATION_WORDS: [&str; 4] = ["INSERT", "UPDATE", "DELETE", "REPLACE"];
const STRUCTURE_WORDS: [&str; 3] = ["CREATE", "ALTER", "DROP"];

/// The first keyword, one statement, and no RETURNING on a mutation. A
/// signal action has no statement and is not checked here.
pub fn check_statement(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    let tokens = sql_tokens(&action.statement);
    let first = tokens
        .iter()
        .find(|token| token.kind == SqlTokKind::Word)
        .map(|token| token.text.to_ascii_uppercase())
        .unwrap_or_default();
    let starts_right = if action.kind == "query" { first == "SELECT" } else { MUTATION_WORDS.contains(&first.as_str()) };
    if !starts_right {
        return Err(first_word_refusal(name, &action.kind, &first));
    }
    if let Some(end) = tokens.iter().position(|token| token.is_symbol(";")) {
        if end + 1 < tokens.len() {
            return Err(refuse("statement.several", format!("Action {name} must contain one statement.")));
        }
    }
    if action.kind == "mutation" && returns_rows(&tokens) {
        return Err(refuse("mutation.returning", format!(
            "Action {name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows."
        )));
    }
    Ok(())
}

/// The refusal for a statement that starts with the wrong word, with a hint
/// for the two mistakes the showcase apps made: a leading WITH, and schema
/// work written as an action.
fn first_word_refusal(name: &str, kind: &str, first: &str) -> Refusal {
    let rule = if kind == "query" {
        format!("Action {name} is a query, so it must start with SELECT.")
    } else {
        format!("Action {name} is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE.")
    };
    let hint = match (kind, first) {
        ("query", "WITH") => " Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).",
        (_, "WITH") => " Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.",
        (_, word) if STRUCTURE_WORDS.contains(&word) => {
            " Tables and indexes belong in schema.sql, and Konae runs no triggers or views."
        }
        _ => "",
    };
    let code = if kind == "query" { "query.start" } else { "mutation.start" };
    refuse(code, format!("{rule}{hint}"))
}

/// A RETURNING keyword outside parentheses. `:returning`, `@returning` and
/// `$returning` are named parameters, not the clause.
fn returns_rows(tokens: &[SqlTok]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        let parameter = index > 0 && [":", "@", "$"].iter().any(|symbol| tokens[index - 1].is_symbol(symbol));
        token.depth == 0 && token.is_word("RETURNING") && !parameter
    })
}

/// The statements of `schema.sql` the Kernel will not apply, named as the
/// Gate names them: a CREATE of something other than a table or an index, a
/// TEMP table or index, `CREATE TABLE ... AS`, the first word of a statement
/// that is not a CREATE, and each refused function it calls. The schema
/// authorizer refuses the same forms; this reading names them first, so both
/// doors answer with one code and sentence.
pub fn refused_schema_statements(schema: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |refused: String| {
        if !found.contains(&refused) {
            found.push(refused);
        }
    };
    for statement in statements(schema) {
        let words: Vec<String> = statement
            .iter()
            .filter(|token| token.kind == SqlTokKind::Word)
            .map(|token| token.text.to_ascii_uppercase())
            .collect();
        let refused = match words.first().map(String::as_str) {
            None => Some("a statement that is not CREATE TABLE or CREATE INDEX".to_owned()),
            Some("CREATE") => refused_create(&statement, &words),
            Some(word) => Some(word.to_owned()),
        };
        if let Some(refused) = refused {
            add(refused);
        }
        for name in refused_functions(&statement) {
            add(format!("{name}()"));
        }
    }
    found
}

/// The refused functions a statement calls, from `DENIED_FUNCTIONS`, which
/// the authorizer refuses too and `refusedFunctionNames` in
/// src/agent/gate.ts repeats. A quoted name still calls the function.
fn refused_functions(tokens: &[SqlTok]) -> Vec<String> {
    tokens
        .windows(2)
        .filter(|pair| matches!(pair[0].kind, SqlTokKind::Word | SqlTokKind::Ident) && pair[1].is_symbol("("))
        .map(|pair| pair[0].text.to_ascii_lowercase())
        .filter(|name| super::denied_function(name))
        .collect()
}

/// The CREATE forms the Kernel will not apply, as `refusedCreate` in
/// src/agent/gate.ts reads them: a trigger, a view or anything but a table
/// or an index; a table or an index in the temp schema; and a table made AS
/// a query, which copies rows in.
fn refused_create(tokens: &[SqlTok], words: &[String]) -> Option<String> {
    let object = if words.get(1).map(String::as_str) == Some("UNIQUE") { words.get(2) } else { words.get(1) };
    let object = match object.map(String::as_str) {
        Some(object @ ("TABLE" | "INDEX")) => object,
        _ => return Some(format!("CREATE {}", words.get(1).map(String::as_str).unwrap_or("")).trim().to_owned()),
    };
    let start = tokens.iter().position(|token| token.is_word(object))?;
    let mut index = start + 1;
    if tokens.get(index).is_some_and(|token| token.is_word("IF")) {
        index += 3;
    }
    let name = |token: Option<&SqlTok>| token.filter(|token| matches!(token.kind, SqlTokKind::Word | SqlTokKind::Ident)).map(|token| token.text.clone());
    if tokens.get(index + 1).is_some_and(|token| token.is_symbol(".")) {
        if let Some(schema) = name(tokens.get(index)).filter(|schema| ["temp", "temporary"].contains(&schema.to_ascii_lowercase().as_str())) {
            return Some(format!("CREATE {object} {schema}.*"));
        }
    }
    if object == "INDEX" {
        return None;
    }
    // The column list opens with the first top-level "(". An AS before it is
    // a CREATE TABLE ... AS query; GENERATED ALWAYS AS sits inside the list.
    let outer = tokens[start].depth;
    for token in tokens.iter().skip(index).filter(|token| token.depth == outer) {
        if token.is_symbol("(") {
            return None;
        }
        if token.is_word("AS") {
            return Some("CREATE TABLE ... AS".to_owned());
        }
    }
    None
}

/// The refusal for a schema with statements the Kernel will not apply.
pub fn schema_refusal(schema: &str) -> Option<Refusal> {
    let refused = refused_schema_statements(schema);
    (!refused.is_empty())
        .then(|| refuse("schema.statement-refused", format!("schema.sql may only create tables and indexes, but it contains {}.", refused.join(", "))))
}

/// Splits a script on `;`, keeping a trigger body (`BEGIN ...; END`) in its
/// statement, as `sqlStatementTokens` in src/lib/sql-tokens.ts does.
fn statements(sql: &str) -> Vec<Vec<SqlTok>> {
    let mut statements = Vec::new();
    let mut current: Vec<SqlTok> = Vec::new();
    let mut trigger_body = false;
    let mut open_cases = 0;
    for token in sql_tokens(sql) {
        if token.kind == SqlTokKind::Word {
            if token.is_word("BEGIN") && !trigger_body && is_trigger(&current) {
                trigger_body = true;
            } else if token.is_word("CASE") && trigger_body {
                open_cases += 1;
            } else if token.is_word("END") && trigger_body {
                if open_cases > 0 {
                    open_cases -= 1;
                } else {
                    trigger_body = false;
                }
            }
        }
        if token.is_symbol(";") && !trigger_body {
            if !current.is_empty() {
                statements.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(token);
    }
    if !current.is_empty() {
        statements.push(current);
    }
    statements
}

fn is_trigger(tokens: &[SqlTok]) -> bool {
    let words: Vec<&SqlTok> = tokens.iter().filter(|token| token.kind == SqlTokKind::Word).collect();
    words.first().is_some_and(|word| word.is_word("CREATE")) && words.iter().skip(1).take(3).any(|word| word.is_word("TRIGGER"))
}

/// The refusal for a database file that arrives holding a trigger or a view,
/// in an import, an initial database or a package. `what` names the file in
/// the sentence. schema.sql cannot create either, so one in a file came from
/// somewhere the Gate never read. SQLite reads `type` without regard to case,
/// so the match does the same.
pub fn stored_trigger_or_view(db: &rusqlite::Connection, what: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    db.query_row(
        "SELECT upper(type), name FROM sqlite_master WHERE lower(type) IN ('trigger', 'view') ORDER BY lower(type), name LIMIT 1",
        [],
        |row| Ok(format!("The {what} holds CREATE {} {}, and Konae runs no triggers or views. Drop it before importing.", row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
}

/// The refusal for a database file that arrives holding a virtual table. The
/// bundled SQLite compiles FTS3, FTS5, RTREE and DBSTAT, the Kernel creates no
/// virtual table in any of its files, and schema.sql cannot declare one. SQLite
/// stores a virtual table as a table row with `rootpage` 0, which the sql text
/// read as tokens confirms, so a comment or spacing in a hand edit does not hide it.
pub fn stored_virtual_table(db: &rusqlite::Connection, what: &str) -> rusqlite::Result<Option<String>> {
    let mut statement = db.prepare("SELECT name, rootpage, sql FROM sqlite_master WHERE lower(type) = 'table' ORDER BY name")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let (name, rootpage, sql): (String, Option<i64>, Option<String>) = (row.get(0)?, row.get(1)?, row.get(2)?);
        let words: Vec<String> = sql_tokens(&sql.unwrap_or_default())
            .into_iter()
            .filter(|token| token.kind == SqlTokKind::Word)
            .take(3)
            .map(|token| token.text.to_ascii_uppercase())
            .collect();
        if rootpage == Some(0) || words == ["CREATE", "VIRTUAL", "TABLE"] {
            return Ok(Some(format!("The {what} holds CREATE VIRTUAL TABLE {name}, and Konae runs no virtual tables. Drop it before importing.")));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The editor runs the same file in src/kernel/sql-rules.test.ts.
    #[test]
    fn statements_and_schemas_are_refused_with_the_editors_words() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("sql-rule-cases.json")).unwrap();
        assert!(cases.len() >= 12);
        for case in cases {
            let outcome = if let Some(schema) = case["schema"].as_str() {
                schema_refusal(schema).map_or(Ok(()), Err)
            } else {
                let program: super::super::ActionProgram =
                    serde_json::from_value(serde_json::json!({"version": 1, "actions": case["actions"]})).unwrap();
                let mut names = program.actions.keys().collect::<Vec<_>>();
                names.sort();
                names.into_iter().try_for_each(|name| super::super::validate_action(name, &program.actions[name]))
            };
            crate::refusal::assert_case(&case, outcome);
        }
    }

    fn hand_edited(rows: &str, sql: &str, rootpage: i64) -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(&format!("CREATE VIRTUAL TABLE {rows} USING fts5(body)")).unwrap();
        db.execute_batch("PRAGMA writable_schema=ON").unwrap();
        db.execute("UPDATE sqlite_master SET sql = ?1, rootpage = ?2 WHERE name = ?3", rusqlite::params![sql, rootpage, rows]).unwrap();
        db
    }

    #[test]
    fn a_table_row_is_accepted_only_when_it_reads_as_an_ordinary_table() {
        for sql in [
            "CREATE \u{FEFF}VIRTUAL TABLE notes USING fts5(body)",
            "CREATE /*x*/ VIRTUAL TABLE notes USING fts5(body)",
            "CREATE\u{a0}VIRTUAL TABLE notes USING fts5(body)",
            "\u{FEFF}CREATE VIRTUAL TABLE notes USING fts5(body)",
            "CREATE VIRTUAL TABLE notes USING fts5(body)",
        ] {
            let db = hand_edited("notes", sql, 2);
            let refusal = stored_virtual_table(&db, "accounts component").unwrap().unwrap_or_else(|| panic!("{sql:?} was accepted"));
            assert!(refusal.contains("notes") && refusal.contains("Drop it before importing."), "{refusal}");
        }
        let db = hand_edited("notes", "CREATE TABLE notes(body)", 0);
        assert!(stored_virtual_table(&db, "usage component").unwrap().is_some(), "a table row with no b-tree of its own is virtual");
    }

    #[test]
    fn the_tables_sqlite_and_the_kernel_write_are_accepted() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS a(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT);
             INSERT INTO a(v) VALUES('x');
             create table \"b c\"(k TEXT PRIMARY KEY, v) WITHOUT ROWID;
             CREATE TABLE s(k INTEGER PRIMARY KEY, v TEXT) STRICT;
             CREATE TABLE IF NOT EXISTS main.q(k, v);
             CREATE INDEX a_v ON a(v);
             ANALYZE;",
        )
        .unwrap();
        let names: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('sqlite_sequence', 'sqlite_stat1')", [], |row| row.get(0)).unwrap();
        assert_eq!(names, 2, "the internal tables are in the file the test checks");
        assert_eq!(stored_virtual_table(&db, "database").unwrap(), None);
    }
}
