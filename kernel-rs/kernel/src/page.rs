use crate::{ActionDefinition, ActionProgram};
use serde_json::Value;
use std::collections::HashSet;

pub fn validate_action(name: &str, action: &ActionDefinition) -> Result<(), crate::refusal::Refusal> {
    let Some(page) = action.page.as_ref() else { return Ok(()) };
    if action.kind != "query" || action.access != "anonymous" {
        return Err(crate::refusal::refuse("page.query", format!("Action {name} declares a page path, so it must be an anonymous query.")));
    }
    if action.http.is_some() {
        return Err(crate::refusal::refuse("page.route-conflict", format!("Action {name} declares both http and page. Declare one.")));
    }
    if selected_columns(&action.statement).is_some_and(|columns| !columns.contains("title")) {
        return Err(crate::refusal::refuse("page.title", format!("Action {name} has a page query that does not return a title column.")));
    }
    let invalid = || crate::refusal::refuse("page.invalid", format!("Action {name} has an invalid page binding."));
    let Some(record) = page.as_object() else { return Err(invalid()) };
    if record.keys().any(|key| !["path", "file", "list"].contains(&key.as_str())) { return Err(invalid()) }
    let Some(path) = record.get("path").and_then(Value::as_str) else { return Err(invalid()) };
    let Some(file) = record.get("file").and_then(Value::as_str) else { return Err(invalid()) };
    if path.len() > 200 || !path.starts_with('/') || path[1..].split('/').any(|part| {
        part.is_empty() || part == "." || part == ".." || if let Some(key) = part.strip_prefix(':') {
            key.is_empty() || !key.as_bytes()[0].is_ascii_alphabetic() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        } else {
            !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        }
    }) {
        return Err(crate::refusal::refuse("page.path-invalid", format!("Action {name} has an invalid page path. Use a leading \"/\" and segments of letters, digits, \".\", \"_\", \"~\" or \"-\", with parameters such as \":slug\" after the first segment.")));
    }
    let parts = path[1..].split('/').collect::<Vec<_>>();
    let mut params = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if part.starts_with(':') {
            let key = &part[1..];
            if index == 0 || key.is_empty() || !key.as_bytes()[0].is_ascii_alphabetic() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
                return Err(crate::refusal::refuse("page.path-invalid", format!("Action {name} has an invalid page path. Use a leading \"/\" and segments of letters, digits, \".\", \"_\", \"~\" or \"-\", with parameters such as \":slug\" after the first segment.")));
            }
            params.push(key);
        }
    }
    let inputs = action.bindings.iter().filter(|binding| binding.source == "input").filter_map(|binding| binding.key.as_deref()).collect::<Vec<_>>();
    if params.iter().collect::<HashSet<_>>().len() != params.len() || params.len() != inputs.len() || params.iter().any(|param| !inputs.contains(param)) {
        return Err(crate::refusal::refuse("page.bindings", format!("Action {name} declares page parameters that do not match its input bindings.")));
    }
    if !safe_file(file) { return Err(invalid()) }
    if let Some(list) = record.get("list") {
        if !list.as_str().is_some_and(|value| value.split('.').count() >= 2 && value.split('.').all(|word| !word.is_empty() && word.as_bytes()[0].is_ascii_lowercase() && word.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))) {
            return Err(crate::refusal::refuse("page.list-invalid", format!("Action {name} has an invalid page list. Name an anonymous query with no input bindings.")));
        }
    }
    Ok(())
}

pub fn check_program(program: &ActionProgram) -> Result<(), crate::refusal::Refusal> {
    let mut paths = std::collections::HashMap::new();
    let mut files = std::collections::HashMap::<&str, (&str, usize)>::new();
    let mut names = program.actions.keys().collect::<Vec<_>>();
    names.sort();
    for name in &names {
        let Some(page) = program.actions[*name].page.as_ref() else { continue };
        let Some(path) = page.get("path").and_then(Value::as_str) else { continue };
        let Some(file) = page.get("file").and_then(Value::as_str) else { continue };
        if file == "index.html" {
            return Err(crate::refusal::refuse("page.file-invalid", format!("Action {name} names index.html as a page file, but index.html is the app entry page.")));
        }
        let depth = path.split('/').filter(|segment| !segment.is_empty()).count();
        if let Some((earlier, earlier_depth)) = files.get(file) {
            if *earlier_depth != depth {
                return Err(crate::refusal::refuse("page.file-depth", format!("Actions {earlier} and {name} name the same page file at different path depths.")));
            }
        } else {
            files.insert(file, (name.as_str(), depth));
        }
        let key = path.split('/').map(|segment| if segment.starts_with(':') { ":" } else { segment }).collect::<Vec<_>>().join("/");
        if let Some(earlier) = paths.get(&key) {
            return Err(crate::refusal::refuse("page.duplicate", format!("Actions {earlier} and {name} both bind page path {path}.")));
        }
        paths.insert(key, name.as_str());
    }
    for (name, action) in &program.actions {
        let Some(list) = action.page.as_ref().and_then(|page| page.get("list")).and_then(Value::as_str) else { continue };
        let Some(target) = program.actions.get(list) else {
            return Err(crate::refusal::refuse("page.list-undeclared", format!("Action {name} names {list} as its page list, which is not declared.")));
        };
        if target.kind != "query" || target.access != "anonymous" || target.bindings.iter().any(|binding| binding.source == "input") {
            return Err(crate::refusal::refuse("page.list-invalid", format!("Action {name} has an invalid page list. Name an anonymous query with no input bindings.")));
        }
        let Some(columns) = selected_columns(&target.statement) else { continue };
        let path = action.page.as_ref().and_then(|page| page.get("path")).and_then(Value::as_str).unwrap_or("");
        let mut expected = path.trim_start_matches('/').split('/').filter_map(|segment| segment.strip_prefix(':')).map(str::to_owned).collect::<HashSet<_>>();
        if columns.contains("updated") { expected.insert("updated".into()); }
        if columns != expected {
            return Err(crate::refusal::refuse("page.list-columns", format!("Action {name} names {list} as its page list, whose columns must match the page parameters and optional updated.")));
        }
    }
    Ok(())
}

fn selected_columns(statement: &str) -> Option<HashSet<String>> {
    let tokens = crate::sql_tokens(statement);
    let select = tokens.iter().position(|token| token.depth == 0 && token.is_word("SELECT"))?;
    let from = tokens.iter().enumerate().skip(select + 1).find(|(_, token)| token.depth == 0 && token.is_any(&["FROM", "WHERE", "GROUP", "ORDER", "LIMIT", "UNION"])).map_or(tokens.len(), |(index, _)| index);
    let projection = &tokens[select + 1..from];
    if projection.len() == 1 && projection[0].depth == 0 && projection[0].is_symbol("*") { return None }
    let mut columns = HashSet::new();
    let mut start = 0;
    for end in 0..=projection.len() {
        if end != projection.len() && !(projection[end].depth == 0 && projection[end].is_symbol(",")) { continue }
        let expression = &projection[start..end];
        let alias = expression.iter().rposition(|token| token.depth == 0 && token.is_word("AS")).and_then(|index| expression.get(index + 1));
        let column = alias.or_else(|| expression.last());
        if let Some(column) = column {
            if matches!(column.kind, crate::SqlTokKind::Word | crate::SqlTokKind::Ident) {
                columns.insert(column.text.to_ascii_lowercase());
            }
        }
        start = end + 1;
    }
    Some(columns)
}

fn safe_file(file: &str) -> bool {
    !file.is_empty() && !file.starts_with('/') && file.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}
