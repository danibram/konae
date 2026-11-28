use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const DATABASE: &str = "secrets.sqlite";
const MEMBER_TTL_MS: i64 = 10 * 60 * 1000;
const RESERVATION_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub(super) struct Credential {
    pub id: String,
    pub app_id: String,
    pub username: String,
    pub destination: String,
    pub kind: String,
    pub user_id: Option<String>,
    pub expires_at: Option<i64>,
    pub salt: Vec<u8>,
    pub verifier: Vec<u8>,
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

fn database(root: &Path) -> Result<Connection> {
    std::fs::create_dir_all(root)?;
    let path = root.join(DATABASE);
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
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
    let mut db = database(root)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("INSERT INTO __konae_ftps_credentials(id,app_id,username,destination,kind,user_id,label,salt,verifier,created_at,expires_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![id, app_id, name, destination, kind, user_id, label, salt, verifier, now_ms(), expires_at])?;
    tx.commit()?;
    let credential = Credential {
        id,
        app_id: app_id.into(),
        username: name,
        destination: destination.into(),
        kind: kind.into(),
        user_id: user_id.map(str::to_owned),
        expires_at,
        salt,
        verifier,
    };
    let upload_id = (kind == "member").then(|| Uuid::new_v4().simple().to_string());
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
    app_key: &str,
    credential_id: &str,
) -> Result<Issued> {
    let mut db = database(root)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let destination: Option<String> = tx.query_row("SELECT destination FROM __konae_ftps_credentials WHERE id=?1 AND app_id=?2 AND kind='device' AND revoked_at IS NULL", params![credential_id, app_id], |r| r.get(0)).optional()?;
    let destination = destination.context("FTPS device credential not found.")?;
    let id = Uuid::new_v4().simple().to_string();
    let password = password();
    let salt = Uuid::new_v4().as_bytes().to_vec();
    let verifier = verifier(&salt, &password);
    let name = username(app_key, &id);
    tx.execute("UPDATE __konae_ftps_credentials SET revoked_at=?3 WHERE id=?1 AND app_id=?2 AND revoked_at IS NULL", params![credential_id, app_id, now_ms()])?;
    tx.execute("INSERT INTO __konae_ftps_credentials(id,app_id,username,destination,kind,user_id,label,salt,verifier,created_at,expires_at)
        VALUES(?1,?2,?3,?4,'device',NULL,'rotated',?5,?6,?7,NULL)", params![id, app_id, name, destination, salt, verifier, now_ms()])?;
    tx.commit()?;
    Ok(Issued {
        credential: Credential {
            id,
            app_id: app_id.into(),
            username: name,
            destination,
            kind: "device".into(),
            user_id: None,
            expires_at: None,
            salt,
            verifier,
        },
        password,
        upload_id: None,
    })
}

pub(super) fn revoke(root: &Path, app_id: &str, credential_id: &str) -> Result<bool> {
    let changed = database(root)?.execute("UPDATE __konae_ftps_credentials SET revoked_at=?3, reserved_upload_id=NULL, reserved_at=NULL WHERE id=?1 AND app_id=?2 AND revoked_at IS NULL", params![credential_id, app_id, now_ms()])?;
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
        expires_at: row.get(6)?,
        salt: row.get(7)?,
        verifier: row.get(8)?,
    })
}

pub(super) fn verify(root: &Path, username: &str, password: &str) -> Result<Option<Credential>> {
    let credential = database(root)?.query_row("SELECT id,app_id,username,destination,kind,user_id,expires_at,salt,verifier FROM __konae_ftps_credentials WHERE username=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2)", params![username, now_ms()], map_credential).optional()?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    let supplied = verifier(&credential.salt, password);
    Ok(bool::from(supplied.as_slice().ct_eq(credential.verifier.as_slice())).then_some(credential))
}

/// Reserves an upload atomically. Expired and revoked credentials cannot reserve.
pub(super) fn reserve(
    root: &Path,
    id: &str,
    upload_id: &str,
    user_id: Option<&str>,
) -> Result<bool> {
    let now = now_ms();
    let changed = database(root)?.execute(
        "UPDATE __konae_ftps_credentials SET reserved_upload_id=?2,reserved_at=?3
        WHERE id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)
        AND consumed_at IS NULL AND (kind='device' OR user_id=?4)
        AND (reserved_upload_id IS NULL OR reserved_at<?5)",
        params![id, upload_id, now, user_id, now - RESERVATION_TTL_MS],
    )?;
    Ok(changed == 1)
}

