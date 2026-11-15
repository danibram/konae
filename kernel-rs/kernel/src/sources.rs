//! The sources the editor saves, kept as files in the app's `app/`.
//!
//! The editor sends and receives one JSON document, `{ id, name, slug,
//! files, encodings, directories, manifest, updatedAt }`. Its files become
//! the files of `app/`, and its editable settings go to `konae.json`. A file
//! whose bytes are UTF-8 is its text. Any other file, such as an image, is
//! base64, and `encodings` names it with the value `base64`, as a Build
//! Artifact does. A save writes the new files beside `app/` and then puts
//! them in its place, so a reader sees the old sources or the new ones, never
//! a mix. Callers hold the project lock.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::layout;
use crate::refusal::{refuse, Refusal};

/// The bytes of each file, by its path inside `app/`.
pub type Files = BTreeMap<String, Vec<u8>>;

/// The most files the sources of a project hold.
pub const MAX_FILES: usize = 1_000;
/// The most bytes every file of `app/` holds together, decoded. As base64 in
/// the JSON of a save or a release, they fit one 32 MiB upload.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// The longest path inside `app/`, in UTF-8 bytes.
pub const MAX_PATH_BYTES: usize = 240;

/// Whether `path` names a file the sources may hold. The folder readers skip
/// a part that starts with a dot or is `node_modules`, so the rule refuses it.
pub fn holds(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && !path.starts_with(char::is_whitespace)
        && !path.ends_with(char::is_whitespace)
        && !path.chars().any(char::is_control)
        && !path.contains('\\')
        && path.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && part != "node_modules")
}

/// The rules the sources of a project meet, from each file's path and decoded
/// size: the file count, then each path in byte order, then the total. The
/// CLI, the editor's save and its import run src/lib/source-rules.ts before
/// they upload, and the `cases` of sources-cases.json hold what both answer.
pub fn check<'a>(files: impl IntoIterator<Item = (&'a str, usize)>) -> Result<(), Refusal> {
    let mut files: Vec<(&str, usize)> = files.into_iter().collect();
    files.sort_by(|a, b| a.0.cmp(b.0));
    if files.len() > MAX_FILES {
        return Err(refuse("sources.file-count", format!("app/ holds {} files. A project's sources hold at most {MAX_FILES} files.", files.len())));
    }
    if let Some((path, _)) = files.iter().find(|(path, _)| !holds(path)) {
        return Err(refuse("sources.path-invalid", format!(
            "The sources cannot hold the path {}. A path inside app/ is at most {MAX_PATH_BYTES} bytes, with no control character, no space at either end, no backslash, no empty part, and no part that starts with a dot or is node_modules.",
            serde_json::to_string(path).unwrap_or_default()
        )));
    }
    let total: usize = files.iter().map(|(_, bytes)| bytes).sum();
    if total > MAX_BYTES {
        return Err(refuse("sources.size", format!("app/ holds {}. A project's sources hold at most 20 MiB. {}", crate::size::text(total), crate::size::largest(files))));
    }
    Ok(())
}

/// [`check`] on the bytes of each file.
pub fn check_files(files: &Files) -> Result<(), Refusal> {
    check(files.iter().map(|(path, bytes)| (path.as_str(), bytes.len())))
}

/// The bytes of `files`. A path `encodings` names is base64; every other file
/// is UTF-8 text. Each refusal names the file as `app/<path>`.
pub fn decode(files: &HashMap<String, String>, encodings: &HashMap<String, String>) -> Result<Files, String> {
    for (path, encoding) in encodings {
        if !files.contains_key(path) {
            return Err(format!("The sources name an encoding for app/{path}, which they do not hold."));
        }
        if encoding != "base64" {
            return Err(format!("The sources encode app/{path} as {encoding}. The only encoding is base64."));
        }
    }
    files
        .iter()
        .map(|(path, contents)| {
            let bytes = if encodings.contains_key(path) {
                base64::engine::general_purpose::STANDARD
                    .decode(contents)
                    .map_err(|_| format!("app/{path} is marked base64 but is not valid base64."))?
            } else {
                contents.clone().into_bytes()
            };
            Ok((path.clone(), bytes))
        })
        .collect()
}

