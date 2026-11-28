use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const MEMBER_TTL_MS: i64 = 10 * 60 * 1000;
const RESERVATION_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Clone)]
pub(super) struct Credential {
    pub id: String,
    pub app_id: String,
    pub username: String,
    pub destination: String,
    pub kind: String,
    pub user_id: Option<String>,
    pub label: Option<String>,
    pub upload_id: Option<String>,
    pub generation: i64,
    pub expires_at: Option<i64>,
    pub salt: Vec<u8>,
    pub verifier: Vec<u8>,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("app_id", &self.app_id)
            .field("username", &self.username)
            .field("destination", &self.destination)
            .field("kind", &self.kind)
            .field("user_id", &self.user_id)
            .field("label", &self.label)
            .field("upload_id", &self.upload_id)
            .field("generation", &self.generation)
            .field("expires_at", &self.expires_at)
            .field("salt", &"[redacted]")
            .field("verifier", &"[redacted]")
            .finish()
    }
}

pub(super) struct Issued {
    pub credential: Credential,
    pub password: String,
    pub upload_id: Option<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn database(root: &Path, app_id: &str) -> Result<Connection> {
    let directory = crate::layout::data(root, app_id);
    std::fs::create_dir_all(&directory)?;
    let path = crate::layout::secrets(root, app_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let db = Connection::open(&path).with_context(|| {
        format!(
            "Could not open private FTPS credential store {}.",
            path.display()
        )
    })?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS __konae_ftps_credentials (
            id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            username TEXT NOT NULL UNIQUE,
            destination TEXT NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('member','device')),
            user_id TEXT,
            label TEXT,
            upload_id TEXT,
            generation INTEGER NOT NULL DEFAULT 1,
            salt BLOB NOT NULL,
            verifier BLOB NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER,
            revoked_at INTEGER,
            reserved_upload_id TEXT,
            reserved_at INTEGER,
            consumed_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS __konae_ftps_app_credentials
            ON __konae_ftps_credentials(app_id, created_at);",
    )?;
    let current = now_ms();
    db.execute("DELETE FROM __konae_ftps_credentials WHERE id IN (SELECT id FROM __konae_ftps_credentials WHERE expires_at IS NOT NULL AND expires_at<=?1 AND reserved_upload_id IS NULL ORDER BY expires_at LIMIT 32)", [current - 24 * 60 * 60 * 1000])?;
    db.execute("UPDATE __konae_ftps_credentials SET reserved_upload_id=NULL,reserved_at=NULL WHERE id IN (SELECT id FROM __konae_ftps_credentials WHERE reserved_at IS NOT NULL AND reserved_at<?1 ORDER BY reserved_at LIMIT 32)", [current - RESERVATION_TTL_MS])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        if wal.exists() {
            std::fs::set_permissions(wal, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(db)
}

fn password() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn verifier(salt: &[u8], password: &str) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"konae-ftps-credential-v1\0");
    hash.update(salt);
    hash.update(password.as_bytes());
    hash.finalize().to_vec()
}

fn username(app_key: &str, id: &str) -> String {
    format!("k1.{app_key}.{id}")
}

fn create(
    root: &Path,
    app_id: &str,
    app_key: &str,
    destination: &str,
    kind: &str,
    user_id: Option<&str>,
    label: Option<&str>,
    expires_at: Option<i64>,
) -> Result<Issued> {
    ensure!(
        kind == "member" || kind == "device",
        "Invalid FTPS credential kind."
    );
    let id = Uuid::new_v4().simple().to_string();
    let password = password();
    let salt = Uuid::new_v4().as_bytes().to_vec();
    let verifier = verifier(&salt, &password);
    let name = username(app_key, &id);
    let upload_id = (kind == "member").then(|| Uuid::new_v4().to_string());
    let mut db = database(root, app_id)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if kind == "member" {
        let outstanding: usize = tx.query_row(
            "SELECT count(*) FROM __konae_ftps_credentials WHERE app_id=?1 AND user_id=?2 AND kind='member' AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>?3",
            params![app_id, user_id, now_ms()], |row| row.get(0),
        )?;
        ensure!(
            outstanding < 4,
            "This account already has four active FTPS upload tickets."
        );
    } else {
        let active_devices: usize = tx.query_row(
            "SELECT count(*) FROM __konae_ftps_credentials WHERE app_id=?1 AND kind='device' AND revoked_at IS NULL",
            [app_id], |row| row.get(0),
        )?;
        ensure!(
            active_devices < 100,
            "This app already has 100 active FTPS devices."
        );
    }
    tx.execute("INSERT INTO __konae_ftps_credentials(id,app_id,username,destination,kind,user_id,label,salt,verifier,created_at,expires_at,upload_id)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![id, app_id, name, destination, kind, user_id, label, salt, verifier, now_ms(), expires_at, upload_id])?;
    tx.commit()?;
    let credential = Credential {
        id,
        app_id: app_id.into(),
        username: name,
        destination: destination.into(),
        kind: kind.into(),
        user_id: user_id.map(str::to_owned),
        label: label.map(str::to_owned),
        upload_id: upload_id.clone(),
        generation: 1,
        expires_at,
        salt,
        verifier,
    };
    Ok(Issued {
        credential,
        password,
        upload_id,
    })
}

pub(super) fn issue_member(
    root: &Path,
    app_id: &str,
    app_key: &str,
    destination: &str,
    user_id: &str,
) -> Result<Issued> {
    create(
        root,
        app_id,
        app_key,
        destination,
        "member",
        Some(user_id),
        None,
        Some(now_ms() + MEMBER_TTL_MS),
    )
}

pub(super) fn create_device(
    root: &Path,
    app_id: &str,
    app_key: &str,
    destination: &str,
    label: &str,
) -> Result<Issued> {
    create(
        root,
        app_id,
        app_key,
        destination,
        "device",
        None,
        Some(label),
        None,
    )
}

pub(super) fn rotate_device(
    root: &Path,
    app_id: &str,
    _app_key: &str,
    credential_id: &str,
) -> Result<Issued> {
    let mut db = database(root, app_id)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored: Option<(String, String, String, i64)> = tx.query_row(
        "SELECT destination,username,label,generation FROM __konae_ftps_credentials WHERE id=?1 AND app_id=?2 AND kind='device' AND revoked_at IS NULL",
        params![credential_id, app_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let (destination, name, label, generation) =
        stored.context("FTPS device credential not found.")?;
    let password = password();
    let salt = Uuid::new_v4().as_bytes().to_vec();
    let verifier = verifier(&salt, &password);
    let next_generation = generation + 1;
    let changed = tx.execute("UPDATE __konae_ftps_credentials SET salt=?3,verifier=?4,generation=?5,reserved_upload_id=NULL,reserved_at=NULL WHERE id=?1 AND app_id=?2 AND revoked_at IS NULL", params![credential_id, app_id, salt, verifier, next_generation])?;
    ensure!(
        changed == 1,
        "FTPS device credential changed during rotation."
    );
    tx.commit()?;
    Ok(Issued {
        credential: Credential {
            id: credential_id.into(),
            app_id: app_id.into(),
            username: name,
            destination,
            kind: "device".into(),
            user_id: None,
            label: Some(label),
            upload_id: None,
            generation: next_generation,
            expires_at: None,
            salt,
            verifier,
        },
        password,
        upload_id: None,
    })
}

pub(super) fn revoke(root: &Path, app_id: &str, credential_id: &str) -> Result<bool> {
    let changed = database(root, app_id)?.execute("UPDATE __konae_ftps_credentials SET revoked_at=?3, reserved_upload_id=NULL, reserved_at=NULL WHERE id=?1 AND app_id=?2 AND kind='device' AND revoked_at IS NULL", params![credential_id, app_id, now_ms()])?;
    Ok(changed == 1)
}

fn map_credential(row: &rusqlite::Row<'_>) -> rusqlite::Result<Credential> {
    Ok(Credential {
        id: row.get(0)?,
        app_id: row.get(1)?,
        username: row.get(2)?,
        destination: row.get(3)?,
        kind: row.get(4)?,
        user_id: row.get(5)?,
        label: row.get(6)?,
        upload_id: row.get(7)?,
        generation: row.get(8)?,
        expires_at: row.get(9)?,
        salt: row.get(10)?,
        verifier: row.get(11)?,
    })
}

pub(super) fn verify(
    root: &Path,
    app_id: &str,
    username: &str,
    password: &str,
) -> Result<Option<Credential>> {
    let credential = database(root, app_id)?.query_row("SELECT id,app_id,username,destination,kind,user_id,label,upload_id,generation,expires_at,salt,verifier FROM __konae_ftps_credentials WHERE app_id=?1 AND username=?2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)", params![app_id, username, now_ms()], map_credential).optional()?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    let supplied = verifier(&credential.salt, password);
    Ok(bool::from(supplied.as_slice().ct_eq(credential.verifier.as_slice())).then_some(credential))
}

pub(super) fn authorized(
    root: &Path,
    app_id: &str,
    credential: &Credential,
    upload_id: &str,
) -> Result<bool> {
    let found: Option<bool> = database(root, app_id)?.query_row(
        "SELECT (kind='device' OR user_id=?2) AND reserved_upload_id=?3 AND consumed_at IS NULL
         FROM __konae_ftps_credentials WHERE id=?1 AND app_id=?4 AND generation=?5 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?6)",
        params![credential.id, credential.user_id, upload_id, app_id, credential.generation, now_ms()], |row| row.get(0),
    ).optional()?;
    Ok(found.unwrap_or(false))
}

pub(super) fn ticket_status(
    root: &Path,
    upload_id: &str,
    app_id: &str,
    user_id: &str,
) -> Result<Option<(bool, bool)>> {
    database(root, app_id)?.query_row(
        "SELECT expires_at<=?4,consumed_at IS NOT NULL FROM __konae_ftps_credentials WHERE upload_id=?1 AND app_id=?2 AND kind='member' AND user_id=?3",
        params![upload_id, app_id, user_id, now_ms()], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(Into::into)
}

/// Reserves an upload atomically. Expired and revoked credentials cannot reserve.
pub(super) fn reserve(
    root: &Path,
    app_id: &str,
    id: &str,
    generation: i64,
    upload_id: &str,
    user_id: Option<&str>,
) -> Result<bool> {
    let now = now_ms();
    let changed = database(root, app_id)?.execute(
        "UPDATE __konae_ftps_credentials SET reserved_upload_id=?2,reserved_at=?3
        WHERE id=?1 AND app_id=?6 AND generation=?7 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)
        AND consumed_at IS NULL AND (kind='device' OR user_id=?4)
        AND (kind='device' OR upload_id=?2)
        AND (reserved_upload_id IS NULL OR reserved_at<?5)",
        params![id, upload_id, now, user_id, now - RESERVATION_TTL_MS, app_id, generation],
    )?;
    Ok(changed == 1)
}

pub(super) fn finish(
    root: &Path,
    app_id: &str,
    id: &str,
    upload_id: &str,
    accepted: bool,
) -> Result<bool> {
    let now = now_ms();
    let mut db = database(root, app_id)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let kind: Option<String> = tx.query_row("SELECT kind FROM __konae_ftps_credentials WHERE id=?1 AND app_id=?3 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id, app_id], |r| r.get(0)).optional()?;
    let Some(kind) = kind else {
        return Ok(false);
    };
    if accepted && kind == "member" {
        tx.execute("UPDATE __konae_ftps_credentials SET consumed_at=?4,reserved_upload_id=NULL,reserved_at=NULL WHERE id=?1 AND app_id=?3 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id, app_id, now])?;
    } else {
        tx.execute("UPDATE __konae_ftps_credentials SET reserved_upload_id=NULL,reserved_at=NULL WHERE id=?1 AND app_id=?3 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id, app_id])?;
    }
    tx.commit()?;
    Ok(true)
}

pub(super) fn mark_receipt(root: &Path, app_id: &str, upload_id: &str) -> Result<bool> {
    let changed = database(root, app_id)?.execute(
        "UPDATE __konae_ftps_credentials SET consumed_at=COALESCE(consumed_at,?2),reserved_upload_id=NULL,reserved_at=NULL
         WHERE app_id=?1 AND kind='member' AND upload_id=?3",
        params![app_id, now_ms(), upload_id],
    )?;
    Ok(changed == 1)
}

pub(super) fn clear_stopped_reservations(root: &Path, app_id: &str) -> Result<usize> {
    let path = crate::layout::secrets(root, app_id);
    if !path.is_file() {
        return Ok(0);
    }
    let connection = Connection::open(path)?;
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='__konae_ftps_credentials' AND type='table')",
        [], |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    Ok(connection.execute(
        "UPDATE __konae_ftps_credentials SET reserved_upload_id=NULL,reserved_at=NULL WHERE reserved_upload_id IS NOT NULL",
        [],
    )?)
}

pub(super) fn list(root: &Path, app_id: &str) -> Result<Value> {
    let db = database(root, app_id)?;
    let mut statement = db.prepare("SELECT id,destination,label,created_at FROM __konae_ftps_credentials WHERE app_id=?1 AND kind='device' AND revoked_at IS NULL ORDER BY created_at DESC")?;
    let rows = statement.query_map([app_id], |row| Ok(json!({"credentialId":row.get::<_,String>(0)?,"destination":row.get::<_,String>(1)?,"label":row.get::<_,Option<String>>(2)?,"createdAt":row.get::<_,i64>(3)?})))?;
    Ok(Value::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("konae-ftps-credentials-{}", Uuid::new_v4()))
    }

    #[test]
    fn verifier_is_salted_and_credentials_are_scoped_revocable_and_one_shot() {
        let root = temp_root();
        let member = issue_member(&root, "app", "app", "photos", "user-1").unwrap();
        assert_eq!(member.upload_id.as_deref().unwrap().len(), 36);
        assert_eq!(
            crate::layout::secrets(&root, "app"),
            root.join("apps/app/data/secrets.sqlite")
        );
        let debug = format!("{:?}", member.credential);
        assert!(!debug.contains(&hex::encode(&member.credential.salt)));
        assert!(!debug.contains(&hex::encode(&member.credential.verifier)));
        assert_eq!(
            verify(&root, "app", &member.credential.username, &member.password)
                .unwrap()
                .unwrap()
                .user_id
                .as_deref(),
            Some("user-1")
        );
        assert!(
            verify(&root, "app", &member.credential.username, "wrong")
                .unwrap()
                .is_none()
        );
        let upload_id = member.upload_id.as_deref().unwrap();
        assert!(
            !reserve(
                &root,
                "app",
                &member.credential.id,
                1,
                upload_id,
                Some("user-2")
            )
            .unwrap()
        );
        assert!(
            reserve(
                &root,
                "app",
                &member.credential.id,
                1,
                upload_id,
                Some("user-1")
            )
            .unwrap()
        );
        assert!(
            !reserve(
                &root,
                "app",
                &member.credential.id,
                1,
                "another-ticket",
                Some("user-1")
            )
            .unwrap()
        );
        assert!(finish(&root, "app", &member.credential.id, upload_id, true).unwrap());
        assert!(
            !reserve(
                &root,
                "app",
                &member.credential.id,
                1,
                upload_id,
                Some("user-1")
            )
            .unwrap()
        );

        let device = create_device(&root, "app", "app", "meters", "field unit").unwrap();
        assert!(reserve(&root, "app", &device.credential.id, 1, "device-a", None).unwrap());
        assert!(finish(&root, "app", &device.credential.id, "device-a", true).unwrap());
        assert!(reserve(&root, "app", &device.credential.id, 1, "device-b", None).unwrap());
        assert!(revoke(&root, "app", &device.credential.id).unwrap());
        assert!(!reserve(&root, "app", &device.credential.id, 1, "device-c", None).unwrap());
        assert!(
            verify(&root, "app", &device.credential.username, &device.password)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn abandoned_reservations_recover_and_rotation_invalidates_old_secret() {
        let root = temp_root();
        let device = create_device(&root, "app", "app", "meters", "sensor").unwrap();
        assert!(reserve(&root, "app", &device.credential.id, 1, "pending", None).unwrap());
        database(&root, "app")
            .unwrap()
            .execute(
                "UPDATE __konae_ftps_credentials SET reserved_at=0 WHERE id=?1",
                [&device.credential.id],
            )
            .unwrap();
        assert!(reserve(&root, "app", &device.credential.id, 1, "interrupted", None).unwrap());
        assert_eq!(clear_stopped_reservations(&root, "app").unwrap(), 1);
        assert!(
            reserve(
                &root,
                "app",
                &device.credential.id,
                1,
                "after-restart",
                None
            )
            .unwrap()
        );
        let rotated = rotate_device(&root, "app", "app", &device.credential.id).unwrap();
        assert_eq!(rotated.credential.id, device.credential.id);
        assert_eq!(rotated.credential.username, device.credential.username);
        assert_eq!(rotated.credential.label, device.credential.label);
        assert_eq!(
            rotated.credential.generation,
            device.credential.generation + 1
        );
        assert!(
            verify(&root, "app", &device.credential.username, &device.password)
                .unwrap()
                .is_none()
        );
        assert!(
            verify(
                &root,
                "app",
                &rotated.credential.username,
                &rotated.password
            )
            .unwrap()
            .is_some()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
