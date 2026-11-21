use rusqlite::types::Value as SqlValue;
use serde_json::Value;

use crate::{ActionDefinition, now_ms};

/// Binds an action's parameters. `actor_id` is the caller's ID: a person's
/// or an API key's.
pub fn resolve_bindings(
    action: &ActionDefinition,
    input: &Value,
    actor_id: Option<&str>,
) -> std::result::Result<Vec<SqlValue>, String> {
    let object = input
        .as_object()
        .ok_or_else(|| "Action input must be an object.".to_string())?;
    let accepted = action
        .bindings
        .iter()
        .filter_map(|binding| {
            if binding.source == "input" {
                binding.key.as_ref()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if let Some(key) = object.keys().find(|key| !accepted.contains(key)) {
        return Err(format!("Unexpected action input: {key}"));
    }
    action
        .bindings
        .iter()
        .map(|binding| match binding.source.as_str() {
            "actor" if binding.field.as_deref() == Some("id") => actor_id
                .map(|id| SqlValue::Text(id.to_owned()))
                .ok_or_else(|| "This action requires an authenticated actor.".to_string()),
            "kernel" if binding.field.as_deref() == Some("now") => {
                Ok(SqlValue::Integer(now_ms() as i64))
            }
            "literal" => json_sql(binding.value.as_ref().unwrap_or(&Value::Null)),
            "input" => {
                let key = binding
                    .key
                    .as_ref()
                    .ok_or_else(|| "Input binding has no key.".to_string())?;
                if binding.nullable && object.get(key).is_none_or(Value::is_null) {
                    return Ok(SqlValue::Null);
                }
                let value = object
                    .get(key)
                    .ok_or_else(|| format!("Missing action input: {key}"))?;
                if binding.value_type.as_deref() == Some("string") {
                    let text = value
                        .as_str()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| format!("{key} must be a non-empty string."))?;

                    if text.encode_utf16().count() > binding.max_length.unwrap_or(10_000) {
                        return Err(format!("{key} is too long."));
                    }
                    Ok(SqlValue::Text(text.to_owned()))
                } else if binding.value_type.as_deref() == Some("number") {

                    value
                        .as_i64()
                        .map(SqlValue::Integer)
                        .or_else(|| value.as_f64().map(SqlValue::Real))
                        .ok_or_else(|| format!("{key} must be a number."))
                } else if binding.value_type.as_deref() == Some("boolean") {
                    value
                        .as_bool()
                        .map(|value| SqlValue::Integer(value as i64))
                        .ok_or_else(|| format!("{key} must be a boolean."))
                } else {
                    Err(format!("Unsupported input type for {key}."))
                }
            }
            _ => Err(format!("Unsupported binding source: {}", binding.source)),
        })
        .collect()
}

pub fn json_sql(value: &Value) -> std::result::Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        Value::Bool(value) => Ok(SqlValue::Integer(*value as i64)),
        Value::Number(value) => value
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| value.as_f64().map(SqlValue::Real))
            .ok_or_else(|| "Invalid number.".into()),
        _ => Err("Literal values must be scalar.".into()),
    }
}