/// The bytes of a saved document's `files` and `encodings`.
pub fn decode_document(source: &Value) -> Result<Files> {
    let files = HashMap::<String, String>::deserialize(&source["files"]).context("The sources' files must map each path to a string.")?;
    let encodings = match &source["encodings"] {
        Value::Null => HashMap::new(),
        value => HashMap::<String, String>::deserialize(value).context("The sources' encodings must map each path to \"base64\".")?,
    };
    decode(&files, &encodings).map_err(anyhow::Error::msg)
}

/// Writes source files into `app/` and their editable settings into `konae.json`.
pub async fn write(root: &Path, id: &str, source: &Value) -> Result<()> {
    let files = decode_document(source)?;
    let name = source["name"].as_str().context("Source name is missing.")?.to_owned();
    let slug = source["slug"].as_str().context("Source slug is missing.")?.to_owned();
    let mut settings = crate::Settings::read(root, id)?;
    let metadata = json!({
        "name": source["name"], "slug": source["slug"], "manifest": source["manifest"],
        "directories": source["directories"], "updatedAt": source["updatedAt"],
    });
    let staged = stage_release(root, id, &files, &metadata).await?;
    if let Err(error_value) = staged.swap().await {
        staged.discard().await;
        return Err(error_value);
    }
    let record_path = layout::record(root, id);
    let previous_record = match tokio::fs::read(&record_path).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => { staged.put_back().await; return Err(error.into()); }
    };
    if let Some(bytes) = previous_record.as_ref() {
        let mut installed: crate::Installed = match serde_json::from_slice(bytes) {
            Ok(installed) => installed,
            Err(error) => { staged.put_back().await; return Err(error.into()); }
        };
        if installed.released.is_none() {
            let mut released_manifest = settings.manifest.clone();
            if let Some(object) = released_manifest.as_object_mut() { object.remove("runtime"); }
            let prior_actions = tokio::fs::read_to_string(staged.retired.join("actions.json")).await.unwrap_or_else(|_| installed.actions.clone());
            let prior_schema = tokio::fs::read_to_string(staged.retired.join("schema.sql")).await.unwrap_or_default();
            installed.released = Some(settings.released.clone().unwrap_or(crate::ReleasedSettings {
                name: settings.name.clone(), slug: settings.slug.clone(), visibility: settings.visibility.clone(),
                signup_policy: settings.signup_policy.clone(), sign_in_methods: settings.sign_in_methods.clone(),
                contact_email: settings.contact_email.clone(), manifest: released_manifest,
                actions: prior_actions, schema_sql: prior_schema, source_digest: installed.source_digest.clone(), released_at: installed.released_at,
            }));
            settings.released = installed.released.clone();
            let installed_bytes = match serde_json::to_vec_pretty(&installed) {
                Ok(bytes) => bytes,
                Err(error) => { staged.put_back().await; return Err(error.into()); }
            };
            if let Err(error) = crate::write_atomic(&record_path, &installed_bytes).await {
                staged.put_back().await;
                return Err(error.into());
            }
        }
        if settings.released.is_none() { settings.released = installed.released.clone(); }
    }
    settings.name = name;
    settings.slug = slug;
    settings.updated_at = source["updatedAt"].as_u64().unwrap_or(settings.updated_at);
    let mut manifest = source["manifest"].clone();
    settings.visibility = manifest["visibility"].as_str().unwrap_or(&settings.visibility).to_owned();
    if let Some(object) = manifest.as_object_mut() { object.remove("visibility"); object.remove("runtime"); }
    settings.manifest = manifest;
    if let Err(error_value) = settings.write(root).await {
        if let Some(bytes) = previous_record { let _ = crate::write_atomic(&record_path, &bytes).await; }
        staged.put_back().await;
        return Err(error_value);
    }
    let _ = tokio::fs::remove_file(layout::private(root, id).join("sources.json")).await;
    staged.finish().await;
    Ok(())
}

/// A release's files written beside `app/`, which the release puts in its
/// place together with `dist/`.
pub struct StagedRelease {
    app: PathBuf,
    staged: PathBuf,
    retired: PathBuf,
    legacy_metadata: PathBuf,
}

