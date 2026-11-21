use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::Settings;

pub(crate) fn registry() -> &'static Value {
    static REGISTRY: OnceLock<Value> = OnceLock::new();
    REGISTRY.get_or_init(|| serde_json::from_str(include_str!("../modules.json")).expect("modules.json is generated JSON"))
}

pub fn defaults() -> BTreeMap<String, Value> {
    let mut modules = BTreeMap::new();
    for (name, description) in registry()["modules"].as_object().into_iter().flatten().filter(|(_, description)| description["switch"] == true) {
        let mut state = Map::new();
        state.insert("on".into(), Value::Bool(false));
        for option in description["options"].as_array().into_iter().flatten() {
            if let (Some(key), Some(default)) = (option["key"].as_str(), option.get("default")) {
                state.insert(key.to_owned(), default.clone());
            }
        }
        modules.insert(name.clone(), Value::Object(state));
    }
    modules
}

pub fn auth_enabled_defaults() -> BTreeMap<String, Value> {
    let mut modules = defaults();
    modules.get_mut("auth").expect("auth is switchable")["on"] = json!(true);
    modules
}

pub fn initial_for_release(initial: Option<&BTreeMap<String, Value>>, uses_files: bool, uses_auth: bool, uses_http: bool, uses_calls: bool) -> BTreeMap<String, Value> {
    let mut modules = initial.cloned().unwrap_or_else(defaults);
    complete(&mut modules);
    if initial.is_none_or(|state| !state.contains_key("files")) && uses_files {
        modules.get_mut("files").expect("files is switchable")["on"] = json!(true);
    }
    if initial.is_none_or(|state| !state.contains_key("auth")) && (uses_auth || uses_files) {
        modules.get_mut("auth").expect("auth is switchable")["on"] = json!(true);
    }
    if initial.is_none_or(|state| !state.contains_key("http")) && uses_http { modules.get_mut("http").expect("http is switchable")["on"] = json!(true); }
    if initial.is_none_or(|state| !state.contains_key("calls")) && uses_calls { modules.get_mut("calls").expect("calls is switchable")["on"] = json!(true); }
    modules
}

pub fn artifact_uses_files<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"miniapp.files"))
}

pub fn sources_use_actions<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"miniapp.actions") || serde_json::from_slice::<Value>(bytes).ok().and_then(|value| value["actions"].as_object().map(|actions| !actions.is_empty())).unwrap_or(false))
}

pub fn named_sources_use_actions<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> bool {
    files.into_iter().any(|(name, bytes)| match name {
        "schema.sql" => creates_schema_object(bytes),
        "actions.json" => sources_use_actions([bytes]),
        _ => contains(bytes, b"miniapp.actions"),
    })
}

pub fn sources_use_http<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> bool {
    files.into_iter().filter(|(name, _)| *name == "actions.json").any(|(_, bytes)| {
        serde_json::from_slice::<Value>(bytes).ok().and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(|action| action["http"].is_object()))).unwrap_or(false)
    })
}

pub fn sources_use_calls<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> bool {
    files.into_iter().filter(|(name, _)| *name == "actions.json").any(|(_, bytes)| {
        serde_json::from_slice::<Value>(bytes).ok().and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(|action| action["kind"] == "call"))).unwrap_or(false)
    })
}

fn creates_schema_object(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let words = text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_').filter(|word| !word.is_empty()).collect::<Vec<_>>();
    words.windows(2).any(|words| words[0].eq_ignore_ascii_case("CREATE") && (words[1].eq_ignore_ascii_case("TABLE") || words[1].eq_ignore_ascii_case("INDEX")))
        || words.windows(3).any(|words| words[0].eq_ignore_ascii_case("CREATE") && (words[1].eq_ignore_ascii_case("TEMP") || words[1].eq_ignore_ascii_case("TEMPORARY") || words[1].eq_ignore_ascii_case("UNIQUE")) && (words[2].eq_ignore_ascii_case("TABLE") || words[2].eq_ignore_ascii_case("INDEX")))
}

pub fn sources_use_files<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"/_konae/files/"))
}

pub fn artifact_uses_auth<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"miniapp.auth") || contains(bytes, b"miniapp.files") || contains(bytes, b"/_konae/files/"))
}

pub fn sources_use_auth<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| {
        contains(bytes, b"miniapp.auth")
            || contains(bytes, b"miniapp.files")
            || contains(bytes, b"/_konae/files/")
            || contains(bytes, b"\"access\":\"authenticated\"")
            || contains(bytes, b"\"access\": \"authenticated\"")
            || contains(bytes, b"\"source\":\"actor\"")
            || contains(bytes, b"\"source\": \"actor\"")
    })
}

fn contains(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|window| window == marker)
}

pub fn complete(modules: &mut BTreeMap<String, Value>) {
    let switches = registry()["modules"].as_object().cloned().unwrap_or_default();
    modules.retain(|name, _| switches.get(name).is_some_and(|description| description["switch"] == true));
    for (name, state) in defaults() {
        modules.entry(name).or_insert(state);
    }
}

pub fn uses_files(root: &std::path::Path, id: &str, options: &Value) -> bool {
    if options["fileAccess"] == "app-users" || file_index_has_files(root, id) { return true; }
    let dist = crate::layout::dist(root, id);
    if walk(&dist, &|bytes| artifact_uses_files([bytes])) { return true; }
    let sources = crate::layout::sources(root, id);
    walk(&sources, &|bytes| sources_use_files([bytes]))
}

