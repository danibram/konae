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
    if initial.is_none() && uses_files {
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
    if options["fileAccess"] == "app-users" {
        return true;
    }
    let index = crate::layout::files(root, id).join("index.json");
    match std::fs::read(&index) {
        Ok(bytes) => match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(files) if !files.is_empty() => return true,
            Ok(_) => {}
            Err(_) => return true,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return true,
    }
    let dist = crate::layout::dist(root, id);
    let mut entries = match std::fs::read_dir(&dist) {
        Ok(entries) => entries,
        Err(_) => return true,
    };
    while let Some(entry) = entries.next() {
        let Ok(entry) = entry else { return true };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { return true };
        if kind.is_symlink() { return true; }
        if kind.is_dir() {
            if uses_files_in_dir(&path) { return true; }
        } else {
            match std::fs::read(path) {
                Ok(bytes) if artifact_uses_files([bytes.as_slice()]) => return true,
                Ok(_) => {}
                Err(_) => return true,
            }
        }
    }
    let sources = crate::layout::sources(root, id);
    if sources.is_dir() && sources_use_files_in_dir(&sources) { return true; }
    false
}

fn uses_files_in_dir(directory: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else { return true };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { return true };
        if kind.is_symlink() { return true; }
        if kind.is_dir() {
            if uses_files_in_dir(&path) { return true; }
        } else {
            match std::fs::read(path) {
                Ok(bytes) if artifact_uses_files([bytes.as_slice()]) => return true,
                Ok(_) => {}
                Err(_) => return true,
            }
        }
    }
    false
}

fn sources_use_files_in_dir(directory: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else { return true };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { return true };
        if kind.is_symlink() { return true; }
        if kind.is_dir() {
            if sources_use_files_in_dir(&path) { return true; }
        } else {
            match std::fs::read(path) {
                Ok(bytes) if sources_use_files([bytes.as_slice()]) => return true,
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

pub fn preserve_release_state(
    next: &mut Settings,
    previous: Option<Settings>,
    initial: Option<&BTreeMap<String, Value>>,
) {
    if let Some(previous) = previous {
        next.updated_at = previous.updated_at;
        next.modules = if previous.released.is_none() {
            initial.cloned().unwrap_or(previous.modules)
        } else {
            previous.modules
        };
    } else if let Some(initial) = initial {
        next.modules = initial.clone();
    }
    complete(&mut next.modules);
}

pub fn read(deployment: &crate::Deployment) -> Value {
    let descriptions = registry();
    let modules = descriptions["modules"].as_object().into_iter().flatten().map(|(name, description)| {
        let switch = description["switch"] == true;
        let state = if switch { deployment.modules.get(name).cloned().unwrap_or_else(|| defaults()[name].clone()) } else { json!({"on":true}) };
        (name.clone(), json!({
            "state": state,
            "switch": switch,
            "message": if switch { Value::Null } else { json!("This module cannot be changed yet.") },
            "summary": description["summary"],
            "options": description["options"],
            "uses": description["uses"],
            "requires": description["requires"],
            "permission": description["permission"],
            "agentRule": description["agentRule"],
        }))
    }).collect::<Map<String, Value>>();
    json!({"projectId":deployment.id,"modules":modules})
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
            let used = module == "files" && used_files(root, id, &current)?;
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
            current[key] = value.clone();
        }
    }
    settings.modules.insert(module.to_owned(), current.clone());
    settings.write(root).await?;
    Ok(json!({"projectId":id,"module":module,"state":current}))
}

fn used_files(root: &std::path::Path, id: &str, state: &Value) -> Result<bool> {
    let index = crate::layout::files(root, id).join("index.json");
    match std::fs::read(index) {
        Ok(bytes) => match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(files) if !files.is_empty() => return Ok(true),
            Ok(_) => {}
            Err(_) => return Ok(true),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if state["fileAccess"] == "app-users" { return Ok(true); }
    Ok(uses_files(root, id, state))
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
    fn a_first_release_turns_on_files_when_its_artifact_uses_the_sdk() {
        let inferred = initial_for_release(None, true);
        assert_eq!(inferred["files"]["on"], true);
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
        let observed = read(&observed);
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
