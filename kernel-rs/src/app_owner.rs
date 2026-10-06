//! The Project Owner's own account inside an app.
//!
//! `app_owner_enter` creates or links the app account whose email is the
//! owner's creator email, gives it the role `owner` in the Deployment's
//! `identity.sqlite`, and returns a one-time entry link on the app's origin.
//! Only the owner's browser session may call it. The link opens a page that
//! posts its token from the fragment, so GET changes nothing; the POST
//! consumes the token and sets the app session cookie. The role then reaches
//! the app through `auth_session` and through the actor binding
//! `{ "source": "actor", "field": "role" }`, so a guard can check it.
//!
//! The Kernel never gives the role `owner` any other way: signup, invitation
//! acceptance and recovery keep the role the account has. An invitation the
//! owner sends to their own creator email marks that pending account `owner`
//! in `invitations.rs`, since only the owner's mailbox can answer it.

use super::*;
use auth::AuthUser;
use mcp::Owner;
use rusqlite::{OptionalExtension, params};
use std::sync::{Mutex, OnceLock};

/// How long an entry link works. It is shown to the owner who asked for it
/// and opened at once, so it does not need to last.
pub(crate) const ENTRY_TTL_SECONDS: i64 = 120;
/// Entry links one installation keeps at once; an older one is dropped first.
const MAX_ENTRIES: usize = 1000;

/// An entry token waiting for its POST. The map is keyed by the token's
/// SHA-256 and lives only in this process: a restart ends every open link,
/// which costs the owner one more click.
struct Entry {
    deployment_id: String,
    user_id: String,
    expires_at: i64,
}

fn entries() -> &'static Mutex<HashMap<String, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/_littlebox/owner-entry", get(entry_page).post(enter))
}

pub(crate) fn admin_tool() -> Value {
    mcp::tool(
        "app_owner_enter",
        "Open a deployment as the Project Owner's own app account. Creates the account for the owner's email the first time, or links the existing one, gives it the role owner that auth_session returns and the actor binding { \"source\": \"actor\", \"field\": \"role\" } yields, and returns a link valid for two minutes that signs that account in on the app. Needs the owner's browser session.",
        json!({"deploymentId": {"type": "string", "minLength": 1}}),
        &["deploymentId"],
        false,
    )
}

/// What happened to the account the owner enters with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Claimed {
    pub(crate) user_id: String,
    pub(crate) created: bool,
    /// The account never proved its email, so someone else may know its
    /// password. Its password and sessions were cleared.
    pub(crate) password_cleared: bool,
}

/// The refusal for an owner whose app account is suspended.
pub(crate) const SUSPENDED: &str = "Your app account on this app is suspended. Resume it in the Users view first.";
/// The refusal for an owner the Kernel has no email for.
pub(crate) const NO_EMAIL: &str = "Your owner account signs in through the hosted identity provider, so this installation has no email to give your app account. Sign up in the app instead.";
/// The refusal for a delegated token.
pub(crate) const SESSION_ONLY: &str = "app_owner_enter signs you in to the app, so it takes the owner's own browser session. A delegated token cannot call it.";

