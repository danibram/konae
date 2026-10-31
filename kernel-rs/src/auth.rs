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
/// Open sessions one account may hold: a sign-in past this ends the oldest.
const MAX_SESSIONS_PER_ACCOUNT: i64 = 20;
/// A verification link stays valid for 24 hours.
pub const VERIFICATION_TTL_SECONDS: i64 = 24 * 60 * 60;
/// An invitation link stays valid for seven days.
pub const INVITATION_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;
/// A recovery link stays valid for 30 minutes.
pub const RECOVERY_TTL_SECONDS: i64 = 30 * 60;
/// The scope of creator accounts in `owners.sqlite`. App accounts use
/// `deployment_scope(id)`, so the two can never collide.
pub const CREATOR_SCOPE: &str = "creator";
/// Stored as the password of an account created pending. It is not a valid
/// encoding, so no password matches it; the person sets one when they verify.
pub(crate) const NO_PASSWORD: &str = "unset";
const MIN_PASSWORD_BYTES: usize = 8;
const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_EMAIL_BYTES: usize = 254;
const PASSWORD_HASH_ITERATIONS: u32 = 600_000;
const PASSWORD_HASH_BYTES: usize = 32;
const PASSWORD_SALT_BYTES: usize = 16;
/// Each identity database may grow to this size. Writes past it fail with
/// "database or disk is full".
pub const MAX_IDENTITY_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
// The salt must be PASSWORD_SALT_BYTES long so unknown emails pay for the full KDF.
const DUMMY_PASSWORD_HASH: &str = "pbkdf2-sha256$600000$6c6974746c65626f782d64756d6d7921$0000000000000000000000000000000000000000000000000000000000000000";

/// The scope of one Deployment's app accounts.
pub fn deployment_scope(deployment_id: &str) -> String {
    format!("deployment:{deployment_id}")
}

/// Whether new accounts must prove control of their email before they are active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Signup creates an active, unverified account and its session, as a
    /// local development installation without email needs.
    Local,
    /// Signup creates a pending account and a verification challenge. Opening
    /// an identity database in this mode moves unverified active accounts to
    /// pending and revokes their sessions.
    Required,
}

impl Verification {
    /// Reads `KONAE_ACCOUNT_VERIFICATION`: `required`, or `local` and unset
    /// for local development.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("KONAE_ACCOUNT_VERIFICATION").as_deref() {
            Err(_) | Ok("") | Ok("local") => Ok(Self::Local),
            Ok("required") => Ok(Self::Required),
            Ok(other) => Err(format!(
                "KONAE_ACCOUNT_VERIFICATION={other} is not valid. Use required or local."
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Required => "required",
        }
    }
}

/// What a challenge proves. A token answers only the purpose it was issued for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Verification,
    Invitation,
    Recovery,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Invitation => "invitation",
            Self::Recovery => "recovery",
        }
    }
}

/// A challenge just issued. The token exists only here and in the message
/// that carries it; the database keeps its hash.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssuedChallenge {
    pub token: String,
    pub email: String,
    pub purpose: Purpose,
    pub scope: String,
    pub expires_at: i64,
}

/// Where a challenge's message is recorded in the transaction that stores
/// the challenge. The identity connection attaches `database()` as
/// `outbox`, and `record` writes there before the commit; an error rolls
/// both back. `token_hash` is the hash the identity database keeps.
pub trait ChallengeOutbox {
    fn database(&self) -> &Path;
    fn record(&self, transaction: &Connection, challenge: &IssuedChallenge, token_hash: &str) -> Result<(), AuthError>;
}

/// One scope's account, password, challenge and session store: a Deployment's
/// `identity.sqlite` or the installation's `owners.sqlite`. The caller must
/// pass the store that belongs to the resolved app or to the creator scope.
pub struct AuthService {
    path: PathBuf,
    scope: String,
    verification: Verification,
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
    AccountPending,
    AccountSuspended,
    /// An invitation reached an email that already has an active account.
    AccountExists,
    InvalidChallenge,
    SignupClosed,
    MailUnavailable,
    /// The mail queue refused to send now: a send limit was reached. Nothing was stored.
    MailLimit(String),
    /// The mail transport is configured but may not send, for example because
    /// its sender domain is not verified. Nothing was stored.
    MailBlocked(String),
    VerificationRequired,
    /// Creator signup without a valid invitation code or installation key.
    AdmissionRefused(&'static str),
    /// An invitation answered with an identity for another email.
    InvitedEmailDiffers,
    /// An outside issuer vouched for the email of the owner's app account,
    /// which never proved that email, so it would take the account back.
    OwnerAccountUnproven,
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
            Self::AccountPending => write!(
                f,
                "Confirm your email before signing in. Use the link we sent, or ask for a new one."
            ),
            Self::AccountSuspended => write!(f, "This account is suspended."),
            Self::AccountExists => write!(
                f,
                "This email already has an account on this app. Sign in with its password instead."
            ),
            Self::InvalidChallenge => write!(f, "This link is invalid, expired or already used."),
            Self::MailUnavailable => write!(
                f,
                "Email is not configured on this installation, so it cannot send account links."
            ),
            Self::MailLimit(message) | Self::MailBlocked(message) => write!(f, "{message}"),
            Self::SignupClosed => {
                write!(f, "This app is not accepting new accounts for this email address.")
            }
            Self::VerificationRequired => {
                write!(f, "New accounts on this installation must confirm their email.")
            }
            Self::AdmissionRefused(message) => write!(f, "{message}"),
            Self::InvitedEmailDiffers => write!(
                f,
                "This invitation is for another email. Continue with the Google account of the invited email."
            ),
            Self::OwnerAccountUnproven => write!(
                f,
                "This email belongs to the app owner's account. The owner opens it with Open as owner in the editor, not with Google."
            ),
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
    /// Opens or creates an identity database and installs its tables, without
    /// a scope for challenges. Used where the caller only installs or reads
    /// the schema, such as importing a portable identity database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        Self::install(path, "", Verification::Local, false)
    }

    /// Opens the store of one scope. In `Required` mode this also moves
    /// accounts that never verified to pending, once per switch to that mode.
    pub fn open_in(
        path: impl AsRef<Path>,
        scope: &str,
        verification: Verification,
    ) -> Result<Self, AuthError> {
        Self::install(path, scope, verification, true)
    }

    fn install(
        path: impl AsRef<Path>,
        scope: &str,
        verification: Verification,
        apply_mode: bool,
    ) -> Result<Self, AuthError> {
        let service = Self {
            path: path.as_ref().to_path_buf(),
            scope: scope.to_owned(),
            verification,
        };
        let mut connection = service.connection()?;
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
        migrate(&mut connection, apply_mode.then_some(verification))?;
        Ok(service)
    }

    /// Creates a member account and immediately issues its session.
    /// The caller enforces the app's signup policy before calling this.
    #[cfg(test)]
    pub fn signup(&self, email: &str, password: &str) -> Result<AuthSession, AuthError> {
        let account = NewAccount::prepare(email, password)?;
        self.create_account(account)
    }

    /// Checks credentials without revealing whether the email exists.
    pub fn login(&self, email: &str, password: &str) -> Result<AuthSession, AuthError> {
        let stored = self.find_login(email, password)?;
        let user = check_password(password, stored)?;
        self.start_session(user)
    }

    /// Stores an account whose password was hashed by `NewAccount::prepare`,
    /// and issues its first session. The UNIQUE email column decides races
    /// between two signups for one address.
    pub fn create_account(&self, account: NewAccount) -> Result<AuthSession, AuthError> {
        self.create_account_admitted(account, |_, _| Ok(()))
    }

