use std::path::Path;

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use rusqlite::types::Value as SqlValue;
use serde_json::{Value, json};

use crate::{
    AppState, MAX_APP_DATABASE_BYTES, QUERY_RESULT_TOO_LARGE, ResultBudget, ResultTooLarge,
    SqlTokKind, bounded_row, bounded_rows, browse_column_json_cost, create_backup,
    database_lock, database_path, error, limit_database_size, list_backups, modules, named_lock,
    refusal_error, restore_backup, restrict_database, schema_transform, sql_json, sql_tokens,
};

/// Runs one operation of the owner's database door on a Deployment: inspect,
/// browse, SQL, row edits and backups. `body` is its JSON input.
pub async fn admin_database(state: &AppState, id: &str, body: &[u8]) -> Response {
    let Some(deployment) = state
        .deployments
        .read()
        .await
        .values()
        .find(|d| d.id == id)
        .cloned()
    else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if body.len() > 200_000 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "Database request is too large.");
    }
    let body: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Database request must be valid JSON.",
            );
        }
    };
    let operation = body.get("operation").and_then(Value::as_str).unwrap_or("");
    if !matches!(operation, "list-backups" | "restore-backup") {
        if let Err(refused) = modules::require_on(&deployment.modules, "actions") { return refusal_error(&refused); }
    }
    let _backup_prepared_pin = if operation == "restore-backup" {
        match crate::app_data::prepare_backup(
            state,
            &deployment.id,
            body.get("backupId").and_then(Value::as_str).unwrap_or(""),
        ).await {
            Ok(pin) => Some(pin),
            Err(failure) => return match crate::refusal::of(&failure) {
                Some(refused) => refusal_error(refused),
                None => error(StatusCode::BAD_REQUEST, &failure.to_string()),
            },
        }
    } else { None };
    let _project_guard = database_lock(&state, &format!("project:{}", deployment.project_id)).await;
    let _release_guard = named_lock(state, &format!("publish:{}", deployment.id), operation == "restore-backup").await;
    let path = database_path(&state, &deployment.id);
    let _guard = database_lock(&state, &deployment.id).await;
    match operation {
        "inspect" => match inspect_database(&path) {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        "browse" => match browse_table(&path, &body) {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        "sql" => {
            match run_admin_sql(&path, body.get("sql").and_then(Value::as_str).unwrap_or("")) {
                Ok(result) => Json(json!({"result": result})).into_response(),
                Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
            }
        }
        "update-cell" => admin_update_cell(&path, &body),
        "insert-row" => admin_insert_row(&path, &body),
        "delete-row" => admin_delete_row(&path, &body),
        "list-backups" => match list_backups(&state, &deployment.id).await {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        "create-backup" => match create_backup(
            &state,
            &deployment.id,
            body.get("label")
                .and_then(Value::as_str)
                .unwrap_or("Manual backup"),
        )
        .await
        {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        "restore-backup" => match restore_backup(
            &state,
            &deployment.id,
            body.get("backupId").and_then(Value::as_str).unwrap_or(""),
        )
        .await
        {
            Ok(result) => Json(json!({"result": result})).into_response(),
            Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
        _ => error(StatusCode::BAD_REQUEST, "Unknown database operation."),
    }
}

pub fn inspect_database(path: &Path) -> Result<Vec<Value>> {
    let db = Connection::open(path)?;
    let mut statement = db.prepare(&format!("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND lower(name) NOT IN ({}) ORDER BY name", crate::modules::reserved_sql_names()))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    names.into_iter().map(|name| {
        let columns = table_columns(&db, &name)?;
        let count_sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(&name));
        let count: i64 = db.query_row(&count_sql, [], |row| row.get(0))?;
        Ok(json!({"name": name, "columns": columns.iter().map(|column| column["name"].clone()).collect::<Vec<_>>(), "totalRows": count}))
    }).collect()
}

pub fn table_columns(db: &Connection, table: &str) -> Result<Vec<Value>> {
    anyhow::ensure!(!schema_transform::is_internal_table(table), "Reserved Kernel table.");
    if !table_exists(db, table)? {
        anyhow::bail!("Table not found: {table}");
    }
    let sql = format!("PRAGMA table_info({})", quote_identifier(table));
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map([], |row| Ok(json!({
        "name": row.get::<_, String>(1)?, "type": row.get::<_, String>(2)?,
        "notNull": row.get::<_, i64>(3)? != 0, "defaultValue": row.get::<_, Option<String>>(4)?,
        "primaryKey": row.get::<_, i64>(5)? != 0,
    })))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn table_exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE lower(type)='table' AND lower(name)=lower(?1)",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub fn browse_table(path: &Path, body: &Value) -> Result<Value> {
    let db = Connection::open(path)?;
    restrict_database(&db)?;
    let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
    let columns = table_columns(&db, table)?;
    let names = columns
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect::<Vec<_>>();
    let page_size = body
        .get("pageSize")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 100) as i64;
    let page = body
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(i64::MAX as u64) as i64;
    let sort = body
        .get("sortColumn")
        .and_then(Value::as_str)
        .filter(|s| names.contains(s))
        .map(quote_identifier)
        .unwrap_or_else(|| "rowid".into());
    let direction = if body.get("sortDirection").and_then(Value::as_str) == Some("DESC") {
        "DESC"
    } else {
        "ASC"
    };
    let sql = format!(
        "SELECT rowid AS __konae_rowid, * FROM {} ORDER BY {sort} {direction} LIMIT ?1 OFFSET ?2",
        quote_identifier(table)
    );
    let mut statement = db.prepare(&sql)?;
    let col_names = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut budget = ResultBudget::default();
    let metadata_bytes = columns.iter().fold(128usize, |bytes, column| {
        bytes.saturating_add(browse_column_json_cost(column))
    });
    anyhow::ensure!(budget.reserve(metadata_bytes), "{QUERY_RESULT_TOO_LARGE}");
    let rows = bounded_rows(statement
        .query_map([page_size, page.saturating_mul(page_size)], |row| {
            let mut values = serde_json::Map::new();
            let mut row_id = Value::Null;
            let mut first_value = true;
            if !budget.begin_row(22) {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
            }
            for (i, name) in col_names.iter().enumerate() {
                if name == "__konae_rowid" {
                    let value = row.get_ref(i)?;
                    if !budget.spend_value(value) {
                        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
                    }
                    row_id = sql_json(value);
                } else {
                    let value = row.get_ref(i)?;
                    if !budget.spend_column(name, value, first_value) {
                        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(ResultTooLarge)));
                    }
                    first_value = false;
                    values.insert(name.clone(), sql_json(value));
                }
            }
            Ok(json!({"rowId": row_id, "values": values}))
        })?)?;
    let count_sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(table));
    let total: i64 = db.query_row(&count_sql, [], |row| row.get(0))?;
    Ok(
        json!({"columns": columns, "rows": rows, "totalRows": total, "page": page, "pageSize": page_size}),
    )
}

