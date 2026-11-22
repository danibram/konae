use crate::{
    ActionDefinition, SqlTokKind, app_data::schema_transform, calls, guard, http, notify,
    page, refusal, schedule, usage,
    actions::{shape as action_shape, sql::unbounded_query_reason, sql_rules, sql_tokens},
};

pub fn validate_action(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    validate_declarations(name, action)?;
    page::validate_action(name, action)?;
    action_shape::check_keys(name, action)
}

pub fn validate_declarations(name: &str, action: &ActionDefinition) -> std::result::Result<(), refusal::Refusal> {
    action_shape::check_action(name, action)?;
    let tokens = sql_tokens(&action.statement);
    if tokens.iter().enumerate().any(|(index, token)| {
        let after_modifier = if token.is_word("UPDATE") && tokens.get(index + 1).is_some_and(|next| next.is_word("OR")) { index + 3 } else { index + 1 };
        let target = if tokens.get(after_modifier + 1).is_some_and(|dot| dot.is_symbol(".")) { tokens.get(after_modifier + 2) } else { tokens.get(after_modifier) };
        token.is_any(&["FROM", "JOIN", "UPDATE", "INTO", "TABLE", "REFERENCES"])
            && target.is_some_and(|target| matches!(target.kind, SqlTokKind::Word | SqlTokKind::Ident) && schema_transform::is_internal_table(&target.text))
    }) {
        return Err(refusal::refuse("schema-transform.statement-refused", format!("Action {name} cannot read or write a reserved Kernel table.")));
    }
    if calls::check_kind(name, action)?.is_some() {
        return Ok(());
    }
    usage::check_signal_action(name, action)?;
    if action.kind == "signal" {
        usage::declaration(name, action)?;
        http::binding(name, action)?;
        guard::declaration(name, action)?;
        notify::declaration(name, action)?;
        schedule::check_subscribe(name, action)?;
        schedule::declaration(name, action)?;
        return Ok(());
    }
    sql_rules::check_statement(name, action)?;
    if action.kind == "query" {
        if let Some(reason) = unbounded_query_reason(&action.statement) {
            return Err(refusal::refuse(
                "query.limit-missing",
                format!("Action {name} {reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one."),
            ));
        }
    }
    usage::declaration(name, action)?;
    http::binding(name, action)?;
    guard::declaration(name, action)?;
    notify::declaration(name, action)?;
    schedule::check_subscribe(name, action)?;
    schedule::declaration(name, action)?;
    Ok(())
}