    /// `create_account` with an admission check that runs inside the same
    /// transaction, after the account row is written. A refusal rolls the
    /// account back.
    pub fn create_account_admitted(
        &self,
        account: NewAccount,
        admit: impl FnOnce(&Connection, Option<&str>) -> Result<(), AuthError>,
    ) -> Result<AuthSession, AuthError> {
        if self.verification == Verification::Required {
            return Err(AuthError::VerificationRequired);
        }
        let user = AuthUser {
            user_id: Uuid::new_v4().to_string(),
            name: account.email.clone(),
            role: "member".into(),
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(AuthError::Storage)?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO app_users (id, email, password_hash, role, created_at, state)
             VALUES (?1, ?2, ?3, 'member', ?4, 'active')",
                params![user.user_id, account.email, account.password_hash, now_seconds()],
            )
            .map_err(AuthError::Storage)?;
        if inserted == 0 {
            // The admission decides first, so a request without a valid code
            // gets the refusal a new email gets and learns nothing about who
            // has an account. Dropping the transaction keeps a valid code.
            admit(&transaction, None)?;
            return Err(AuthError::EmailTaken);
        }
        admit(&transaction, Some(&user.user_id))?;
        let session = issue_session(&transaction, user)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(session)
    }

    /// Reads the stored login for an email. Malformed input reads nothing and
    /// fails as wrong credentials, so it reveals nothing either.
    pub fn find_login(&self, email: &str, password: &str) -> Result<Option<StoredLogin>, AuthError> {
        let email = normalize_email(email).map_err(|_| AuthError::InvalidCredentials)?;
        validate_password(password).map_err(|_| AuthError::InvalidCredentials)?;
        let connection = self.connection()?;
        let record = connection.query_row(
            "SELECT id, email, password_hash, role FROM app_users WHERE email = ?1 COLLATE NOCASE",
            [&email],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        ).optional().map_err(AuthError::Storage)?;
        Ok(record.map(|(user_id, email, password_hash, role)| StoredLogin {
            user: AuthUser {
                user_id,
                name: email,
                role,
            },
            password_hash,
        }))
    }

