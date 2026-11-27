use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use anyhow::Result;
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use oxc_span::SourceType;
use oxc_syntax::module_record::ExportExportName;
use crate::refusal::{refuse, Refusal};
use super::cron::{self, Cron};

pub const ABI: &str = "konae-functions/1";
pub const MAX_FUNCTIONS: usize = 32;
pub const MAX_BUNDLE: usize = 1024 * 1024;
pub const MAX_SERVER: usize = 8 * 1024 * 1024;
pub const MAX_MANIFEST: usize = 256 * 1024;
type Checked<T> = std::result::Result<T, Refusal>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub functions: BTreeMap<String, Function>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub crons: Arc<BTreeMap<String, Cron>>,
}

impl Manifest {
    pub fn source_files(&self) -> Result<Vec<(String, String)>> {
        let mut files = vec![("functions.json".into(), serde_json::to_string(&serde_json::json!({
            "version": self.version, "functions": self.functions,
        }))?)];
        if !self.crons.is_empty() {
            files.push(("crons.json".into(), serde_json::to_string(&serde_json::json!({
                "version": 1, "crons": self.crons,
            }))?));
        }
        Ok(files)
    }

    pub fn check_modules(&self, modules: &BTreeMap<String, Value>) -> Checked<()> {
        if !self.functions.is_empty() { crate::modules::require_on(modules, "functions")?; }
        if self.functions.values().any(|function| function.access == "authenticated") {
            crate::modules::check_requires("functions", "auth", crate::modules::is_on(modules, "auth"))?;
        }
        if self.functions.values().any(|function| !function.actions.is_empty()) {
            crate::modules::check_requires("functions", "actions", crate::modules::is_on(modules, "actions"))?;
        }
        Ok(())
    }
}