pub fn run_admin_sql(path: &Path, sql: &str) -> Result<Value> {
    if sql.trim().is_empty() {
        anyhow::bail!("Write a SQL statement first.");
    }
    if sql.len() > 100_000 {
        anyhow::bail!("SQL script is too large.");
    }
    let tokens = sql_tokens(sql);
    if tokens.windows(3).any(|tokens| tokens[0].is_word("RENAME") && tokens[1].is_word("TO") && matches!(tokens[2].kind, SqlTokKind::Word | SqlTokKind::Ident) && schema_transform::is_internal_table(&tokens[2].text)) {
        anyhow::bail!("SQL cannot rename a table to a reserved Kernel table.");
    }
    let db = Connection::open(path)?;
    restrict_database(&db)?;
    let started = std::time::Instant::now();
    let first = sql
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("SQL")
        .to_ascii_uppercase();
    if ["SELECT", "PRAGMA", "EXPLAIN", "WITH"].contains(&first.as_str()) {
        let mut statement = db.prepare(sql)?;
        let names = statement
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        let mut result_bytes = 0;
        let mut budget = ResultBudget::default();
        while let Some(row) = rows.next()? {
            let Some(record) = bounded_row(row, &names, &mut budget)? else {
                anyhow::bail!("SQL result exceeds 2 MiB.");
            };
            values.push(record);
            result_bytes += serde_json::to_vec(values.last().unwrap())?.len();
            if result_bytes > 2 * 1024 * 1024 {
                anyhow::bail!("SQL result exceeds 2 MiB.");
            }
            if values.len() > 1000 {
                anyhow::bail!("SQL returned more than 1000 rows.");
            }
        }
        Ok(
            json!({"columns": names, "rows": values, "changes": 0, "elapsedMs": started.elapsed().as_millis(), "statement": first}),
        )
    } else {
        let mut db = db;
        let tx = db.transaction()?;
        tx.execute_batch(sql)?;
        let changes = tx.changes();
        tx.commit()?;
        Ok(
            json!({"columns": [], "rows": [], "changes": changes, "elapsedMs": started.elapsed().as_millis(), "statement": first}),
        )
    }
}

