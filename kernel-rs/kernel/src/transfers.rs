//! Transfers: how bytes travel over MCP in blocks, 4 MiB for an
//! administrative upload or download such as a release or an export. The SDK
//! sends an app user's file in one request body, `PUT /_konae/transfers/<id>`,
//! on a transfer opened the same way. Each transfer is a file under `transfers/` and a row in
//! `transfers.sqlite`, which also keeps the receipt of each finished file
//! upload so a retry gets the same answer.

use super::*;
use rusqlite::params;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// The largest block of an app user's transfer. The runtime endpoint reads a
/// request of at most 200 KB, and the SDK built into every published app
/// refuses a larger read block.
pub const CHUNK: usize = 64 * 1024;
/// The largest block of an owner's transfer: a 26 MB release in at most seven
/// writes and a 256 MiB database export in 64 reads. One write holds about
/// five copies of its block in memory (body, parsed JSON, arguments, decoded
/// bytes and the check's re-encoding), about 27 MB at this size.
pub const ADMIN_CHUNK: usize = 4 * 1024 * 1024;
/// The largest request the owner's MCP reads: one `transfer_write` of
/// [`ADMIN_CHUNK`] bytes in base64 plus its JSON-RPC envelope.
pub const ADMIN_MCP_BODY_BYTES: usize = ADMIN_CHUNK.div_ceil(3) * 4 + 64 * 1024;

/// The block size a transfer of `principal` moves in: [`CHUNK`] for an app
/// user or visitor, [`ADMIN_CHUNK`] for an owner.
pub fn chunk_bytes(principal: &str) -> usize {
    if principal.starts_with("runtime:") { CHUNK } else { ADMIN_CHUNK }
}
pub const MAX_TRANSFER: usize = 32 * 1024 * 1024;
/// Active transfers every principal outside the runtime shares.
pub const MAX_ADMIN_TRANSFERS: usize = 128;
pub const MAX_DEPLOYMENT_TRANSFERS: usize = 32;
/// Active transfers the signed-out visitors of one Deployment share, apart
/// from the signed-in users' pool.
pub const MAX_VISITOR_TRANSFERS: usize = 16;
/// Active transfers one administrative principal may hold.
pub const MAX_ADMIN_PRINCIPAL_TRANSFERS: usize = 8;
/// Active transfers one app user may hold. Files are at most 2 MiB, so an app
/// needs one upload or download at a time; two allows a retry.
pub const MAX_RUNTIME_PRINCIPAL_TRANSFERS: usize = 2;
pub const ADMIN_TRANSFER_TTL_MS: u64 = 60 * 60 * 1000;
pub const RUNTIME_TRANSFER_TTL_MS: u64 = 10 * 60 * 1000;

/// The database of transfers and file upload receipts, under the data root.
pub const TRANSFERS_DATABASE: &str = "transfers.sqlite";

