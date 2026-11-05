//! Entering an app as one of its users, through a one-time link (ADR-0027,
//! decision 3).
//!
//! Whoever manages the app, an owner in Konae or an operator on the machine,
//! calls `issue` with the account and a label that says who enters. The link
//! opens `/_konae/enter` on the app's own origin. The page posts the token
//! from the fragment, so GET changes nothing, and the POST consumes it and
//! sets the app session cookie for that account.
//!
//! The link lives in the app's `identity.sqlite`, so a command on the machine
//! can issue one for a running Kernel. The session it starts carries the
//! label in `app_sessions.entered_by`. Usage and Signals do not count what
//! such a session does (`entered`), and the caller of `issue` records who
//! entered as whom in its own audit log, since the Kernel keeps no owners.

use super::*;
use rusqlite::{OptionalExtension, params};

/// How long a link works. It is shown to the person who asked for it and
/// opened at once, so it does not need to last.
pub const ENTRY_TTL_SECONDS: i64 = 120;
/// Open links one app keeps at once; the oldest goes first.
const MAX_LINKS: i64 = 100;

pub const NO_ACCOUNT: &str = "This app has no account with that ID.";
pub const NOT_ACTIVE: &str = "Only an active account can be entered. Resume a suspended account first; a pending one has not finished signing up.";
pub const LABEL_INVALID: &str = "Say who enters, in at most 200 characters.";

pub fn routes() -> Router<AppState> {
    Router::new().route("/_konae/enter", get(entry_page).post(enter))
}

/// Whether a signed-in account came through an entry link. Usage and
/// Signals do not count such a session as its user.
pub fn entered(actor: Option<&AuthUser>) -> bool {
    actor.is_some_and(|user| user.entered_by.is_some())
}

/// Whether the session a request carries came through an entry link, for
/// counters that do not otherwise read the session.
pub fn entered_request(state: &AppState, deployment: &Deployment, headers: &HeaderMap) -> bool {
    matches!(authenticate_deployment(state, deployment, headers), Ok(Some(user)) if user.entered_by.is_some())
}

/// Issues a one-time link that signs `entered_by` in as the account
/// `user_id` of `deployment`. Answers the account's email, the link and when
/// it stops working. The caller holds no lock; this takes the identity lock.
pub async fn issue(state: &AppState, deployment: &Deployment, user_id: &str, entered_by: &str) -> Result<Value> {
    anyhow::ensure!(!entered_by.trim().is_empty() && entered_by.len() <= 200, LABEL_INVALID);
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = auth::now_seconds() + ENTRY_TTL_SECONDS;
    let email = {
        let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
        let auth = accounts::app_store(state, deployment).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))?;
        let (user_id, entered_by, hash) = (user_id.to_owned(), entered_by.to_owned(), auth::hash_token(&token));
        tokio::task::spawn_blocking(move || {
            auth.transaction(|tx| {
                let account: Option<(String, String)> = tx
                    .query_row("SELECT email, state FROM app_users WHERE id = ?1", [&user_id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                let Some((email, account_state)) = account else { return Ok(Err(NO_ACCOUNT)) };
                if account_state != "active" {
                    return Ok(Err(NOT_ACTIVE));
                }
                let now = auth::now_seconds();
                tx.execute("DELETE FROM entry_links WHERE expires_at <= ?1", [now])?;
                tx.execute(
                    "DELETE FROM entry_links WHERE rowid NOT IN (SELECT rowid FROM entry_links ORDER BY expires_at DESC LIMIT ?1)",
                    [MAX_LINKS - 1],
                )?;
                tx.execute(
                    "INSERT INTO entry_links (token_hash, user_id, entered_by, expires_at) VALUES (?1, ?2, ?3, ?4)",
                    params![hash, user_id, entered_by, expires_at],
                )?;
                Ok(Ok(email))
            })
        })
        .await
        .map_err(|_| anyhow::anyhow!("The request did not complete."))?
        .map_err(|error_value| anyhow::anyhow!(error_value.to_string()))?
        .map_err(anyhow::Error::msg)?
    };
    Ok(json!({
        "userId": user_id,
        "email": email,
        "url": format!("{}/_konae/enter#entry={token}", deployment.url),
        "expiresAt": expires_at,
    }))
}