/// Stages every release's sources so app/ and dist/ describe the same build.
pub async fn stage_release(root: &Path, id: &str, files: &Files, source_metadata: &Value) -> Result<StagedRelease> {
    let private = layout::private(root, id);
    let staged = private.join(format!("app-{}", Uuid::new_v4().simple()));
    let directories: Vec<&str> = source_metadata["directories"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if let Err(error_value) = stage(&staged, files, &directories).await {
        let _ = tokio::fs::remove_dir_all(&staged).await;
        return Err(error_value.into());
    }
    let nonce = Uuid::new_v4().simple().to_string();
    let retired = private.join(format!("app-retired-{nonce}"));
    Ok(StagedRelease { app: layout::sources(root, id), staged, retired, legacy_metadata: private.join("sources.json") })
}

impl StagedRelease {
    /// Puts the staged files in the place of `app/`. The old `app/` stays
    /// aside until [`Self::finish`] removes it or [`Self::put_back`] restores it.
    pub async fn swap(&self) -> Result<()> {
        let had = tokio::fs::try_exists(&self.app).await?;
        if had {
            tokio::fs::rename(&self.app, &self.retired).await?;
        }
        if let Err(error_value) = tokio::fs::rename(&self.staged, &self.app).await {
            if had {
                let _ = tokio::fs::rename(&self.retired, &self.app).await;
            }
            return Err(error_value.into());
        }
        Ok(())
    }

    /// Puts the old `app/` back after a later step of the release failed.
    pub async fn put_back(self) {
        if let Err(error_value) = tokio::fs::rename(&self.app, &self.staged).await {
            eprintln!("Could not move the sources of a failed release out of {}: {error_value}", self.app.display());
            return;
        }
        if tokio::fs::try_exists(&self.retired).await.unwrap_or(false) {
            if let Err(error_value) = tokio::fs::rename(&self.retired, &self.app).await {
                eprintln!("Could not put back {} after a failed release: {error_value}", self.app.display());
            }
        }
        let _ = tokio::fs::remove_dir_all(&self.staged).await;
    }

    /// Removes the old `app/` once the release serves.
    pub async fn finish(self) {
        let _ = tokio::fs::remove_dir_all(&self.retired).await;
        let _ = tokio::fs::remove_file(&self.legacy_metadata).await;
    }

    /// Removes the staged files of a release that never swapped.
    pub async fn discard(self) {
        let _ = tokio::fs::remove_dir_all(&self.staged).await;
    }
}

async fn stage(staged: &Path, files: &Files, directories: &[&str]) -> Result<()> {
    tokio::fs::create_dir_all(staged).await?;
    for directory in directories {
        tokio::fs::create_dir_all(staged.join(directory)).await?;
    }
    for (path, bytes) in files {
        anyhow::ensure!(crate::safe_relative_path(path), "{path} is not a path inside app/.");
        let target = staged.join(path);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&target, bytes).await?;
    }
    Ok(())
}

/// The digest of a release's sources, which the Deployment returns as
/// `sourceDigest` so the editor can tell whether its files are the ones the
/// app runs. SHA-256 in lowercase hex over the files sorted by the bytes of
/// their path. Each file adds its path, a NUL, its length in bytes as decimal
/// digits, a NUL and its bytes. src/deployments/source-digest.ts computes the
/// same, and source-digest-cases.json holds the cases both run.
pub fn digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    use sha2::Digest;
    let sorted: std::collections::BTreeMap<&str, &[u8]> = files.into_iter().collect();
    let mut hasher = sha2::Sha256::new();
    for (path, bytes) in sorted {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    hex::encode(hasher.finalize())
}

/// Every file of `app/`, by its path inside it. Like `konae publish` and the
/// editor's import, it skips dot entries and `node_modules` at any depth.
async fn walk(app: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    let mut pending: Vec<PathBuf> = vec![app.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await.with_context(|| format!("{} is not readable.", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry.path().strip_prefix(app)?.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                files.push((relative, entry.path()));
            }
        }
    }
    Ok(files)
}

/// The digest of the files in `app/`.
pub async fn digest_folder(app: &Path) -> Result<String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (relative, path) in walk(app).await? {
        files.push((relative, tokio::fs::read(path).await?));
    }
    Ok(digest(files.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice()))))
}