pub fn store(state: &AppState) -> Result<Connection> {
    let db = Connection::open(state.data_root.join(TRANSFERS_DATABASE))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS transfers(id TEXT PRIMARY KEY, principal TEXT NOT NULL, body TEXT NOT NULL, expires INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS file_receipts(transfer_id TEXT PRIMARY KEY, principal TEXT NOT NULL, request_hash TEXT NOT NULL, body TEXT NOT NULL);")?;
    Ok(db)
}

pub fn transfer_path(state: &AppState, id: &str) -> Result<PathBuf> {
    anyhow::ensure!(Uuid::parse_str(id).is_ok(), "Invalid transfer ID.");
    Ok(state.data_root.join("transfers").join(id))
}


pub fn transfer(state: &AppState, principal: &str, a: &Value) -> Result<Value> {
    let id = a["transferId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Transfer ID required."))?;
    transfer_path(state, id)?;
    let body: Option<String> = store(state)?
        .query_row(
            "SELECT body FROM transfers WHERE id=?1 AND principal=?2 AND expires>?3",
            params![id, principal, now_ms()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(&body.ok_or_else(|| {
        anyhow::anyhow!("Transfer not found or expired.")
    })?)?)
}


async fn cleanup_transfers(state: &AppState) -> Result<()> {
    let ids = {
        let db = store(state)?;
        let mut query = db.prepare("SELECT id FROM transfers WHERE expires<=?1")?;
        query
            .query_map([now_ms()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        let _ = tokio::fs::remove_file(transfer_path(state, &id)?).await;
        store(state)?.execute(
            "DELETE FROM transfers WHERE id=?1 AND expires<=?2",
            params![id, now_ms()],
        )?;
    }
    Ok(())
}


pub async fn new_transfer(state: &AppState, principal: &str, value: Value) -> Result<Value> {
    cleanup_transfers(state).await?;
    let _guard = database_lock(state, "transfer-quota").await;
    let db = store(state)?;
    let count: usize = db.query_row(
        "SELECT COUNT(*) FROM transfers WHERE principal=?1",
        [principal],
        |r| r.get(0),
    )?;
    // App users share one pool per deployment and administrative callers
    // share another, so runtime traffic cannot exhaust the slots releases and
    // exports need.
    // App user transfers also expire sooner, so abandoned ones free the
    // Deployment's pool quickly.
    // Signed-out visitors of a public app, one principal per client address,
    // share a smaller pool of their own, so visitors from a handful of
    // networks cannot hold the slots signed-in users upload with.
    let (pool, excluded, limit, per_principal, ttl) = match principal.strip_prefix("runtime:") {
        Some(rest) => {
            let deployment = rest.split(':').next().unwrap_or("");
            let visitors = format!("runtime:{deployment}:anon:");
            if principal.starts_with(&visitors) {
                (visitors, None, MAX_VISITOR_TRANSFERS, MAX_RUNTIME_PRINCIPAL_TRANSFERS, RUNTIME_TRANSFER_TTL_MS)
            } else {
                (format!("runtime:{deployment}:"), Some(visitors), MAX_DEPLOYMENT_TRANSFERS, MAX_RUNTIME_PRINCIPAL_TRANSFERS, RUNTIME_TRANSFER_TTL_MS)
            }
        }
        None => (String::new(), Some("runtime:".to_owned()), MAX_ADMIN_TRANSFERS, MAX_ADMIN_PRINCIPAL_TRANSFERS, ADMIN_TRANSFER_TTL_MS),
    };
    let total: usize = db.query_row(
        "SELECT COUNT(*) FROM transfers WHERE substr(principal,1,length(?1))=?1
           AND (?2 IS NULL OR substr(principal,1,length(?2))<>?2)",
        params![&pool, excluded],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        count < per_principal && total < limit,
        "Too many active transfers. Cancel completed transfers before starting another."
    );
    let id = Uuid::new_v4().to_string();
    let expires = now_ms() + ttl;
    tokio::fs::create_dir_all(state.data_root.join("transfers")).await?;
    tokio::fs::write(transfer_path(state, &id)?, []).await?;
    db.execute(
        "INSERT INTO transfers(id,principal,body,expires) VALUES(?1,?2,?3,?4)",
        params![id, principal, value.to_string(), expires],
    )?;
    Ok(
        json!({"transferId":id,"size":value["size"],"sha256":value["sha256"],"chunkBytes":chunk_bytes(principal),"expiresAt":expires}),
    )
}


pub async fn download(
    state: &AppState,
    principal: &str,
    bytes: &[u8],
    project: Option<&str>,
) -> Result<Value> {
    anyhow::ensure!(
        bytes.len() <= MAX_TRANSFER,
        "Download exceeds transfer limit."
    );
    let result=new_transfer(state,principal,json!({"direction":"download","size":bytes.len(),"sha256":digest(bytes),"complete":true,"projectId":project})).await?;
    tokio::fs::write(
        transfer_path(state, result["transferId"].as_str().unwrap())?,
        bytes,
    )
    .await?;
    Ok(result)
}


/// Streams a response body into a new download transfer, hashing it on the
/// way, so a file is never held in memory whole.
pub async fn download_body(state: &AppState, principal: &str, body: Body) -> Result<Value> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let directory = state.data_root.join("transfers");
    tokio::fs::create_dir_all(&directory).await?;
    let scratch = directory.join(format!(".download-{}", Uuid::new_v4()));
    let written = async {
        let mut output = tokio::fs::File::create(&scratch).await?;
        let mut hasher = sha2::Sha256::default();
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            sha2::Digest::update(&mut hasher, &chunk);
            output.write_all(&chunk).await?;
            size += chunk.len() as u64;
        }
        output.flush().await?;
        Ok::<_, anyhow::Error>((size, hex::encode(sha2::Digest::finalize(hasher))))
    }
    .await;
    let created = match written {
        Ok((size, sha256)) => new_transfer(state, principal, json!({"direction":"download","size":size,"sha256":sha256,"complete":true,"projectId":null})).await,
        Err(error_value) => Err(error_value),
    };
    let moved = match created {
        Ok(created) => match tokio::fs::rename(&scratch, transfer_path(state, created["transferId"].as_str().unwrap_or_default())?).await {
            Ok(()) => return Ok(created),
            Err(error_value) => Err(error_value.into()),
        },
        Err(error_value) => Err(error_value),
    };
    let _ = tokio::fs::remove_file(&scratch).await;
    moved
}


/// Writes a request body as the whole of an open upload transfer and
/// completes it. A body longer than the transfer's size stops at the first
/// byte past it. A retry after a broken request starts over from byte 0.
pub async fn receive_body(state: &AppState, principal: &str, id: &str, body: Body) -> Result<Value> {
    use futures_util::StreamExt;
    let path = transfer_path(state, id)?;
    {
        let _guard = database_lock(state, &format!("transfer:{id}")).await;
        let t = transfer(state, principal, &json!({"transferId": id}))?;
        anyhow::ensure!(t["direction"] == "upload" && t["complete"] != true, "Transfer is immutable.");
        let size = t["size"].as_u64().unwrap_or(0);
        let mut file = tokio::fs::File::create(&path).await?;
        let mut written = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            written += chunk.len() as u64;
            anyhow::ensure!(written <= size, "Upload size or SHA-256 mismatch.");
            file.write_all(&chunk).await?;
        }
        file.sync_data().await?;
    }
    transfer_call(state, principal, "transfer_complete", &json!({"transferId": id}), true).await
}

