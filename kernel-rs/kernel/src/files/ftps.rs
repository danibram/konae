use super::*;

pub async fn accept_ftps_staged(
    state: &AppState,
    deployment: &Deployment,
    upload_id: &str,
    owner_id: &str,
    relative_path: &str,
    staged_bytes: &Path,
    authorize: impl FnOnce() -> anyhow::Result<()> + Send,
) -> anyhow::Result<String> {
    anyhow::ensure!(valid_file_id(upload_id), "Invalid FTPS upload ID.");
    anyhow::ensure!(
        modules::is_on(&deployment.modules, "files"),
        "Files module is off."
    );
    anyhow::ensure!(crate::accepted_owner_id(owner_id), "FTPS owner is invalid.");
    let size = tokio::fs::metadata(staged_bytes).await?.len();
    anyhow::ensure!(
        size > 0 && size <= state.files.limits.max_file_bytes,
        "FTPS upload exceeds the file size limit."
    );
    let path = staged_bytes.to_owned();
    let (actual_size, sha256) =
        tokio::task::spawn_blocking(move || copies::checksum_file(&path)).await??;
    anyhow::ensure!(actual_size == size, "The staged FTPS upload changed size.");
    let name = relative_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("upload")
        .replace(['/', '\\'], "-")
        .chars()
        .filter(|c| !c.is_control())
        .take(180)
        .collect::<String>();
    let id = Uuid::new_v4().to_string();
    let _copy = state.copies.operations.read().await;
    state
        .files
        .store
        .put(&deployment.id, &id, staged_bytes)
        .await?;
    let result = async {
        let _data = database_lock(state, &deployment.id).await;
        let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
        if let Some((receipt_owner, file_id, status)) = journal_receipt(state, deployment, upload_id)? {
            anyhow::ensure!(receipt_owner == owner_id, "FTPS upload receipt belongs to another owner.");
            if status == "pending" { reconcile_ftps_uploads(state, deployment).await?; }
            return Ok(file_id);
        }
        reconcile_ftps_uploads(state, deployment).await?;
        authorize()?;
        let directory = crate::layout::files(&state.data_root, &deployment.id);
        let mut files = read_file_index(&directory).await.map_err(|_| anyhow::anyhow!("File index is invalid."))?;
        if let Some(refused) = file_room(state, &deployment.id, &files, size).await {
            let body = axum::body::to_bytes(refused.into_body(), 4096).await.unwrap_or_default();
            let message = serde_json::from_slice::<Value>(&body).ok().and_then(|body| body["error"].as_str().map(ToOwned::to_owned)).unwrap_or_else(|| "Files quota or count limit reached.".to_owned());
            anyhow::bail!("{message}");
        }
        let metadata = json!({"id":id,"name":name,"type":"application/octet-stream","size":size,"sha256":sha256,"visibility":"private","ownerId":owner_id,"ownerOnly":true,"ftpsUploadId":upload_id,"createdAt":now_ms()});
        let mut connection = journal_create(state, deployment)?;
        connection.execute_batch("PRAGMA synchronous=FULL;")?;
        {
            let transaction = connection.transaction()?;
            transaction.execute("INSERT INTO __konae_ftps_uploads(upload_id,owner_id,file_id,metadata_json,state) VALUES(?1,?2,?3,?4,'pending')", rusqlite::params![upload_id, owner_id, id, metadata.to_string()])?;
            transaction.commit()?;
        }
        drop(connection);
        files.insert(0, metadata);
        tokio::fs::create_dir_all(&directory).await?;
        write_index_durable(&directory.join("index.json"), &serde_json::to_vec_pretty(&files)?).await?;
        let indexed = journal_create(state, deployment)?.execute("UPDATE __konae_ftps_uploads SET state='indexed' WHERE upload_id=?1 AND state='pending'", [upload_id])?;
        anyhow::ensure!(indexed == 1, "FTPS receipt disappeared before indexing.");
        Ok(id.clone())
    }.await;
    match result {
        Ok(file_id) => {
            if file_id != id {
                let _ = state.files.store.delete(&deployment.id, &id).await;
            }
            Ok(file_id)
        }
        Err(failure) => {
            if matches!(blob_is_journaled(state, deployment, &id), Ok(false)) {
                let _ = state.files.store.delete(&deployment.id, &id).await;
            }
            Err(failure)
        }
    }
}