/// Gives the account for `email` the role `owner`, creating it active when
/// there is none. `proved` says the installation requires verification, so
/// the creator scope already proved this address and the account counts as
/// verified. A pending account, which may be a stranger's unanswered signup,
/// is taken over: its open links and password go. An active account that
/// never verified loses its password and sessions for the same reason. A
/// suspended account is refused.
pub(crate) fn claim(tx: &rusqlite::Transaction<'_>, email: &str, proved: bool) -> Result<Result<Claimed, &'static str>, AuthError> {
    let now = auth::now_seconds();
    let verified_at = proved.then_some(now);
    let account: Option<(String, String, Option<i64>)> = tx
        .query_row(
            "SELECT id, state, verified_at FROM app_users WHERE email = ?1 COLLATE NOCASE",
            [email],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (user_id, created, clear) = match account {
        None => {
            let user_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO app_users (id, email, password_hash, role, created_at, state, verified_at)
                 VALUES (?1, ?2, ?3, 'owner', ?4, 'active', ?5)",
                params![user_id, email, auth::NO_PASSWORD, now, verified_at],
            )?;
            return Ok(Ok(Claimed { user_id, created: true, password_cleared: false }));
        }
        Some((_, state, _)) if state == "suspended" => return Ok(Err(SUSPENDED)),
        Some((user_id, state, _)) if state == "pending" => (user_id, false, true),
        Some((user_id, _, verified)) => (user_id, false, verified.is_none()),
    };
    if clear {
        tx.execute(
            "UPDATE app_users SET password_hash = ?1, state = 'active', verified_at = COALESCE(verified_at, ?2) WHERE id = ?3",
            params![auth::NO_PASSWORD, verified_at, user_id],
        )?;
        tx.execute("DELETE FROM app_sessions WHERE user_id = ?1", [&user_id])?;
        tx.execute("DELETE FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL", [&user_id])?;
    }
    tx.execute("UPDATE app_users SET role = 'owner' WHERE id = ?1", [&user_id])?;
    Ok(Ok(Claimed { user_id, created, password_cleared: clear }))
}

/// The creator email of an owner, from `owners.sqlite`. None for an owner
/// signed in through the hosted identity adapter, which keeps no email here.
pub(crate) fn owner_email(state: &AppState, owner_id: &str) -> Option<String> {
    let path = state.data_root.join("owners.sqlite");
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row("SELECT email FROM app_users WHERE id = ?1", [owner_id], |row| row.get(0)).optional().ok().flatten()
}

pub(crate) async fn admin_call(state: &AppState, owner: &Owner, arguments: &Value) -> Result<Value> {
    anyhow::ensure!(owner.session, SESSION_ONLY);
    let id = arguments["deploymentId"].as_str().unwrap_or_default();
    let deployment = mcp::owned_deployment(state, owner, id, "manage_users").await?;
    let email = owner_email(state, &owner.id).ok_or_else(|| anyhow::anyhow!(NO_EMAIL))?;
    let proved = state.accounts.verification == auth::Verification::Required;
    let claimed = {
        let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
        let auth = accounts::app_store(state, &deployment)?;
        let email = email.clone();
        tokio::task::spawn_blocking(move || auth.transaction(|tx| claim(tx, &email, proved)))
            .await
            .map_err(|_| anyhow::anyhow!("The request did not complete."))??
    };
    let claimed = claimed.map_err(|message| anyhow::anyhow!(message))?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = auth::now_seconds() + ENTRY_TTL_SECONDS;
    {
        let mut open = entries().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = auth::now_seconds();
        open.retain(|_, entry| entry.expires_at > now);
        if open.len() >= MAX_ENTRIES {
            if let Some(oldest) = open.iter().min_by_key(|(_, entry)| entry.expires_at).map(|(key, _)| key.clone()) {
                open.remove(&oldest);
            }
        }
        open.insert(
            auth::hash_token(&token),
            Entry { deployment_id: deployment.id.clone(), user_id: claimed.user_id.clone(), expires_at },
        );
    }
    Ok(json!({
        "userId": claimed.user_id,
        "email": email,
        "role": "owner",
        "account": if claimed.created { "created" } else { "existing" },
        "passwordCleared": claimed.password_cleared,
        "url": format!("{}/_littlebox/owner-entry#entry={token}", deployment.url),
        "expiresAt": expires_at,
    }))
}

/// Takes the entry for `token` if it belongs to this Deployment and has not
/// expired. A token is removed on its first use, valid or not here.
fn take(deployment_id: &str, token: &str) -> Option<String> {
    let mut open = entries().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = open.remove(&auth::hash_token(token))?;
    (entry.deployment_id == deployment_id && entry.expires_at > auth::now_seconds()).then_some(entry.user_id)
}

#[derive(serde::Deserialize)]
struct EntryAnswer {
    token: String,
}

async fn enter(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024).await else {
        return error(StatusCode::BAD_REQUEST, "The request is too large.");
    };
    let invalid = || error(StatusCode::GONE, &AuthError::InvalidChallenge.to_string());
    let Ok(answer) = serde_json::from_slice::<EntryAnswer>(&bytes) else { return invalid() };
    let Some(user_id) = take(&deployment.id, &answer.token) else { return invalid() };
    let _identity_guard = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    let auth = match accounts::app_store(&state, &deployment) {
        Ok(auth) => auth,
        Err(error_value) => return auth_error(error_value),
    };
    let session = tokio::task::spawn_blocking(move || {
        // The role and email are read again: the account may have been
        // suspended or changed since the link was issued.
        let user = auth.read(|db| {
            db.query_row("SELECT email, role FROM app_users WHERE id = ?1", [&user_id], |row| {
                Ok(AuthUser { user_id: user_id.clone(), name: row.get(0)?, role: row.get(1)? })
            })
            .optional()
            .map_err(AuthError::Storage)
        })?;
        auth.start_session(user.ok_or(AuthError::InvalidChallenge)?)
    })
    .await
    .unwrap_or(Err(AuthError::PasswordHash));
    match session {
        Ok(session) => {
            usage::count(&state.data_root, &deployment.id, usage::Operation::Login);
            accounts::app_session_response(&state, session)
        }
        Err(error_value) => auth_error(error_value),
    }
}

async fn entry_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if deployment_for(&state, &headers).await.is_none() {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    }
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        ENTRY_PAGE,
    )
        .into_response()
}

