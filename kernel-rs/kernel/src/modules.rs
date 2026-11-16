use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::Settings;

#[cfg(test)]
fn case_outcome(case: &Value) -> Option<crate::refusal::Refusal> {
    let module = case["module"].as_str().unwrap();
    match case["rule"].as_str().unwrap() {
        "expected" if case["state"] != case["expected"] => Some(crate::refusal::refuse(
            "module.changed",
            format!("Module {module} changed since it was read. Read modules_read and try again."),
        )),
        "on" if case["state"]["on"] != true => require_on(&BTreeMap::new(), module).err(),
        "requires" if case["requiredState"]["on"] != true => {
            let required = case["required"].as_str().unwrap();
            Some(crate::refusal::refuse(
                "module.requires",
                format!(
                    "Module {module} requires {required}, which is off. Turn {required} on first."
                ),
            ))
        }
        "in-use" if case["used"] == true => require_not_in_use(module, true).err(),
        _ => None,
    }
}

fn registry() -> Value {
    serde_json::from_str(include_str!("modules.json")).expect("modules.json is generated JSON")
}

pub fn defaults() -> BTreeMap<String, Value> {
    let mut modules = BTreeMap::new();
    for (name, description) in registry()["modules"].as_object().into_iter().flatten() {
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

pub fn complete(modules: &mut BTreeMap<String, Value>) {
    for (name, state) in defaults() {
        modules.entry(name).or_insert(state);
    }
}

pub fn require_on(modules: &BTreeMap<String, Value>, module: &str) -> std::result::Result<(), crate::refusal::Refusal> {
    if modules.get(module).is_some_and(|state| state["on"] == true) {
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
}

pub fn read(root: &std::path::Path, id: &str) -> Result<Value> {
    let settings = Settings::read(root, id)?;
    let descriptions = registry();
    let states = settings.modules;
    let modules = descriptions["modules"].as_object().into_iter().flatten().map(|(name, description)| {
        let state = states.get(name).cloned().unwrap_or_else(|| defaults()[name].clone());
        (name.clone(), json!({
            "state": state,
            "summary": description["summary"],
            "options": description["options"],
            "uses": description["uses"],
            "requires": description["requires"],
            "permission": description["permission"],
            "agentRule": description["agentRule"],
        }))
    }).collect::<Map<String, Value>>();
    Ok(json!({"projectId":id,"modules":modules}))
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
    if description.is_null() {
        anyhow::bail!("Unknown SDK module {module}.");
    }
    let mut current = settings
        .modules
        .get(module)
        .cloned()
        .unwrap_or_else(|| defaults()[module].clone());
    if expected.is_some_and(|expected| expected != &current) {
        return Err(crate::refusal::refuse(
            "module.changed",
            format!("Module {module} changed since it was read. Read modules_read and try again."),
        )
        .into());
    }
    if let Some(on) = on {
        if on {
            for required in description["requires"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let required_state = settings
                    .modules
                    .get(required)
                    .cloned()
                    .unwrap_or_else(|| defaults()[required].clone());
                if required_state["on"] != true {
                    return Err(crate::refusal::refuse("module.requires", format!("Module {module} requires {required}, which is off. Turn {required} on first.")).into());
                }
            }
        }
        current["on"] = json!(on);
    }
    if let Some(options) = options {
        let incoming = options
            .as_object()
            .ok_or_else(|| anyhow!("Module options must be an object."))?;
        for (key, value) in incoming {
            let option = description["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|item| item["key"].as_str() == Some(key.as_str()))
                .ok_or_else(|| anyhow!("Unknown option {key} for module {module}."))?;
            if option["type"] == "enum"
                && !option["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate == value)
            {
                anyhow::bail!("Invalid value for {module}.{key}.");
            }
            if option["type"] == "path"
                && !value.is_null()
                && !value.as_str().is_some_and(|path| {
                    !path.is_empty()
                        && !path.starts_with('/')
                        && !path.split('/').any(|part| part == "..")
                })
            {
                anyhow::bail!("Invalid value for {module}.{key}.");
            }
            current[key] = value.clone();
        }
    }
    settings.modules.insert(module.to_owned(), current.clone());
    settings.write(root).await?;
    Ok(json!({"projectId":id,"module":module,"state":current}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_cases_match_the_editor_answers() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("module-cases.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let outcome = case_outcome(&case);
            match (case["error"].as_str(), outcome) {
                (None, None) => {}
                (Some(message), Some(refusal)) => {
                    assert_eq!(
                        case["code"].as_str(),
                        Some(refusal.code),
                        "{}",
                        case["case"]
                    );
                    assert_eq!(
                        Some(message),
                        Some(refusal.message.as_str()),
                        "{}",
                        case["case"]
                    );
                }
                (expected, actual) => {
                    panic!("{} expected {expected:?}, got {actual:?}", case["case"])
                }
            }
        }
    }

    #[test]
    fn every_known_module_has_an_explicit_default_state() {
        let modules = defaults();
        assert_eq!(
            modules.keys().map(String::as_str).collect::<Vec<_>>(),
            ["actions", "auth", "calls", "files", "http", "seo"]
        );
        assert!(modules.values().all(|state| state["on"] == false));
        let mut partial = BTreeMap::from([("seo".to_owned(), json!({"on":true,"canonical":"path","image":null}))]);
        complete(&mut partial);
        assert_eq!(partial.len(), modules.len());
        assert_eq!(partial["seo"]["on"], true);
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
        let observed = read(&root, "app").unwrap();
        assert_eq!(observed["modules"]["seo"]["state"]["on"], true);
        assert_eq!(observed["modules"]["seo"]["state"]["canonical"], "path");
        let error = set(&root, "app", "seo", Some(false), None, Some(&expected)).await.unwrap_err();
        let refusal = error.downcast_ref::<crate::refusal::Refusal>().unwrap();
        assert_eq!(refusal.code, "module.changed");
        let _ = std::fs::remove_dir_all(root);
    }
}