pub fn ftps_upload_file_id(
    data_root: &Path,
    app_id: &str,
    owner_id: &str,
    upload_id: &str,
) -> anyhow::Result<Option<String>> {
    let db_path = crate::layout::database(data_root, app_id);
    if !db_path.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !has_journal(&db)? {
        return Ok(None);
    }
    let receipt = db
        .query_row(
            "SELECT owner_id,file_id FROM __konae_ftps_uploads WHERE upload_id=?1",
            [upload_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(receipt.and_then(|(owner, file)| (owner == owner_id).then_some(file)))
}

pub async fn ftps_upload_file_id_locked(
    state: &AppState,
    deployment: &Deployment,
    owner_id: &str,
    upload_id: &str,
) -> anyhow::Result<Option<String>> {
    let _data = database_lock(state, &deployment.id).await;
    reconcile_ftps_uploads(state, deployment).await?;
    ftps_upload_file_id(&state.data_root, &deployment.id, owner_id, upload_id)
}

pub async fn reconcile_ftps_uploads(
    state: &AppState,
    deployment: &Deployment,
) -> anyhow::Result<()> {
    let db_path = crate::layout::database(&state.data_root, &deployment.id);
    if !db_path.is_file() {
        return Ok(());
    }
    let pending = {
        let db = Connection::open(&db_path)?;
        if !has_journal(&db)? {
            return Ok(());
        }
        let mut statement = db.prepare("SELECT upload_id,file_id,metadata_json FROM __konae_ftps_uploads WHERE state='pending' ORDER BY upload_id")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if pending.is_empty() {
        return Ok(());
    }
    let directory = crate::layout::files(&state.data_root, &deployment.id);
    let mut files = read_file_index(&directory)
        .await
        .map_err(|_| anyhow::anyhow!("File index is invalid."))?;
    let mut changed = false;
    for (upload_id, file_id, metadata_json) in &pending {
        let metadata: Value = serde_json::from_str(metadata_json)?;
        if let Some(existing) = files
            .iter()
            .find(|file| file["id"].as_str() == Some(file_id.as_str()))
        {
            anyhow::ensure!(
                existing["ownerId"] == metadata["ownerId"]
                    && existing["ftpsUploadId"].as_str() == Some(upload_id.as_str()),
                "FTPS journal and file index disagree."
            );
        } else {
            files.insert(0, metadata);
            changed = true;
        }
    }
    if changed {
        tokio::fs::create_dir_all(&directory).await?;
        write_index_durable(
            &directory.join("index.json"),
            &serde_json::to_vec_pretty(&files)?,
        )
        .await?;
    }
    let db = Connection::open(db_path)?;
    db.execute_batch("PRAGMA synchronous=FULL;")?;
    for (upload_id, _, _) in pending {
        db.execute("UPDATE __konae_ftps_uploads SET state='indexed' WHERE upload_id=?1 AND state='pending'", [upload_id])?;
    }
    Ok(())
}

fn journal_create(state: &AppState, deployment: &Deployment) -> anyhow::Result<Connection> {
    let path = crate::layout::database(&state.data_root, &deployment.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Connection::open(path)?;
    db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS __konae_ftps_uploads(upload_id TEXT PRIMARY KEY,owner_id TEXT NOT NULL,file_id TEXT NOT NULL UNIQUE,metadata_json TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('pending','indexed'))) WITHOUT ROWID;")?;
    Ok(db)
}

fn has_journal(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='__konae_ftps_uploads')", [], |row| row.get(0))?)
}

fn journal_receipt(
    state: &AppState,
    deployment: &Deployment,
    upload_id: &str,
) -> anyhow::Result<Option<(String, String, String)>> {
    let path = crate::layout::database(&state.data_root, &deployment.id);
    if !path.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !has_journal(&db)? {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT owner_id,file_id,state FROM __konae_ftps_uploads WHERE upload_id=?1",
            [upload_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

fn blob_is_journaled(
    state: &AppState,
    deployment: &Deployment,
    file_id: &str,
) -> anyhow::Result<bool> {
    let path = crate::layout::database(&state.data_root, &deployment.id);
    if !path.is_file() {
        return Ok(false);
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !has_journal(&db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM __konae_ftps_uploads WHERE file_id=?1)",
        [file_id],
        |row| row.get(0),
    )?)
}

async fn write_index_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("File index has no parent directory."))?;
    tokio::fs::create_dir_all(parent).await?;
    let temp = path.with_file_name(format!(".index.json.tmp-{}", Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&temp).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temp, path).await?;
    let parent = parent.to_owned();
    tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
        .await
        .map_err(std::io::Error::other)??;
    Ok(())
}
