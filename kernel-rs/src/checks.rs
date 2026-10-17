//! schema.sql may add CHECK constraints that a Database Model cannot state, and
//! Drizzle rebuilds a table from the model alone (`CREATE TABLE __new_x`, copy,
//! drop, rename). The statements a migration runs carry each CHECK over from
//! the live table's SQL. `carryChecks` in src/lib/carry-checks.ts does the same
//! for the editor's preview, and check-carry-cases.json holds the cases both
//! test suites run.

use super::refusal::{refuse, Refusal};
use super::{sql_tokens, SqlTok, SqlTokKind};

struct Check {
    owner: Option<String>,
    text: String,
    names: Vec<String>,
}

#[derive(Default)]
struct Table {
    columns: Vec<String>,
    checks: Vec<Check>,
    /// The char offset of the `)` that closes the definitions.
    close: Option<usize>,
}

const CONSTRAINT_WORDS: [&str; 5] = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"];

/// The statements to run for `statements`, Drizzle's plan, against a database
/// whose tables `live_table` reads. A rebuilt table gets the live table's
/// CHECKs, as written. A column's own CHECK leaves with the column. Any other
/// CHECK that names a column the plan removes refuses the whole migration.
/// Drizzle's `PRAGMA foreign_keys` lines are left out: inside the transaction
/// a migration runs in, SQLite ignores them, and the Kernel's authorizer
/// refuses every pragma a migration does not need.
pub(super) fn carry_checks(statements: &[String], live_table: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, Refusal> {
    let mut renames = std::collections::HashMap::new();
    for statement in statements {
        let tokens = sql_tokens(statement);
        if is_word(&tokens, 0, "ALTER") && is_word(&tokens, 1, "TABLE") && is_word(&tokens, 3, "RENAME") && is_word(&tokens, 4, "TO") && tokens.len() > 5 {
            renames.insert(name_of(&tokens[2]).to_lowercase(), name_of(&tokens[5]));
        }
    }
    let mut result = Vec::with_capacity(statements.len());
    for statement in statements {
        let tokens = sql_tokens(statement);
        if is_word(&tokens, 0, "PRAGMA") && tokens.get(1).is_some_and(|token| name_of(token).eq_ignore_ascii_case("foreign_keys")) {
            continue;
        }
        if is_word(&tokens, 0, "CREATE") && is_word(&tokens, 1, "TABLE") {
            let at = if is_word(&tokens, 2, "IF") { 5 } else { 2 };
            let target = tokens.get(at).and_then(|token| renames.get(&name_of(token).to_lowercase()));
            if let Some((target, live)) = target.and_then(|target| live_table(target).map(|live| (target, live))) {
                let before = read_table(&live);
                let after = read_table(statement);
                let kept: Vec<String> = after.columns.iter().map(|column| column.to_lowercase()).collect();
                let mut carried = Vec::new();
                for check in &before.checks {
                    if check.owner.as_ref().is_some_and(|owner| !kept.contains(&owner.to_lowercase())) {
                        continue;
                    }
                    if let Some(removed) = removed_name(check, &before.columns, &kept) {
                        return Err(refusal(target, &removed));
                    }
                    carried.push(check.text.as_str());
                }
                if let (false, Some(close)) = (carried.is_empty(), after.close) {
                    let chars: Vec<char> = statement.chars().collect();
                    let head: String = chars[..close].iter().collect();
                    let tail: String = chars[close..].iter().collect();
                    result.push(format!("{},\n\t{}\n{}", head.trim_end(), carried.join(",\n\t"), tail));
                    continue;
                }
            }
        }
        if is_word(&tokens, 0, "ALTER") && is_word(&tokens, 1, "TABLE") && is_word(&tokens, 3, "DROP") {
            let at = if is_word(&tokens, 4, "COLUMN") { 5 } else { 4 };
            let table = name_of(&tokens[2]);
            if let (Some(column), Some(live)) = (tokens.get(at), live_table(&table)) {
                let column = name_of(column).to_lowercase();
                let before = read_table(&live);
                let kept: Vec<String> = before.columns.iter().map(|name| name.to_lowercase()).filter(|name| *name != column).collect();
                for check in &before.checks {
                    if check.owner.as_ref().is_some_and(|owner| owner.to_lowercase() == column) {
                        continue;
                    }
                    if let Some(removed) = removed_name(check, &before.columns, &kept) {
                        return Err(refusal(&table, &removed));
                    }
                }
            }
        }
        result.push(statement.clone());
    }
    Ok(result)
}

fn refusal(table: &str, column: &str) -> Refusal {
    refuse("schema.check-names-removed-column", format!("Table {table} has a CHECK that names column {column}, which this migration removes. Rebuild {table} without that CHECK in the SQL console, or keep {column}, then apply the migration again."))
}

/// The first column of the old table that `check` names and the new table
/// lacks, as the old table writes it.
fn removed_name(check: &Check, columns: &[String], kept: &[String]) -> Option<String> {
    check.names.iter().find_map(|name| {
        columns.iter().find(|column| column.eq_ignore_ascii_case(name))
            .filter(|column| !kept.contains(&column.to_lowercase()))
            .cloned()
    })
}

/// The columns and CHECKs of a CREATE TABLE statement, and where the `)` that
/// closes its definitions is.
fn read_table(sql: &str) -> Table {
    let chars: Vec<char> = sql.chars().collect();
    let tokens = sql_tokens(sql);
    let mut table = Table::default();
    let Some(open) = tokens.iter().position(|token| token.is_symbol("(") && token.depth == 0) else { return table };
    let mut start = open + 1;
    for index in open + 1..tokens.len() {
        let token = &tokens[index];
        let closes = token.is_symbol(")") && token.depth == 0;
        if closes || (token.is_symbol(",") && token.depth == 1) {
            read_definition(&chars, &tokens[start..index], &mut table);
            start = index + 1;
            if closes {
                table.close = Some(token.start);
                break;
            }
        }
    }
    table
}

fn read_definition(chars: &[char], tokens: &[SqlTok], table: &mut Table) {
    let Some(first) = tokens.first() else { return };
    let owner = if first.is_any(&CONSTRAINT_WORDS) { None } else { Some(name_of(first)) };
    if let Some(owner) = &owner {
        table.columns.push(owner.clone());
    }
    let mut index = 0;
    while index < tokens.len() {
        if !(tokens[index].is_word("CHECK") && tokens[index].depth == 1 && tokens.get(index + 1).is_some_and(|open| open.is_symbol("("))) {
            index += 1;
            continue;
        }
        let Some(close) = (index + 2..tokens.len()).find(|&at| tokens[at].is_symbol(")") && tokens[at].depth == 1) else { return };
        let start = if index >= 2 && tokens[index - 2].is_word("CONSTRAINT") { tokens[index - 2].start } else { tokens[index].start };
        let names = tokens[index + 2..close].iter()
            .filter(|token| matches!(token.kind, SqlTokKind::Word | SqlTokKind::Ident))
            .map(|token| token.text.clone())
            .collect();
        table.checks.push(Check { owner: owner.clone(), text: chars[start..tokens[close].end].iter().collect(), names });
        index = close + 1;
    }
}

fn is_word(tokens: &[SqlTok], at: usize, word: &str) -> bool {
    tokens.get(at).is_some_and(|token| token.is_word(word))
}

fn name_of(token: &SqlTok) -> String {
    match token.kind {
        SqlTokKind::Word | SqlTokKind::Ident => token.text.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The editor runs the same file in src/lib/carry-checks.test.ts.
    #[test]
    fn a_migration_carries_the_live_checks_with_the_editors_words() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("check-carry-cases.json")).unwrap();
        assert!(cases.len() >= 11);
        for case in cases {
            let label = case["case"].as_str().unwrap();
            let statements: Vec<String> = serde_json::from_value(case["statements"].clone()).unwrap();
            let outcome = carry_checks(&statements, |name| case["tables"][name].as_str().map(str::to_owned));
            if let Ok(statements) = &outcome {
                assert_eq!(statements, &serde_json::from_value::<Vec<String>>(case["expected"].clone()).unwrap(), "{label}");
            }
            crate::refusal::assert_case(&case, outcome);
        }
    }
}
