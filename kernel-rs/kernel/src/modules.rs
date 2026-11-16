use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::Settings;

fn registry() -> &'static Value {
    static REGISTRY: OnceLock<Value> = OnceLock::new();
    REGISTRY.get_or_init(|| serde_json::from_str(include_str!("modules.json")).expect("modules.json is generated JSON"))
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

pub fn initial_for_release(initial: Option<&BTreeMap<String, Value>>, uses_files: bool) -> BTreeMap<String, Value> {
    let mut modules = initial.cloned().unwrap_or_else(defaults);
    complete(&mut modules);
    if initial.is_none_or(|state| !state.contains_key("files")) && uses_files {
        modules.get_mut("files").expect("files is switchable")["on"] = json!(true);
    }
    modules
}

pub fn artifact_uses_files<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"miniapp.files"))
}

pub fn sources_use_files<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files.into_iter().any(|bytes| contains(bytes, b"/_konae/files/"))
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

pub fn is_on(modules: &BTreeMap<String, Value>, module: &str) -> bool {
    let description = registry()["modules"][module].clone();
    description["switch"] != true || modules.get(module).is_some_and(|state| state["on"] == true)
}

pub fn require_on(modules: &BTreeMap<String, Value>, module: &str) -> std::result::Result<(), crate::refusal::Refusal> {
    if is_on(modules, module) {
        return Ok(());
    }
    Err(crate::refusal::refuse("module.off", format!("Module {module} is off. Turn it on with module_set.")))
}

pub fn require_not_in_use(module: &str, used: bool) -> std::result::Result<(), crate::refusal::Refusal> {
    if !used {
        return Ok(());
    }
    Err(crate::refusal::refuse(
        "module.in-use",
        format!("Module {module} is in use by the published app. Remove its use before turning it off."),
    ))
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
        let module_state = if switch { deployment.modules.get(name).cloned().unwrap_or_else(|| defaults()[name].clone()) } else { json!({"on":true}) };
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
            require_not_in_use(module, published_app_uses_module(root, id, module, &current))?;
        }
        current["on"] = json!(on);
    }
    if let Some(options) = options {
        let incoming = options
            .as_object()
            .ok_or_else(|| anyhow!("Module options must be an object."))?;
        for (key, value) in incoming {
            check_option(module, &description, key, value)?;
            current[key] = value.clone();
        }
    }
    settings.modules.insert(module.to_owned(), current.clone());
    settings.write(root).await?;
    Ok(json!({"projectId":id,"module":module,"state":current}))
}

fn published_app_uses_module(root: &std::path::Path, id: &str, module: &str, state: &Value) -> bool {
    match module {
        "files" => uses_files(root, id, state),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_cases_match_the_editor_answers() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("module-cases.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let module = case["module"].as_str().unwrap();
            let outcome = match case["rule"].as_str().unwrap() {
                "expected" => check_expected(module, &case["state"], Some(&case["expected"])),
                "switch" => check_switch(module, case["switch"] == true),
                "known" => check_known(module, case["known"] == true),
                "on" => require_on(&BTreeMap::from([(module.to_owned(), case["state"].clone())]), module),
                "requires" => check_requires(module, case["required"].as_str().unwrap(), case["requiredState"]["on"] == true),
                "in-use" => require_not_in_use(module, case["used"] == true),
                "option-unknown" | "option-invalid" => check_option(module, &json!({"options":case["options"]}), case["option"].as_str().unwrap(), &case["value"]),
                _ => panic!("unknown module case {}", case["case"]),
            };
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn file_use_cases_match_the_kernel_rule() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("module-use-cases.json")).unwrap();
        for case in cases {
            let sources = case["sources"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let artifact = case["artifact"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::as_bytes);
            let used = sources_use_files(sources) || artifact_uses_files(artifact);
            assert_eq!(used, case["usesFiles"], "{}", case["case"]);
        }
    }

    #[test]
    fn only_modules_with_switches_have_default_state() {
        let modules = defaults();
        assert_eq!(modules.keys().map(String::as_str).collect::<Vec<_>>(), ["files", "seo"]);
        assert!(modules.values().all(|state| state["on"] == false));
        let mut partial = BTreeMap::from([("seo".to_owned(), json!({"on":true,"canonical":"path","image":null}))]);
        complete(&mut partial);
        assert_eq!(partial.len(), modules.len());
        assert_eq!(partial["seo"]["on"], true);
        assert!(is_on(&partial, "actions"));
        assert!(is_on(&partial, "seo"));
        assert!(!is_on(&modules, "seo"));
    }

    #[test]
    fn persisted_module_state_rejects_non_object_values_and_invalid_options() {
        let invalid = BTreeMap::from([("seo".to_owned(), json!(true))]);
        assert!(validate_state(&invalid).is_err());
        let invalid_option = BTreeMap::from([("seo".to_owned(), json!({"on":true,"canonical":"../secret"}))]);
        assert!(validate_state(&invalid_option).is_err());
    }

    #[test]
    fn a_first_release_turns_on_files_when_its_artifact_uses_the_sdk() {
        let inferred = initial_for_release(None, true);
        assert_eq!(inferred["files"]["on"], true);
        let seo_only = BTreeMap::from([("seo".to_owned(), json!({"on":false}))]);
        assert_eq!(initial_for_release(Some(&seo_only), true)["files"]["on"], true);
        let explicit = BTreeMap::from([("files".to_owned(), json!({"on":false,"fileAccess":null}))]);
        assert_eq!(initial_for_release(Some(&explicit), true)["files"]["on"], false);
        assert_eq!(initial_for_release(None, false)["files"]["on"], false);
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
    async fn set_refuses_non_switch_modules_unknown_options_and_invalid_values() {
        let root = std::env::temp_dir().join(format!("konae-modules-refusals-{}", uuid::Uuid::new_v4()));
        Settings::new("app", "App", "app", &json!({"visibility":"public"})).write(&root).await.unwrap();
        let not_switch = set(&root, "app", "actions", Some(false), None, None).await.unwrap_err();
        assert_eq!(not_switch.downcast_ref::<crate::refusal::Refusal>().unwrap().code, "module.not-switch");
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