/// The page an entry link opens. It reads the token from the fragment, drops
/// it from the address bar and posts it; GET does nothing else.
const ENTRY_PAGE: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Opening the app</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;font:16px system-ui,sans-serif;color:#20232a;background:#f4f5f7}main{max-width:380px}#error{color:#a32727}</style>
<main><h1>Opening the app as its owner</h1><p id="error" role="alert"></p></main>
<script>
const token=new URLSearchParams(location.hash.slice(1)).get('entry');history.replaceState(null,'',location.pathname);const fail=(text)=>{document.querySelector('#error').textContent=text+' Open the app again from the Users view in the editor.'};if(!token){fail('This link has no entry token.')}else{fetch('/_littlebox/owner-entry',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({token})}).then(async(response)=>{if(response.ok){location.replace('/');return}const body=await response.json().catch(()=>({}));fail(body.error||'The app did not open.')},()=>fail('The app did not answer.'))}
</script></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, auth::AuthService) {
        let root = std::env::temp_dir().join(format!("littlebox-app-owner-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let auth = auth::AuthService::open_in(root.join("identity.sqlite"), "deployment:app", auth::Verification::Local).unwrap();
        (root, auth)
    }

    fn row(auth: &auth::AuthService, email: &str) -> (String, String, String, Option<i64>) {
        auth.read(|db| {
            db.query_row("SELECT role, state, password_hash, verified_at FROM app_users WHERE email = ?1", [email], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(AuthError::Storage)
        })
        .unwrap()
    }

    #[test]
    fn the_owner_gets_an_active_owner_account_and_no_other_account_changes() {
        let (root, auth) = store();
        auth.signup("ana@example.test", "ana's password").unwrap();
        let first = auth.transaction(|tx| claim(tx, "olga@example.test", true)).unwrap().unwrap();
        assert!(first.created && !first.password_cleared);
        let (role, state, password, verified) = row(&auth, "olga@example.test");
        assert_eq!((role.as_str(), state.as_str(), password.as_str()), ("owner", "active", auth::NO_PASSWORD));
        assert!(verified.is_some(), "under required verification the creator scope proved the address");
        // A second claim links the same account and changes nothing else.
        let again = auth.transaction(|tx| claim(tx, "OLGA@example.test", true)).unwrap().unwrap();
        assert_eq!(again, Claimed { user_id: first.user_id.clone(), created: false, password_cleared: false });
        assert_eq!(row(&auth, "ana@example.test").0, "member", "only the owner's email gets the role");
        // In local mode nothing is proved, so the account stays unverified.
        let local = auth.transaction(|tx| claim(tx, "lea@example.test", false)).unwrap().unwrap();
        assert!(local.created);
        assert_eq!(row(&auth, "lea@example.test").3, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unproved_account_under_the_owners_email_loses_its_password_and_sessions() {
        let (root, auth) = store();
        // Someone signed up first with the owner's email on a local installation.
        let stranger = auth.signup("olga@example.test", "the stranger's password").unwrap();
        let claimed = auth.transaction(|tx| claim(tx, "olga@example.test", false)).unwrap().unwrap();
        assert_eq!(claimed, Claimed { user_id: stranger.user.user_id.clone(), created: false, password_cleared: true });
        assert!(auth.authenticate(Some(&stranger.token)).unwrap().is_none(), "the stranger's session ends");
        assert!(auth.login("olga@example.test", "the stranger's password").is_err(), "and their password no longer works");
        assert_eq!(row(&auth, "olga@example.test").0, "owner");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_pending_account_is_taken_over_and_a_suspended_one_is_refused() {
        let (root, _local) = store();
        let required = auth::AuthService::open_in(root.join("identity.sqlite"), "deployment:app", auth::Verification::Required).unwrap();
        required.request_signup("olga@example.test").unwrap().unwrap();
        let claimed = required.transaction(|tx| claim(tx, "olga@example.test", true)).unwrap().unwrap();
        assert!(!claimed.created && claimed.password_cleared);
        let (role, state, _, verified) = row(&required, "olga@example.test");
        assert_eq!((role.as_str(), state.as_str()), ("owner", "active"));
        assert!(verified.is_some());
        let open: i64 = required
            .read(|db| db.query_row("SELECT COUNT(*) FROM account_challenges WHERE consumed_at IS NULL", [], |r| r.get(0)).map_err(AuthError::Storage))
            .unwrap();
        assert_eq!(open, 0, "the pending signup's link no longer works");
        required.suspend("olga@example.test").unwrap();
        assert_eq!(required.transaction(|tx| claim(tx, "olga@example.test", true)).unwrap(), Err(SUSPENDED));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_entry_token_works_once_on_its_own_deployment_until_it_expires() {
        let put = |token: &str, deployment: &str, expires_at: i64| {
            entries().lock().unwrap().insert(
                auth::hash_token(token),
                Entry { deployment_id: deployment.into(), user_id: "olga".into(), expires_at },
            );
        };
        let later = auth::now_seconds() + ENTRY_TTL_SECONDS;
        put("token-one", "app-a", later);
        assert_eq!(take("app-a", "token-one").as_deref(), Some("olga"));
        assert_eq!(take("app-a", "token-one"), None, "a token works once");
        put("token-two", "app-a", later);
        assert_eq!(take("app-b", "token-two"), None, "another app refuses it");
        assert_eq!(take("app-a", "token-two"), None, "and the refusal used it up");
        put("token-three", "app-a", auth::now_seconds() - 1);
        assert_eq!(take("app-a", "token-three"), None, "an expired token is refused");
    }
}
