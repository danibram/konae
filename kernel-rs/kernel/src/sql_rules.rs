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

/// The refusal for an app database that arrives holding a trigger or a view,
/// in an import, an initial database or a package. schema.sql cannot create
/// either, so one in a file came from somewhere the Gate never read.
pub fn stored_trigger_or_view(db: &rusqlite::Connection) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    db.query_row(
        "SELECT upper(type), name FROM sqlite_master WHERE type IN ('trigger', 'view') ORDER BY type, name LIMIT 1",
        [],
        |row| Ok(format!("The database holds CREATE {} {}, and Konae runs no triggers or views. Drop it before importing.", row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
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
}
