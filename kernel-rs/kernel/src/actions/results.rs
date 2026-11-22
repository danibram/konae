use anyhow::Result;
use serde_json::{Value, json};

pub(crate) const QUERY_RESULT_TOO_LARGE: &str = "Query result exceeds 2 MiB or 1000 rows.";

/// What an oversized action result answers when a column is read.
#[derive(Debug)]
pub(crate) struct ResultTooLarge;

impl std::fmt::Display for ResultTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(QUERY_RESULT_TOO_LARGE)
    }
}

impl std::error::Error for ResultTooLarge {}

/// What a result has cost as JSON so far, counted as its columns are read.
pub struct ResultBudget {
    bytes: usize,
    rows: usize,
}

impl ResultBudget {
    pub(crate) fn reserve(&mut self, bytes: usize) -> bool {
        self.bytes = self.bytes.saturating_add(bytes);
        self.within_limit()
    }

    pub(crate) fn begin_row(&mut self, row_overhead: usize) -> bool {
        self.bytes = self.bytes
            .saturating_add(usize::from(self.rows > 0))
            .saturating_add(row_overhead);
        self.rows = self.rows.saturating_add(1);
        self.within_limit()
    }

    pub(crate) fn spend_column(&mut self, name: &str, value: rusqlite::types::ValueRef<'_>, first: bool) -> bool {
        self.bytes = self.bytes
            .saturating_add(usize::from(!first))
            .saturating_add(json_string_cost(name.as_bytes()))
            .saturating_add(1)
            .saturating_add(sql_json_cost(value));
        self.within_limit()
    }

    pub(crate) fn spend_value(&mut self, value: rusqlite::types::ValueRef<'_>) -> bool {
        self.bytes = self.bytes.saturating_add(sql_json_cost(value));
        self.within_limit()
    }

    fn within_limit(&self) -> bool {
        self.bytes <= 2 * 1024 * 1024
    }

    /// Adds the serialized cost of a value without an object key.
    pub fn spend(&mut self, value: rusqlite::types::ValueRef<'_>) -> bool {
        self.spend_value(value)
    }
}

pub(crate) fn json_string_cost(bytes: &[u8]) -> usize {
    let mut cost = 2usize;
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                cost = cost.saturating_add(valid.chars().map(json_char_cost).sum::<usize>());
                break;
            }
            Err(error_value) => {
                let valid = &remaining[..error_value.valid_up_to()];
                cost = cost.saturating_add(
                    std::str::from_utf8(valid)
                        .expect("Utf8Error marks the prefix as valid")
                        .chars()
                        .map(json_char_cost)
                        .sum::<usize>(),
                );
                remaining = &remaining[error_value.valid_up_to()..];
                let invalid_bytes = error_value.error_len().unwrap_or(remaining.len()).max(1);
                cost = cost.saturating_add(invalid_bytes.saturating_mul(3));
                remaining = &remaining[invalid_bytes..];
            }
        }
    }
    cost
}

fn json_char_cost(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '\u{0}'..='\u{1f}' => 6,
        _ => character.len_utf8(),
    }
}

fn sql_json_cost(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => 4,
        ValueRef::Integer(value) => value.to_string().len(),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(|number| number.to_string().len())
            .unwrap_or(4),
        ValueRef::Text(text) => json_string_cost(text),
        ValueRef::Blob(blob) => blob.len().saturating_mul(2).saturating_add(2),
    }
}

pub(crate) fn browse_column_json_cost(column: &Value) -> usize {
    let name = column["name"].as_str().unwrap_or_default();
    let column_type = column["type"].as_str().unwrap_or_default();
    json_string_cost(name.as_bytes())
        .saturating_add(json_string_cost(column_type.as_bytes()))
        .saturating_add(column["defaultValue"].as_str().map(|value| json_string_cost(value.as_bytes())).unwrap_or(4))
        .saturating_add(69)
}

/// A row as a JSON object, or `None` once `budget` is spent. The columns
/// after the one that spent it are not read: SQLite expands a `zeroblob`
/// only when its column is read.
pub fn bounded_row(row: &rusqlite::Row<'_>, names: &[String], budget: &mut ResultBudget) -> rusqlite::Result<Option<Value>> {
    let mut object = serde_json::Map::new();
    if !budget.begin_row(2) {
        return Ok(None);
    }
    for (index, name) in names.iter().enumerate() {
        let value = row.get_ref(index)?;
        if !budget.spend_column(name, value, index == 0) {
            return Ok(None);
        }
        object.insert(name.clone(), sql_json(value));
    }
    Ok(Some(Value::Object(object)))
}

impl Default for ResultBudget {
    fn default() -> Self {
        Self { bytes: 2, rows: 0 }
    }
}

pub fn bounded_rows(rows: impl Iterator<Item = rusqlite::Result<Value>>) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    let mut bytes = 0;
    for row in rows.take(1001) {
        let row = row?;
        bytes += serde_json::to_vec(&row)?.len();
        anyhow::ensure!(bytes <= 2 * 1024 * 1024 && values.len() < 1000, "{QUERY_RESULT_TOO_LARGE}");
        values.push(row);
    }
    Ok(values)
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    /// Columns this thread has turned into JSON, so a test can see where a
    /// result stopped being read.
    pub static COLUMNS_CONVERTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn sql_json(value: rusqlite::types::ValueRef<'_>) -> Value {
    #[cfg(any(test, feature = "testing"))]
    COLUMNS_CONVERTED.with(|count| count.set(count.get() + 1));
    match value {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(value) => json!(value),
        rusqlite::types::ValueRef::Real(value) => json!(value),
        rusqlite::types::ValueRef::Text(value) => {
            Value::String(String::from_utf8_lossy(value).into_owned())
        }
        rusqlite::types::ValueRef::Blob(value) => Value::String(hex::encode(value)),
    }
}
