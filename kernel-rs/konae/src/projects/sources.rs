use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::mcp::{self, Owner};
use crate::state::Konae;
use crate::transfers;
use crate::*;

pub(crate) fn validate_sources(source: &Value, id: &str) -> Result<()> {
    let string = |value: &Value, max: usize| value.as_str().is_some_and(|s| s.len() <= max);
    anyhow::ensure!(
        source["id"] == id
            && string(&source["name"], 80)
            && source["name"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
            && string(&source["slug"], 100)
            && source["updatedAt"].as_u64().is_some(),
        "Invalid project metadata. Send {{ id, name, slug, files, directories, manifest, updatedAt }} with the project's own id, a name of 1 to 80 characters, a slug of up to 100 and updatedAt in milliseconds."
    );
    anyhow::ensure!(
        source["files"].is_object(),
        "Invalid project files. Send files as an object of each path inside app/ and its contents."
    );
    crate::sources::check_files(&crate::sources::decode_document(source)?)?;
    let directories = source["directories"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid project directories."))?;
    anyhow::ensure!(
        directories.len() <= crate::sources::MAX_FILES
            && directories
                .iter()
                .all(|entry| entry.as_str().is_some_and(crate::sources::holds)),
        "Invalid project directories."
    );
    let manifest = &source["manifest"];
    anyhow::ensure!(
        manifest["version"] == 1
            && ["public", "private"].contains(&manifest["visibility"].as_str().unwrap_or("")),
        "Invalid project manifest. Send version 1 and visibility \"public\" or \"private\"."
    );
    Ok(())
}

pub(crate) fn merge_source_delta(current: &Value, delta: &Value, id: &str) -> Result<Value> {
    anyhow::ensure!(delta["format"] == "delta", "Invalid source update format.");
    let mut merged = current.clone();
    let mut files = merged["files"]
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Saved project files are unavailable."))?;
    let mut encodings = merged["encodings"].as_object().cloned().unwrap_or_default();
    let changed = delta["files"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid changed project files."))?;
    let changed_encodings = delta["encodings"].as_object();
    if let Some(encodings) = changed_encodings {
        anyhow::ensure!(
            encodings
                .iter()
                .all(|(path, encoding)| changed.contains_key(path) && encoding == "base64"),
            "Invalid changed project file encoding."
        );
    }
    for (path, contents) in changed {
        anyhow::ensure!(contents.is_string(), "Invalid changed project file.");
        files.insert(path.clone(), contents.clone());
        if changed_encodings.is_some_and(|items| items.get(path).is_some()) {
            encodings.insert(path.clone(), json!("base64"));
        } else {
            encodings.remove(path);
        }
    }
    let removals = delta["removedPaths"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid removed project paths."))?;
    for path in removals {
        let path = path
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid removed project path."))?;
        files.remove(path);
        encodings.remove(path);
    }
    merged["files"] = Value::Object(files);
    if encodings.is_empty() {
        merged.as_object_mut().unwrap().remove("encodings");
    } else {
        merged["encodings"] = Value::Object(encodings);
    }
    let metadata = delta["metadata"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid project metadata update."))?;
    for key in ["name", "slug", "directories", "manifest", "updatedAt"] {
        let value = metadata
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("Source metadata is missing {key}."))?;
        merged[key] = value.clone();
    }
    merged["id"] = json!(id);
    Ok(merged)
}

pub(crate) fn ensure_no_component(upload: &Value) -> Result<()> {
    match upload["component"].as_str() {
        Some(component) => anyhow::bail!(
            "This upload names the {component} component of an export. Upload sources and releases without a component."
        ),
        None => Ok(()),
    }
}

pub(crate) async fn read(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Value> {
    let id = a["projectId"].as_str().unwrap();
    mcp::owned_project(state, owner, id, "read")?;
    let request_id = a["requestId"].as_str();
    let cancel_request_id = a["cancelRequestId"].as_str();
    anyhow::ensure!(
        request_id.is_none() || cancel_request_id.is_none(),
        "Choose requestId or cancelRequestId."
    );
    for request_id in request_id.into_iter().chain(cancel_request_id) {
        anyhow::ensure!(
            !request_id.is_empty()
                && request_id.len() <= 64
                && request_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "Invalid source read request ID."
        );
    }
    let owner_principal = format!("owner:{}", owner.id);
    if let Some(request_id) = cancel_request_id {
        let _guard = database_lock(state, &format!("project:{id}")).await;
        let db = mcp::store(state)?;
        db.execute(
            "DELETE FROM source_read_requests WHERE expires<=?1",
            [now_ms()],
        )?;
        let existing: Option<(String, Option<String>)> = db
                    .query_row(
                        "SELECT project,transfer FROM source_read_requests WHERE owner=?1 AND request_id=?2",
                        params![owner.id, request_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
        if let Some((project, transfer_id)) = existing {
            anyhow::ensure!(
                project == id,
                "Source read request ID belongs to another project."
            );
            if let Some(transfer_id) = transfer_id {
                let _ = transfers::transfer_call(
                    state,
                    &owner_principal,
                    "transfer_cancel",
                    &json!({"transferId":transfer_id}),
                    false,
                )
                .await;
            }
        }
        db.execute(
                    "INSERT INTO source_read_requests(owner,request_id,project,transfer,cancelled,expires) VALUES(?1,?2,?3,NULL,1,?4) ON CONFLICT(owner,request_id) DO UPDATE SET cancelled=1,expires=excluded.expires",
                    params![owner.id, request_id, id, now_ms().saturating_add(300_000)],
                )?;
        return Ok(json!({"cancelled":true}));
    }

    let _guard = database_lock(state, &format!("project:{id}")).await;
    let db = mcp::store(state)?;
    db.execute(
        "DELETE FROM source_read_requests WHERE expires<=?1",
        [now_ms()],
    )?;
    let revision: u64 =
        mcp::store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    if a["metadataOnly"] == true {
        return match crate::sources::read_metadata(&state.data_root, id).await? {
            Some(project) => Ok(json!({"revision":revision,"project":project,"transfer":null})),
            None => Ok(json!({"revision":0,"transfer":null})),
        };
    }
    if let Some(request_id) = request_id {
        let existing: Option<(String, bool)> = db
                    .query_row(
                        "SELECT project,cancelled FROM source_read_requests WHERE owner=?1 AND request_id=?2",
                        params![owner.id, request_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
        if let Some((project, cancelled)) = existing {
            anyhow::ensure!(
                project == id,
                "Source read request ID belongs to another project."
            );
            if cancelled {
                return Ok(json!({"revision":revision,"transfer":null,"cancelled":true}));
            }
            anyhow::bail!("Source read request ID is already active.");
        }
    }
    match crate::sources::read(&state.data_root, id).await? {
        Some(source) => {
            let transfer =
                transfers::download(state, principal, &serde_json::to_vec(&source)?, Some(id))
                    .await?;
            if let Some(request_id) = request_id {
                db.execute(
                            "INSERT INTO source_read_requests(owner,request_id,project,transfer,cancelled,expires) VALUES(?1,?2,?3,?4,0,?5)",
                            params![owner.id, request_id, id, transfer["transferId"].as_str(), now_ms().saturating_add(300_000)],
                        )?;
            }
            Ok(
                json!({"revision":revision,"project":{"id":source["id"],"name":source["name"],"slug":source["slug"],"manifest":source["manifest"]},"transfer":transfer}),
            )
        }
        None => Ok(json!({"revision":0,"transfer":null})),
    }
}

pub(crate) async fn save(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Value> {
    let id = a["projectId"].as_str().unwrap();
    let transfer_id = a["transferId"].as_str().unwrap();
    let upload = transfers::transfer(state, principal, &json!({"transferId":transfer_id}))?;
    anyhow::ensure!(
        upload["complete"] == true && upload["direction"] == "upload" && upload["projectId"] == id,
        "Completed upload for this project required."
    );
    ensure_no_component(&upload)?;
    let bytes = tokio::fs::read(transfers::transfer_path(state, transfer_id)?).await?;
    let update: Value = serde_json::from_slice(&bytes)?;
    let _guard = database_lock(state, &format!("project:{id}")).await;
    mcp::owned_project(state, owner, id, "write")?;
    let current: u64 =
        mcp::store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    anyhow::ensure!(
        current == a["expectedRevision"].as_u64().unwrap(),
        "Source revision conflict. Reload the saved project before writing."
    );
    let previous = crate::sources::read(&state.data_root, id).await?;
    let source = if update["format"] == "delta" {
        let saved = previous
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Saved project sources are unavailable."))?;
        merge_source_delta(&saved, &update, id)?
    } else {
        update
    };
    anyhow::ensure!(
        current == 0 || previous.is_some(),
        "Saved project sources are unavailable."
    );
    validate_sources(&source, id)?;
    let revision = current
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("Source revision exhausted."))?;
    let pending = crate::sources::stage_write(&state.data_root, id, &source).await?;
    let update_result = mcp::store(state).and_then(|db| {
        db.execute(
            "UPDATE projects SET sources=?2 WHERE id=?1",
            params![id, revision],
        )
        .map_err(Into::into)
    });
    if let Err(error) = update_result {
        if let Err(restore) = pending.rollback().await {
            anyhow::bail!(
                "Saving sources failed: {error}. Restoring the saved sources failed: {restore:#}"
            );
        }
        return Err(error.into());
    }
    pending.commit().await?;
    Ok(json!({"revision":revision}))
}