pub async fn transfer_call(
    state: &AppState,
    principal: &str,
    name: &str,
    a: &Value,
    runtime: bool,
) -> Result<Value> {
    if name == "transfer_begin" {
        let size = a["size"].as_u64().unwrap_or(0);
        let hash = a["sha256"].as_str().unwrap_or("");
        let component = a["component"].as_str().filter(|_| !runtime);
        let limit = match component {
            _ if runtime => state.files.limits.max_file_bytes,
            Some(component) => super::package::component_limit(component, &state.files.limits)?,
            None => MAX_TRANSFER as u64,
        };
        match component {
            Some(component) => anyhow::ensure!(
                size > 0 && size <= limit,
                "Transfer size is outside the limit of {limit} bytes for the {component} component."
            ),
            None if runtime => anyhow::ensure!(size > 0 && size <= limit, "{}", state.files.limits.file_size_refused()),
            None => anyhow::ensure!(size > 0 && size <= limit, "Transfer size is outside the limit."),
        }
        anyhow::ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid SHA-256 checksum."
        );
        let mut body = json!({"direction":"upload","size":size,"sha256":hash,"complete":false,"projectId":a["projectId"]});
        if let Some(component) = component {
            super::package::ensure_upload_space(state, size)?;
            body["component"] = json!(component);
        }
        return new_transfer(state, principal, body).await;
    }
    let id = a["transferId"].as_str().unwrap();
    let path = transfer_path(state, id)?;
    let _guard = database_lock(state, &format!("transfer:{id}")).await;
    let mut t = transfer(state, principal, a)?;
    match name {
        "transfer_cancel" => {
            store(state)?.execute(
                "DELETE FROM transfers WHERE id=?1 AND principal=?2",
                params![id, principal],
            )?;
            let _ = tokio::fs::remove_file(path).await;
            Ok(json!({"ok":true}))
        }
        "transfer_status" => Ok(
            json!({"transferId":id,"offset":tokio::fs::metadata(path).await?.len(),"complete":t["complete"],"size":t["size"],"sha256":t["sha256"]}),
        ),
        "transfer_read" => {
            anyhow::ensure!(t["complete"] == true, "Transfer is incomplete.");
            let offset = a["offset"].as_u64().unwrap();
            let size = t["size"].as_u64().unwrap();
            anyhow::ensure!(offset <= size, "Offset exceeds transfer size.");
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut data = vec![0; chunk_bytes(principal).min((size - offset) as usize)];
            file.read_exact(&mut data).await?;
            Ok(
                json!({"data":encode_base64(&data),"offset":offset,"nextOffset":offset+data.len() as u64,"size":size,"sha256":t["sha256"]}),
            )
        }
        "transfer_write" => {
            anyhow::ensure!(
                t["direction"] == "upload" && t["complete"] != true,
                "Transfer is immutable."
            );
            let encoded = a["data"].as_str().unwrap();
            let bytes = decode_base64(encoded)?;
            anyhow::ensure!(
                !bytes.is_empty() && bytes.len() <= chunk_bytes(principal) && encode_base64(&bytes) == encoded,
                "Invalid base64 block."
            );
            let offset = a["offset"].as_u64().unwrap();
            let size = t["size"].as_u64().unwrap();
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .await?;
            let length = file.metadata().await?.len();
            anyhow::ensure!(
                offset <= length && offset + bytes.len() as u64 <= size,
                "Invalid block offset or size."
            );
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            if offset < length {
                anyhow::ensure!(
                    offset + bytes.len() as u64 <= length,
                    "Retry overlaps the upload head."
                );
                let mut previous = vec![0; bytes.len()];
                file.read_exact(&mut previous).await?;
                anyhow::ensure!(previous == bytes, "Retry does not match the stored block.");
            } else {
                file.write_all(&bytes).await?;
                file.sync_data().await?;
            }
            Ok(json!({"nextOffset":file.metadata().await?.len()}))
        }
        "transfer_complete" => {
            anyhow::ensure!(t["direction"] == "upload", "Not an upload.");
            // Hashes in 64 KiB reads, so a package component never sits in memory.
            let (size, sha256) =
                tokio::task::spawn_blocking(move || super::copies::checksum_file(&path)).await??;
            anyhow::ensure!(
                size == t["size"].as_u64().unwrap() && sha256 == t["sha256"],
                "Upload size or SHA-256 mismatch."
            );
            t["complete"] = json!(true);
            store(state)?.execute(
                "UPDATE transfers SET body=?1 WHERE id=?2",
                params![t.to_string(), id],
            )?;
            Ok(json!({"transferId":id,"complete":true,"sha256":t["sha256"]}))
        }
        _ => anyhow::bail!("Unknown transfer tool."),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_body_completes_an_upload_and_a_wrong_one_leaves_it_open() {
        let root = std::env::temp_dir().join(format!("konae-transfer-body-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let bytes = vec![7u8; 3 * CHUNK + 5];
        let sha256 = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes));
        let alice = "runtime:d1:alice";
        let begun = transfer_call(&state, alice, "transfer_begin", &json!({"size": bytes.len(), "sha256": sha256}), true).await.unwrap();
        let id = begun["transferId"].as_str().unwrap();
        let send = |principal: &'static str, body: Vec<u8>| receive_body(&state, principal, id, Body::from(body));
        let stranger = send("runtime:d1:bob", bytes.clone()).await.unwrap_err().to_string();
        assert_eq!(stranger, "Transfer not found or expired.");
        let longer = send(alice, [bytes.clone(), vec![1]].concat()).await.unwrap_err().to_string();
        assert_eq!(longer, "Upload size or SHA-256 mismatch.");
        let mut changed = bytes.clone();
        changed[CHUNK] = 8;
        assert_eq!(send(alice, changed).await.unwrap_err().to_string(), "Upload size or SHA-256 mismatch.");
        // The retry starts over, so the earlier bytes leave nothing behind.
        let done = send(alice, bytes.clone()).await.unwrap();
        assert_eq!(done["complete"], true);
        assert_eq!(tokio::fs::read(transfer_path(&state, id).unwrap()).await.unwrap(), bytes);
        assert_eq!(send(alice, bytes.clone()).await.unwrap_err().to_string(), "Transfer is immutable.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn visitors_cannot_hold_the_transfers_signed_in_users_need() {
        let root = std::env::temp_dir().join(format!("konae-visitor-transfers-{}", Uuid::new_v4()));
        let (state, _) = crate::tests::login_test_state(&root, "public").await;
        let begin = |principal: String| { let state = state.clone(); async move { new_transfer(&state, &principal, json!({"direction": "download", "size": 1})).await } };
        for client in 0..MAX_VISITOR_TRANSFERS / MAX_RUNTIME_PRINCIPAL_TRANSFERS {
            for _ in 0..MAX_RUNTIME_PRINCIPAL_TRANSFERS {
                begin(format!("runtime:d1:anon:198.51.100.{client}")).await.unwrap();
            }
        }
        let refused = begin("runtime:d1:anon:203.0.113.9".into()).await.unwrap_err().to_string();
        assert!(refused.contains("Too many active transfers"), "{refused}");
        // Signed-in users still have their whole pool.
        for user in 0..MAX_DEPLOYMENT_TRANSFERS / MAX_RUNTIME_PRINCIPAL_TRANSFERS {
            for _ in 0..MAX_RUNTIME_PRINCIPAL_TRANSFERS {
                begin(format!("runtime:d1:user-{user}")).await.unwrap();
            }
        }
        assert!(begin("runtime:d1:user-extra".into()).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn app_users_cannot_take_the_transfer_slots_releases_need() {
        let root = std::env::temp_dir().join(format!("konae-quota-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = AppState::for_test(&root);
        let body = || json!({"direction":"download","size":1,"sha256":"00","complete":true});
        let per_user = MAX_RUNTIME_PRINCIPAL_TRANSFERS;
        for _ in 0..per_user {
            new_transfer(&state, "runtime:app-a:user-0", body()).await.unwrap();
        }
        assert!(
            new_transfer(&state, "runtime:app-a:user-0", body()).await.is_err(),
            "one app user holds at most {per_user} transfers"
        );
        for user in 1..MAX_DEPLOYMENT_TRANSFERS / per_user {
            for _ in 0..per_user {
                new_transfer(&state, &format!("runtime:app-a:user-{user}"), body()).await.unwrap();
            }
        }
        assert!(new_transfer(&state, "runtime:app-a:another", body()).await.is_err());
        new_transfer(&state, "runtime:app-b:user", body()).await.unwrap();
        let admin = new_transfer(&state, "admin:alice", body()).await.unwrap();
        let runtime = new_transfer(&state, "runtime:app-c:user", body()).await.unwrap();
        let ttl = |t: &Value| t["expiresAt"].as_u64().unwrap() - now_ms();
        assert!(ttl(&runtime) <= RUNTIME_TRANSFER_TTL_MS);
        assert!(ttl(&admin) > RUNTIME_TRANSFER_TTL_MS);
        for _ in 1..MAX_ADMIN_PRINCIPAL_TRANSFERS {
            new_transfer(&state, "admin:alice", body()).await.unwrap();
        }
        assert!(new_transfer(&state, "admin:alice", body()).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
