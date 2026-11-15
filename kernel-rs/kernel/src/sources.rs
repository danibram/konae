//! The sources the editor saves, kept as files in the app's `app/`.
//!
//! The editor sends and receives one JSON document, `{ id, name, slug,
//! files, encodings, directories, manifest, updatedAt }`. Its files become
//! the files of `app/`, and the rest goes to `.konae/sources.json`. A file
//! whose bytes are UTF-8 is its text. Any other file, such as an image, is
//! base64, and `encodings` names it with the value `base64`, as a Build
//! Artifact does. A save writes the new files beside `app/` and then puts
//! them in its place, so a reader sees the old sources or the new ones, never
//! a mix. Callers hold the project lock.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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

/// Writes `source` into the app `id`'s `app/` and its metadata into `.konae/`.
pub async fn write(root: &Path, id: &str, source: &Value) -> Result<()> {
    let files = decode_document(source)?;
    let private = layout::private(root, id);
    tokio::fs::create_dir_all(&private).await?;
    let metadata = json!({
        "name": source["name"], "slug": source["slug"], "manifest": source["manifest"],
        "directories": source["directories"], "updatedAt": source["updatedAt"],
    });
    let staged = stage_release(root, id, &files, &metadata).await?;
    if let Err(error_value) = staged.swap().await {
        staged.discard().await;
        return Err(error_value);
    }
    staged.finish().await;
    Ok(())
}

/// A release's files written beside `app/`, which the release puts in its
/// place together with `dist/`.
pub struct StagedRelease {
    app: PathBuf,
    staged: PathBuf,
    retired: PathBuf,
    metadata: PathBuf,
    staged_metadata: PathBuf,
    retired_metadata: PathBuf,
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
    let metadata = private.join(layout::SOURCES_METADATA);
    let staged_metadata = private.join(format!("sources-{nonce}.json"));
    if let Err(error_value) = crate::write_atomic(&staged_metadata, &serde_json::to_vec_pretty(source_metadata)?).await {
        let _ = tokio::fs::remove_dir_all(&staged).await;
        return Err(error_value.into());
    }
    let retired = private.join(format!("app-retired-{nonce}"));
    let retired_metadata = private.join(format!("sources-retired-{nonce}.json"));
    Ok(StagedRelease { app: layout::sources(root, id), staged, retired, metadata, staged_metadata, retired_metadata })
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
        let had_metadata = match tokio::fs::try_exists(&self.metadata).await {
            Ok(had) => had,
            Err(error_value) => {
                let _ = tokio::fs::remove_dir_all(&self.app).await;
                if had { let _ = tokio::fs::rename(&self.retired, &self.app).await; }
                return Err(error_value.into());
            }
        };
        if had_metadata {
            if let Err(error_value) = tokio::fs::rename(&self.metadata, &self.retired_metadata).await {
                let _ = tokio::fs::remove_dir_all(&self.app).await;
                if had { let _ = tokio::fs::rename(&self.retired, &self.app).await; }
                return Err(error_value.into());
            }
        }
        if let Err(error_value) = tokio::fs::rename(&self.staged_metadata, &self.metadata).await {
            let _ = tokio::fs::remove_dir_all(&self.app).await;
            if had { let _ = tokio::fs::rename(&self.retired, &self.app).await; }
            if had_metadata { let _ = tokio::fs::rename(&self.retired_metadata, &self.metadata).await; }
            return Err(error_value.into());
        }
        Ok(())
    }

    /// Puts the old `app/` back after a later step of the release failed.
    pub async fn put_back(self) {
        let _ = tokio::fs::remove_file(&self.metadata).await;
        if tokio::fs::try_exists(&self.retired_metadata).await.unwrap_or(false) {
            if let Err(error_value) = tokio::fs::rename(&self.retired_metadata, &self.metadata).await {
                eprintln!("Could not put back {} after a failed release: {error_value}", self.metadata.display());
            }
        }
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
        let _ = tokio::fs::remove_file(&self.retired_metadata).await;
        let _ = tokio::fs::remove_file(&self.staged_metadata).await;
    }

    /// Removes the staged files of a release that never swapped.
    pub async fn discard(self) {
        let _ = tokio::fs::remove_dir_all(&self.staged).await;
        let _ = tokio::fs::remove_file(&self.staged_metadata).await;
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
    let metadata = match tokio::fs::read(layout::private(root, id).join(layout::SOURCES_METADATA)).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let app = layout::sources(root, id);
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
    let mut source = json!({
        "id": id, "name": metadata["name"], "slug": metadata["slug"], "files": files,
        "directories": metadata["directories"], "manifest": metadata["manifest"], "updatedAt": metadata["updatedAt"],
    });
    if !encodings.is_empty() {
        source["encodings"] = Value::Object(encodings);
    }
    Ok(Some(source))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "directories": ["public"], "manifest": {"version": 1}, "updatedAt": 5,
        });
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
    async fn a_file_that_is_not_utf8_is_saved_as_its_bytes_and_reads_back_as_base64() {
        let root = std::env::temp_dir().join(format!("konae-sources-{}", Uuid::new_v4()));
        let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xff];
        let with_bom = "\u{feff}{\"name\":\"A\"}";
        let source = json!({
            "id": "app_a", "name": "A", "slug": "a",
            "files": {"index.html": "<main></main>", "public/logo.png": base64::engine::general_purpose::STANDARD.encode(png), "public/site.webmanifest": with_bom},
            "encodings": {"public/logo.png": "base64"},
            "directories": [], "manifest": {"version": 1}, "updatedAt": 5,
        });
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
        let source = json!({"id": "app_a", "name": "A", "slug": "a", "files": {"index.html": "<main></main>"}, "directories": [], "manifest": {"version": 1}, "updatedAt": 5});
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
