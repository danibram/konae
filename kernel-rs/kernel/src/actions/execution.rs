use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};

use crate::{
    ActionDefinition,
    actions::{
        QUERY_RESULT_TOO_LARGE, ResultBudget, ResultTooLarge, bounded_row, bounded_rows, sql_json,
    },
    app_db, notify,
};

/// Runs an action's statement on a blocking thread while the caller holds
/// the Deployment's database lock. The statement deadline is checked between
/// SQLite steps, and one step, such as `instr` or `replace` over two large
/// values, can run past it. On a blocking thread that delays only this
/// Deployment, never the async workers every other app shares.
/// What a statement did. `answer` is what the caller reads. `rows` are the
/// changed rows a mutation's notice reads back through the Kernel's own
/// RETURNING clause (ADR-0012); they never reach the answer.
pub struct Ran {
    pub answer: Value,
    pub rows: Vec<serde_json::Map<String, Value>>,
    pub sqlite: std::time::Duration,
}

impl Ran {
    /// What a notice reads of this run, at `now` in milliseconds.
    pub fn changed(&self, now: u64) -> notify::Changed<'_> {
        notify::Changed {
            count: self.answer["changes"].as_u64().unwrap_or(0),
            rows: &self.rows,
            now,
        }
    }
}

pub async fn run_action(
    path: PathBuf,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
) -> Result<Ran> {
    run_action_holding(path, action, params, None).await
}

/// Like `run_action`, for a statement that holds one of the Deployment's
/// slots (`Guards::statement_permit`). The permit moves into the blocking
/// thread, so the slot stays held until the statement ends, whether or not
/// the caller still waits for it.
pub async fn run_action_in_slot(
    path: PathBuf,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Ran> {
    run_action_holding(path, action, params, Some(permit)).await
}

async fn run_action_holding(
    path: PathBuf,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Result<Ran> {
    let action = action.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if action.kind == "query" {
            let connection = app_db::open(&path)?;
            let started = std::time::Instant::now();
            run_query_on_connection(&connection, &action, params).map(|rows| Ran {
                answer: Value::from(rows),
                rows: Vec::new(),
                sqlite: started.elapsed(),
            })
        } else {
            run_mutation(&path, &action, params)
        }
    })
    .await
    .map_err(anyhow::Error::from)?
}

pub fn run_query(
    path: &Path,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
) -> Result<Vec<Value>> {
    let connection = app_db::open(path)?;
    run_query_on_connection(&connection, action, params)
}

pub fn run_query_on_connection(
    connection: &Connection,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
) -> Result<Vec<Value>> {
    let mut statement = connection.prepare_cached(&action.statement)?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut budget = ResultBudget::default();
    let rows = bounded_rows(statement.query_map(params_from_iter(params), |row| {
        bounded_row(row, &names, &mut budget)?
            .ok_or_else(|| rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)))
    })?)?;
    if rows.len() > 1000 {
        anyhow::bail!("Action returned more than 1000 rows.");
    }
    Ok(rows)
}

pub fn run_mutation(path: &Path, action: &ActionDefinition, params: Vec<SqlValue>) -> Result<Ran> {
    let mut connection = app_db::open(path)?;
    let transaction = connection.transaction()?;
    let ran = run_mutation_on_connection(&transaction, action, params)?;
    transaction.commit()?;
    Ok(ran)
}

pub fn run_mutation_on_connection(
    connection: &Connection,
    action: &ActionDefinition,
    params: Vec<SqlValue>,
) -> Result<Ran> {
    let sqlite_started = std::time::Instant::now();
    let columns = action
        .notify
        .as_ref()
        .map(|_| notify::returning("", action))
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut budget = ResultBudget::default();
    let changes = if columns.is_empty() {
        connection
            .prepare_cached(&action.statement)?
            .execute(params_from_iter(params))? as u64
    } else {
        let mut statement =
            connection.prepare_cached(&notify::with_returning(&action.statement, &columns))?;
        let mut cursor = statement.query(params_from_iter(params))?;
        let mut serialized_bytes = 2usize;

        while let Some(row) = cursor.next()? {
            if rows.len() < notify::MAX_RETURNED_ROWS {
                let mut object = serde_json::Map::new();
                if !budget.begin_row(2) {
                    anyhow::bail!("{QUERY_RESULT_TOO_LARGE}");
                }
                for (index, column) in columns.iter().enumerate() {
                    let value = row.get_ref(index)?;
                    if !budget.spend_column(column, value, index == 0) {
                        anyhow::bail!("{QUERY_RESULT_TOO_LARGE}");
                    }
                    object.insert(column.clone(), sql_json(value));
                }
                serialized_bytes = serialized_bytes
                    .saturating_add(usize::from(!rows.is_empty()))
                    .saturating_add(serde_json::to_vec(&object)?.len());
                if serialized_bytes > 2 * 1024 * 1024 {
                    anyhow::bail!("{QUERY_RESULT_TOO_LARGE}");
                }
                rows.push(object);
            }
        }
        drop(cursor);
        drop(statement);
        connection.changes()
    };
    let last_insert_id = connection.last_insert_rowid();
    Ok(Ran {
        answer: json!({"changes": changes, "lastInsertId": last_insert_id}),
        rows,
        sqlite: sqlite_started.elapsed(),
    })
}
