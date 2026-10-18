//! The refusals on the way from a Database Model to a published database:
//! a release names its accepted model, the database matches that model, and
//! a migration plan changes something. src/lib/model-rules.ts gives the same
//! sentences, and database-model-cases.json holds the cases both test suites
//! run.

use super::refusal::{refuse, Refusal};

/// A release publishes the model the database was migrated to: the edited
/// source is the accepted one, and the accepted one has a revision.
pub(super) fn accepted(source: &str, accepted_source: &str, revision: &str) -> Result<(), Refusal> {
    if source == accepted_source && !revision.is_empty() {
        return Ok(());
    }
    Err(refuse("schema.model-not-accepted", "Publish the accepted database model. Apply its migration before publishing."))
}

/// `matches` is whether the schema the accepted model derives and the schema
/// the database holds compare equal under `mcp::schema_tokens`.
pub(super) fn deployed(matches: bool) -> Result<(), Refusal> {
    if matches {
        return Ok(());
    }
    Err(refuse("schema.model-differs-from-database", "The accepted database model does not match the deployed schema. Apply its migration before publishing."))
}

/// A plan with no statements has nothing to apply.
pub(super) fn planned(statements: &[String]) -> Result<(), Refusal> {
    if !statements.is_empty() {
        return Ok(());
    }
    Err(refuse("schema.no-changes", "The Drizzle model has no schema changes to migrate."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::Value;

    fn database(statements: &Value) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        for sql in statements.as_array().unwrap() {
            db.execute_batch(sql.as_str().unwrap()).unwrap();
        }
        db
    }

    /// src/lib/model-rules.test.ts expects the same tokens from the editor's port.
    #[test]
    fn the_tokens_read_an_added_column_in_the_models_place_and_leave_checks_out() {
        let db = database(&serde_json::json!(["CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, CONSTRAINT short CHECK (length(id) < 9))", "ALTER TABLE notes ADD body TEXT DEFAULT TRUE NOT NULL"]));
        let tokens = super::super::mcp::schema_tokens(&db).unwrap();
        assert_eq!((tokens[0].0.as_str(), tokens[0].1.as_str()), ("table", "notes"));
        assert_eq!(tokens[0].2.join(" "), "create table notes ( body text not null default 1 , id integer primary key autoincrement )");
    }

    /// The editor runs the same file in src/lib/model-rules.test.ts.
    #[test]
    fn database_model_cases_get_the_editors_words() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("database-model-cases.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let outcome = match case["rule"].as_str().unwrap() {
                "accepted" => {
                    let model = &case["model"];
                    accepted(model["source"].as_str().unwrap(), model["acceptedSource"].as_str().unwrap(), model["revision"].as_str().unwrap())
                }
                "deployed" => {
                    let expected = super::super::mcp::schema_tokens(&database(&case["modelSchema"])).unwrap();
                    let actual = super::super::mcp::schema_tokens(&database(&case["databaseSchema"])).unwrap();
                    deployed(expected == actual)
                }
                "planned" => planned(&serde_json::from_value::<Vec<String>>(case["statements"].clone()).unwrap()),
                rule => panic!("{rule} is not a rule"),
            };
            super::super::refusal::assert_case(&case, outcome);
        }
    }
}
