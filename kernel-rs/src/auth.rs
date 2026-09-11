use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SESSION_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;
const MIN_PASSWORD_BYTES: usize = 8;
const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_EMAIL_BYTES: usize = 254;
const PASSWORD_HASH_ITERATIONS: u32 = 600_000;
const PASSWORD_HASH_BYTES: usize = 32;
const PASSWORD_SALT_BYTES: usize = 16;
const DUMMY_PASSWORD_HASH: &str = "pbkdf2-sha256$600000$6c6974746c65626f782d64756d6d792d73616c74$0000000000000000000000000000000000000000000000000000000000000000";

/// One app's password and session store. The caller must pass that app's own
/// identity database path and resolve the app before authenticating a cookie.
pub struct AuthService {
    path: PathBuf,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub user_id: String,
    pub name: String,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub token: String,
    pub user: AuthUser,
    pub expires_at: i64,
}

#[derive(Debug)]
pub enum AuthError {
    InvalidEmail,
    WeakPassword,
    InvalidCredentials,
    EmailTaken,
    Storage(rusqlite::Error),
    PasswordHash,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEmail => write!(f, "Enter a valid email address."),
            Self::WeakPassword => write!(f, "Password must contain 8 to 1024 bytes."),
            Self::InvalidCredentials => write!(f, "Email or password is incorrect."),
            Self::EmailTaken => write!(f, "An account with that email already exists."),
            Self::Storage(_) | Self::PasswordHash => write!(f, "Authentication storage failed."),
        }
    }
}

impl Error for AuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for AuthError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

impl AuthService {
    /// Opens or creates the per-app identity database and installs its tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        let service = Self {
            path: path.as_ref().to_path_buf(),
        };
        let connection = service.connection()?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS app_users (
               id TEXT PRIMARY KEY,
               email TEXT NOT NULL UNIQUE COLLATE NOCASE,
               password_hash TEXT NOT NULL,
               role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'member')),
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS app_sessions (
               token_hash TEXT PRIMARY KEY,
               user_id TEXT NOT NULL REFERENCES app_users(id) ON DELETE CASCADE,
               expires_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS app_sessions_expiry ON app_sessions(expires_at);",
            )
            .map_err(AuthError::Storage)?;
        Ok(service)
    }

    /// Creates a member account and immediately issues its session.
    /// Open signup is for the personal local prototype and is not production-ready.
    pub fn signup(&self, email: &str, password: &str) -> Result<AuthSession, AuthError> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let password_hash = hash_password(password)?;
        let user = AuthUser {
            user_id: Uuid::new_v4().to_string(),
            name: email.clone(),
            role: "member".into(),
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(AuthError::Storage)?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO app_users (id, email, password_hash, role, created_at)
             VALUES (?1, ?2, ?3, 'member', ?4)",
                params![user.user_id, email, password_hash, now_seconds()],
            )
            .map_err(AuthError::Storage)?;
        if inserted == 0 {
            return Err(AuthError::EmailTaken);
        }
        let session = issue_session(&transaction, user)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(session)
    }

    /// Checks credentials without revealing whether the email exists.
    pub fn login(&self, email: &str, password: &str) -> Result<AuthSession, AuthError> {
        let email = normalize_email(email).map_err(|_| AuthError::InvalidCredentials)?;
        validate_password(password).map_err(|_| AuthError::InvalidCredentials)?;
        let connection = self.connection()?;
        let record = connection.query_row(
            "SELECT id, email, password_hash, role FROM app_users WHERE email = ?1 COLLATE NOCASE",
            [&email],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        ).optional().map_err(AuthError::Storage)?;
        let Some((user_id, email, password_hash, role)) = record else {
            let _ = verify_password(password, DUMMY_PASSWORD_HASH);
            return Err(AuthError::InvalidCredentials);
        };
        verify_password(password, &password_hash)?;
        issue_session(
            &connection,
            AuthUser {
                user_id,
                name: email,
                role,
            },
        )
    }

    /// Resolves a token only inside this app's identity database.
    pub fn authenticate(&self, token: Option<&str>) -> Result<Option<AuthUser>, AuthError> {
        let Some(token) = token.filter(|value| value.len() <= 128 && !value.is_empty()) else {
            return Ok(None);
        };
        let token_hash = hash_token(token);
        let connection = self.connection()?;
        let record = connection
            .query_row(
                "SELECT u.id, u.email, u.role, s.expires_at
             FROM app_sessions s JOIN app_users u ON u.id = s.user_id
             WHERE s.token_hash = ?1",
                [&token_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(AuthError::Storage)?;
        let Some((user_id, email, role, expires_at)) = record else {
            return Ok(None);
        };
        if expires_at <= now_seconds() {
            connection
                .execute(
                    "DELETE FROM app_sessions WHERE token_hash = ?1",
                    [&token_hash],
                )
                .map_err(AuthError::Storage)?;
            return Ok(None);
        }
        Ok(Some(AuthUser {
            user_id,
            name: email,
            role,
        }))
    }

    /// Revokes a token. Repeated logout calls are harmless.
    pub fn logout(&self, token: Option<&str>) -> Result<(), AuthError> {
        let Some(token) = token.filter(|value| value.len() <= 128 && !value.is_empty()) else {
            return Ok(());
        };
        self.connection()?
            .execute(
                "DELETE FROM app_sessions WHERE token_hash = ?1",
                [hash_token(token)],
            )
            .map_err(AuthError::Storage)?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, AuthError> {
        let connection = Connection::open(&self.path).map_err(AuthError::Storage)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(AuthError::Storage)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(AuthError::Storage)?;
        Ok(connection)
    }
}

fn issue_session(connection: &Connection, user: AuthUser) -> Result<AuthSession, AuthError> {
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let now = now_seconds();
    connection
        .execute("DELETE FROM app_sessions WHERE expires_at <= ?1", [now])
        .map_err(AuthError::Storage)?;
    let expires_at = now + SESSION_TTL_SECONDS;
    connection
        .execute(
            "INSERT INTO app_sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![hash_token(&token), user.user_id, expires_at],
        )
        .map_err(AuthError::Storage)?;
    Ok(AuthSession {
        token,
        user,
        expires_at,
    })
}

fn normalize_email(email: &str) -> Result<String, AuthError> {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(AuthError::InvalidEmail);
    };
    if email.len() > MAX_EMAIL_BYTES
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || email.chars().any(char::is_whitespace)
    {
        return Err(AuthError::InvalidEmail);
    }
    Ok(email.to_lowercase())
}

fn validate_password(password: &str) -> Result<(), AuthError> {
    if !(MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&password.len()) {
        return Err(AuthError::WeakPassword);
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let mut salt = [0_u8; PASSWORD_SALT_BYTES];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| AuthError::PasswordHash)?;
    let mut hash = [0_u8; PASSWORD_HASH_BYTES];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(PASSWORD_HASH_ITERATIONS).ok_or(AuthError::PasswordHash)?,
        &salt,
        password.as_bytes(),
        &mut hash,
    );
    Ok(format!(
        "pbkdf2-sha256${PASSWORD_HASH_ITERATIONS}${}${}",
        hex::encode(salt),
        hex::encode(hash)
    ))
}