    /// Issues a session for a user whose password was checked. Fails as wrong
    /// credentials if the user was deleted since the check, and refuses an
    /// account that is pending or suspended.
    pub fn start_session(&self, user: AuthUser) -> Result<AuthSession, AuthError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(AuthError::Storage)?;
        let state: Option<String> = transaction
            .query_row(
                "SELECT state FROM app_users WHERE id = ?1",
                [&user.user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AuthError::Storage)?;
        match state.as_deref() {
            None => return Err(AuthError::InvalidCredentials),
            Some("active") => {}
            Some("suspended") => return Err(AuthError::AccountSuspended),
            Some(_) => return Err(AuthError::AccountPending),
        }
        let session = issue_session(&transaction, user)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(session)
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
             WHERE s.token_hash = ?1 AND u.state = 'active'",
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

    /// Starts a signup in `Required` mode. A new email gets a pending account
    /// with no password and a verification challenge. A pending email keeps
    /// its account ID and gets a fresh challenge that replaces the earlier
    /// one, so a stranger who registered the address first cannot block its
    /// owner. An active or suspended email gets nothing. The caller answers
    /// all three the same way, so the answer does not reveal who has an account.
    #[allow(dead_code)] // Kept for callers without an outbox; tests use it.
    pub fn request_signup(&self, email: &str) -> Result<Option<IssuedChallenge>, AuthError> {
        self.request_signup_with(email, None, |_, _| Ok(()))
    }

    /// `request_signup` with an admission check and no outbox.
    #[allow(dead_code)] // Used by admission tests and callers without a queue.
    pub fn request_signup_admitted(
        &self,
        email: &str,
        admit: impl FnOnce(&Connection, Option<&str>) -> Result<(), AuthError>,
    ) -> Result<Option<IssuedChallenge>, AuthError> {
        self.request_signup_with(email, None, admit)
    }

    /// `request_signup` with an outbox and an admission check, both inside
    /// the transaction that stores the challenge. The outbox records the
    /// challenge's message so neither commits without the other. `admit`
    /// receives the account the request created or re-linked, or `None` when
    /// the email is active or suspended; a refusal rolls back the pending
    /// account and its challenge.
    pub fn request_signup_with(
        &self,
        email: &str,
        outbox: Option<&dyn ChallengeOutbox>,
        admit: impl FnOnce(&Connection, Option<&str>) -> Result<(), AuthError>,
    ) -> Result<Option<IssuedChallenge>, AuthError> {
        let email = normalize_email(email)?;
        let mut connection = self.connection_with(outbox)?;
        let transaction = immediate(&mut connection)?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT id, state FROM app_users WHERE email = ?1 COLLATE NOCASE",
                [&email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(AuthError::Storage)?;
        let user_id = match existing {
            Some((user_id, state)) if state == "pending" => user_id,
            Some(_) => {
                admit(&transaction, None)?;
                transaction.commit().map_err(AuthError::Storage)?;
                return Ok(None);
            }
            None => {
                let user_id = Uuid::new_v4().to_string();
                transaction
                    .execute(
                        "INSERT INTO app_users (id, email, password_hash, role, created_at, state)
                         VALUES (?1, ?2, ?3, 'member', ?4, 'pending')",
                        params![user_id, email, NO_PASSWORD, now_seconds()],
                    )
                    .map_err(AuthError::Storage)?;
                user_id
            }
        };
        admit(&transaction, Some(&user_id))?;
        let challenge = self.issue_challenge(&transaction, &user_id, &email, Purpose::Verification)?;
        if let Some(outbox) = outbox {
            outbox.record(&transaction, &challenge, &hash_token(&challenge.token))?;
        }
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(Some(challenge))
    }

    /// Starts a recovery. An active account gets a recovery challenge; a
    /// pending one gets a new verification challenge instead, which also
    /// sets its password. Suspended, unknown and malformed emails get nothing.
    /// The caller gives every case the same answer.
    #[allow(dead_code)] // Kept for callers without an outbox; tests use it.
    pub fn request_recovery(&self, email: &str) -> Result<Option<IssuedChallenge>, AuthError> {
        self.request_recovery_with(email, None)
    }

    /// Records the message for a challenge another transaction already
    /// issued, such as an invitation, through the same outbox that signup and
    /// recovery use. The job commits on its own; the challenge exists already.
    pub fn record_issued(&self, outbox: &dyn ChallengeOutbox, challenge: &IssuedChallenge) -> Result<(), AuthError> {
        let mut connection = self.connection_with(Some(outbox))?;
        let transaction = immediate(&mut connection)?;
        outbox.record(&transaction, challenge, &hash_token(&challenge.token))?;
        transaction.commit().map_err(AuthError::Storage)
    }

    /// `request_recovery` with an outbox, as `request_signup_with`.
    pub fn request_recovery_with(
        &self,
        email: &str,
        outbox: Option<&dyn ChallengeOutbox>,
    ) -> Result<Option<IssuedChallenge>, AuthError> {
        let Ok(email) = normalize_email(email) else {
            return Ok(None);
        };
        let mut connection = self.connection_with(outbox)?;
        let transaction = immediate(&mut connection)?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT id, state FROM app_users WHERE email = ?1 COLLATE NOCASE",
                [&email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(AuthError::Storage)?;
        let purpose = match existing.as_ref().map(|(_, state)| state.as_str()) {
            Some("active") => Purpose::Recovery,
            Some("pending") => Purpose::Verification,
            _ => return Ok(None),
        };
        let (user_id, _) = existing.unwrap();
        let challenge = self.issue_challenge(&transaction, &user_id, &email, purpose)?;
        if let Some(outbox) = outbox {
            outbox.record(&transaction, &challenge, &hash_token(&challenge.token))?;
        }
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(Some(challenge))
    }

    /// Answers a verification or recovery challenge with the password the
    /// person sets now. One transaction consumes the token, sets the
    /// password, activates a pending account, revokes every earlier session
    /// and issues a new one. Verification needs a pending account and
    /// `admits(email)` to hold, so the caller can recheck its signup policy;
    /// recovery needs an active account. Any refusal leaves the token unused.
    pub fn complete_challenge(
        &self,
        purpose: Purpose,
        token: &str,
        password: NewPassword,
        admits: impl FnOnce(&str) -> bool,
    ) -> Result<AuthSession, AuthError> {
        if purpose == Purpose::Invitation {
            return Err(AuthError::InvalidChallenge);
        }
        self.complete_challenge_with(purpose, token, password, admits, |_, _, _| Ok(()))
    }

    /// `complete_challenge` for every purpose, with `on_consumed(transaction,
    /// user_id, token_hash)` run inside the transaction before it commits.
    /// An invitation, like a verification, needs a pending account and
    /// `admits(email)`; an active account is told to sign in instead, and a
    /// suspended one is refused, both inside the transaction.
    pub(crate) fn complete_challenge_with(
        &self,
        purpose: Purpose,
        token: &str,
        password: NewPassword,
        admits: impl FnOnce(&str) -> bool,
        on_consumed: impl FnOnce(&Connection, &str, &str) -> Result<(), AuthError>,
    ) -> Result<AuthSession, AuthError> {
        if self.scope.is_empty() {
            return Err(AuthError::InvalidChallenge);
        }
        let token_hash = hash_token(token);
        let now = now_seconds();
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let record: Option<(String, String, String)> = transaction
            .query_row(
                "SELECT u.id, u.email, u.state FROM account_challenges c JOIN app_users u ON u.id = c.user_id
                 WHERE c.token_hash = ?1 AND c.purpose = ?2 AND c.scope = ?3
                   AND c.consumed_at IS NULL AND c.expires_at > ?4",
                params![token_hash, purpose.as_str(), self.scope, now],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(AuthError::Storage)?;
        let Some((user_id, email, state)) = record else {
            return Err(AuthError::InvalidChallenge);
        };
        match (purpose, state.as_str()) {
            (_, "suspended") => return Err(AuthError::AccountSuspended),
            (Purpose::Invitation, "active") => return Err(AuthError::AccountExists),
            (Purpose::Verification | Purpose::Invitation, "pending") | (Purpose::Recovery, "active") => {}
            _ => return Err(AuthError::InvalidChallenge),
        }
        if purpose != Purpose::Recovery && !admits(&email) {
            return Err(AuthError::SignupClosed);
        }
        let consumed = transaction
            .execute(
                "UPDATE account_challenges SET consumed_at = ?1
                 WHERE token_hash = ?2 AND consumed_at IS NULL",
                params![now, token_hash],
            )
            .map_err(AuthError::Storage)?;
        if consumed != 1 {
            return Err(AuthError::InvalidChallenge);
        }
        on_consumed(&transaction, &user_id, &token_hash)?;
        transaction
            .execute(
                "UPDATE app_users SET password_hash = ?1, state = 'active',
                   verified_at = COALESCE(verified_at, ?2)
                 WHERE id = ?3",
                params![password.hash, now, user_id],
            )
            .map_err(AuthError::Storage)?;
        transaction
            .execute(
                "DELETE FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL",
                [&user_id],
            )
            .map_err(AuthError::Storage)?;
        transaction
            .execute("DELETE FROM app_sessions WHERE user_id = ?1", [&user_id])
            .map_err(AuthError::Storage)?;
        let role: String = transaction
            .query_row("SELECT role FROM app_users WHERE id = ?1", [&user_id], |row| row.get(0))
            .map_err(AuthError::Storage)?;
        let session = issue_session(
            &transaction,
            AuthUser {
                user_id,
                name: email,
                role,
            },
        )?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(session)
    }

    /// Whether an account exists and is active. Delegated credentials that
    /// name an account check this on every use.
    pub fn is_active(&self, user_id: &str) -> Result<bool, AuthError> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM app_users WHERE id = ?1 AND state = 'active')",
                [user_id],
                |row| row.get(0),
            )
            .map_err(AuthError::Storage)
    }

    /// Suspends an account by email. Its sessions and open challenges end at
    /// once; its ID, password and rows stay. Returns the account ID.
    pub fn suspend(&self, email: &str) -> Result<String, AuthError> {
        let email = normalize_email(email)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let user_id: String = transaction
            .query_row(
                "SELECT id FROM app_users WHERE email = ?1 COLLATE NOCASE",
                [&email],
                |row| row.get(0),
            )
            .optional()
            .map_err(AuthError::Storage)?
            .ok_or(AuthError::InvalidCredentials)?;
        transaction
            .execute("UPDATE app_users SET state = 'suspended' WHERE id = ?1", [&user_id])
            .map_err(AuthError::Storage)?;
        transaction
            .execute("DELETE FROM app_sessions WHERE user_id = ?1", [&user_id])
            .map_err(AuthError::Storage)?;
        transaction
            .execute(
                "DELETE FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL",
                [&user_id],
            )
            .map_err(AuthError::Storage)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(user_id)
    }

    /// Lifts a suspension. The account returns to active if it ever verified,
    /// or if this installation does not require verification; otherwise it
    /// returns to pending and must verify. Returns the account ID and state.
    pub fn resume(&self, email: &str) -> Result<(String, String), AuthError> {
        let email = normalize_email(email)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let user_id: String = transaction
            .query_row(
                "SELECT id FROM app_users WHERE email = ?1 COLLATE NOCASE AND state = 'suspended'",
                [&email],
                |row| row.get(0),
            )
            .optional()
            .map_err(AuthError::Storage)?
            .ok_or(AuthError::InvalidCredentials)?;
        transaction
            .execute(
                "UPDATE app_users SET state = CASE
                   WHEN verified_at IS NOT NULL OR ?2 THEN 'active' ELSE 'pending' END
                 WHERE id = ?1",
                params![user_id, self.verification == Verification::Local],
            )
            .map_err(AuthError::Storage)?;
        let state: String = transaction
            .query_row("SELECT state FROM app_users WHERE id = ?1", [&user_id], |row| row.get(0))
            .map_err(AuthError::Storage)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok((user_id, state))
    }

    /// Signs in or creates an account for an email an outside issuer
    /// vouched for, in one write transaction.
    ///
    /// The account is the one that holds the email, matched without regard
    /// to case as `app_users.email` is. `SignIn` signs an active account in
    /// and creates one for an email no account holds only when
    /// `admits_new(email)` holds; a pending account keeps waiting for its
    /// email link. `Invitation` answers that invitation when the email is the
    /// invited one, under `admits_invited(email)`.
    /// `on_invitation(transaction, user_id, token_hash)` runs before the
    /// commit, as in `complete_challenge_with`. Any refusal rolls back.
    pub(crate) fn sign_in_with_identity(
        &self,
        identity: &VerifiedIdentity,
        intent: &IdentityIntent,
        admits_new: impl FnOnce(&str) -> bool,
        admits_invited: impl FnOnce(&str) -> bool,
        on_invitation: impl FnOnce(&Connection, &str, &str) -> Result<(), AuthError>,
    ) -> Result<IdentitySignIn, AuthError> {
        let email = normalize_email(&identity.email)?;
        let now = now_seconds();
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let outcome = match intent {
            IdentityIntent::Invitation { token_hash } => {
                let record: Option<(String, String, String, String)> = transaction
                    .query_row(
                        "SELECT u.id, u.email, u.state, u.role FROM account_challenges c JOIN app_users u ON u.id = c.user_id
                         WHERE c.token_hash = ?1 AND c.purpose = 'invitation' AND c.scope = ?2
                           AND c.consumed_at IS NULL AND c.expires_at > ?3",
                        params![token_hash, self.scope, now],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(AuthError::Storage)?;
                let Some((user_id, invited, state, role)) = record else {
                    return Err(AuthError::InvalidChallenge);
                };
                match state.as_str() {
                    "suspended" => return Err(AuthError::AccountSuspended),
                    "active" => return Err(AuthError::AccountExists),
                    _ => {}
                }
                if !invited.eq_ignore_ascii_case(&email) {
                    return Err(AuthError::InvitedEmailDiffers);
                }
                if !admits_invited(&invited) {
                    return Err(AuthError::SignupClosed);
                }
                let consumed = transaction
                    .execute(
                        "UPDATE account_challenges SET consumed_at = ?1 WHERE token_hash = ?2 AND consumed_at IS NULL",
                        params![now, token_hash],
                    )
                    .map_err(AuthError::Storage)?;
                if consumed != 1 {
                    return Err(AuthError::InvalidChallenge);
                }
                on_invitation(&transaction, &user_id, token_hash)?;
                transaction
                    .execute(
                        "UPDATE app_users SET password_hash = CASE WHEN password_hash = ?1 THEN ?2 ELSE password_hash END,
                           state = 'active', verified_at = COALESCE(verified_at, ?3)
                         WHERE id = ?4",
                        params![NO_PASSWORD, unusable_password_hash()?, now, user_id],
                    )
                    .map_err(AuthError::Storage)?;
                transaction
                    .execute("DELETE FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL", [&user_id])
                    .map_err(AuthError::Storage)?;
                transaction
                    .execute("DELETE FROM app_sessions WHERE user_id = ?1", [&user_id])
                    .map_err(AuthError::Storage)?;
                let session = issue_session(&transaction, AuthUser { user_id, name: invited, role })?;
                IdentitySignIn { session, outcome: IdentityOutcome::Created }
            }
            IdentityIntent::SignIn => {
                let existing: Option<(String, String, String, String, Option<i64>)> = transaction
                    .query_row(
                        "SELECT id, email, role, state, verified_at FROM app_users WHERE email = ?1 COLLATE NOCASE",
                        [&email],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .optional()
                    .map_err(AuthError::Storage)?;
                match existing {
                    Some((user_id, account_email, role, state, verified_at)) => {
                        match state.as_str() {
                            "active" => {}
                            "suspended" => return Err(AuthError::AccountSuspended),
                            _ => return Err(AuthError::AccountPending),
                        }
                        if verified_at.is_none() && role == "owner" {
                            // The owner's app account comes from the creator
                            // email, which local verification never proved.
                            // Google does not take it back or end the
                            // sessions the owner opened from the editor.
                            return Err(AuthError::OwnerAccountUnproven);
                        }
                        if verified_at.is_none() {
                            // Nobody proved this email before: under local
                            // verification anyone may have typed it with a
                            // password of their own. The person the issuer
                            // vouches for holds the address, so that password
                            // and every session made with it stop working.
                            // Recovery by email sets a new password.
                            transaction
                                .execute(
                                    "UPDATE app_users SET password_hash = ?1, verified_at = ?2 WHERE id = ?3",
                                    params![unusable_password_hash()?, now, user_id],
                                )
                                .map_err(AuthError::Storage)?;
                            transaction
                                .execute("DELETE FROM app_sessions WHERE user_id = ?1", [&user_id])
                                .map_err(AuthError::Storage)?;
                            transaction
                                .execute("DELETE FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL", [&user_id])
                                .map_err(AuthError::Storage)?;
                        }
                        let session = issue_session(&transaction, AuthUser { user_id, name: account_email, role })?;
                        IdentitySignIn { session, outcome: IdentityOutcome::SignedIn }
                    }
                    None => {
                        if !admits_new(&email) {
                            return Err(AuthError::SignupClosed);
                        }
                        let user_id = Uuid::new_v4().to_string();
                        transaction
                            .execute(
                                "INSERT INTO app_users (id, email, password_hash, role, created_at, state, verified_at)
                                 VALUES (?1, ?2, ?3, 'member', ?4, 'active', ?4)",
                                params![user_id, email, unusable_password_hash()?, now],
                            )
                            .map_err(AuthError::Storage)?;
                        let session = issue_session(&transaction, AuthUser { user_id, name: email.clone(), role: "member".into() })?;
                        IdentitySignIn { session, outcome: IdentityOutcome::Created }
                    }
                }
            }
        };
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(outcome)
    }

    /// Runs `work` in one write transaction on this store that takes the
    /// lock at BEGIN, for flows such as invitations that keep their own rows
    /// beside the accounts.
    pub(crate) fn transaction<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, AuthError>,
    ) -> Result<T, AuthError> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let value = work(&transaction)?;
        transaction.commit().map_err(AuthError::Storage)?;
        Ok(value)
    }

    /// Reads this store outside any transaction.
    pub(crate) fn read<T>(&self, work: impl FnOnce(&Connection) -> Result<T, AuthError>) -> Result<T, AuthError> {
        work(&self.connection()?)
    }

    /// Stores a new challenge and invalidates the account's earlier open
    /// challenges of the same purpose, so only the newest link works.
    pub(crate) fn issue_challenge(
        &self,
        transaction: &Connection,
        user_id: &str,
        email: &str,
        purpose: Purpose,
    ) -> Result<IssuedChallenge, AuthError> {
        if self.scope.is_empty() {
            return Err(AuthError::InvalidChallenge);
        }
        let now = now_seconds();
        let ttl = match purpose {
            Purpose::Verification => VERIFICATION_TTL_SECONDS,
            Purpose::Invitation => INVITATION_TTL_SECONDS,
            Purpose::Recovery => RECOVERY_TTL_SECONDS,
        };
        transaction
            .execute(
                "DELETE FROM account_challenges
                 WHERE expires_at <= ?1 OR (user_id = ?2 AND purpose = ?3 AND consumed_at IS NULL)",
                params![now, user_id, purpose.as_str()],
            )
            .map_err(AuthError::Storage)?;
        let token = random_token()?;
        let expires_at = now + ttl;
        transaction
            .execute(
                "INSERT INTO account_challenges (token_hash, user_id, email, purpose, scope, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![hash_token(&token), user_id, email, purpose.as_str(), self.scope, now, expires_at],
            )
            .map_err(AuthError::Storage)?;
        Ok(IssuedChallenge {
            token,
            email: email.to_owned(),
            purpose,
            scope: self.scope.clone(),
            expires_at,
        })
    }

    /// A connection with the outbox's database attached as `outbox`. SQLite
    /// cannot attach inside a transaction, so this happens before it begins.
    fn connection_with(&self, outbox: Option<&dyn ChallengeOutbox>) -> Result<Connection, AuthError> {
        let connection = self.connection()?;
        if let Some(outbox) = outbox {
            connection
                .execute("ATTACH DATABASE ?1 AS outbox", [outbox.database().to_string_lossy()])
                .map_err(AuthError::Storage)?;
        }
        Ok(connection)
    }

    fn connection(&self) -> Result<Connection, AuthError> {
        let connection = Connection::open(&self.path).map_err(AuthError::Storage)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(AuthError::Storage)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(AuthError::Storage)?;
        // SQLite keeps this per connection, so every connection sets it.
        let page_size: u64 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(AuthError::Storage)?;
        let pages = (MAX_IDENTITY_DATABASE_BYTES / page_size.max(1)).max(1);
        connection
            .query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))
            .map_err(AuthError::Storage)?;
        Ok(connection)
    }
}