pub fn enable_cron_modules(modules: &mut BTreeMap<String, Value>, manifest: Option<&Manifest>) {
    let Some(manifest) = manifest.filter(|manifest| !manifest.crons.is_empty()) else { return };
    let uses_actions = manifest.crons.values()
        .any(|cron| manifest.functions.get(&cron.function).is_some_and(|function| !function.actions.is_empty()));
    for module in std::iter::once("functions").chain(uses_actions.then_some("actions")) {
        modules.entry(module.into()).or_insert_with(|| serde_json::json!({}))["on"] = Value::Bool(true);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub entry: String,
    pub mount: String,
    pub methods: Vec<String>,
    pub access: String,
    pub actions: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerArtifact {
    pub version: u8,
    pub abi: String,
    pub manifest: Manifest,
    pub bundles: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Published {
    pub version: u8,
    pub abi: String,
    pub manifest: Manifest,
    pub hashes: BTreeMap<String, String>,
}

fn keys(value: &Value, allowed: &[&str], label: &str) -> Checked<()> {
    if let Some(key) = value.as_object().into_iter().flat_map(|v| v.keys()).find(|k| !allowed.contains(&k.as_str())) {
        return Err(refuse("function.key-unknown", format!("{label} has unknown key {key}.")));
    }
    Ok(())
}
pub(super) fn identifier(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn entry_path(path: &str) -> bool {
    path.starts_with("functions/") && crate::safe_relative_path(path)
        && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
        && [".ts", ".js", ".mts", ".mjs"].iter().any(|ext| path.ends_with(ext))
}
fn mount_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 256 && path[1..].split('/').all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}
pub fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.strip_prefix(b).is_some_and(|tail| tail.starts_with('/')) || b.strip_prefix(a).is_some_and(|tail| tail.starts_with('/'))
}
pub fn read(source: &str) -> Checked<Manifest> {
    let value: Value = serde_json::from_str(source).map_err(|_| refuse("function.invalid", "functions.json must be a JSON object with version 1 and a functions object."))?;
    if !value.is_object() || value["version"] != 1 || !value["functions"].is_object() {
        return Err(refuse("function.invalid", "functions.json must be a JSON object with version 1 and a functions object."));
    }
    keys(&value, &["version", "functions"], "functions.json")?;
    let declarations = value["functions"].as_object().unwrap();
    if declarations.len() > MAX_FUNCTIONS { return Err(refuse("function.count-over", "functions.json may declare at most 32 functions.")); }
    let mut functions = BTreeMap::new();
    for (name, value) in declarations {
        if !identifier(name) { return Err(refuse("function.name-invalid", format!("Function {name} must have a name of 1 to 64 ASCII letters, digits, underscores or hyphens, starting with a letter."))); }
        if !value.is_object() { return Err(refuse("function.definition-invalid", format!("Function {name} must be an object."))); }
        keys(value, &["entry", "mount", "methods", "access", "actions"], &format!("Function {name}"))?;
        let entry = value["entry"].as_str().unwrap_or("");
        if !entry_path(entry) { return Err(refuse("function.entry-invalid", format!("Function {name} entry must name a .ts, .js, .mts or .mjs file under functions/."))); }
        let access = value.get("access").map_or(Some("authenticated"), Value::as_str).unwrap_or("");
        let (mount, methods) = if access == "scheduled" {
            if value.get("mount").is_some_and(|mount| mount != "/")
                || value.get("methods").is_some_and(|methods| methods != &serde_json::json!(["POST"]))
            {
                return Err(refuse("function.scheduled-route", format!("Function {name} with access scheduled uses mount / and method POST internally. Omit mount and methods.")));
            }
            ("/", BTreeSet::from(["POST"]))
        } else {
            let mount = value["mount"].as_str().unwrap_or("");
            if !mount_path(mount) { return Err(refuse("function.mount-invalid", format!("Function {name} mount must be an absolute path of nonempty ASCII letter, digit, underscore or hyphen segments."))); }
            if ["/_konae", "/oauth", "/.well-known", "/server", "/functions", "/mcp", "/assets"].iter().any(|reserved| overlaps(mount, reserved)) {
                return Err(refuse("function.mount-reserved", format!("Function {name} mount {mount} is reserved.")));
            }
            let invalid = || refuse("function.methods-invalid", format!("Function {name} methods must be a nonempty array of GET or POST without duplicates."));
            let methods = value["methods"].as_array().filter(|a| !a.is_empty()).ok_or_else(invalid)?;
            let mut methods_set = BTreeSet::new();
            for method in methods {
                let method = method.as_str().filter(|method| matches!(*method, "GET" | "POST")).ok_or_else(invalid)?;
                if !methods_set.insert(method) { return Err(invalid()); }
            }
            (mount, methods_set)
        };
        if !["authenticated", "anonymous", "scheduled"].contains(&access) { return Err(refuse("function.access-invalid", format!("Function {name} access must be authenticated, anonymous or scheduled."))); }
        let actions: Vec<String> = match value.get("actions") {
            None => Vec::new(),
            Some(v) => {
                let entries = v.as_array().ok_or_else(|| refuse("function.actions-invalid", format!("Function {name} actions must be an array of unique action names.")))?;
                let mut seen = BTreeSet::new();
                for entry in entries { if !entry.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128 && seen.insert(s)) { return Err(refuse("function.actions-invalid", format!("Function {name} actions must be an array of unique action names."))); } }
                seen.into_iter().map(str::to_owned).collect()
            }
        };
        functions.insert(name.clone(), Function { entry: entry.into(), mount: mount.into(), methods: methods.into_iter().map(str::to_owned).collect(), access: access.into(), actions });
    }
    let manifest = Manifest { version: 1, functions, crons: Arc::default() };
    check_manifest_size(&manifest)?;
    Ok(manifest)
}

pub fn read_sources(functions: Option<&str>, crons: Option<&str>) -> Checked<Option<Manifest>> {
    let mut manifest = functions.map(read).transpose()?;
    if let Some(source) = crons {
        let declarations = cron::read(source, manifest.as_ref().map(|manifest| &manifest.functions))?;
        if let Some(manifest) = &mut manifest {
            manifest.crons = Arc::new(declarations);
            check_manifest_size(manifest)?;
        }
    }
    Ok(manifest)
}

fn read_manifest(value: &Value) -> Checked<Manifest> {
    let mut functions = value.clone();
    let crons = functions.as_object_mut().and_then(|value| value.remove("crons"))
        .map(|crons| serde_json::json!({"version": 1, "crons": crons}).to_string());
    read_sources(Some(&functions.to_string()), crons.as_deref()).map(Option::unwrap)
}

fn check_manifest_size(manifest: &Manifest) -> Checked<()> {
    let published = Published { version: 1, abi: ABI.into(), manifest: manifest.clone(),
        hashes: manifest.functions.keys().map(|name| (name.clone(), "0".repeat(64))).collect() };
    if serde_json::to_vec_pretty(&published).expect("Published contains JSON values").len() > MAX_MANIFEST {
        return Err(refuse("function.manifest-size", "Server manifest exceeds 262144 UTF-8 bytes."));
    }
    Ok(())
}

pub fn validate(manifest: &Manifest, files: &BTreeSet<String>, modules: &BTreeMap<String, Value>, actions: &Value, static_paths: &[String]) -> Checked<()> {
    validate_context(manifest, Some(files), modules, actions, static_paths)
}

pub fn validate_published(manifest: &Manifest, modules: &BTreeMap<String, Value>, actions: &Value, static_paths: &[String]) -> Checked<()> {
    validate_context(manifest, None, modules, actions, static_paths)
}

fn validate_context(manifest: &Manifest, files: Option<&BTreeSet<String>>, modules: &BTreeMap<String, Value>, actions: &Value, static_paths: &[String]) -> Checked<()> {
    for (name, function) in &manifest.functions {
        if files.is_some_and(|files| !files.contains(&function.entry)) { return Err(refuse("function.entry-missing", format!("Function {name} entry {} is missing from the app sources.", function.entry))); }
    }
    manifest.check_modules(modules)?;
    for (name, function) in &manifest.functions {
        for action in &function.actions {
            let definition = &actions[action];
            if definition.is_null() { return Err(refuse("function.action-missing", format!("Function {name} names unknown action {action}."))); }
            let then_target = actions.as_object().is_some_and(|all| all.values().any(|other| other["kind"] == "call" && other["then"]["action"] == action.as_str()));
            check_action_shape(name, action, definition["kind"].as_str().unwrap_or(""), definition["access"].as_str().unwrap_or(""),
                ["notify", "call", "signal", "schedule", "subscribe", "http", "grants"].iter().any(|key| definition.get(key).is_some()), then_target)?;
            check_scheduled_action(name, function, action, definition["access"].as_str().unwrap_or(""))?;
        }
        if function.access == "scheduled" { continue; }
        for (other, declaration) in manifest.functions.range(..name.clone()) {
            if declaration.access != "scheduled" && overlaps(&function.mount, &declaration.mount) { return Err(refuse("function.mount-conflict", format!("Function {name} mount {} overlaps function {other}.", function.mount))); }
        }
        let mut paths = static_paths.to_vec();
        if let Some(actions) = actions.as_object() {
            for action in actions.values() {
                for field in ["http", "page"] { if let Some(path) = action[field]["path"].as_str() { paths.push(path.to_owned()); } }
            }
        }
        paths.sort();
        for path in paths {
            let path = format!("/{}", path.trim_start_matches('/'));
            let prefix = path.split("/:").next().unwrap_or(&path);
            if overlaps(&function.mount, prefix) { return Err(refuse("function.mount-conflict", format!("Function {name} mount {} overlaps route {path}.", function.mount))); }
        }
    }
    Ok(())
}

pub fn check_scheduled_action(function_name: &str, function: &Function, action: &str, access: &str) -> Checked<()> {
    if function.access == "scheduled" && !matches!(access, "anonymous" | "scheduled") {
        return Err(refuse("function.scheduled-action", format!("Scheduled Function {function_name} can only call anonymous or scheduled actions. Check {action}.")));
    }
    if function.access != "scheduled" && access == "scheduled" {
        return Err(refuse("function.scheduled-action", format!("Function {function_name} cannot call action {action}. That action runs only inside a scheduled Function.")));
    }
    Ok(())
}

fn check_action_shape(function: &str, action: &str, kind: &str, access: &str, effects: bool, then_target: bool) -> Checked<()> {
    if !matches!(kind, "query" | "mutation") || access == "granted" || effects || then_target {
        return Err(refuse("function.action-unsupported", format!("Function {function} action {action} must be a query or mutation without external effects.")));
    }
    Ok(())
}

pub fn check_function_action(function: &str, name: &str, action: &crate::ActionDefinition, program: &crate::ActionProgram) -> Checked<()> {
    check_action_shape(function, name, &action.kind, &action.access,
        action.notify.is_some() || action.signal.is_some() || action.schedule.is_some() || action.subscribe.is_some() || action.http.is_some() || action.call.any() || action.extra.contains_key("grants"),
        crate::calls::then_of(program, name).is_some())
}

pub fn parse_server_artifact(value: &Value) -> Checked<ServerArtifact> {
    if !value.is_object() || value["version"] != 1 || value["abi"] != ABI {
        return Err(refuse("function.artifact-abi", "Server Artifact requires version 1 and ABI konae-functions/1."));
    }
    keys(value, &["version", "abi", "manifest", "bundles"], "Server Artifact")?;
    let manifest = read_manifest(&value["manifest"])?;
    if serde_json::to_value(&manifest).expect("Manifest contains JSON values") != value["manifest"] {
        return Err(refuse("function.artifact-manifest", "Server Artifact manifest does not match functions.json."));
    }
    let entries = value["bundles"].as_object().filter(|entries| entries.keys().eq(manifest.functions.keys()))
        .ok_or_else(|| refuse("function.artifact-bundles", "Server Artifact bundles must match the declared function names."))?;
    let mut bundles = BTreeMap::new();
    for (name, bundle) in entries {
        let text = bundle.as_str().ok_or_else(|| refuse("function.bundle-size", format!("Function {name} bundle must contain 1 to 1048576 UTF-8 bytes.")))?;
        bundles.insert(name.clone(), text.to_owned());
    }
    let artifact = ServerArtifact { version: 1, abi: ABI.into(), manifest, bundles };
    validate_artifact(Some(&artifact.manifest), Some(&artifact))?;
    Ok(artifact)
}

pub fn parse_deploy_input(payload: Value) -> Result<crate::DeployInput> {
    if let Some(artifact) = payload.get("serverArtifact").filter(|value| !value.is_null()) {
        parse_server_artifact(artifact)?;
    }
    Ok(serde_json::from_value(payload)?)
}

pub fn validate_artifact(manifest: Option<&Manifest>, artifact: Option<&ServerArtifact>) -> Checked<Option<Published>> {
    let Some(artifact) = artifact else {
        if manifest.is_some_and(|m| !m.functions.is_empty()) { return Err(refuse("function.artifact-missing", "Declared functions require a Server Artifact.")); }
        return Ok(None);
    };
    if artifact.version != 1 || artifact.abi != ABI { return Err(refuse("function.artifact-abi", "Server Artifact requires version 1 and ABI konae-functions/1.")); }
    let normalized = read_manifest(&serde_json::to_value(&artifact.manifest).expect("Manifest contains JSON values"))?;
    if normalized != artifact.manifest { return Err(refuse("function.artifact-manifest", "Server Artifact manifest does not match functions.json.")); }
    if artifact.bundles.keys().ne(artifact.manifest.functions.keys()) { return Err(refuse("function.artifact-bundles", "Server Artifact bundles must match the declared function names.")); }
    let mut total = 0;
    let mut hashes = BTreeMap::new();
    for (name, bundle) in &artifact.bundles {
        if bundle.is_empty() || bundle.len() > MAX_BUNDLE { return Err(refuse("function.bundle-size", format!("Function {name} bundle must contain 1 to 1048576 UTF-8 bytes."))); }
        total += bundle.len();
        hashes.insert(name.clone(), format!("{:x}", Sha256::digest(bundle.as_bytes())));
    }
    if total > MAX_SERVER { return Err(refuse("function.artifact-size", "Server Artifact exceeds 8388608 UTF-8 bytes.")); }
    if manifest != Some(&artifact.manifest) {
        let file = if manifest.is_some_and(|manifest| manifest.functions == artifact.manifest.functions) { "crons.json" } else { "functions.json" };
        return Err(refuse("function.artifact-manifest", format!("Server Artifact manifest does not match {file}.")));
    }
    for (name, bundle) in &artifact.bundles {
        if !valid_bundle(bundle) { return Err(bundle_contract_refusal(name)); }
    }
    Ok(Some(Published { version: 1, abi: ABI.into(), manifest: artifact.manifest.clone(), hashes }))
}

fn valid_bundle(source: &str) -> bool {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();
    parsed.errors.is_empty()
        && parsed.module_record.requested_modules.is_empty()
        && parsed.module_record.dynamic_imports.is_empty()
        && parsed.module_record.local_export_entries.iter().any(|entry| {
            matches!(&entry.export_name, ExportExportName::Name(name) if name.name == "handle")
        })
}

fn bundle_contract_refusal(name: &str) -> Refusal {
    refuse("function.bundle-contract", format!("Function {name} bundle must be a self-contained ESM module exporting handle."))
}

pub fn read_artifact(directory: &Path) -> Result<Option<ServerArtifact>> {
    Ok(read_artifact_with_metadata(directory)?.map(|(artifact, _)| artifact))
}

pub(crate) fn read_artifact_with_metadata(directory: &Path) -> Result<Option<(ServerArtifact, Published)>> {
    if !directory.exists() { return Ok(None); }
    if std::fs::read_dir(directory)?.next().is_none() { return Ok(None); }
    let file = directory.join("manifest.json");
    anyhow::ensure!(!std::fs::symlink_metadata(directory)?.file_type().is_symlink(), "Server directory must not be a symlink.");
    let metadata = std::fs::symlink_metadata(&file)?;
    anyhow::ensure!(metadata.is_file() && metadata.len() <= MAX_MANIFEST as u64, "Invalid server manifest file.");
    let published: Published = serde_json::from_slice(&std::fs::read(file)?)?;
    let normalized = read_manifest(&serde_json::to_value(&published.manifest)?)?;
    anyhow::ensure!(normalized == published.manifest, "Server manifest must be normalized.");
    let mut bundles = BTreeMap::new();
    for name in published.manifest.functions.keys() {
        let path = directory.join(format!("{name}.mjs"));
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(metadata.is_file() && metadata.len() <= MAX_BUNDLE as u64, "Invalid server bundle file.");
        bundles.insert(name.clone(), std::fs::read_to_string(path)?);
    }
    let artifact = ServerArtifact { version: published.version, abi: published.abi.clone(), manifest: published.manifest.clone(), bundles };
    let checked = validate_artifact(Some(&published.manifest), Some(&artifact))?.unwrap();
    anyhow::ensure!(checked == published, "Server Artifact hashes do not match its bundles.");
    let expected: BTreeSet<_> = std::iter::once("manifest.json".to_owned()).chain(published.manifest.functions.keys().map(|name| format!("{name}.mjs"))).collect();
    for entry in std::fs::read_dir(directory)? { anyhow::ensure!(expected.contains(&entry?.file_name().to_string_lossy().into_owned()), "Server directory contains an undeclared file."); }
    Ok(Some((artifact, checked)))
}

pub async fn stage(root: &Path, artifact: Option<&ServerArtifact>) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    if let Some(artifact) = artifact {
        let published = validate_artifact(Some(&artifact.manifest), Some(artifact))?.unwrap();
        tokio::fs::write(root.join("manifest.json"), serde_json::to_vec_pretty(&published)?).await?;
        for (name, bundle) in &artifact.bundles { tokio::fs::write(root.join(format!("{name}.mjs")), bundle).await?; }
    }
    Ok(())
}

