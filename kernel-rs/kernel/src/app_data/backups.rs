use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, copies, database_path, import, layout, now_ms, replace_database_file};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBackup {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub size_bytes: u64,
    pub key: String,



    #[serde(default)]
    pub holds: Vec<String>,
}

impl DatabaseBackup {
    /// The backup as `list-backups` lists it.
    pub fn view(&self) -> Value {
        json!({"id": self.id, "label": self.label, "createdAt": self.created_at, "sizeBytes": self.size_bytes, "holds": self.holds})
    }
}

pub fn backup_dir(state: &AppState, id: &str) -> PathBuf {
    layout::backups(&state.data_root, id)
}

pub async fn list_backups(state: &AppState, id: &str) -> Result<Vec<Value>> {
    let dir = backup_dir(state, id);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut backups = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let backup: DatabaseBackup = serde_json::from_slice(&tokio::fs::read(entry.path()).await?)?;
        backups.push(backup.view());
    }
    backups.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
    Ok(backups)
}

pub async fn create_backup(state: &AppState, id: &str, label: &str) -> Result<Value> {
    let db_path = database_path(state, id);
    let dir = backup_dir(state, id);
    tokio::fs::create_dir_all(&dir).await?;
    let backup_id = Uuid::new_v4().to_string();
    let snapshot_path = dir.join(format!("{backup_id}.sqlite"));
    copies::vacuum_into(&db_path, &snapshot_path)?;
    let size = tokio::fs::metadata(&snapshot_path).await?.len();
    let backup = DatabaseBackup {
        id: backup_id.clone(),
        label: label.chars().take(120).collect(),
        created_at: now_ms(),
        size_bytes: size,
        key: format!("{backup_id}.sqlite"),
        holds: Vec::new(),
    };
    tokio::fs::write(
        dir.join(format!("{backup_id}.json")),
        serde_json::to_vec(&backup)?,
    )
    .await?;
    Ok(backup.view())
}

pub async fn restore_backup(state: &AppState, id: &str, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }

    let directory = backup_dir(state, id);
    let record = copies::history::checked_record(&directory, backup_id)?;
    if !record.holds.is_empty() {
        let deployment = state.deployments.read().await.values().find(|d| d.id == id).cloned().context("Deployment not found.")?;
        return import::restore_held(state, &deployment, &record).await;
    }
    let backup_path = backup_dir(state, id).join(format!("{backup_id}.sqlite"));
    let bytes = tokio::fs::read(&backup_path).await?;
    create_backup(state, id, "Before restore").await?;
    let temp_path = database_path(state, id).with_extension("restore.sqlite");
    tokio::fs::write(&temp_path, &bytes).await?;
    let check = Connection::open(&temp_path)?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    drop(check);
    if integrity != "ok" {
        let _ = tokio::fs::remove_file(&temp_path).await;
        anyhow::bail!("The backup failed SQLite's integrity check.");
    }
    let _generation = state.copies.generations.read().await;
    replace_database_file(&temp_path, &database_path(state, id))?;
    Ok(json!({"id":backup_id,"sizeBytes":bytes.len()}))
}

/// Prepares a source backup's published functions before the caller takes app locks.
pub async fn prepare_backup(state: &AppState, id: &str, backup_id: &str) -> Result<crate::functions::PreparedPin> {
    if backup_id.is_empty() || backup_id.contains('/') || backup_id.contains("..") {
        anyhow::bail!("Invalid backup ID.");
    }
    let record = copies::history::checked_record(&backup_dir(state, id), backup_id)?;
    if !record.holds.iter().any(|part| part == import::HOLDS_SOURCES) {
        return Ok(crate::functions::PreparedPin::default());
    }
    let release_path = backup_dir(state, id).join(&record.key).join("release.json");
    if !tokio::fs::try_exists(&release_path).await? {
        return Ok(crate::functions::PreparedPin::default());
    }
    let release: Value = serde_json::from_slice(&tokio::fs::read(release_path).await?)?;
    let artifact: Option<crate::functions::ServerArtifact> =
        serde_json::from_value(release["serverArtifact"].clone())?;
    crate::functions::prepare_artifact(state, artifact.as_ref()).await
}