/// An email an outside issuer vouched for: its ID token passed every check,
/// including that the issuer is the authority for the address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub email: String,
}

/// What a sign-in with an outside identity was started for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityIntent {
    SignIn,
    /// Answer the invitation whose challenge has this token hash.
    Invitation { token_hash: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityOutcome {
    SignedIn,
    Created,
}

#[derive(Debug)]
pub struct IdentitySignIn {
    pub session: AuthSession,
    pub outcome: IdentityOutcome,
}

/// The password hash of an account no person chose a password for, such as
/// one created by a Google sign-in. It is a well-formed hash of 32 random
/// bytes with a random salt, so a password check runs the full KDF and
/// fails, and the person sets a password through recovery by email.
fn unusable_password_hash() -> Result<String, AuthError> {
    let mut hash = [0_u8; PASSWORD_HASH_BYTES];
    let mut salt = [0_u8; PASSWORD_SALT_BYTES];
    let random = SystemRandom::new();
    random.fill(&mut hash).map_err(|_| AuthError::PasswordHash)?;
    random.fill(&mut salt).map_err(|_| AuthError::PasswordHash)?;
    Ok(format!("pbkdf2-sha256${PASSWORD_HASH_ITERATIONS}${}${}", hex::encode(salt), hex::encode(hash)))
}

/// A signup whose input passed validation and whose password is hashed.
/// Hashing is slow, so callers prepare this before taking any lock.
pub struct NewAccount {
    email: String,
    password_hash: String,
}

impl NewAccount {
    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn prepare(email: &str, password: &str) -> Result<Self, AuthError> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        Ok(Self {
            email,
            password_hash: hash_password(password)?,
        })
    }
}