pub fn published_use(root: &Path, id: &str, module: &str) -> bool {
    let settings = crate::Settings::read(root, id).ok();
    let Some(published) = settings.and_then(|s| s.released).and_then(|s| s.server) else { return false };
    published.manifest.functions.values().any(|f| match module { "functions" => true, "auth" => f.access == "authenticated", "actions" => !f.actions.is_empty(), _ => false })
}


pub fn revision_digest(dist_digest: &str, server: Option<&Published>) -> String {
    match server {
        None => dist_digest.to_owned(),
        Some(server) => {
            let mut digest = Sha256::new();
            digest.update(dist_digest.as_bytes());
            digest.update(serde_json::to_vec(server).expect("server metadata serializes"));
            format!("{:x}", digest.finalize())
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn add_function(payload: &mut Value, bundle: &str) {
        let manifest = serde_json::json!({"version":1,"functions":{"orders":{"entry":"functions/orders.ts","mount":"/api/orders","methods":["GET","POST"],"access":"anonymous","actions":[]}}});
        payload["project"]["files"]["functions.json"] = Value::String(manifest.to_string());
        payload["project"]["files"]["functions/orders.ts"] = Value::String(bundle.into());
        payload["serverArtifact"] = serde_json::json!({"version":1,"abi":ABI,"manifest":manifest,"bundles":{"orders":bundle}});
        payload["initialModules"] = serde_json::json!({"functions":{"on":true}});
    }
    pub(crate) fn release(id: &str, bundle: &str) -> Value {
        let mut payload = serde_json::json!({
            "project":{"id":id,"name":"Functions","slug":id,"manifest":{"routing":"spa"},"files":{}},
            "visibility":"public", "artifact":{"version":1,"entrypoint":"index.html","files":{"index.html":"<p>public</p>"}}
        });
        add_function(&mut payload, bundle);
        payload
    }
    pub(crate) async fn publish(state: &crate::AppState, payload: &Value) -> crate::Deployment {
        let response = crate::deploy(state, payload.to_string().as_bytes()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 100_000).await.unwrap();
        assert_eq!(status, axum::http::StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_value(serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()).unwrap()
    }

    #[test]
    fn server_artifact_limits_and_correspondence() {
        let input: crate::DeployInput = serde_json::from_value(release("limits", "export const handle=()=>null;")).unwrap();
        let artifact = input.server_artifact.unwrap();
        let manifest = artifact.manifest.clone();
        assert_eq!(validate_artifact(Some(&manifest), None).unwrap_err().code, "function.artifact-missing");
        assert_eq!(validate_artifact(None, Some(&artifact)).unwrap_err().code, "function.artifact-manifest");
        let mut changed = artifact.clone(); changed.abi = "other/1".into();
        assert_eq!(validate_artifact(Some(&manifest), Some(&changed)).unwrap_err().code, "function.artifact-abi");
        let mut changed = artifact.clone(); changed.bundles.clear();
        assert_eq!(validate_artifact(Some(&manifest), Some(&changed)).unwrap_err().code, "function.artifact-bundles");
        let full_bundle = format!("export const handle=()=>null;/*{}*/", "x".repeat(MAX_BUNDLE - "export const handle=()=>null;/**/".len()));
        let mut changed = artifact.clone(); changed.bundles.insert("orders".into(), full_bundle.clone());
        assert!(validate_artifact(Some(&manifest), Some(&changed)).is_ok());
        changed.bundles.get_mut("orders").unwrap().push('x');
        assert_eq!(validate_artifact(Some(&manifest), Some(&changed)).unwrap_err().code, "function.bundle-size");
        let mut changed = artifact.clone();
        for n in 0..8 {
            let name = format!("f{n}");
            changed.manifest.functions.insert(name.clone(), manifest.functions["orders"].clone());
            changed.bundles.insert(name, full_bundle.clone());
        }
        assert_eq!(validate_artifact(Some(&changed.manifest), Some(&changed)).unwrap_err().code, "function.artifact-size");
    }

    #[tokio::test]
    async fn functions_publish_without_evaluation_and_server_only_changes_get_a_new_version() {
        let root = std::env::temp_dir().join(format!("konae-functions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut state = crate::AppState::for_test(&root);
        state.execution_mode = crate::execution::ExecutionMode::Live;
        let payload = release("functions", "throw new Error('publishing must not evaluate this'); export function handle() {}");
        let a = publish(&state, &payload).await;
        let server = crate::layout::server(&root, "functions");
        let mut artifact = read_artifact(&server).unwrap().unwrap();
        artifact.bundles.insert("orders".into(), "export function handle() { return new Response('B'); }".into());
        stage(&server, Some(&artifact)).await.unwrap();
        let b = crate::folder::open(&state, "functions", None).await.unwrap();
        assert_ne!(a.artifact_version, b.artifact_version);
        assert_ne!(a.server, b.server);
        assert_eq!(std::fs::read(crate::layout::dist(&root, "functions").join("index.html")).unwrap(), b"<p>public</p>");
        std::fs::write(crate::layout::sources(&root, "functions").join("functions.json"), "invalid draft").unwrap();
        let c = crate::folder::open(&state, "functions", None).await.unwrap();
        assert_eq!(b.server, c.server);
        assert_eq!(b.artifact_version, c.artifact_version);
        std::fs::write(server.join("orders.mjs"), "export function handle() { return new Response('C'); }").unwrap();
        assert!(crate::folder::open(&state, "functions", None).await.err().unwrap().to_string().contains("hashes"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn functions_module_is_off_by_default_and_cannot_be_disabled_while_published() {
        assert!(!crate::modules::is_on(&crate::modules::defaults(), "functions"));
        let root = std::env::temp_dir().join(format!("konae-functions-modules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::AppState::for_test(&root);
        let mut payload = release("modules", "export function handle() {}");
        payload.as_object_mut().unwrap().remove("initialModules");
        let input = serde_json::from_value(payload.clone()).unwrap();
        let error = crate::runtime::validate_release(&state, &input, None).await.err().unwrap();
        assert_eq!(crate::refusal::of(&error).unwrap().code, "module.off");
        payload["initialModules"] = serde_json::json!({"functions":{"on":true}});
        publish(&state, &payload).await;
        let error = crate::modules::set(&root, "modules", "functions", Some(false), None, None).await.unwrap_err();
        assert_eq!(crate::refusal::of(&error).unwrap().code, "module.in-use");
        payload.as_object_mut().unwrap().remove("initialModules");
        payload.as_object_mut().unwrap().remove("serverArtifact");
        payload["project"]["files"] = serde_json::json!({});
        publish(&state, &payload).await;
        crate::modules::set(&root, "modules", "functions", Some(false), None, Some(&serde_json::json!({"on":true}))).await.unwrap();
        assert!(!crate::modules::is_on(&crate::Settings::read(&root, "modules").unwrap().modules, "functions"));
        assert!(read_artifact(&crate::layout::server(&root, "modules")).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn shared_server_artifact_cases_and_portable_boundaries() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../functions-artifact-cases.json")).unwrap();
        for case in cases {
            let outcome = (|| -> Checked<Option<ServerArtifact>> {
                let source = if case["sourceFromArtifact"] == true { Some(case["artifact"]["manifest"].to_string()) }
                    else { case["source"].as_str().map(str::to_owned) };
                let manifest = source.as_deref().map(read).transpose()?;
                let mut artifact = case.get("artifact").filter(|value| !value.is_null()).map(parse_server_artifact).transpose()?;
                if let Some(bytes) = case["filledBundleBytes"].as_u64() {
                    for bundle in artifact.as_mut().unwrap().bundles.values_mut() {
                        let padding = "x".repeat(bytes as usize - bundle.len() - 4);
                        bundle.push_str(&format!("/*{padding}*/"));
                    }
                }
                validate_artifact(manifest.as_ref(), artifact.as_ref())?;
                Ok(artifact)
            })();
            match (case.get("code"), outcome) {
                (Some(code), Err(error)) => {
                    assert_eq!(error.code, code.as_str().unwrap(), "{}", case["name"]);
                    assert_eq!(error.message, case["message"].as_str().unwrap(), "{}", case["name"]);
                }
                (None, Ok(artifact)) => {
                    assert_eq!(serde_json::to_value(&artifact).unwrap(), case["artifact"], "{}", case["name"]);
                    if let Some(bytes) = case["metadataBytes"].as_u64() {
                        let root = std::env::temp_dir().join(format!("konae-metadata-{}", uuid::Uuid::new_v4()));
                        stage(&root, artifact.as_ref()).await.unwrap();
                        assert_eq!(std::fs::metadata(root.join("manifest.json")).unwrap().len(), bytes);
                        let reopened = read_artifact(&root).unwrap();
                        assert_eq!(serde_json::to_value(reopened).unwrap(), case["artifact"]);
                        if bytes == MAX_MANIFEST as u64 {
                            let file = root.join("manifest.json");
                            let mut padded = std::fs::read(&file).unwrap();
                            padded.push(b' ');
                            std::fs::write(file, padded).unwrap();
                            assert_eq!(read_artifact(&root).unwrap_err().to_string(), "Invalid server manifest file.");
                        }
                        std::fs::remove_dir_all(root).unwrap();
                    }
                }
                (_, outcome) => panic!("{}: {outcome:?}", case["name"]),
            }
        }
    }

    #[test]
    fn deploy_input_keeps_artifact_refusal_before_serde() {
        let mut payload = release("raw-boundary", "export const handle=()=>new Response('ok');");
        payload["serverArtifact"]["extra"] = Value::Bool(true);
        let error = parse_deploy_input(payload).err().unwrap();
        let refused = crate::refusal::of(&error).unwrap();
        assert_eq!(refused.code, "function.key-unknown");
        assert_eq!(refused.message, "Server Artifact has unknown key extra.");
    }

    #[test]
    fn shared_function_cases() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../functions-cases.json")).unwrap();
        for case in cases {
            let outcome = read(case["source"].as_str().unwrap()).and_then(|manifest| {
                let files = serde_json::from_value(case["files"].clone()).unwrap();
                let modules = serde_json::from_value(case["modules"].clone()).unwrap();
                let paths: Vec<String> = serde_json::from_value(case["staticPaths"].clone()).unwrap();
                validate(&manifest, &files, &modules, &case["actions"], &paths)?;
                Ok(manifest)
            });
            match (case.get("code"), outcome) {
                (Some(code), Err(error)) => { assert_eq!(error.code, code.as_str().unwrap(), "{}", case["name"]); assert_eq!(error.message, case["message"].as_str().unwrap(), "{}", case["name"]); }
                (None, Ok(manifest)) => assert_eq!(serde_json::to_value(manifest).unwrap(), case["expected"], "{}", case["name"]),
                (_, outcome) => panic!("{}: {outcome:?}", case["name"]),
            }
        }
    }
}


impl crate::DeployInput {
    /// Builds the active declarations from an imported snapshot, leaving saved sources separate.
    pub fn restored(&self) -> Result<Self> {
        let Some(snapshot) = &self.initial_released else { return Ok(self.clone()); };
        let dist = crate::artifact::decode(&self.artifact).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(snapshot.artifact_digest.as_deref() == Some(crate::folder::version_of(&dist).as_str()), "Published snapshot does not match the static artifact digest.");
        let server = validate_artifact(self.server_artifact.as_ref().map(|a| &a.manifest), self.server_artifact.as_ref())?;
        anyhow::ensure!(server == snapshot.server, "Published snapshot does not match the Server Artifact.");
        let mut restored = self.clone();
        restored.project.files.clear();
        restored.project.encodings.clear();
        restored.project.files.insert("actions.json".into(), snapshot.actions.clone());
        restored.project.files.insert("schema.sql".into(), snapshot.schema_sql.clone());
        if let Some(source) = &snapshot.schema_transforms { restored.project.files.insert("schema-transforms.json".into(), source.clone()); }
        if let Some(server) = &snapshot.server { restored.project.files.extend(server.manifest.source_files()?); }
        restored.project.manifest = snapshot.manifest.clone();
        restored.visibility = snapshot.visibility.clone();
        Ok(restored)
    }
}