pub fn uses_auth(root: &std::path::Path, id: &str) -> bool {
    let settings_path = crate::layout::settings(root, id);
    if let Ok(bytes) = std::fs::read(settings_path) {
        if let Ok(settings) = serde_json::from_slice::<Value>(&bytes) {
            if settings["visibility"] != "public" || settings["released"].get("visibility").is_some_and(|visibility| visibility != "public") { return true; }
            if settings["manifest"]["fileAccess"] == "app-users" || settings["released"]["manifest"]["fileAccess"] == "app-users" { return true; }
        } else { return true; }
    } else { return true; }
    let identity = crate::layout::data(root, id).join("identity.sqlite");
    if identity.exists() {
        if let Ok(connection) = rusqlite::Connection::open_with_flags(identity, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            let has_table = |name: &str| connection.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(())).is_ok();
            if has_table("app_users") && connection.query_row("SELECT COUNT(*) FROM app_users", [], |row| row.get::<_, i64>(0)).unwrap_or(1) > 0 { return true; }
            if has_table("app_invitations") && connection.query_row("SELECT COUNT(*) FROM app_invitations", [], |row| row.get::<_, i64>(0)).unwrap_or(1) > 0 { return true; }
        } else { return true; }
    }
    if uses_files(root, id, &json!({})) { return true; }
    walk(&crate::layout::sources(root, id), &|bytes| sources_use_auth([bytes]))
        || walk(&crate::layout::dist(root, id), &|bytes| artifact_uses_auth([bytes]))
}