/// A password chosen while answering a challenge, checked and hashed before
/// any lock is taken.
pub struct NewPassword {
    hash: String,
}

impl NewPassword {
    pub fn prepare(password: &str) -> Result<Self, AuthError> {
        validate_password(password)?;
        Ok(Self {
            hash: hash_password(password)?,
        })
    }
}

/// A user record read for a login, before the password check.
pub struct StoredLogin {
    user: AuthUser,
    password_hash: String,
}

/// Runs the password check. Unknown emails check against a dummy hash, so
/// they take as long as known ones. This is the slow step; run it without
/// holding a lock.
pub fn check_password(password: &str, stored: Option<StoredLogin>) -> Result<AuthUser, AuthError> {
    let Some(stored) = stored.filter(|stored| stored.password_hash != NO_PASSWORD) else {
        let _ = verify_password(password, DUMMY_PASSWORD_HASH);
        return Err(AuthError::InvalidCredentials);
    };
    verify_password(password, &stored.password_hash)?;
    Ok(stored.user)
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
    // Sign-ins are rate limited but succeed every time with the right
    // password, so without a cap one account could fill the identity file.
    // The oldest session beyond the cap ends.
    connection
        .execute(
            "DELETE FROM app_sessions WHERE user_id = ?1 AND rowid NOT IN
               (SELECT rowid FROM app_sessions WHERE user_id = ?1 ORDER BY expires_at DESC, rowid DESC LIMIT ?2)",
            params![user.user_id, MAX_SESSIONS_PER_ACCOUNT],
        )
        .map_err(AuthError::Storage)?;
    Ok(AuthSession {
        token,
        user,
        expires_at,
    })
}

/// Brings an identity database to the current account schema without
/// changing any ID or row, then applies the verification mode. Columns are
/// added, never rebuilt. Accounts that existed before states default to
/// active and unverified.
fn migrate(connection: &mut Connection, verification: Option<Verification>) -> Result<(), AuthError> {
    // Every request opens a store, so the common case reads without taking
    // the write lock.
    let current: bool = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM pragma_table_info('app_users') WHERE name IN ('state', 'verified_at')) = 2
               AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'account_meta')
               AND (?1 IS NULL OR EXISTS(SELECT 1 FROM account_meta WHERE key = 'verification' AND value = ?1))",
            [verification.map(Verification::as_str)],
            |row| row.get(0),
        )
        .or_else(|error_value| match error_value {
            // account_meta does not exist yet, so the subquery cannot compile.
            rusqlite::Error::SqliteFailure(_, _) => Ok(false),
            other => Err(other),
        })
        .map_err(AuthError::Storage)?;
    if current {
        return Ok(());
    }
    let transaction = immediate(connection)?;
    let columns: Vec<String> = transaction
        .prepare("PRAGMA table_info(app_users)")
        .map_err(AuthError::Storage)?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(AuthError::Storage)?
        .collect::<rusqlite::Result<_>>()
        .map_err(AuthError::Storage)?;
    if !columns.iter().any(|name| name == "state") {
        transaction
            .execute(
                "ALTER TABLE app_users ADD COLUMN state TEXT NOT NULL DEFAULT 'active'
                 CHECK (state IN ('pending', 'active', 'suspended'))",
                [],
            )
            .map_err(AuthError::Storage)?;
    }
    if !columns.iter().any(|name| name == "verified_at") {
        transaction
            .execute("ALTER TABLE app_users ADD COLUMN verified_at INTEGER", [])
            .map_err(AuthError::Storage)?;
    }
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS account_challenges (
               token_hash TEXT PRIMARY KEY,
               user_id TEXT REFERENCES app_users(id) ON DELETE CASCADE,
               email TEXT NOT NULL COLLATE NOCASE,
               purpose TEXT NOT NULL CHECK (purpose IN ('verification', 'invitation', 'recovery')),
               scope TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL,
               consumed_at INTEGER
             );
             CREATE INDEX IF NOT EXISTS account_challenges_user ON account_challenges(user_id, purpose);
             CREATE INDEX IF NOT EXISTS account_challenges_expiry ON account_challenges(expires_at);
             CREATE TABLE IF NOT EXISTS account_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(AuthError::Storage)?;
    let Some(verification) = verification else {
        return transaction.commit().map_err(AuthError::Storage);
    };
    let recorded: Option<String> = transaction
        .query_row("SELECT value FROM account_meta WHERE key = 'verification'", [], |row| row.get(0))
        .optional()
        .map_err(AuthError::Storage)?;
    if recorded.as_deref() != Some(verification.as_str()) {
        if verification == Verification::Required {
            // A password chosen without proof of the email is not trusted:
            // the person must verify and set a new password to enter.
            transaction
                .execute(
                    "UPDATE app_users SET state = 'pending' WHERE state = 'active' AND verified_at IS NULL",
                    [],
                )
                .map_err(AuthError::Storage)?;
            transaction
                .execute(
                    "DELETE FROM app_sessions WHERE user_id IN (SELECT id FROM app_users WHERE state <> 'active')",
                    [],
                )
                .map_err(AuthError::Storage)?;
        }
        transaction
            .execute(
                "INSERT INTO account_meta (key, value) VALUES ('verification', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [verification.as_str()],
            )
            .map_err(AuthError::Storage)?;
    }
    transaction.commit().map_err(AuthError::Storage)
}

/// A write transaction that takes the lock at once, so two writers serialize
/// at BEGIN instead of failing at their first write.
fn immediate(connection: &mut Connection) -> Result<rusqlite::Transaction<'_>, AuthError> {
    connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(AuthError::Storage)
}

fn random_token() -> Result<String, AuthError> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::PasswordHash)?;
    Ok(hex::encode(bytes))
}

pub(crate) fn normalize_email(email: &str) -> Result<String, AuthError> {
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
        // Mail libraries and providers read `name<a@b>`, comments and lists
        // as address syntax, so `x<victim@b>` would reach victim@b as another
        // account with its own limits. A plain address has none of these.
        || email.chars().any(|c| c.is_control() || "<>()[],;:\\\"".contains(c))
    {
        return Err(AuthError::InvalidEmail);
    }
    Ok(email.to_lowercase())
}