/// The saved sources as the editor sent them, or `None` when the app has none.
/// The document has `encodings` only when a file is not UTF-8, so sources
/// that are all text read back as they were saved.
pub async fn read(root: &Path, id: &str) -> Result<Option<Value>> {
    let app = layout::sources(root, id);
    if !tokio::fs::try_exists(&app).await? { return Ok(None); }
    let settings = crate::Settings::read(root, id)?;
    let legacy_path = layout::private(root, id).join("sources.json");
    let metadata = match tokio::fs::read(legacy_path).await {
        Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).context("Saved source metadata is malformed.")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut files = Map::new();
    let mut encodings = Map::new();
    for (relative, path) in walk(&app).await? {
        let bytes = tokio::fs::read(path).await.with_context(|| format!("app/{relative} is not readable."))?;
        let contents = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                encodings.insert(relative.clone(), Value::String("base64".into()));
                base64::engine::general_purpose::STANDARD.encode(error.into_bytes())
            }
        };
        files.insert(relative, Value::String(contents));
    }
    anyhow::ensure!(!files.is_empty(), "app/ has no readable source files.");
    let mut manifest = settings.manifest.clone();
    if let Some(object) = manifest.as_object_mut() { object.insert("visibility".into(), Value::String(settings.visibility.clone())); }
    let mut directories = metadata.as_ref().and_then(|value| value["directories"].as_array()).cloned().unwrap_or_default();
    collect_empty_directories(&app, &app, &mut directories).await?;
    directories.sort_by_key(|value| value.as_str().unwrap_or_default().to_owned());
    directories.dedup();
    let mut source = json!({
        "id": id, "name": metadata.as_ref().and_then(|m| m["name"].as_str()).unwrap_or(&settings.name),
        "slug": metadata.as_ref().and_then(|m| m["slug"].as_str()).unwrap_or(&settings.slug), "files": files,
        "directories": directories, "manifest": metadata.as_ref().and_then(|m| m.get("manifest")).unwrap_or(&manifest),
        "updatedAt": metadata.as_ref().and_then(|m| m["updatedAt"].as_u64()).unwrap_or(settings.updated_at),
    });
    if !encodings.is_empty() {
        source["encodings"] = Value::Object(encodings);
    }
    Ok(Some(source))
}

/// Reads the saved source metadata without walking `app/` or opening its files.
pub async fn read_metadata(root: &Path, id: &str) -> Result<Option<Value>> {
    if !tokio::fs::try_exists(layout::sources(root, id)).await? { return Ok(None); }
    let settings = crate::Settings::read(root, id)?;
    let path = layout::private(root, id).join("sources.json");
    let legacy = match tokio::fs::read(path).await { Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).context("Saved source metadata is malformed.")?), Err(error) if error.kind() == std::io::ErrorKind::NotFound => None, Err(error) => return Err(error.into()) };
    let mut manifest = settings.manifest.clone();
    if let Some(object) = manifest.as_object_mut() { object.insert("visibility".into(), Value::String(settings.visibility.clone())); }
    Ok(Some(json!({ "id": id, "name": legacy.as_ref().and_then(|m| m["name"].as_str()).unwrap_or(&settings.name), "slug": legacy.as_ref().and_then(|m| m["slug"].as_str()).unwrap_or(&settings.slug), "manifest": legacy.as_ref().and_then(|m| m.get("manifest")).unwrap_or(&manifest), "directories": legacy.as_ref().and_then(|m| m.get("directories")).cloned().unwrap_or(json!([])), "updatedAt": legacy.as_ref().and_then(|m| m["updatedAt"].as_u64()).unwrap_or(settings.updated_at) })))
}