fn file_index_has_files(root: &std::path::Path, id: &str) -> bool {
    let index = crate::layout::files(root, id).join("index.json");
    match std::fs::read(index) {
        Ok(bytes) => serde_json::from_slice::<Vec<Value>>(&bytes).map_or(true, |files| !files.is_empty()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn walk(directory: &std::path::Path, matches: &dyn Fn(&[u8]) -> bool) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else { return true };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { return true };
        if kind.is_symlink() { return true; }
        if kind.is_dir() {
            if walk(&path, matches) { return true; }
        } else {
            match std::fs::read(path) {
                Ok(bytes) if matches(&bytes) => return true,
                Ok(_) => {}
                Err(_) => return true,
            }
        }
    }
    false
}

pub fn check_expected(module: &str, current: &Value, expected: Option<&Value>) -> std::result::Result<(), crate::refusal::Refusal> {
    if expected.is_some_and(|expected| expected != current) {
        return Err(crate::refusal::refuse("module.changed", format!("Module {module} changed since it was read. Read modules_read and try again.")));
    }
    Ok(())
}

pub fn check_switch(module: &str, switch: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !switch {
        return Err(crate::refusal::refuse("module.not-switch", format!("Module {module} cannot be changed yet.")));
    }
    Ok(())
}

pub fn check_known(module: &str, known: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !known {
        return Err(crate::refusal::refuse("module.unknown", format!("Unknown module {module}.")));
    }
    Ok(())
}

pub fn check_requires(module: &str, required: &str, on: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !on {
        return Err(crate::refusal::refuse("module.requires", format!("Module {module} requires {required}, which is off. Turn {required} on first.")));
    }
    Ok(())
}

pub fn check_dependent_requires(module: &str, dependent: &str, dependent_on: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if dependent_on {
        return Err(crate::refusal::refuse("module.requires", format!("Module {module} is required by {dependent}, which is on. Turn {dependent} off first.")));
    }
    Ok(())
}

pub fn check_in_use_action(module: &str, action: &str) -> std::result::Result<(), crate::refusal::Refusal> {
    let message = match module {
        "http" => format!("Module http is in use by action {action}, which declares an HTTP route. Remove its route before turning it off."),
        "calls" => format!("Module calls is in use by action {action}, which sends an outgoing call. Remove its call before turning it off."),
        _ => return Err(crate::refusal::refuse("module.unknown", format!("Unknown module {module}."))),
    };
    Err(crate::refusal::refuse("module.in-use", message))
}

pub fn release_use_order() -> Vec<String> {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../module-cases.json")).expect("module cases are valid JSON");
    cases.into_iter().find(|case| case["rule"] == "release-order").and_then(|case| case["order"].as_array().cloned())
        .unwrap_or_default().into_iter().filter_map(|module| module.as_str().map(str::to_owned)).collect()
}

pub fn check_private_auth(on: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !on { return Err(crate::refusal::refuse("module.requires", "A private app requires auth, which is off. Turn auth on first.")); }
    Ok(())
}

pub fn check_requirements(modules: &BTreeMap<String, Value>) -> std::result::Result<(), crate::refusal::Refusal> {
    for (module, description) in registry()["modules"].as_object().into_iter().flatten() {
        if description["switch"] != true || !is_on(modules, module) { continue; }
        for required in description["requires"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            check_requires(module, required, is_on(modules, required))?;
        }
    }
    Ok(())
}

pub fn is_on(modules: &BTreeMap<String, Value>, module: &str) -> bool {
    let description = registry()["modules"][module].clone();
    description["switch"] != true || modules.get(module).is_some_and(|state| state["on"] == true)
}

pub fn require_on(modules: &BTreeMap<String, Value>, module: &str) -> std::result::Result<(), crate::refusal::Refusal> {
    if is_on(modules, module) {
        return Ok(());
    }
    Err(crate::refusal::refuse("module.off", format!("Module {module} is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.")))
}

pub fn require_not_in_use(module: &str, used: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !used {
        return Ok(());
    }
    let message = if module == "actions" {
        "Module actions is in use by actions.json, which declares Server Actions, HTTP routes, calls, schedules, notices, Signals or page paths, or by schema.sql or miniapp.actions. Remove those uses before turning it off.".to_owned()
    } else {
        format!("Module {module} is in use by the published app. Remove its use before turning it off.")
    };
    Err(crate::refusal::refuse("module.in-use", message))
}

fn check_option(module: &str, description: &Value, key: &str, value: &Value) -> std::result::Result<(), crate::refusal::Refusal> {
    let Some(option) = description["options"].as_array().into_iter().flatten().find(|item| item["key"].as_str() == Some(key)) else {
        return Err(crate::refusal::refuse("module.option-unknown", format!("Unknown option {key} for module {module}.")));
    };
    let valid = (value.is_null() && option.get("default").is_some_and(Value::is_null)) || match option["type"].as_str() {
        Some("boolean") => value.is_boolean(),
        Some("string") => value.is_string(),
        Some("number") => value.is_number(),
        Some("enum") => option["values"].as_array().into_iter().flatten().any(|candidate| candidate == value),
        Some("signup-policy") => value.is_null() || (matches!(value["mode"].as_str(), Some("open" | "invitation" | "closed" | "allowlist")) && value["allowlist"].as_array().is_some_and(|items| items.iter().all(Value::is_string))),
        Some("sign-in-methods") => value.as_array().is_some_and(|items| items.len() == 1 && items[0] == "password" || items.len() == 2 && items[0] == "password" && items[1] == "google"),
        Some("path") => value.is_null() || value.as_str().is_some_and(|path| !path.is_empty() && !path.starts_with('/') && !path.contains('\\') && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")),
        _ => false,
    };
    if !valid {
        let message = if option["type"] == "enum" {
            let values = option["values"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>();
            format!("Module {module} option {key} must be one of {}.", values.join(", "))
        } else if option["type"] == "path" {
            format!("Module {module} option {key} must be a relative app path or null.")
        } else {
            format!("Module {module} option {key} must be a {}.", option["type"].as_str().unwrap_or("valid value"))
        };
        return Err(crate::refusal::refuse("module.option-invalid", message));
    }
    Ok(())
}

pub fn validate_state(modules: &BTreeMap<String, Value>) -> std::result::Result<(), crate::refusal::Refusal> {
    for (module, state) in modules {
        let description = &registry()["modules"][module];
        check_known(module, !description.is_null())?;
        let Some(object) = state.as_object() else { return Err(crate::refusal::refuse("module.option-invalid", format!("Module {module} state must be an object."))); };
        if !object.get("on").is_some_and(Value::is_boolean) { return Err(crate::refusal::refuse("module.option-invalid", format!("Module {module} option on must be a boolean."))); }
        for (key, value) in object.iter().filter(|(key, _)| key.as_str() != "on") { check_option(module, description, key, value)?; }
    }
    Ok(())
}

pub fn preserve_release_state(
    next: &mut Settings,
    previous: Option<Settings>,
    initial: Option<&BTreeMap<String, Value>>,
) {
    if let Some(previous) = previous {
        next.updated_at = previous.updated_at;
        next.modules = if previous.released.is_none() {
            next.modules.clone()
        } else {
            previous.modules
        };
    } else if let Some(initial) = initial {
        next.modules = initial.clone();
    }
    complete(&mut next.modules);
}

pub fn read(state: &crate::AppState, deployment: &crate::Deployment) -> Value {
    let descriptions = registry();
    let modules = descriptions["modules"].as_object().into_iter().flatten().map(|(name, description)| {
        let switch = description["switch"] == true;
        let mut module_state = if switch { deployment.modules.get(name).cloned().unwrap_or_else(|| defaults()[name].clone()) } else { json!({"on":true}) };
        if name == "auth" {
            module_state["signupPolicy"] = serde_json::to_value(&deployment.signup_policy).unwrap_or(Value::Null);
            module_state["signInMethods"] = serde_json::to_value(&deployment.sign_in_methods).unwrap_or(Value::Null);
        }
        let mut module = json!({
            "state": module_state,
            "switch": switch,
            "message": if switch { Value::Null } else { json!("This module cannot be changed yet.") },
            "summary": description["summary"],
            "options": description["options"],
            "uses": description["uses"],
            "requires": description["requires"],
            "permission": description["permission"],
            "agentRule": description["agentRule"],
        });
        if name == "seo" {
            let address = crate::seo::address(state, deployment);
            let root = crate::layout::dist(&state.data_root, &deployment.id);
            let mut excluded = Vec::new();
            collect_pages(&root, &root, &mut excluded);
            module["current"] = json!({"canonical":{"url":address.url,"reason":address.reason},"excluded":excluded});
        }
        (name.clone(), module)
    }).collect::<Map<String, Value>>();
    json!({"projectId":deployment.id,"modules":modules})
}

fn collect_pages(root: &std::path::Path, dir: &std::path::Path, excluded: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() { collect_pages(root, &path, excluded); continue; }
        if !path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")) { continue; }
        let Some(relative) = path.strip_prefix(root).ok().map(|path| path.to_string_lossy().replace('\\', "/")) else { continue };
        let has_title = std::fs::read(&path).ok().is_some_and(|html| crate::seo::title(&html).is_some());
        if !has_title { excluded.push(json!({"page":relative,"reason":"The page has no title."})); }
    }
}

pub async fn set(
    root: &std::path::Path,
    id: &str,
    module: &str,
    on: Option<bool>,
    options: Option<&Value>,
    expected: Option<&Value>,
) -> Result<Value> {
    let mut settings = Settings::read(root, id)?;
    let description = registry()["modules"][module].clone();
    check_known(module, !description.is_null())?;
    check_switch(module, description["switch"] == true)?;
    let mut current = settings
        .modules
        .get(module)
        .cloned()
        .unwrap_or_else(|| defaults()[module].clone());
    check_expected(module, &current, expected)?;
    if let Some(on) = on {
        if on {
            for required in description["requires"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                    if !is_on(&settings.modules, required) {
                        check_requires(module, required, false)?;
                }
            }
        } else {
            for (dependent, description) in registry()["modules"].as_object().into_iter().flatten() {
                let depends_on_module = description["switch"] == true && description["requires"].as_array().into_iter().flatten().any(|required| required == module);
                if depends_on_module && is_on(&settings.modules, dependent) {
                    check_dependent_requires(module, dependent, true)?;
                }
            }
            let used = published_app_uses_module(root, id, module, &current);
            if module == "http" && used {
                let action = released_action_name(root, id, |action| action["http"].is_object()).unwrap_or_else(|| "a published action".to_owned());
                check_in_use_action(module, &action)?;
            }
            if module == "calls" && used {
                let action = released_action_name(root, id, |action| action["kind"] == "call").unwrap_or_else(|| "a published action".to_owned());
                check_in_use_action(module, &action)?;
            }
            require_not_in_use(module, used)?;
        }
        current["on"] = json!(on);
    }
    if let Some(options) = options {
        let incoming = options
            .as_object()
            .ok_or_else(|| anyhow!("Module options must be an object."))?;
        for (key, value) in incoming {
            check_option(module, &description, key, value)?;
            let value = if module == "auth" && key == "signupPolicy" && !value.is_null() {
                let policy: crate::SignupPolicy = serde_json::from_value(value.clone())?;
                serde_json::to_value(crate::SignupPolicy::from_input(policy.mode, &policy.allowlist).map_err(anyhow::Error::msg)?)?
            } else { value.clone() };
            current[key] = value;
        }
    }
    if module == "auth" {
        if let Some(policy) = current.get("signupPolicy").filter(|value| !value.is_null()) {
            settings.signup_policy = serde_json::from_value(policy.clone())?;
        }
        if let Some(methods) = current.get("signInMethods") {
            settings.sign_in_methods = serde_json::from_value(methods.clone())?;
        }
    }
    settings.modules.insert(module.to_owned(), current.clone());
    settings.write(root).await?;
    Ok(json!({"projectId":id,"module":module,"state":current}))
}

fn published_app_uses_module(root: &std::path::Path, id: &str, module: &str, state: &Value) -> bool {
    if crate::functions::published_use(root, id, module) { return true; }
    match module {
        "files" => uses_files(root, id, state),
        "auth" => uses_auth(root, id),
        "actions" => uses_actions(root, id),
        "http" => uses_http(root, id),
        "calls" => uses_calls(root, id),
        _ => false,
    }
}

fn released_action_use(root: &std::path::Path, id: &str, predicate: impl Fn(&Value) -> bool) -> bool {
    if let Ok(bytes) = std::fs::read(crate::layout::record(root, id)) {
        if let Ok(installed) = serde_json::from_slice::<Value>(&bytes) {
            let actions = installed.get("actions").and_then(Value::as_str).unwrap_or("");
            return serde_json::from_str::<Value>(actions).ok().and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(&predicate))).unwrap_or(true);
        }
        return true;
    }
    let actions = std::fs::read(crate::layout::sources(root, id).join("actions.json")).unwrap_or_default();
    serde_json::from_slice::<Value>(&actions).ok().and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(predicate))).unwrap_or(true)
}

