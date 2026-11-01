//! The sources the editor saves, kept as files in the app's `app/`.
//!
//! The editor sends and receives one JSON document, `{ id, name, slug,
//! files, directories, manifest, updatedAt }`. Its files become the files of
//! `app/`, and the rest goes to `.konae/sources.json`. A save writes the new
//! files beside `app/` and then puts them in its place, so a reader sees the
//! old sources or the new ones, never a mix. Callers hold the project lock.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::layout;

/// Writes `source` into the app `id`'s `app/` and its metadata into `.konae/`.
pub(crate) async fn write(root: &Path, id: &str, source: &Value) -> Result<()> {
    let private = layout::private(root, id);
    let staged = private.join(format!("app-{}", Uuid::new_v4().simple()));
    let result = async {
        tokio::fs::create_dir_all(&staged).await?;
        for directory in source["directories"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            tokio::fs::create_dir_all(staged.join(directory)).await?;
        }
        for (path, text) in source["files"].as_object().into_iter().flatten() {
            let target = staged.join(path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&target, text.as_str().unwrap_or_default()).await?;
        }
        let app = layout::sources(root, id);
        let retired = private.join(format!("app-retired-{}", Uuid::new_v4().simple()));
        if tokio::fs::try_exists(&app).await? {
            tokio::fs::rename(&app, &retired).await?;
        }
        tokio::fs::rename(&staged, &app).await?;
        let _ = tokio::fs::remove_dir_all(&retired).await;
        let metadata = json!({
            "name": source["name"], "slug": source["slug"], "manifest": source["manifest"],
            "directories": source["directories"], "updatedAt": source["updatedAt"],
        });
        crate::write_atomic(&private.join(layout::SOURCES_METADATA), &serde_json::to_vec_pretty(&metadata)?).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staged).await;
    result
}

/// The saved sources as the editor sent them, or `None` when the app has none.
pub(crate) async fn read(root: &Path, id: &str) -> Result<Option<Value>> {
    let metadata = match tokio::fs::read(layout::private(root, id).join(layout::SOURCES_METADATA)).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let app = layout::sources(root, id);
    let mut files = Map::new();
    let mut pending: Vec<PathBuf> = vec![app.clone()];
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await.with_context(|| format!("{} is not readable.", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let relative = entry.path().strip_prefix(&app)?.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            let text = tokio::fs::read_to_string(entry.path()).await.with_context(|| format!("app/{relative} is not text."))?;
            files.insert(relative, Value::String(text));
        }
    }
    Ok(Some(json!({
        "id": id, "name": metadata["name"], "slug": metadata["slug"], "files": files,
        "directories": metadata["directories"], "manifest": metadata["manifest"], "updatedAt": metadata["updatedAt"],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