pub fn admin_update_cell(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        let column = body.get("columnName").and_then(Value::as_str).unwrap_or("");
        let names = table_columns(&db, table)?;
        if !names.iter().any(|c| c["name"].as_str() == Some(column)) {
            anyhow::bail!("Unknown column.");
        }
        let sql = format!(
            "UPDATE {} SET {}=?1 WHERE rowid=?2",
            quote_identifier(table),
            quote_identifier(column)
        );
        db.execute(
            &sql,
            (
                value_to_sql(body.get("value"))?,
                value_to_sql(body.get("rowId"))?,
            ),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn admin_insert_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        let columns = table_columns(&db, table)?;
        let values = body
            .get("values")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let names = values.keys().cloned().collect::<Vec<_>>();
        if names
            .iter()
            .any(|n| !columns.iter().any(|c| c["name"].as_str() == Some(n)))
        {
            anyhow::bail!("Unknown column.");
        }
        if names.is_empty() {
            db.execute(
                &format!("INSERT INTO {} DEFAULT VALUES", quote_identifier(table)),
                [],
            )?;
        } else {
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_identifier(table),
                names
                    .iter()
                    .map(|n| quote_identifier(n))
                    .collect::<Vec<_>>()
                    .join(","),
                (1..=names.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let params = names
                .iter()
                .map(|n| value_to_sql(values.get(n)))
                .collect::<Result<Vec<_>>>()?;
            db.execute(&sql, params_from_iter(params))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn admin_delete_row(path: &Path, body: &Value) -> Response {
    let result = (|| -> Result<()> {
        let db = Connection::open(path)?;
        limit_database_size(&db, MAX_APP_DATABASE_BYTES)?;
        let table = body.get("tableName").and_then(Value::as_str).unwrap_or("");
        table_columns(&db, table)?;
        db.execute(
            &format!("DELETE FROM {} WHERE rowid=?1", quote_identifier(table)),
            [value_to_sql(body.get("rowId"))?],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"result":{"ok":true}})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

pub fn value_to_sql(value: Option<&Value>) -> Result<SqlValue> {
    let Some(value) = value else {
        return Ok(SqlValue::Null);
    };
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::String(v) => Ok(SqlValue::Text(v.clone())),
        Value::Bool(v) => Ok(SqlValue::Integer(*v as i64)),
        Value::Number(v) => v
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| v.as_f64().map(SqlValue::Real))
            .ok_or_else(|| anyhow::anyhow!("Invalid number.")),
        _ => anyhow::bail!("Unsupported SQLite value."),
    }
}