pub fn uses_http(root: &std::path::Path, id: &str) -> bool {
    released_action_use_or_false_when_source_is_missing(root, id, |action| action["http"].is_object())
}

pub fn uses_calls(root: &std::path::Path, id: &str) -> bool {
    released_action_use_or_false_when_source_is_missing(root, id, |action| action["kind"] == "call")
}

pub fn folder_uses_actions(root: &std::path::Path, id: &str) -> bool {
    let sources = crate::layout::sources(root, id);
    let actions = match std::fs::read(sources.join("actions.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"{\"version\":1,\"actions\":{}}".to_vec(),
        Err(_) => return true,
    };
    let schema = match std::fs::read(sources.join("schema.sql")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return true,
    };
    if crate::action_shape::read(std::str::from_utf8(&actions).unwrap_or("")).is_err() { return true; }
    named_sources_use_actions([("actions.json", actions.as_slice()), ("schema.sql", schema.as_slice())])
        || walk(&crate::layout::dist(root, id), &|bytes| contains(bytes, b"miniapp.actions"))
        || database_has_tables(&crate::layout::database(root, id))
}

pub fn folder_uses_http(root: &std::path::Path, id: &str) -> bool {
    folder_action_use(root, id, |action| action["http"].is_object())
}

pub fn folder_uses_calls(root: &std::path::Path, id: &str) -> bool {
    folder_action_use(root, id, |action| action["kind"] == "call")
}

fn folder_action_use(root: &std::path::Path, id: &str, predicate: impl Fn(&Value) -> bool) -> bool {
    let path = crate::layout::sources(root, id).join("actions.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    serde_json::from_slice::<Value>(&bytes).ok()
        .and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(&predicate)))
        .unwrap_or(true)
}

fn released_action_use_or_false_when_source_is_missing(root: &std::path::Path, id: &str, predicate: impl Fn(&Value) -> bool) -> bool {
    let record = crate::layout::record(root, id);
    if record.exists() { return released_action_use(root, id, predicate); }
    match std::fs::read(crate::layout::sources(root, id).join("actions.json")) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok().and_then(|value| value["actions"].as_object().map(|actions| actions.values().any(&predicate))).unwrap_or(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn released_action_name(root: &std::path::Path, id: &str, predicate: impl Fn(&Value) -> bool) -> Option<String> {
    let bytes = std::fs::read(crate::layout::record(root, id)).ok()?;
    let installed = serde_json::from_slice::<Value>(&bytes).ok()?;
    let actions = installed.get("actions")?.as_str()?;
    let program = serde_json::from_str::<Value>(actions).ok()?;
    program["actions"].as_object()?.iter().find_map(|(name, action)| predicate(action).then(|| name.clone()))
}

pub fn uses_actions(root: &std::path::Path, id: &str) -> bool {
    if let Ok(bytes) = std::fs::read(crate::layout::record(root, id)) {
        if let Ok(installed) = serde_json::from_slice::<Value>(&bytes) {
            let actions = installed.get("actions").and_then(Value::as_str).unwrap_or("");
            let record_schema = installed.pointer("/released/schemaSql").and_then(Value::as_str);
            let settings_schema = record_schema.is_none().then(|| {
                std::fs::read(crate::layout::settings(root, id)).ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|settings| settings.pointer("/released/schemaSql").and_then(Value::as_str).map(str::to_owned))
            }).flatten();
            let schema_snapshot = record_schema.or(settings_schema.as_deref());
            let schema = schema_snapshot.unwrap_or("");
            if crate::action_shape::read(actions).is_err() { return true; }
            if named_sources_use_actions([("actions.json", actions.as_bytes()), ("schema.sql", schema.as_bytes())]) { return true; }
            if walk(&crate::layout::dist(root, id), &|bytes| contains(bytes, b"miniapp.actions")) { return true; }
            return schema_snapshot.is_none() && database_has_tables(&crate::layout::database(root, id));
        }
        return true;
    }
    let sources = crate::layout::sources(root, id);
    let actions = std::fs::read(sources.join("actions.json")).unwrap_or_default();
    let schema = std::fs::read(sources.join("schema.sql")).unwrap_or_default();
    named_sources_use_actions([("actions.json", actions.as_slice()), ("schema.sql", schema.as_slice())])
}

fn database_has_tables(path: &std::path::Path) -> bool {
    if !path.is_file() { return false; }
    let connection = match rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(connection) => connection,
        Err(_) => return true,
    };
    match connection.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 1", [], |_| Ok(())) {
        Ok(()) => true,
        Err(rusqlite::Error::QueryReturnedNoRows) => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_cases_match_the_editor_answers() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-cases.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            if case["rule"] == "release-order" { continue; }
            let module = case["module"].as_str().unwrap();
            let outcome = match case["rule"].as_str().unwrap() {
                "expected" => check_expected(module, &case["state"], Some(&case["expected"])),
                "switch" => check_switch(module, case["switch"] == true),
                "known" => check_known(module, case["known"] == true),
                "on" => require_on(&BTreeMap::from([(module.to_owned(), case["state"].clone())]), module),
                "private-auth" => check_private_auth(case["requiredState"]["on"] == true),
                "requires" => check_requires(module, case["required"].as_str().unwrap(), case["requiredState"]["on"] == true),
                "dependent-requires" => check_dependent_requires(module, case["dependent"].as_str().unwrap(), case["dependentState"]["on"] == true),
                "in-use-action" => check_in_use_action(module, case["action"].as_str().unwrap()),
                "release-order" => Ok(()),
                "in-use" => require_not_in_use(module, case["used"] == true),
                "option-unknown" | "option-invalid" => check_option(module, &json!({"options":case["options"]}), case["option"].as_str().unwrap(), &case["value"]),
                _ => panic!("unknown module case {}", case["case"]),
            };
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn release_module_use_order_matches_the_shared_case() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-cases.json")).unwrap();
        let order = cases.into_iter().find(|case| case["rule"] == "release-order").unwrap()["order"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(release_use_order(), order);
    }

    #[test]
    fn file_use_cases_match_the_kernel_rule() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-use-cases.json")).unwrap();
        for case in cases {
            if case.get("usesFiles").is_none() { continue; }
            let sources = case["sources"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let artifact = case["artifact"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let used = sources_use_files(sources) || artifact_uses_files(artifact);
            assert_eq!(used, case["usesFiles"], "{}", case["case"]);
        }
    }

    #[test]
    fn auth_use_cases_match_the_kernel_rule() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-use-cases.json")).unwrap();
        for case in cases {
            let Some(expected) = case.get("usesAuth") else { continue };
            let sources = case["sources"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let artifact = case["artifact"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            assert_eq!(sources_use_auth(sources) || artifact_uses_auth(artifact), *expected, "{}", case["case"]);
        }
    }

    #[test]
    fn actions_use_cases_match_the_kernel_rule() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-use-cases.json")).unwrap();
        for case in cases {
            let Some(expected) = case.get("usesActions") else { continue };
            let sources = case["sources"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let artifact = case["artifact"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let source_action = if let Some(files) = case.get("sourceFiles").and_then(Value::as_object) {
                named_sources_use_actions(files.iter().map(|(name, value)| (name.as_str(), value.as_str().unwrap().as_bytes())))
            } else { sources_use_actions(sources) };
            assert_eq!(source_action || sources_use_actions(artifact), *expected, "{}", case["case"]);
        }
    }

    #[test]
    fn http_and_calls_use_cases_match_the_kernel_rule() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../module-use-cases.json")).unwrap();
        for case in cases {
            let Some(files) = case.get("sourceFiles").and_then(Value::as_object) else { continue };
            let files = files.iter().map(|(name, value)| (name.as_str(), value.as_str().unwrap().as_bytes())).collect::<Vec<_>>();
            if let Some(expected) = case.get("usesHttp") { assert_eq!(sources_use_http(files.iter().copied()), *expected, "{}", case["case"]); }
            if let Some(expected) = case.get("usesCalls") { assert_eq!(sources_use_calls(files.iter().copied()), *expected, "{}", case["case"]); }
        }
    }

    #[test]
    fn missing_actions_sources_do_not_make_a_static_app_use_http_or_calls() {
        let root = std::env::temp_dir().join(format!("konae-missing-actions-use-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(crate::layout::sources(&root, "app")).unwrap();
        assert!(!uses_http(&root, "app"));
        assert!(!uses_calls(&root, "app"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_modules_with_switches_have_default_state() {
        let modules = defaults();
        assert_eq!(modules.keys().map(String::as_str).collect::<Vec<_>>(), ["actions", "auth", "calls", "files", "http", "seo"]);
        assert!(modules.values().all(|state| state["on"] == false));
        let mut partial = BTreeMap::from([("seo".to_owned(), json!({"on":true,"canonical":"path","image":null}))]);
        complete(&mut partial);
        assert_eq!(partial.len(), modules.len());
        assert_eq!(partial["seo"]["on"], true);
        assert!(!is_on(&partial, "actions"));
        assert!(is_on(&partial, "seo"));
        assert!(!is_on(&modules, "seo"));
    }

    #[test]
    fn action_keys_require_actions_in_the_module_contract() {
        let modules = registry()["modules"].as_object().unwrap().clone();
        for module in ["http", "calls", "page"] {
            let requires = modules[module]["requires"].as_array().unwrap();
            assert!(requires.iter().any(|required| required == "actions"), "{module}");
        }
    }

    #[tokio::test]
    async fn action_use_follows_the_installed_release_after_a_source_save() {
        let root = std::env::temp_dir().join(format!("konae-published-actions-{}", uuid::Uuid::new_v4()));
        let app = crate::layout::folder(&root, "app");
        std::fs::create_dir_all(crate::layout::sources(&root, "app")).unwrap();
        std::fs::create_dir_all(crate::layout::private(&root, "app")).unwrap();
        std::fs::write(crate::layout::sources(&root, "app").join("actions.json"), r#"{"version":1,"actions":{}}"#).unwrap();
        std::fs::write(crate::layout::record(&root, "app"), serde_json::to_vec(&json!({"actions":"{\"version\":1,\"actions\":{\"notes.list\":{\"kind\":\"query\"}}}","released":{"schemaSql":"CREATE TABLE notes(id INTEGER PRIMARY KEY);"}})).unwrap()).unwrap();
        assert!(uses_actions(&root, "app"));
        let _ = std::fs::remove_dir_all(app);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn action_use_keeps_the_module_on_when_the_installed_page_mentions_actions() {
        let root = std::env::temp_dir().join(format!("konae-actions-artifact-use-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(crate::layout::private(&root, "app")).unwrap();
        std::fs::create_dir_all(crate::layout::dist(&root, "app")).unwrap();
        std::fs::write(crate::layout::record(&root, "app"), serde_json::to_vec(&json!({"actions":"{\"version\":1,\"actions\":{}}","released":{"schemaSql":""}})).unwrap()).unwrap();
        std::fs::write(crate::layout::dist(&root, "app").join("index.html"), "<p>miniapp.actions is the SDK name</p>").unwrap();
        assert!(uses_actions(&root, "app"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn action_use_reads_released_schema_from_settings_for_older_records() {
        let root = std::env::temp_dir().join(format!("konae-legacy-released-schema-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(crate::layout::private(&root, "app")).unwrap();
        std::fs::write(crate::layout::record(&root, "app"), serde_json::to_vec(&json!({"actions":"{\"version\":1,\"actions\":{}}"})).unwrap()).unwrap();
        std::fs::write(crate::layout::settings(&root, "app"), serde_json::to_vec(&json!({"released":{"schemaSql":"CREATE TABLE notes(id INTEGER PRIMARY KEY);"}})).unwrap()).unwrap();
        assert!(uses_actions(&root, "app"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn action_use_is_conservative_for_unreadable_installed_sources_and_database_tables() {
        let root = std::env::temp_dir().join(format!("konae-actions-uncertain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(crate::layout::private(&root, "malformed")).unwrap();
        std::fs::write(crate::layout::record(&root, "malformed"), serde_json::to_vec(&json!({"actions":"{"})).unwrap()).unwrap();
        assert!(uses_actions(&root, "malformed"));
        std::fs::create_dir_all(crate::layout::private(&root, "tables")).unwrap();
        std::fs::create_dir_all(crate::layout::data(&root, "tables")).unwrap();
        std::fs::write(crate::layout::record(&root, "tables"), serde_json::to_vec(&json!({"actions":"{\"version\":1,\"actions\":{}}"})).unwrap()).unwrap();
        let db = rusqlite::Connection::open(crate::layout::database(&root, "tables")).unwrap();
        db.execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY);").unwrap();
        drop(db);
        assert!(uses_actions(&root, "tables"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn persisted_module_state_rejects_non_object_values_and_invalid_options() {
        let invalid = BTreeMap::from([("seo".to_owned(), json!(true))]);
        assert!(validate_state(&invalid).is_err());
        let invalid_option = BTreeMap::from([("seo".to_owned(), json!({"on":true,"canonical":"../secret"}))]);
        assert!(validate_state(&invalid_option).is_err());
    }

    #[test]
    fn a_first_release_turns_on_undecided_modules_the_capsule_uses() {
        let inferred = initial_for_release(None, true, false, false, false);
        assert_eq!(inferred["files"]["on"], true);
        assert_eq!(inferred["auth"]["on"], true);
        let seo_only = BTreeMap::from([("seo".to_owned(), json!({"on":false}))]);
        assert_eq!(initial_for_release(Some(&seo_only), true, false, false, false)["files"]["on"], true);
        assert_eq!(initial_for_release(Some(&seo_only), false, true, false, false)["auth"]["on"], true);
        let explicit = BTreeMap::from([("files".to_owned(), json!({"on":false,"fileAccess":null}))]);
        assert_eq!(initial_for_release(Some(&explicit), true, false, false, false)["files"]["on"], false);
        let explicit_auth = BTreeMap::from([("auth".to_owned(), json!({"on":false,"signupPolicy":null,"signInMethods":["password"]}))]);
        assert_eq!(initial_for_release(Some(&explicit_auth), false, true, false, false)["auth"]["on"], false);
        assert_eq!(initial_for_release(None, false, false, false, false)["files"]["on"], false);
        assert!(initial_for_release(None, false, false, true, false)["http"]["on"] == true);
        assert!(initial_for_release(None, false, false, false, true)["calls"]["on"] == true);
        assert!(artifact_uses_files([b"miniapp.files.upload".as_slice()]));
        assert!(!artifact_uses_files([b"/_konae/files/id".as_slice()]));
        assert!(sources_use_files([b"/_konae/files/id".as_slice()]));
        assert!(!artifact_uses_files([b"no file API".as_slice()]));
    }

    #[tokio::test]
    async fn module_state_survives_release_rewrite_and_export_import() {
        let source =
            std::env::temp_dir().join(format!("konae-modules-export-{}", uuid::Uuid::new_v4()));
        let target =
            std::env::temp_dir().join(format!("konae-modules-import-{}", uuid::Uuid::new_v4()));
        let mut before = Settings::new("app", "App", "app", &json!({"visibility":"public"}));
        before.modules.get_mut("seo").unwrap()["on"] = json!(true);
        before.modules.get_mut("seo").unwrap()["canonical"] = json!("path");
        before.modules.get_mut("files").unwrap()["on"] = json!(true);
        before.updated_at = 42;
        before.write(&source).await.unwrap();

        let deployment = before.clone().deployment(
            crate::Installed {
                hostname: "app.example.test".into(),
                created_at: 0,
                released_at: None,
                source_digest: None,
                actions: "{}".into(),
                artifact_version: "v1".into(),
                released: None,
            },
            "https://example.test",
        );
        let mut next = Settings::of(&deployment);
        preserve_release_state(
            &mut next,
            Some(Settings::read(&source, "app").unwrap()),
            None,
        );
        assert_eq!(next.updated_at, 42);
        assert_eq!(next.modules["seo"]["on"], true);
        assert_eq!(next.modules["seo"]["canonical"], "path");
        assert_eq!(next.modules["files"]["on"], true);
        next.write(&source).await.unwrap();

        let exported = crate::sources::settings_for_export(&source, "app")
            .await
            .unwrap();
        exported.write(&target).await.unwrap();
        let imported = Settings::read(&target, "app").unwrap();
        assert_eq!(imported.modules["seo"]["on"], true);
        assert_eq!(imported.modules["seo"]["canonical"], "path");
        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(target);
    }

    #[tokio::test]
    async fn set_then_read_returns_the_change_and_rejects_a_stale_expected_state() {
        let root = std::env::temp_dir().join(format!("konae-modules-set-{}", uuid::Uuid::new_v4()));
        let settings = Settings::new("app", "App", "app", &json!({"visibility":"public"}));
        settings.write(&root).await.unwrap();
        let expected = settings.modules["seo"].clone();
        set(&root, "app", "seo", Some(true), Some(&json!({"canonical":"path"})), Some(&expected)).await.unwrap();
        let changed = Settings::read(&root, "app").unwrap();
        let observed = changed.deployment(crate::Installed {
            hostname: "app.example.test".into(), created_at: 0, released_at: None,
            source_digest: None, actions: "{}".into(), artifact_version: "v1".into(), released: None,
        }, "https://example.test");
        let app_state = crate::AppState::for_test(&root);
        let observed = read(&app_state, &observed);
        assert_eq!(observed["modules"]["seo"]["state"]["on"], true);
        assert_eq!(observed["modules"]["seo"]["state"]["canonical"], "path");
        let error = set(&root, "app", "seo", Some(false), None, Some(&expected)).await.unwrap_err();
        let refusal = error.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.changed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn set_accepts_actions_and_refuses_unknown_options_and_invalid_values() {
        let root = std::env::temp_dir().join(format!("konae-modules-refusals-{}", uuid::Uuid::new_v4()));
        Settings::new("app", "App", "app", &json!({"visibility":"public"})).write(&root).await.unwrap();
        let actions = set(&root, "app", "actions", Some(true), None, None).await.unwrap();
        assert_eq!(actions["state"]["on"], true);
        let unknown = set(&root, "app", "missing", Some(true), None, None).await.unwrap_err();
        assert_eq!(unknown.downcast_ref::<crate::refusal::Refusal>().unwrap().code, "module.unknown");
        let option = set(&root, "app", "seo", None, Some(&json!({"unknown":true})), None).await.unwrap_err();
        assert_eq!(option.downcast_ref::<crate::refusal::Refusal>().unwrap().code, "module.option-unknown");
        let value = set(&root, "app", "seo", None, Some(&json!({"canonical":"bogus"})), None).await.unwrap_err();
        let refusal = value.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.option-invalid");
        assert!(refusal.message.contains("domain, hostname, path"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn http_cannot_turn_off_while_a_published_route_is_in_use() {
        let root = std::env::temp_dir().join(format!("konae-http-module-in-use-{}", uuid::Uuid::new_v4()));
        let mut settings = Settings::new("app", "App", "app", &json!({"visibility":"public"}));
        settings.modules.insert("actions".into(), defaults()["actions"].clone());
        settings.modules.insert("http".into(), defaults()["http"].clone());
        settings.modules.get_mut("actions").unwrap()["on"] = json!(true);
        settings.modules.get_mut("http").unwrap()["on"] = json!(true);
        settings.write(&root).await.unwrap();
        std::fs::create_dir_all(crate::layout::private(&root, "app")).unwrap();
        let actions = r#"{"version":1,"actions":{"hooks.receive":{"kind":"mutation","http":{"method":"POST","path":"/hooks"}}}}"#;
        let installed = crate::Installed { hostname:"app.example.test".into(), created_at:0, released_at:Some(1), source_digest:None, actions:actions.into(), artifact_version:"v1".into(), released:None };
        std::fs::write(crate::layout::record(&root, "app"), serde_json::to_vec(&installed).unwrap()).unwrap();
        let error = set(&root, "app", "http", Some(false), None, None).await.unwrap_err();
        let refusal = error.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.in-use");
        assert_eq!(refusal.message, "Module http is in use by action hooks.receive, which declares an HTTP route. Remove its route before turning it off.");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn calls_cannot_turn_off_while_a_published_call_is_in_use() {
        let root = std::env::temp_dir().join(format!("konae-calls-module-in-use-{}", uuid::Uuid::new_v4()));
        let mut settings = Settings::new("app", "App", "app", &json!({"visibility":"public"}));
        settings.modules.insert("actions".into(), defaults()["actions"].clone());
        settings.modules.insert("calls".into(), defaults()["calls"].clone());
        settings.modules.get_mut("actions").unwrap()["on"] = json!(true);
        settings.modules.get_mut("calls").unwrap()["on"] = json!(true);
        settings.write(&root).await.unwrap();
        std::fs::create_dir_all(crate::layout::private(&root, "app")).unwrap();
        let actions = r#"{"version":1,"actions":{"notify.send":{"kind":"call","method":"POST","url":"https://example.test/hook","bindings":[]}}}"#;
        let installed = crate::Installed { hostname:"app.example.test".into(), created_at:0, released_at:Some(1), source_digest:None, actions:actions.into(), artifact_version:"v1".into(), released:None };
        std::fs::write(crate::layout::record(&root, "app"), serde_json::to_vec(&installed).unwrap()).unwrap();
        let error = set(&root, "app", "calls", Some(false), None, None).await.unwrap_err();
        let refusal = error.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.in-use");
        assert_eq!(refusal.message, "Module calls is in use by action notify.send, which sends an outgoing call. Remove its call before turning it off.");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn module_set_refuses_turning_files_off_while_the_index_has_a_file() {
        let root = std::env::temp_dir().join(format!("konae-files-in-use-{}", uuid::Uuid::new_v4()));
        let mut settings = Settings::new("app", "App", "app", &json!({"visibility":"public"}));
        settings.modules.get_mut("files").unwrap()["on"] = json!(true);
        settings.write(&root).await.unwrap();
        let index = crate::layout::files(&root, "app");
        std::fs::create_dir_all(&index).unwrap();
        std::fs::write(index.join("index.json"), r#"[{"id":"file-1"}]"#).unwrap();
        let error = set(&root, "app", "files", Some(false), None, None).await.unwrap_err();
        assert!(error.to_string().contains("Module files is in use by the published app. Remove its use before turning it off."));
        std::fs::remove_dir_all(root).unwrap();
    }
}