fn collect_empty_directories<'a>(root: &'a Path, at: &'a Path, out: &'a mut Vec<Value>) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>> {
  Box::pin(async move {
    let mut entries = tokio::fs::read_dir(at).await?;
    let mut occupied = false;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" { occupied = true; continue; }
        if entry.file_type().await?.is_dir() {
            occupied = true;
            if !collect_empty_directories(root, &entry.path(), out).await? {
                out.push(Value::String(entry.path().strip_prefix(root)?.to_string_lossy().into_owned()));
            }
        } else { occupied = true; }
    }
    Ok(occupied)
  })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_settings(root: &Path, id: &str, source: &Value) {
        tokio::fs::create_dir_all(layout::folder(root, id)).await.unwrap();
        crate::Settings::new(id, source["name"].as_str().unwrap(), source["slug"].as_str().unwrap(), &source["manifest"]).write(root).await.unwrap();
    }

    #[tokio::test]
    async fn the_digest_matches_every_case_the_editor_runs() {
        let cases: Value = serde_json::from_str(include_str!("source-digest-cases.json")).unwrap();
        for case in cases["cases"].as_array().unwrap() {
            let files = decode_document(case).unwrap();
            let expected = case["digest"].as_str().unwrap();
            assert_eq!(digest(files.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice()))), expected);

            // The same files written to app/ give the same digest, and dot entries and node_modules do not count.
            let app = std::env::temp_dir().join(format!("konae-digest-{}", Uuid::new_v4()));
            for (path, bytes) in &files {
                let target = app.join(path);
                tokio::fs::create_dir_all(target.parent().unwrap()).await.unwrap();
                tokio::fs::write(&target, bytes).await.unwrap();
            }
            tokio::fs::create_dir_all(app.join("node_modules/x")).await.unwrap();
            tokio::fs::write(app.join("node_modules/x/index.js"), "1").await.unwrap();
            tokio::fs::write(app.join(".DS_Store"), "1").await.unwrap();
            assert_eq!(digest_folder(&app).await.unwrap(), expected);
            let _ = tokio::fs::remove_dir_all(app).await;
        }
    }

    #[tokio::test]
    async fn saved_sources_are_the_files_of_app_and_read_back_the_same() {
        let root = std::env::temp_dir().join(format!("konae-sources-{}", Uuid::new_v4()));
        assert!(read(&root, "app_a").await.unwrap().is_none(), "an app without saved sources has none");
        let source = json!({
            "id": "app_a", "name": "A", "slug": "a",
            "files": {"index.html": "<main></main>", "src/main.ts": "console.log(1)", "actions.json": "{}"},
            "directories": ["public"], "manifest": {"version": 1, "visibility":"private"}, "updatedAt": 5,
        });
        setup_settings(&root, "app_a", &source).await;
        write(&root, "app_a", &source).await.unwrap();
        assert_eq!(std::fs::read_to_string(layout::sources(&root, "app_a").join("src/main.ts")).unwrap(), "console.log(1)");
        assert!(layout::sources(&root, "app_a").join("public").is_dir());
        assert_eq!(read(&root, "app_a").await.unwrap().unwrap(), source);

        let mut next = source.clone();
        next["files"] = json!({"index.html": "<main>2</main>"});
        next["directories"] = json!([]);
        write(&root, "app_a", &next).await.unwrap();
        assert_eq!(read(&root, "app_a").await.unwrap().unwrap(), next, "a file the new save leaves out is gone");
        let leftovers = std::fs::read_dir(layout::private(&root, "app_a")).unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("app-"))
            .count();
        assert_eq!(leftovers, 0, "no staged or retired sources stay behind");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn metadata_read_does_not_open_saved_files() {
        let root = std::env::temp_dir().join(format!("konae-source-metadata-{}", Uuid::new_v4()));
        let source = json!({
            "id":"app_a", "name":"A", "slug":"a", "files":{"image.png":"AA=="},
            "encodings":{"image.png":"base64"}, "directories":[], "manifest":{"version":1,"visibility":"private"}, "updatedAt":5
        });
        setup_settings(&root, "app_a", &source).await;
        write(&root, "app_a", &source).await.unwrap();
        tokio::fs::remove_file(layout::sources(&root, "app_a").join("image.png")).await.unwrap();
        let metadata = read_metadata(&root, "app_a").await.unwrap().unwrap();
        assert_eq!(metadata["id"], "app_a");
        assert_eq!(metadata["name"], "A");
        assert_eq!(metadata["manifest"]["version"], 1);
        assert!(read_metadata(&root, "missing").await.unwrap().is_none());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn legacy_editor_metadata_migrates_into_settings_without_replacing_app_files() {
        let root = std::env::temp_dir().join(format!("konae-legacy-sources-{}", Uuid::new_v4()));
        let id = "app_legacy";
        let deployed = crate::Settings::new(id, "Deployed", "deployed", &json!({"version":1,"visibility":"private","routing":"pages"}));
        deployed.write(&root).await.unwrap();
        let app = layout::sources(&root, id);
        tokio::fs::create_dir_all(app.join("empty/nested")).await.unwrap();
        tokio::fs::write(app.join("index.html"), "folder edit").await.unwrap();
        let legacy = json!({"name":"Saved draft","slug":"saved-draft","manifest":{"version":1,"visibility":"public","routing":"spa","fileAccess":"app-users"},"directories":["custom/empty"],"updatedAt":77});
        let private = layout::private(&root, id);
        tokio::fs::create_dir_all(&private).await.unwrap();
        tokio::fs::write(private.join("sources.json"), serde_json::to_vec(&legacy).unwrap()).await.unwrap();
        let opened = read(&root, id).await.unwrap().unwrap();
        assert_eq!(opened["name"], "Saved draft");
        assert_eq!(opened["files"]["index.html"], "folder edit");
        assert_eq!(opened["manifest"]["routing"], "spa");
        assert!(opened["directories"].as_array().unwrap().contains(&json!("custom/empty")));
        let mut changed = opened.clone();
        changed["files"]["index.html"] = json!("edited draft");
        write(&root, id, &changed).await.unwrap();
        assert!(!private.join("sources.json").exists());
        assert_eq!(read(&root, id).await.unwrap().unwrap()["files"]["index.html"], "edited draft");
        let settings = crate::Settings::read(&root, id).unwrap();
        assert_eq!(settings.name, "Saved draft");
        assert_eq!(settings.visibility, "public");
        assert_eq!(settings.manifest["routing"], "spa");
        assert!(app.join("empty/nested").is_dir());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_v08_folder_without_editor_metadata_reads_at_revision_zero() {
        let root = std::env::temp_dir().join(format!("konae-v08-sources-{}", Uuid::new_v4()));
        let id = "app_v08";
        crate::Settings::new(id, "Folder app", "folder-app", &json!({"version":1,"visibility":"public"})).write(&root).await.unwrap();
        let app = layout::sources(&root, id);
        tokio::fs::create_dir_all(app.join("public")).await.unwrap();
        tokio::fs::write(app.join("index.html"), "<main>old format</main>").await.unwrap();
        tokio::fs::write(app.join("public/logo.png"), [0x89, 0x50, 0xff]).await.unwrap();
        let source = read(&root, id).await.unwrap().unwrap();
        assert_eq!(source["updatedAt"], 0);
        assert_eq!(source["files"]["index.html"], "<main>old format</main>");
        assert_eq!(decode_document(&source).unwrap()["public/logo.png"], vec![0x89, 0x50, 0xff]);
        assert!(read_metadata(&root, id).await.unwrap().is_some());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn saving_ahead_sources_keeps_the_released_rules_after_reload() {
        let root = std::env::temp_dir().join(format!("konae-released-settings-{}", Uuid::new_v4()));
        let id = "app_release";
        let settings = crate::Settings::new(id, "Live app", "live-app", &json!({"version":1,"visibility":"private","fileAccess":"app-users"}));
        settings.write(&root).await.unwrap();
        let app = layout::sources(&root, id);
        tokio::fs::create_dir_all(&app).await.unwrap();
        tokio::fs::write(app.join("actions.json"), "old actions").await.unwrap();
        tokio::fs::write(app.join("schema.sql"), "CREATE TABLE live(id INTEGER);").await.unwrap();
        let mut deployment = settings.deployment(crate::Installed {
            hostname: "live-app.localhost".into(), created_at: 10, released_at: Some(20), source_digest: Some("old-digest".into()),
            actions: "old actions".into(), artifact_version: "v-old".into(), released: None,
        }, "http://localhost");
        deployment.released_schema = "CREATE TABLE live(id INTEGER);".into();
        crate::write_record_for_test(&root, &deployment);
        let draft = json!({
            "id":id,"name":"Draft app","slug":"draft-app","files":{"actions.json":"new actions","schema.sql":"CREATE TABLE draft(id INTEGER);","index.html":"<main>draft</main>"},
            "directories":[],"manifest":{"version":1,"visibility":"public","routing":"spa"},"updatedAt":30
        });
        write(&root, id, &draft).await.unwrap();
        let running = crate::load_deployments(&root, "http://localhost").await.unwrap().remove("live-app.localhost").unwrap();
        assert_eq!(running.project_name, "Live app");
        assert_eq!(running.manifest["visibility"], "private");
        assert_eq!(running.manifest["fileAccess"], "app-users");
        assert_eq!(running.actions, "old actions");
        assert_eq!(running.released_schema, "CREATE TABLE live(id INTEGER);");
        let saved_settings = crate::Settings::read(&root, id).unwrap();
        assert_eq!(saved_settings.name, "Draft app");
        assert_eq!(saved_settings.visibility, "public");
        assert_eq!(saved_settings.released.as_ref().unwrap().source_digest.as_deref(), Some("old-digest"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_refused_save_leaves_app_settings_and_installed_record_unchanged() {
        let root = std::env::temp_dir().join(format!("konae-refused-source-save-{}", Uuid::new_v4()));
        let id = "app_save_failure";
        let settings = crate::Settings::new(id, "Live", "live", &json!({"version":1,"visibility":"public"}));
        settings.write(&root).await.unwrap();
        let initial = json!({"id":id,"name":"Live","slug":"live","files":{"index.html":"old"},"directories":[],"manifest":{"version":1,"visibility":"public"},"updatedAt":1});
        write(&root, id, &initial).await.unwrap();
        let installed = crate::Installed { hostname:"live.localhost".into(), created_at:1, released_at:Some(2), source_digest:Some("old".into()), actions:"old actions".into(), artifact_version:"v1".into(), released:None };
        let deployment = settings.deployment(installed, "http://localhost");
        crate::write_record_for_test(&root, &deployment);
        let app = layout::sources(&root, id);
        let settings_path = layout::settings(&root, id);
        let record_path = layout::record(&root, id);
        let before_app = std::fs::read(app.join("index.html")).unwrap();
        let before_settings = std::fs::read(&settings_path).unwrap();
        let before_record = std::fs::read(&record_path).unwrap();

        let mut refused = initial.clone();
        refused.as_object_mut().unwrap().remove("name");
        refused["files"]["index.html"] = json!("must not replace the live source");
        assert!(write(&root, id, &refused).await.is_err());
        assert_eq!(std::fs::read(app.join("index.html")).unwrap(), before_app);
        assert_eq!(std::fs::read(&settings_path).unwrap(), before_settings);
        assert_eq!(std::fs::read(&record_path).unwrap(), before_record);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn a_file_that_is_not_utf8_is_saved_as_its_bytes_and_reads_back_as_base64() {
        let root = std::env::temp_dir().join(format!("konae-sources-{}", Uuid::new_v4()));
        let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xff];
        let with_bom = "\u{feff}{\"name\":\"A\"}";
        let source = json!({
            "id": "app_a", "name": "A", "slug": "a",
            "files": {"index.html": "<main></main>", "public/logo.png": base64::engine::general_purpose::STANDARD.encode(png), "public/site.webmanifest": with_bom},
            "encodings": {"public/logo.png": "base64"},
            "directories": [], "manifest": {"version": 1, "visibility":"private"}, "updatedAt": 5,
        });
        setup_settings(&root, "app_a", &source).await;
        write(&root, "app_a", &source).await.unwrap();
        assert_eq!(std::fs::read(layout::sources(&root, "app_a").join("public/logo.png")).unwrap(), png);
        assert_eq!(std::fs::read_to_string(layout::sources(&root, "app_a").join("public/site.webmanifest")).unwrap(), with_bom, "a byte order mark stays");
        assert_eq!(read(&root, "app_a").await.unwrap().unwrap(), source);
        assert_eq!(
            digest_folder(&layout::sources(&root, "app_a")).await.unwrap(),
            digest(decode_document(&source).unwrap().iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice()))),
        );

        let mut stale = source.clone();
        stale["encodings"] = json!({"public/gone.png": "base64"});
        assert_eq!(write(&root, "app_a", &stale).await.unwrap_err().to_string(), "The sources name an encoding for app/public/gone.png, which they do not hold.");
        let mut garbled = source.clone();
        garbled["files"]["public/logo.png"] = json!("not base64!");
        assert_eq!(write(&root, "app_a", &garbled).await.unwrap_err().to_string(), "app/public/logo.png is marked base64 but is not valid base64.");
        assert_eq!(read(&root, "app_a").await.unwrap().unwrap(), source, "a refused save changes nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_release_writes_every_file_into_app_as_its_bytes_and_a_failed_one_puts_app_back() {
        let root = std::env::temp_dir().join(format!("konae-sources-{}", Uuid::new_v4()));
        let files: Files = [("index.html".to_owned(), b"<main></main>".to_vec()), ("public/logo.png".to_owned(), vec![0x89, 0, 0xff])].into();
        let metadata = json!({"name":"A","slug":"a","manifest":{"version":1},"directories":[],"updatedAt":1});
        let staged = stage_release(&root, "app_a", &files, &metadata).await.unwrap();
        assert!(!layout::sources(&root, "app_a").exists(), "staging leaves app/ alone");
        staged.swap().await.unwrap();
        staged.finish().await;
        assert_eq!(std::fs::read(layout::sources(&root, "app_a").join("public/logo.png")).unwrap(), vec![0x89, 0, 0xff]);

        let next: Files = [("index.html".to_owned(), b"<main>2</main>".to_vec())].into();
        let staged = stage_release(&root, "app_a", &next, &metadata).await.unwrap();
        staged.swap().await.unwrap();
        assert_eq!(std::fs::read(layout::sources(&root, "app_a").join("index.html")).unwrap(), b"<main>2</main>");
        staged.put_back().await;
        assert_eq!(std::fs::read(layout::sources(&root, "app_a").join("index.html")).unwrap(), b"<main></main>", "a failed release puts the old app/ back");
        assert!(layout::sources(&root, "app_a").join("public/logo.png").exists());
        let leftovers = std::fs::read_dir(layout::private(&root, "app_a")).unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("app-"))
            .count();
        assert_eq!(leftovers, 0, "no staged or retired sources stay behind");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_source_rules_match_every_case_the_editor_runs() {
        let file: Value = serde_json::from_str(include_str!("sources-cases.json")).unwrap();
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for case in cases {
            let mut files: Vec<(String, usize)> = case["files"].as_object().unwrap().iter().map(|(path, bytes)| (path.clone(), bytes.as_u64().unwrap() as usize)).collect();
            files.extend((0..case["more"].as_u64().unwrap_or(0)).map(|n| (format!("more/{n}.txt"), 1)));
            crate::refusal::assert_case(case, check(files.iter().map(|(path, bytes)| (path.as_str(), *bytes))));
        }
    }

    #[tokio::test]
    async fn saved_sources_and_the_digest_skip_dot_entries_and_node_modules() {
        let root = std::env::temp_dir().join(format!("konae-sources-{}", Uuid::new_v4()));
        let source = json!({"id": "app_a", "name": "A", "slug": "a", "files": {"index.html": "<main></main>"}, "directories": [], "manifest": {"version": 1, "visibility":"private"}, "updatedAt": 5});
        setup_settings(&root, "app_a", &source).await;
        write(&root, "app_a", &source).await.unwrap();
        let app = layout::sources(&root, "app_a");
        std::fs::write(app.join(".DS_Store"), [0u8, 0, 0, 1, 0xff]).unwrap();
        std::fs::create_dir_all(app.join("public/.cache")).unwrap();
        std::fs::write(app.join("public/.cache/x"), "1").unwrap();
        std::fs::create_dir_all(app.join("node_modules/x")).unwrap();
        std::fs::write(app.join("node_modules/x/index.js"), "1").unwrap();
        assert_eq!(read(&root, "app_a").await.unwrap().unwrap(), source);
        assert_eq!(digest_folder(&app).await.unwrap(), digest([("index.html", b"<main></main>".as_slice())]));
        let _ = std::fs::remove_dir_all(root);
    }
}