pub(super) fn finish(root: &Path, id: &str, upload_id: &str, accepted: bool) -> Result<bool> {
    let now = now_ms();
    let mut db = database(root)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let kind: Option<String> = tx.query_row("SELECT kind FROM __konae_ftps_credentials WHERE id=?1 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id], |r| r.get(0)).optional()?;
    let Some(kind) = kind else {
        return Ok(false);
    };
    if accepted && kind == "member" {
        tx.execute("UPDATE __konae_ftps_credentials SET consumed_at=?3,reserved_upload_id=NULL,reserved_at=NULL WHERE id=?1 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id, now])?;
    } else {
        tx.execute("UPDATE __konae_ftps_credentials SET reserved_upload_id=NULL,reserved_at=NULL WHERE id=?1 AND reserved_upload_id=?2 AND revoked_at IS NULL", params![id, upload_id])?;
    }
    tx.commit()?;
    Ok(true)
}

pub(super) fn recover(root: &Path) -> Result<usize> {
    Ok(database(root)?.execute("UPDATE __konae_ftps_credentials SET reserved_upload_id=NULL,reserved_at=NULL WHERE reserved_at IS NOT NULL AND reserved_at<?1", [now_ms() - RESERVATION_TTL_MS])?)
}

pub(super) fn list(root: &Path, app_id: &str) -> Result<Value> {
    let db = database(root)?;
    let mut statement = db.prepare("SELECT id,destination,kind,label,created_at,expires_at,revoked_at,consumed_at FROM __konae_ftps_credentials WHERE app_id=?1 ORDER BY created_at DESC")?;
    let rows = statement.query_map([app_id], |row| Ok(json!({"credentialId":row.get::<_,String>(0)?,"destination":row.get::<_,String>(1)?,"kind":row.get::<_,String>(2)?,"label":row.get::<_,Option<String>>(3)?,"createdAt":row.get::<_,i64>(4)?,"expiresAt":row.get::<_,Option<i64>>(5)?,"revoked":row.get::<_,Option<i64>>(6)?.is_some(),"consumed":row.get::<_,Option<i64>>(7)?.is_some()})))?;
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
        assert_eq!(
            verify(&root, &member.credential.username, &member.password)
                .unwrap()
                .unwrap()
                .user_id
                .as_deref(),
            Some("user-1")
        );
        assert!(
            verify(&root, &member.credential.username, "wrong")
                .unwrap()
                .is_none()
        );
        assert!(
            reserve(&root, &member.credential.id, "upload-a", Some("user-2")).unwrap() == false
        );
        assert!(reserve(&root, &member.credential.id, "upload-a", Some("user-1")).unwrap());
        assert!(!reserve(&root, &member.credential.id, "upload-b", Some("user-1")).unwrap());
        assert!(finish(&root, &member.credential.id, "upload-a", true).unwrap());
        assert!(!reserve(&root, &member.credential.id, "upload-b", Some("user-1")).unwrap());

        let device = create_device(&root, "app", "app", "meters", "field unit").unwrap();
        assert!(reserve(&root, &device.credential.id, "device-a", None).unwrap());
        assert!(finish(&root, &device.credential.id, "device-a", true).unwrap());
        assert!(reserve(&root, &device.credential.id, "device-b", None).unwrap());
        assert!(revoke(&root, "app", &device.credential.id).unwrap());
        assert!(!reserve(&root, &device.credential.id, "device-c", None).unwrap());
        assert!(
            verify(&root, &device.credential.username, &device.password)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn abandoned_reservations_recover_and_rotation_invalidates_old_secret() {
        let root = temp_root();
        let device = create_device(&root, "app", "app", "meters", "sensor").unwrap();
        assert!(reserve(&root, &device.credential.id, "pending", None).unwrap());
        database(&root)
            .unwrap()
            .execute(
                "UPDATE __konae_ftps_credentials SET reserved_at=0 WHERE id=?1",
                [&device.credential.id],
            )
            .unwrap();
        assert_eq!(recover(&root).unwrap(), 1);
        let rotated = rotate_device(&root, "app", "app", &device.credential.id).unwrap();
        assert!(
            verify(&root, &device.credential.username, &device.password)
                .unwrap()
                .is_none()
        );
        assert!(
            verify(&root, &rotated.credential.username, &rotated.password)
                .unwrap()
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