/// Takes the link for `token` and returns the account and who enters. A
/// link is removed on its first use, valid or not.
fn take(tx: &rusqlite::Transaction<'_>, token: &str) -> std::result::Result<Option<AuthUser>, AuthError> {
    let hash = auth::hash_token(token);
    let link: Option<(String, String, i64)> = tx
        .query_row("SELECT user_id, entered_by, expires_at FROM entry_links WHERE token_hash = ?1", [&hash], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    tx.execute("DELETE FROM entry_links WHERE token_hash = ?1", [&hash])?;
    let Some((user_id, entered_by, _)) = link.filter(|(_, _, expires_at)| *expires_at > auth::now_seconds()) else {
        return Ok(None);
    };
    // The email is read again: the account may have changed since the link was issued.
    let email: Option<String> = tx.query_row("SELECT email FROM app_users WHERE id = ?1", [&user_id], |row| row.get(0)).optional()?;
    Ok(email.map(|name| AuthUser { user_id, name, entered_by: Some(entered_by) }))
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
    let cookie_name = app_cookie_name(&state, &deployment.id, &headers);
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024).await else {
        return error(StatusCode::BAD_REQUEST, "The request is too large.");
    };
    let invalid = || error(StatusCode::GONE, &AuthError::InvalidChallenge.to_string());
    let Ok(answer) = serde_json::from_slice::<EntryAnswer>(&bytes) else { return invalid() };
    let _identity = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    let auth = match accounts::app_store(&state, &deployment) {
        Ok(auth) => auth,
        Err(error_value) => return auth_error(error_value),
    };
    let session = tokio::task::spawn_blocking(move || {
        let user = auth.transaction(|tx| take(tx, &answer.token))?;
        // start_session refuses an account that is no longer active.
        auth.start_session(user.ok_or(AuthError::InvalidChallenge)?)
    })
    .await
    .unwrap_or(Err(AuthError::PasswordHash));
    match session {
        // An entry is no sign-in of the account, so Usage counts nothing.
        Ok(session) => accounts::app_session_response(&cookie_name, session),
        Err(error_value) => auth_error(error_value),
    }
}

async fn entry_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if deployment_for(&state, &headers).await.is_none() {
        return error(StatusCode::NOT_FOUND, "Konae deployment not found.");
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
const ENTRY_PAGE: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Opening the app</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;font:16px system-ui,sans-serif;color:#20232a;background:#f4f5f7}main{max-width:380px}#error{color:#a32727}@media (prefers-color-scheme:dark){body{background:#14161a;color:#e8eaed}#error{color:#ff8a8a}}</style><link rel="icon" href="data:,">
<main><h1>Opening the app</h1><p id="error" role="alert"></p></main>
<script>
const match=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);const prefix=match?'/p/'+match[1]:'';const token=new URLSearchParams(location.hash.slice(1)).get('entry');history.replaceState(null,'',location.pathname);const fail=(text)=>{document.querySelector('#error').textContent=text+' Ask for a new link.'};if(!token){fail('This link has no entry token.')}else{fetch(prefix+'/_konae/enter',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({token})}).then(async(response)=>{if(response.ok){location.replace(prefix+'/');return}const body=await response.json().catch(()=>({}));fail(body.error||'The app did not open.')},()=>fail('The app did not answer.'))}
</script></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (PathBuf, AuthService) {
        let root = std::env::temp_dir().join(format!("konae-enter-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let auth = AuthService::open_in(root.join("identity.sqlite"), "deployment:app", auth::Verification::Local).unwrap();
        (root, auth)
    }

    fn link(auth: &AuthService, user_id: &str, expires_at: i64) -> String {
        let token = Uuid::new_v4().simple().to_string();
        auth.transaction(|tx| {
            tx.execute(
                "INSERT INTO entry_links (token_hash, user_id, entered_by, expires_at) VALUES (?1, ?2, 'owner:olga', ?3)",
                params![auth::hash_token(&token), user_id, expires_at],
            )?;
            Ok(())
        })
        .unwrap();
        token
    }

    #[test]
    fn a_link_opens_once_and_marks_the_session() {
        let (root, auth) = store();
        let ana = auth.signup("ana@example.test", "ana's password").unwrap().user;
        let token = link(&auth, &ana.user_id, auth::now_seconds() + 60);
        let user = auth.transaction(|tx| take(tx, &token)).unwrap().unwrap();
        assert_eq!((user.user_id.as_str(), user.entered_by.as_deref()), (ana.user_id.as_str(), Some("owner:olga")));
        let session = auth.start_session(user).unwrap();
        let signed_in = auth.authenticate(Some(&session.token)).unwrap().unwrap();
        assert_eq!(signed_in.entered_by.as_deref(), Some("owner:olga"));
        assert!(entered(Some(&signed_in)));
        assert!(auth.transaction(|tx| take(tx, &token)).unwrap().is_none(), "a link opens once");
        // The account's own sign-in is not marked.
        let own = auth.login("ana@example.test", "ana's password").unwrap();
        assert!(!entered(auth.authenticate(Some(&own.token)).unwrap().as_ref()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_expired_link_opens_nothing_and_is_removed() {
        let (root, auth) = store();
        let ana = auth.signup("ana@example.test", "ana's password").unwrap().user;
        let token = link(&auth, &ana.user_id, auth::now_seconds() - 1);
        assert!(auth.transaction(|tx| take(tx, &token)).unwrap().is_none());
        let left: i64 = auth.read(|db| db.query_row("SELECT COUNT(*) FROM entry_links", [], |row| row.get(0)).map_err(AuthError::Storage)).unwrap();
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_page_reads_its_token_from_the_fragment() {
        assert!(ENTRY_PAGE.contains("location.hash"));
        assert!(ENTRY_PAGE.contains("/_konae/enter"));
        assert!(!ENTRY_PAGE.contains("owner"));
    }
}