pub(crate) fn validate_password(password: &str) -> Result<(), AuthError> {
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

pub(crate) fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDb(PathBuf);

    #[test]
    fn an_email_with_address_syntax_is_refused() {
        assert_eq!(normalize_email(" Ana.B+club@Example.COM ").unwrap(), "ana.b+club@example.com");
        for email in ["x<victim@example.com>", "victim@example.com>", "(c)victim@example.com", "victim@example.com,b",
            "\"a b\"@example.com", "a;b@example.com", "a:b@example.com", "a\\b@example.com", "[a]@example.com", "a\u{7}@example.com"] {
            assert!(matches!(normalize_email(email), Err(AuthError::InvalidEmail)), "{email}");
        }
    }

    impl TempDb {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("konae-auth-{}.sqlite", Uuid::new_v4()));
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
    fn a_sign_in_past_the_cap_ends_the_oldest_session_of_that_account_only() {
        let app = TempDb::new();
        let service = app.service();
        let first = service.signup("one@example.test", "correct horse battery").unwrap();
        let other = service.signup("two@example.test", "another correct password").unwrap();
        let mut sessions = vec![first.token.clone()];
        for _ in 1..MAX_SESSIONS_PER_ACCOUNT {
            sessions.push(service.login("one@example.test", "correct horse battery").unwrap().token);
        }
        assert!(service.authenticate(Some(&first.token)).unwrap().is_some(), "the cap holds every session up to it");
        service.login("one@example.test", "correct horse battery").unwrap();
        assert!(service.authenticate(Some(&first.token)).unwrap().is_none(), "the oldest session ended");
        assert!(service.authenticate(Some(&sessions[1])).unwrap().is_some());
        assert!(service.authenticate(Some(&other.token)).unwrap().is_some(), "another account keeps its session");
        let count: i64 = Connection::open(&app.0).unwrap()
            .query_row("SELECT COUNT(*) FROM app_sessions WHERE user_id = ?1", [&first.user.user_id], |row| row.get(0)).unwrap();
        assert_eq!(count, MAX_SESSIONS_PER_ACCOUNT);
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

    #[test]
    fn identity_databases_are_capped() {
        let app = TempDb::new();
        let auth = app.service();
        let connection = auth.connection().unwrap();
        let page_size: u64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0)).unwrap();
        let pages: u64 = connection.query_row("PRAGMA max_page_count", [], |row| row.get(0)).unwrap();
        assert_eq!(pages * page_size, MAX_IDENTITY_DATABASE_BYTES);
    }

    #[test]
    fn a_session_is_refused_for_a_user_deleted_after_the_password_check() {
        let app = TempDb::new();
        let auth = app.service();
        auth.signup("gone@example.test", "correct horse battery").unwrap();
        let stored = auth.find_login("gone@example.test", "correct horse battery").unwrap();
        let user = check_password("correct horse battery", stored).unwrap();
        auth.connection().unwrap().execute("DELETE FROM app_users", []).unwrap();
        assert!(matches!(auth.start_session(user), Err(AuthError::InvalidCredentials)));
        assert!(matches!(check_password("correct horse battery", None), Err(AuthError::InvalidCredentials)));
    }

    const PASSWORD: &str = "correct horse battery";

    impl TempDb {
        fn scoped(&self, scope: &str, verification: Verification) -> AuthService {
            AuthService::open_in(&self.0, scope, verification).unwrap()
        }
        fn required(&self) -> AuthService {
            self.scoped(&deployment_scope("app-one"), Verification::Required)
        }
    }

    fn password(value: &str) -> NewPassword {
        NewPassword::prepare(value).unwrap()
    }

    fn state_of(db: &TempDb, email: &str) -> (String, String, Option<i64>) {
        Connection::open(&db.0)
            .unwrap()
            .query_row(
                "SELECT id, state, verified_at FROM app_users WHERE email = ?1",
                [email],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    fn sign_in(auth: &AuthService, email: &str, password: &str) -> Result<AuthSession, AuthError> {
        let stored = auth.find_login(email, password)?;
        let user = check_password(password, stored)?;
        auth.start_session(user)
    }

    #[test]
    fn a_pending_account_gets_no_session_until_it_verifies() {
        let app = TempDb::new();
        let auth = app.required();
        assert!(matches!(
            auth.create_account(NewAccount::prepare("new@example.test", PASSWORD).unwrap()),
            Err(AuthError::VerificationRequired)
        ));
        let challenge = auth.request_signup("New@Example.test").unwrap().unwrap();
        assert_eq!(challenge.purpose, Purpose::Verification);
        assert_eq!(challenge.email, "new@example.test");
        assert_eq!(state_of(&app, "new@example.test").1, "pending");
        // No password was stored at signup, so none signs in.
        assert!(matches!(sign_in(&auth, "new@example.test", PASSWORD), Err(AuthError::InvalidCredentials)));
        let user = AuthUser { user_id: state_of(&app, "new@example.test").0, name: "new@example.test".into(), role: "member".into() };
        assert!(matches!(auth.start_session(user), Err(AuthError::AccountPending)));
        let session = auth
            .complete_challenge(Purpose::Verification, &challenge.token, password(PASSWORD), |_| true)
            .unwrap();
        assert_eq!(auth.authenticate(Some(&session.token)).unwrap().unwrap().user_id, session.user.user_id);
        let (_, state, verified_at) = state_of(&app, "new@example.test");
        assert_eq!(state, "active");
        assert!(verified_at.is_some());
        assert!(sign_in(&auth, "new@example.test", PASSWORD).is_ok());
        // The database keeps only the token's hash.
        let stored: i64 = Connection::open(&app.0).unwrap()
            .query_row("SELECT COUNT(*) FROM account_challenges WHERE token_hash = ?1", [&challenge.token], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 0);
    }

    #[test]
    fn signup_answers_nothing_for_existing_accounts_and_reissues_for_pending_ones() {
        let app = TempDb::new();
        let auth = app.required();
        let first = auth.request_signup("person@example.test").unwrap().unwrap();
        let id = state_of(&app, "person@example.test").0;
        let second = auth.request_signup("person@example.test").unwrap().unwrap();
        assert_eq!(state_of(&app, "person@example.test").0, id, "a repeated signup keeps the account ID");
        assert!(matches!(
            auth.complete_challenge(Purpose::Verification, &first.token, password(PASSWORD), |_| true),
            Err(AuthError::InvalidChallenge)
        ), "a new link replaces the earlier one");
        auth.complete_challenge(Purpose::Verification, &second.token, password(PASSWORD), |_| true).unwrap();
        assert!(auth.request_signup("person@example.test").unwrap().is_none());
        auth.suspend("person@example.test").unwrap();
        assert!(auth.request_signup("person@example.test").unwrap().is_none());
        assert_eq!(state_of(&app, "person@example.test").1, "suspended");
    }

    #[test]
    fn a_challenge_is_refused_twice_after_expiry_on_another_scope_or_for_another_purpose() {
        let app = TempDb::new();
        let other_app = TempDb::new();
        let auth = app.required();
        let challenge = auth.request_signup("person@example.test").unwrap().unwrap();
        let refused = |service: &AuthService, purpose: Purpose| {
            matches!(
                service.complete_challenge(purpose, &challenge.token, password(PASSWORD), |_| true),
                Err(AuthError::InvalidChallenge)
            )
        };
        // Another Deployment's scope over the same file, the creator scope,
        // another Deployment's database, and the wrong purposes.
        assert!(refused(&app.scoped(&deployment_scope("app-two"), Verification::Required), Purpose::Verification));
        assert!(refused(&app.scoped(CREATOR_SCOPE, Verification::Required), Purpose::Verification));
        assert!(refused(&AuthService::open(&app.0).unwrap(), Purpose::Verification));
        assert!(refused(&other_app.required(), Purpose::Verification));
        assert!(refused(&auth, Purpose::Recovery));
        assert!(refused(&auth, Purpose::Invitation));
        // A policy that no longer admits the email refuses without using the token.
        assert!(matches!(
            auth.complete_challenge(Purpose::Verification, &challenge.token, password(PASSWORD), |_| false),
            Err(AuthError::SignupClosed)
        ));
        auth.complete_challenge(Purpose::Verification, &challenge.token, password(PASSWORD), |_| true).unwrap();
        assert!(refused(&auth, Purpose::Verification), "a challenge is single use");

        let late = auth.request_recovery("person@example.test").unwrap().unwrap();
        Connection::open(&app.0).unwrap()
            .execute("UPDATE account_challenges SET expires_at = ?1", [now_seconds() - 1])
            .unwrap();
        assert!(matches!(
            auth.complete_challenge(Purpose::Recovery, &late.token, password("a new password"), |_| true),
            Err(AuthError::InvalidChallenge)
        ));
        assert!(sign_in(&auth, "person@example.test", PASSWORD).is_ok());
    }

    #[test]
    fn challenges_expire_after_their_purpose_window() {
        let app = TempDb::new();
        let auth = app.required();
        let verification = auth.request_signup("person@example.test").unwrap().unwrap();
        let now = now_seconds();
        assert!((verification.expires_at - now - VERIFICATION_TTL_SECONDS).abs() <= 2);
        auth.complete_challenge(Purpose::Verification, &verification.token, password(PASSWORD), |_| true).unwrap();
        let now = now_seconds();
        let recovery = auth.request_recovery("person@example.test").unwrap().unwrap();
        assert_eq!(recovery.purpose, Purpose::Recovery);
        assert!((recovery.expires_at - now - RECOVERY_TTL_SECONDS).abs() <= 2);
    }

    #[test]
    fn a_third_partys_password_does_not_survive_verification() {
        let app = TempDb::new();
        // Someone registers another person's address on an installation that
        // did not verify email, and picks the password.
        let local = app.scoped(&deployment_scope("app-one"), Verification::Local);
        let squatter = local.signup("victim@example.test", "squatter password").unwrap();
        let id = squatter.user.user_id.clone();
        // The installation turns verification on.
        let auth = app.required();
        assert!(auth.authenticate(Some(&squatter.token)).unwrap().is_none());
        assert!(matches!(sign_in(&auth, "victim@example.test", "squatter password"), Err(AuthError::AccountPending)));
        // The owner of the address asks for a link and sets their password.
        let challenge = auth.request_recovery("victim@example.test").unwrap().unwrap();
        assert_eq!(challenge.purpose, Purpose::Verification);
        let session = auth
            .complete_challenge(Purpose::Verification, &challenge.token, password("the real password"), |_| true)
            .unwrap();
        assert_eq!(session.user.user_id, id);
        assert!(matches!(sign_in(&auth, "victim@example.test", "squatter password"), Err(AuthError::InvalidCredentials)));
        assert!(sign_in(&auth, "victim@example.test", "the real password").is_ok());

        // On a verifying installation a squatter's signup stores no password,
        // and the victim's own signup reaches the same pending account.
        let fresh = TempDb::new();
        let auth = fresh.required();
        auth.request_signup("victim@example.test").unwrap().unwrap();
        let mine = auth.request_signup("victim@example.test").unwrap().unwrap();
        auth.complete_challenge(Purpose::Verification, &mine.token, password("the real password"), |_| true).unwrap();
        assert!(sign_in(&auth, "victim@example.test", "the real password").is_ok());
    }

    #[test]
    fn two_concurrent_verifications_activate_once() {
        let app = TempDb::new();
        let challenge = app.required().request_signup("person@example.test").unwrap().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|index| {
                let (path, token, barrier) = (app.0.clone(), challenge.token.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let auth = AuthService::open_in(&path, &deployment_scope("app-one"), Verification::Required).unwrap();
                    let chosen = password(&format!("password number {index}"));
                    barrier.wait();
                    auth.complete_challenge(Purpose::Verification, &token, chosen, |_| true)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results.iter().all(|result| matches!(result, Ok(_) | Err(AuthError::InvalidChallenge))));
        let sessions: i64 = Connection::open(&app.0).unwrap()
            .query_row("SELECT COUNT(*) FROM app_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
    }

    #[test]
    fn recovery_reveals_nothing_and_revokes_earlier_sessions() {
        let app = TempDb::new();
        let auth = app.required();
        let verification = auth.request_signup("person@example.test").unwrap().unwrap();
        let first = auth.complete_challenge(Purpose::Verification, &verification.token, password(PASSWORD), |_| true).unwrap();
        let second = sign_in(&auth, "person@example.test", PASSWORD).unwrap();
        assert!(auth.request_recovery("nobody@example.test").unwrap().is_none());
        assert!(auth.request_recovery("not an email").unwrap().is_none());
        let recovery = auth.request_recovery("person@example.test").unwrap().unwrap();
        assert!(matches!(
            auth.complete_challenge(Purpose::Verification, &recovery.token, password("a new password"), |_| true),
            Err(AuthError::InvalidChallenge)
        ), "a recovery token does not verify");
        let fresh = auth.complete_challenge(Purpose::Recovery, &recovery.token, password("a new password"), |_| false).unwrap();
        assert!(auth.authenticate(Some(&first.token)).unwrap().is_none());
        assert!(auth.authenticate(Some(&second.token)).unwrap().is_none());
        assert!(auth.authenticate(Some(&fresh.token)).unwrap().is_some());
        assert!(matches!(sign_in(&auth, "person@example.test", PASSWORD), Err(AuthError::InvalidCredentials)));
        assert!(sign_in(&auth, "person@example.test", "a new password").is_ok());
    }

    fn google(email: &str) -> VerifiedIdentity {
        VerifiedIdentity { email: email.into() }
    }

    #[test]
    fn a_google_account_has_no_usable_password_until_recovery_sets_one() {
        let app = TempDb::new();
        let auth = app.required();
        let signed = auth
            .sign_in_with_identity(&google("Person@Example.test"), &IdentityIntent::SignIn, |_| true, |_| true, |_, _, _| Ok(()))
            .unwrap();
        assert_eq!(signed.outcome, IdentityOutcome::Created);
        let user = signed.session.user;
        assert_eq!(user.name, "person@example.test");
        // No password opens it, and the check runs the full KDF like any other.
        assert!(matches!(sign_in(&auth, "person@example.test", "any password at all"), Err(AuthError::InvalidCredentials)));
        assert!(app.required().is_active(&user.user_id).unwrap());
        // Recovery by email gives it a password, and Google still reaches it.
        let recovery = auth.request_recovery("person@example.test").unwrap().unwrap();
        auth.complete_challenge(Purpose::Recovery, &recovery.token, password(PASSWORD), |_| true).unwrap();
        assert!(sign_in(&auth, "person@example.test", PASSWORD).is_ok());
        let again = auth.sign_in_with_identity(&google("person@example.test"), &IdentityIntent::SignIn, |_| false, |_| false, |_, _, _| Ok(())).unwrap();
        assert_eq!(again.session.user.user_id, user.user_id);
    }

    #[test]
    fn a_verified_email_signs_in_the_account_that_holds_it() {
        let app = TempDb::new();
        let auth = app.required();
        let verification = auth.request_signup("person@example.test").unwrap().unwrap();
        let session = auth.complete_challenge(Purpose::Verification, &verification.token, password(PASSWORD), |_| true).unwrap();
        // The same email, in another case, reaches the password account without asking the policy.
        let signed = auth.sign_in_with_identity(&google("PERSON@example.test"), &IdentityIntent::SignIn, |_| false, |_| false, |_, _, _| Ok(())).unwrap();
        assert_eq!((signed.outcome, signed.session.user.user_id.as_str()), (IdentityOutcome::SignedIn, session.user.user_id.as_str()));
        assert!(sign_in(&auth, "person@example.test", PASSWORD).is_ok(), "the password keeps working");
        // A proven password account keeps its password.
        assert!(auth.authenticate(Some(&session.token)).unwrap().is_some());
        // A refused admission creates nothing.
        let closed = auth.sign_in_with_identity(&google("stranger@example.test"), &IdentityIntent::SignIn, |_| false, |_| true, |_, _, _| Ok(()));
        assert!(matches!(closed, Err(AuthError::SignupClosed)));
        assert!(auth.find_login("stranger@example.test", PASSWORD).unwrap().is_none());
        // A pending signup keeps waiting for its email link.
        auth.request_signup("waiting@example.test").unwrap().unwrap();
        let pending = auth.sign_in_with_identity(&google("waiting@example.test"), &IdentityIntent::SignIn, |_| true, |_| true, |_, _, _| Ok(()));
        assert!(matches!(pending, Err(AuthError::AccountPending)));
        // Suspension ends Google sign-in too.
        auth.suspend("person@example.test").unwrap();
        let suspended = auth.sign_in_with_identity(&google("person@example.test"), &IdentityIntent::SignIn, |_| true, |_| true, |_, _, _| Ok(()));
        assert!(matches!(suspended, Err(AuthError::AccountSuspended)));
    }

    #[test]
    fn a_google_sign_in_takes_an_unproven_password_account_back_from_whoever_typed_the_email() {
        let app = TempDb::new();
        let auth = app.scoped(&deployment_scope("app-one"), Verification::Local);
        // Under local verification anyone can make an active account for an address they do not hold.
        let squatter = auth.create_account(NewAccount::prepare("victim@gmail.com", PASSWORD).unwrap()).unwrap();
        let signed = auth.sign_in_with_identity(&google("victim@gmail.com"), &IdentityIntent::SignIn, |_| false, |_| false, |_, _, _| Ok(())).unwrap();
        assert_eq!(signed.session.user.user_id, squatter.user.user_id, "the address keeps its account");
        assert!(auth.authenticate(Some(&squatter.token)).unwrap().is_none(), "the typist's session ends");
        assert!(matches!(sign_in(&auth, "victim@gmail.com", PASSWORD), Err(AuthError::InvalidCredentials)), "and the typist's password");
        assert!(auth.authenticate(Some(&signed.session.token)).unwrap().is_some());
        // From then on the address is proven, so a later Google sign-in leaves the account alone.
        let again = auth.sign_in_with_identity(&google("victim@gmail.com"), &IdentityIntent::SignIn, |_| false, |_| false, |_, _, _| Ok(())).unwrap();
        assert!(auth.authenticate(Some(&signed.session.token)).unwrap().is_some());
        assert_eq!(again.outcome, IdentityOutcome::SignedIn);
    }

    #[test]
    fn a_google_sign_in_does_not_take_back_the_owners_unproven_account() {
        let app = TempDb::new();
        let auth = app.scoped(&deployment_scope("app-one"), Verification::Local);
        let mut connection = auth.connection().unwrap();
        let tx = immediate(&mut connection).unwrap();
        let claimed = crate::app_owner::claim(&tx, "creator@gmail.com", false).unwrap().unwrap();
        let owner_session = issue_session(&tx, AuthUser { user_id: claimed.user_id.clone(), name: "creator@gmail.com".into(), role: "owner".into() }).unwrap();
        tx.commit().unwrap();
        let refused = auth.sign_in_with_identity(&google("Creator@gmail.com"), &IdentityIntent::SignIn, |_| true, |_| true, |_, _, _| Ok(()));
        assert!(matches!(refused, Err(AuthError::OwnerAccountUnproven)));
        assert!(auth.authenticate(Some(&owner_session.token)).unwrap().is_some(), "the owner's session survives");
        let (role, verified_at): (String, Option<i64>) = auth
            .connection()
            .unwrap()
            .query_row("SELECT role, verified_at FROM app_users WHERE id = ?1", [&claimed.user_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!((role.as_str(), verified_at), ("owner", None), "nothing changed");
        // Once the owner's email is proven, Google signs the owner in to the same account.
        auth.connection().unwrap().execute("UPDATE app_users SET verified_at = 1 WHERE id = ?1", [&claimed.user_id]).unwrap();
        let signed = auth.sign_in_with_identity(&google("creator@gmail.com"), &IdentityIntent::SignIn, |_| true, |_| true, |_, _, _| Ok(())).unwrap();
        assert_eq!((signed.session.user.user_id.as_str(), signed.session.user.role.as_str()), (claimed.user_id.as_str(), "owner"));
        assert!(auth.authenticate(Some(&owner_session.token)).unwrap().is_some(), "a proven account keeps its sessions");
    }

    #[test]
    fn a_suspended_account_gets_no_session_and_no_challenge() {
        let app = TempDb::new();
        let auth = app.required();
        let verification = auth.request_signup("person@example.test").unwrap().unwrap();
        let session = auth.complete_challenge(Purpose::Verification, &verification.token, password(PASSWORD), |_| true).unwrap();
        let recovery = auth.request_recovery("person@example.test").unwrap().unwrap();
        let id = auth.suspend("person@example.test").unwrap();
        assert_eq!(id, session.user.user_id);
        assert!(auth.authenticate(Some(&session.token)).unwrap().is_none());
        assert!(matches!(sign_in(&auth, "person@example.test", PASSWORD), Err(AuthError::AccountSuspended)));
        assert!(matches!(
            auth.complete_challenge(Purpose::Recovery, &recovery.token, password("a new password"), |_| true),
            Err(AuthError::InvalidChallenge)
        ));
        assert!(auth.request_recovery("person@example.test").unwrap().is_none());
        // A session row that survived somehow still does not authenticate.
        Connection::open(&app.0).unwrap()
            .execute("INSERT INTO app_sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
                params![hash_token("leftover"), id, now_seconds() + 60])
            .unwrap();
        assert!(auth.authenticate(Some("leftover")).unwrap().is_none());
        assert_eq!(auth.resume("person@example.test").unwrap(), (id.clone(), "active".to_owned()));
        assert!(sign_in(&auth, "person@example.test", PASSWORD).is_ok());
        // An account suspended before it verified returns to pending.
        auth.request_signup("unverified@example.test").unwrap();
        auth.suspend("unverified@example.test").unwrap();
        assert_eq!(auth.resume("unverified@example.test").unwrap().1, "pending");
    }

    #[test]
    fn legacy_accounts_need_verification_and_keep_their_ids_and_rows() {
        let app = TempDb::new();
        let local = app.scoped(&deployment_scope("app-one"), Verification::Local);
        let old = local.signup("old@example.test", PASSWORD).unwrap();
        let connection = Connection::open(&app.0).unwrap();
        connection.execute_batch("CREATE TABLE notes (owner TEXT NOT NULL, body TEXT NOT NULL)").unwrap();
        connection.execute("INSERT INTO notes VALUES (?1, 'kept')", [&old.user.user_id]).unwrap();
        // Local mode keeps working unverified.
        assert!(local.authenticate(Some(&old.token)).unwrap().is_some());

        let auth = app.required();
        let (id, state, verified_at) = state_of(&app, "old@example.test");
        assert_eq!((id.as_str(), state.as_str(), verified_at), (old.user.user_id.as_str(), "pending", None));
        assert!(auth.authenticate(Some(&old.token)).unwrap().is_none());
        let owner: String = connection.query_row("SELECT owner FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(owner, id);

        // Verifying keeps the ID; reopening does not move a verified account back.
        let challenge = auth.request_recovery("old@example.test").unwrap().unwrap();
        auth.complete_challenge(Purpose::Verification, &challenge.token, password("a new password"), |_| true).unwrap();
        app.required();
        assert_eq!(state_of(&app, "old@example.test").1, "active");

        // Accounts created unverified during a later local period are moved
        // when the installation requires verification again.
        let local = app.scoped(&deployment_scope("app-one"), Verification::Local);
        local.signup("later@example.test", PASSWORD).unwrap();
        app.required();
        assert_eq!(state_of(&app, "later@example.test").1, "pending");
        assert_eq!(state_of(&app, "old@example.test").1, "active");
    }

    #[test]
    fn an_identity_database_from_before_states_migrates_in_place() {
        let app = TempDb::new();
        let connection = Connection::open(&app.0).unwrap();
        connection.execute_batch(
            "CREATE TABLE app_users (id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE COLLATE NOCASE,
               password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'member')),
               created_at INTEGER NOT NULL);
             CREATE TABLE app_sessions (token_hash TEXT PRIMARY KEY,
               user_id TEXT NOT NULL REFERENCES app_users(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL);",
        ).unwrap();
        connection.execute(
            "INSERT INTO app_users (id, email, password_hash, created_at) VALUES ('legacy-id', 'legacy@example.test', ?1, 1)",
            [hash_password(PASSWORD).unwrap()],
        ).unwrap();
        let local = app.scoped(&deployment_scope("app-one"), Verification::Local);
        assert_eq!(state_of(&app, "legacy@example.test"), ("legacy-id".to_owned(), "active".to_owned(), None));
        assert_eq!(sign_in(&local, "legacy@example.test", PASSWORD).unwrap().user.user_id, "legacy-id");
    }

    #[test]
    fn dummy_hash_runs_the_full_kdf() {
        // PasswordHash would mean verify_password rejected the encoding before
        // deriving, which makes unknown emails answer faster than known ones.
        assert!(matches!(
            verify_password("correct horse battery", DUMMY_PASSWORD_HASH),
            Err(AuthError::InvalidCredentials)
        ));
    }
}