fn verify_password(password: &str, encoded: &str) -> Result<(), AuthError> {
    let mut parts = encoded.split('$');
    if parts.next() != Some("pbkdf2-sha256") {
        return Err(AuthError::PasswordHash);
    }
    let iterations = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .ok_or(AuthError::PasswordHash)?;
    let salt = parts
        .next()
        .and_then(|value| hex::decode(value).ok())
        .filter(|value| value.len() == PASSWORD_SALT_BYTES)
        .ok_or(AuthError::PasswordHash)?;
    let expected = parts
        .next()
        .and_then(|value| hex::decode(value).ok())
        .filter(|value| value.len() == PASSWORD_HASH_BYTES)
        .ok_or(AuthError::PasswordHash)?;
    if parts.next().is_some() {
        return Err(AuthError::PasswordHash);
    }
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(iterations).ok_or(AuthError::PasswordHash)?,
        &salt,
        password.as_bytes(),
        &expected,
    )
    .map_err(|_| AuthError::InvalidCredentials)
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDb(PathBuf);

    impl TempDb {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("littlebox-auth-{}.sqlite", Uuid::new_v4()));
            Self(path)
        }
        fn service(&self) -> AuthService {
            AuthService::open(&self.0).unwrap()
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
            let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
        }
    }

    #[test]
    fn two_apps_isolate_users_and_sessions() {
        let app_one = TempDb::new();
        let app_two = TempDb::new();
        let one = app_one.service();
        let two = app_two.service();
        let session = one
            .signup("one@example.test", "correct horse battery")
            .unwrap();
        let other_user = one
            .signup("two@example.test", "another correct password")
            .unwrap();
        assert_ne!(session.user.user_id, other_user.user.user_id);
        assert_eq!(session.user.role, "member");
        assert!(one.authenticate(Some(&session.token)).unwrap().is_some());
        assert_eq!(
            one.authenticate(Some(&other_user.token))
                .unwrap()
                .unwrap()
                .user_id,
            other_user.user.user_id
        );
        assert!(two.authenticate(Some(&session.token)).unwrap().is_none());
        assert!(
            two.login("one@example.test", "correct horse battery")
                .is_err()
        );
    }

    #[test]
    fn users_get_stable_ids_across_logout_and_login() {
        let app = TempDb::new();
        let auth = app.service();
        let first = auth
            .signup("Person@Example.test", "correct horse battery")
            .unwrap();
        auth.logout(Some(&first.token)).unwrap();
        assert!(auth.authenticate(Some(&first.token)).unwrap().is_none());
        let second = auth
            .login("person@example.test", "correct horse battery")
            .unwrap();
        assert_eq!(first.user.user_id, second.user.user_id);
        assert_ne!(first.token, second.token);
    }

    #[test]
    fn signup_bounds_inputs_and_does_not_store_plaintext_passwords() {
        let app = TempDb::new();
        let auth = app.service();
        assert!(matches!(
            auth.signup("invalid", "correct horse battery"),
            Err(AuthError::InvalidEmail)
        ));
        assert!(matches!(
            auth.signup("person@example.test", "short"),
            Err(AuthError::WeakPassword)
        ));
        let session = auth
            .signup("person@example.test", "correct horse battery")
            .unwrap();
        let connection = Connection::open(&app.0).unwrap();
        let stored: String = connection
            .query_row(
                "SELECT password_hash FROM app_users WHERE id = ?1",
                [&session.user.user_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored.starts_with("pbkdf2-sha256$600000$"));
        assert!(!stored.contains("correct horse battery"));
        assert!(matches!(
            auth.signup("PERSON@example.test", "another correct password"),
            Err(AuthError::EmailTaken)
        ));
    }
}
