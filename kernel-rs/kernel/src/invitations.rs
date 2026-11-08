//! App invitations and the owner's user tools.
//!
//! An invitation fixes one email on one Deployment. Its secret is an
//! `invitation` challenge in `account_challenges`, so it expires, works once
//! and does nothing on another Deployment or for another purpose, like every
//! other account link. The `app_invitations` row beside it is the history:
//! who invited whom, when, whether the mail transport took the message, and
//! whether the person accepted. A full copy deletes the challenges and
//! `forget_tokens` clears the rows' token hashes, so an export carries the
//! recipients and never a working link; the destination resends them.
//!
//! Inviting creates a pending account for a new email, the way a signup that
//! waits for verification does. Accepting answers the challenge in one
//! transaction that rechecks the Deployment's signup policy and the
//! account's state, sets the password, activates the account, marks the
//! invitation accepted and issues the session.

use super::*;
use accounts::app_store;
use auth::{AuthSession, IssuedChallenge, NewPassword, Purpose};
use rusqlite::params;

/// The most recipients one `app_invitations_create` call accepts.
pub const MAX_BATCH: usize = 50;
/// The most rows one page of `app_users_list` or `app_invitations_list` returns.
pub const MAX_PAGE: usize = 100;
/// How long the answer stored for an idempotency key is kept.
const IDEMPOTENCY_SECONDS: i64 = 7 * 24 * 60 * 60;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/_konae/invitation", get(invitation_page))
        .route("/_konae/auth/invitation", post(inspect_route))
        .route("/_konae/auth/accept", post(accept_route))
}

/// Whether a valid invitation may activate an account for this email under
/// the policy. Open, Invitation only and Closed admit it: Closed stops people
/// from signing themselves up, and the owner chose the invited person. An
/// Allowlist still has to list the email, because the list is the owner's
/// own rule about addresses. ADR-0003.
pub fn admits_invitation(policy: &SignupPolicy, email: &str) -> bool {
    match policy.mode {
        SignupMode::Open | SignupMode::Invitation | SignupMode::Closed => true,
        SignupMode::Allowlist => policy.allows(email),
    }
}

fn install(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_invitations (
           id TEXT PRIMARY KEY,
           email TEXT NOT NULL COLLATE NOCASE,
           deployment TEXT NOT NULL,
           inviter TEXT NOT NULL,
           user_id TEXT,
           created_user INTEGER NOT NULL DEFAULT 0,
           state TEXT NOT NULL CHECK (state IN ('pending', 'accepted', 'revoked')),
           token_hash TEXT,
           created_at INTEGER NOT NULL,
           expires_at INTEGER NOT NULL,
           sends INTEGER NOT NULL DEFAULT 0,
           delivery TEXT NOT NULL DEFAULT 'not-sent' CHECK (delivery IN ('not-sent', 'queued', 'failed')),
           accepted_at INTEGER,
           revoked_at INTEGER
         );
         CREATE UNIQUE INDEX IF NOT EXISTS app_invitations_one_pending ON app_invitations(email) WHERE state = 'pending';
         CREATE INDEX IF NOT EXISTS app_invitations_created ON app_invitations(created_at, id);
         CREATE TABLE IF NOT EXISTS app_invitation_requests (
           inviter TEXT NOT NULL,
           key TEXT NOT NULL,
           request TEXT NOT NULL,
           response TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           PRIMARY KEY (inviter, key)
         );",
    )
}

/// Clears what a copy of an identity database must not carry: the token
/// hashes, whose challenges the caller deletes, and the answers stored for
/// idempotency keys. Pending invitations stay as recipients that need a
/// resend. Runs on the sanitized copy of an export and on an import.
pub fn forget_tokens(connection: &Connection) -> rusqlite::Result<()> {
    install(connection)?;
    connection.execute_batch(
        "UPDATE app_invitations SET token_hash = NULL, delivery = 'not-sent' WHERE token_hash IS NOT NULL;
         DELETE FROM app_invitation_requests;",
    )
}

/// Opens the Deployment's account store with the invitation tables installed.
pub fn store(state: &AppState, deployment: &Deployment) -> Result<AuthService, AuthError> {
    let auth = app_store(state, deployment)?;
    auth.read(|connection| install(connection).map_err(AuthError::Storage))?;
    Ok(auth)
}

fn storage(error_value: rusqlite::Error) -> AuthError {
    AuthError::Storage(error_value)
}

// ---------------------------------------------------------------------------
// Acceptance

/// What the invitation page shows before the person chooses a password: the
/// invited email, the app and whether accepting can work. Changes nothing.
pub async fn inspect(state: &AppState, deployment: &Deployment, token: &str) -> Result<Value, AuthError> {
    let auth = store(state, deployment)?;
    let token_hash = auth::hash_token(token);
    let scope = auth::deployment_scope(&deployment.id);
    let found: Option<(String, String)> = blocking(move || {
        auth.read(|connection| {
            connection
                .query_row(
                    "SELECT u.email, u.state FROM account_challenges c
                     JOIN app_users u ON u.id = c.user_id
                     JOIN app_invitations i ON i.token_hash = c.token_hash AND i.state = 'pending'
                     WHERE c.token_hash = ?1 AND c.purpose = 'invitation' AND c.scope = ?2
                       AND c.consumed_at IS NULL AND c.expires_at > ?3",
                    params![token_hash, scope, auth::now_seconds()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)
        })
    })
    .await?;
    let Some((email, account)) = found else {
        return Err(AuthError::InvalidChallenge);
    };
    let status = match account.as_str() {
        "active" => "existing-account",
        "suspended" => return Err(AuthError::AccountSuspended),
        _ if !admits_invitation(&deployment.signup_policy, &email) => "signup-closed",
        _ => "new-account",
    };
    Ok(json!({"email": email, "app": deployment.project_name, "status": status}))
}

/// Accepts an invitation with the password the person chooses now. The
/// Deployment's current policy and the account's state are read inside the
/// transaction that consumes the token.
pub async fn accept(
    state: &AppState,
    deployment: &Deployment,
    token: &str,
    password: &str,
) -> Result<AuthSession, AuthError> {
    let password = password.to_owned();
    let chosen = blocking(move || NewPassword::prepare(&password)).await?;
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let auth = store(state, deployment)?;
    let deployments = state.deployments.clone();
    let id = deployment.id.clone();
    let token = token.to_owned();
    let session = blocking(move || accept_in(&auth, &token, chosen, |email| {
        deployments
            .blocking_read()
            .values()
            .find(|current| current.id == id)
            .is_some_and(|current| admits_invitation(&current.signup_policy, email))
    }))
    .await?;
    super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Signup);
    Ok(session)
}

fn accept_in(
    auth: &AuthService,
    token: &str,
    chosen: NewPassword,
    admits: impl FnOnce(&str) -> bool,
) -> Result<AuthSession, AuthError> {
    auth.complete_challenge_with(Purpose::Invitation, token, chosen, admits, mark_accepted)
}

/// Marks the pending invitation of a consumed challenge accepted, inside the
/// transaction that consumed it. Used by the password and the Google answer.
pub fn mark_accepted(transaction: &Connection, user_id: &str, token_hash: &str) -> Result<(), AuthError> {
    let marked = transaction
        .execute(
            "UPDATE app_invitations SET state = 'accepted', accepted_at = ?1, user_id = ?2, token_hash = NULL
             WHERE token_hash = ?3 AND state = 'pending'",
            params![auth::now_seconds(), user_id, token_hash],
        )
        .map_err(storage)?;
    if marked == 1 { Ok(()) } else { Err(AuthError::InvalidChallenge) }
}

#[derive(Deserialize)]
struct TokenInput {
    token: String,
}

#[derive(Deserialize)]
struct AcceptInput {
    token: String,
    password: String,
}

async fn inspect_route(State(state): State<AppState>, request: Request) -> Response {
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let input: TokenInput = match accounts::read_body(&state, request, &deployment.id, |_| "account-link".into()).await {
        Ok((input, _client)) => input,
        Err(response) => return response,
    };
    match inspect(&state, &deployment, &input.token).await {
        Ok(value) => {
            let mut response = Json(json!({"result": value})).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error_value) => auth_error(error_value),
    }
}

async fn accept_route(State(state): State<AppState>, request: Request) -> Response {
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let cookie_name = crate::app_cookie_name(&state, &deployment.id, request.headers());
    let input: AcceptInput = match accounts::read_body(&state, request, &deployment.id, |_| "account-link".into()).await {
        Ok((input, _client)) => input,
        Err(response) => return response,
    };
    match accept(&state, &deployment, &input.token, &input.password).await {
        Ok(session) => accounts::app_session_response(&cookie_name, session),
        Err(error_value) => auth_error(error_value),
    }
}

async fn invitation_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Konae deployment not found.");
    };
    // With Google offered, the page can accept with the Google account of the
    // invited email instead of a password.
    let page = if google::offered(&state, &deployment) {
        INVITATION_PAGE.replacen(
            "<button>Accept and enter</button></form>",
            r#"<button>Accept and enter</button><button id="google" type="button">Accept with Google</button></form>"#,
            1,
        )
    } else {
        INVITATION_PAGE.to_owned()
    };
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        page,
    )
        .into_response()
}

/// The page an invitation link opens. The token stays in the fragment. The
/// page asks the Kernel which email the token was issued for, shows it read
/// only, and posts the token with the chosen password. GET changes nothing.
const INVITATION_PAGE: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Your invitation</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}form,label{display:grid;gap:8px}form{gap:16px;margin-top:24px}input,button{font:inherit;border-radius:8px;padding:11px 12px}input{width:100%;border:1px solid #c9ced6}input[readonly]{background:#f4f5f7;color:#20232a}button{border:0;background:#273449;color:white;cursor:pointer}#error{min-height:1.5em;margin:0;color:#a32727}[hidden]{display:none}@media (prefers-color-scheme:dark){body{background:#14161a;color:#e8eaed}main{background:#1d2026;border-color:#2e333b;box-shadow:none}p{color:#a9b0bc}input{background:#14161a;color:#e8eaed;border-color:#3a404a}input[readonly]{background:#23272e;color:#e8eaed}button{background:#e8eaed;color:#14161a}button+button{background:#2a2f37;color:#e8eaed}#error{color:#ff8a8a}a{color:#8ab4ff}}</style><link rel="icon" href="data:,">
<main><h1 id="title">Your invitation</h1><p id="note">Checking your invitation.</p>
<form id="accept" hidden><label>Email <input id="email" type="email" readonly></label><label>Choose a password <input name="password" type="password" minlength="8" required autocomplete="new-password"></label><button>Accept and enter</button></form>
<p id="error" role="alert"></p><p><a id="signin" href="/_konae/login">Sign in instead</a></p></main>
<script>
const token=new URLSearchParams(location.hash.slice(1)).get('invitation');const $=s=>document.querySelector(s);const pathMatch=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);const prefix=pathMatch?'/p/'+pathMatch[1]:'';$('#signin').href=prefix+'/_konae/login';
async function post(path,body){const r=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});const b=await r.json().catch(()=>({}));if(!r.ok)throw Error(b.error||'Request failed.');return b.result;}
if(!token){$('#note').textContent='This page needs the link from your invitation email.';}else{history.replaceState(null,'',location.pathname);post(prefix+'/_konae/auth/invitation',{token}).then(r=>{$('#title').textContent='Join '+r.app;$('#email').value=r.email;if(r.status==='existing-account'){$('#note').textContent='This email already has an account here. Sign in with its password.';return}if(r.status==='signup-closed'){$('#note').textContent='This app is not accepting new accounts right now. Ask whoever invited you.';return}$('#note').textContent='Choose the password for your account. Accepting activates it and signs you in.';$('#accept').hidden=false;},x=>{$('#note').textContent='';$('#error').textContent=x.message});}
$('#google')?.addEventListener('click',async()=>{try{const r=await post(prefix+'/_konae/auth/google/start',{invitation:token});location.assign(r.location);}catch(x){$('#error').textContent=x.message}});
$('#accept').addEventListener('submit',async e=>{e.preventDefault();try{await post(prefix+'/_konae/auth/accept',{token,password:new FormData(e.currentTarget).get('password')});location.replace(prefix+'/');}catch(x){$('#error').textContent=x.message}});
</script></html>"#;

// ---------------------------------------------------------------------------
// Owner tools

/// Who sends an invitation: the ID its idempotency keys and rows record, and
/// the email the message names, when there is one.
pub struct Inviter {
    pub id: String,
    pub email: Option<String>,
}

/// A refusal that names what the owner should do instead, or a storage error.
enum Refusal {
    Said(&'static str),
    Auth(AuthError),
}

impl From<AuthError> for Refusal {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(value: rusqlite::Error) -> Self {
        Self::Auth(AuthError::Storage(value))
    }
}

/// Runs `work` in the store's write transaction. A refusal rolls back
/// everything the closure wrote and reaches the owner as its message.
fn write<T>(auth: &AuthService, work: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, Refusal>) -> Result<T> {
    let mut refused = None;
    let result = auth.transaction(|tx| match work(tx) {
        Ok(value) => Ok(value),
        Err(Refusal::Auth(error_value)) => Err(error_value),
        Err(Refusal::Said(message)) => {
            refused = Some(message);
            // Any error rolls the transaction back; the message replaces it.
            Err(AuthError::InvalidChallenge)
        }
    });
    match (result, refused) {
        (_, Some(message)) => anyhow::bail!(message),
        (result, None) => Ok(result?),
    }
}

/// Runs blocking store work off the async runtime.
async fn off<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| anyhow::anyhow!("The request did not complete."))?
}

/// Runs one of the app user and invitation tools on a Deployment the caller
/// already authorized.
pub async fn run(state: &AppState, deployment: &Deployment, sender: &Inviter, name: &str, a: &Value) -> Result<Value> {
    let inviter = sender.id.as_str();
    let limit = a["limit"].as_u64().map_or(50, |value| value as usize).clamp(1, MAX_PAGE);
    let cursor = a["cursor"].as_str().map(str::to_owned);
    let text = |key: &str| a[key].as_str().unwrap_or_default().to_owned();
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let auth = Arc::new(store(state, deployment)?);
    let (inviter, target) = (inviter.to_owned(), deployment.clone());
    match name {
        "app_users_list" => {
            let filter = a["state"].as_str().map(str::to_owned);
            off(move || Ok(auth.read(|db| list_users(db, filter.as_deref(), cursor.as_deref(), limit))?)).await
        }
        "app_invitations_list" => off(move || Ok(auth.read(|db| list_invitations(db, cursor.as_deref(), limit))?)).await,
        "app_user_suspend" | "app_user_resume" => {
            let (user_id, suspend) = (text("userId"), name == "app_user_suspend");
            let value = off(move || set_suspended(&auth, &user_id, suspend)).await?;
            if suspend {
                let entry = super::recovery::Entry::new("app-user", Some(&deployment.id), &text("userId"), "suspended");
                super::recovery::record(state, entry).await?;
            }
            Ok(value)
        }
        "app_invitation_revoke" => {
            let invitation = text("invitationId");
            off(move || write(&auth, |tx| revoke(tx, &invitation))).await
        }
        "app_invitations_create" | "app_invitation_resend" => {
            // Without a transport nothing is created, so no invitation waits
            // for a message that cannot leave.
            accounts::mail_ready(state)?;
            let key = text("idempotencyKey");
            let prepared = if name == "app_invitations_create" {
                let emails: Vec<String> = serde_json::from_value(a["emails"].clone())?;
                anyhow::ensure!(emails.len() <= MAX_BATCH, "Invite at most {MAX_BATCH} people per call.");
                let (auth, inviter, key) = (auth.clone(), inviter.clone(), key.clone());
                off(move || write(&auth, |tx| create(tx, &auth, &target, &inviter, &key, &emails))).await?
            } else {
                let invitation = text("invitationId");
                let (auth, inviter, key) = (auth.clone(), inviter.clone(), key.clone());
                off(move || write(&auth, |tx| resend(tx, &auth, &target, &inviter, &key, &invitation))).await?
            };
            let Prepared { mut response, issued, request } = prepared;
            if issued.is_empty() {
                return Ok(response);
            }
            // The rows that record the invitations are committed before any
            // message leaves, so a retry with the same key finds them. Each
            // message then goes through the account mail outbox and its budgets.
            let page = format!("{}/_konae/invitation", super::domain::app_url(state, deployment));
            let invited_by = sender.email.clone();
            let mut outcomes: HashMap<String, &'static str> = HashMap::new();
            for (invitation, challenge) in issued {
                let scope = challenge.scope.clone();
                let delivery = match accounts::deliver_issued(state, deployment, challenge, page.clone(), None, invited_by.clone()).await {
                    Ok(()) => "queued",
                    Err(error_value) => {
                        eprintln!("Invitation email for {scope} failed: {error_value}");
                        "failed"
                    }
                };
                outcomes.insert(invitation, delivery);
            }
            let mark = |entry: &mut Value| {
                if let Some(delivery) = entry["invitationId"].as_str().and_then(|id| outcomes.get(id)) {
                    entry["delivery"] = json!(delivery);
                }
            };
            match response.get_mut("results").and_then(Value::as_array_mut) {
                Some(results) => results.iter_mut().for_each(mark),
                None => mark(&mut response),
            }
            let stored = response.clone();
            off(move || {
                auth.transaction(|tx| {
                    for (invitation, delivery) in &outcomes {
                        tx.execute(
                            "UPDATE app_invitations SET delivery = ?1, sends = sends + 1 WHERE id = ?2",
                            params![delivery, invitation],
                        )
                        .map_err(storage)?;
                    }
                    remember(tx, &inviter, &key, &request, &stored).map_err(storage)
                })?;
                Ok(response)
            })
            .await
        }
        _ => anyhow::bail!("Unknown tool."),
    }
}

/// The answer to a create or resend, the links to send once the rows that
/// record them are committed, and the request its idempotency key names. A
/// replayed key has no links.
struct Prepared {
    response: Value,
    issued: Vec<(String, IssuedChallenge)>,
    request: String,
}

/// Returns the answer stored for this key, or None for a new key. A key used
/// for a different request is refused.
fn replay(tx: &Connection, inviter: &str, key: &str, request: &str) -> Result<Option<Value>, Refusal> {
    tx.execute(
        "DELETE FROM app_invitation_requests WHERE created_at < ?1",
        [auth::now_seconds() - IDEMPOTENCY_SECONDS],
    )?;
    let stored: Option<(String, String)> = tx
        .query_row(
            "SELECT request, response FROM app_invitation_requests WHERE inviter = ?1 AND key = ?2",
            params![inviter, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match stored {
        None => Ok(None),
        Some((previous, response)) if previous == request => Ok(Some(serde_json::from_str(&response).unwrap_or(Value::Null))),
        Some(_) => Err(Refusal::Said("This idempotencyKey was already used for a different request. Use a new key.")),
    }
}

fn remember(tx: &Connection, inviter: &str, key: &str, request: &str, response: &Value) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO app_invitation_requests (inviter, key, request, response, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(inviter, key) DO UPDATE SET response = excluded.response",
        params![inviter, key, request, response.to_string(), auth::now_seconds()],
    )?;
    Ok(())
}

/// The account an invitation would activate: an existing pending one, or a
/// new pending one. Returns None for an active or suspended account.
fn pending_account(tx: &Connection, email: &str) -> Result<Result<(String, bool), &'static str>, Refusal> {
    let account: Option<(String, String)> = tx
        .query_row(
            "SELECT id, state FROM app_users WHERE email = ?1 COLLATE NOCASE",
            [email],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match account {
        Some((_, state)) if state == "active" => Err("already-member"),
        Some((_, state)) if state == "suspended" => Err("suspended"),
        Some((id, _)) => Ok((id, false)),
        None => {
            let id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO app_users (id, email, password_hash, created_at, state) VALUES (?1, ?2, ?3, ?4, 'pending')",
                params![id, email, auth::NO_PASSWORD, auth::now_seconds()],
            )?;
            Ok((id, true))
        }
    })
}

fn create(
    tx: &rusqlite::Transaction<'_>,
    auth: &AuthService,
    deployment: &Deployment,
    inviter: &str,
    key: &str,
    emails: &[String],
) -> Result<Prepared, Refusal> {
    let request = format!("create:{}", serde_json::to_string(emails).unwrap_or_default());
    if let Some(response) = replay(tx, inviter, key, &request)? {
        return Ok(Prepared { response, issued: Vec::new(), request });
    }
    let now = auth::now_seconds();
    let mut results = Vec::new();
    let mut issued = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in emails {
        let Ok(email) = auth::normalize_email(raw) else {
            results.push(json!({"email": raw, "status": "invalid-email"}));
            continue;
        };
        if !seen.insert(email.clone()) {
            continue;
        }
        let acceptable = admits_invitation(&deployment.signup_policy, &email);
        let pending: Option<String> = tx
            .query_row("SELECT id FROM app_invitations WHERE email = ?1 AND state = 'pending'", [&email], |row| row.get(0))
            .optional()?;
        if let Some(id) = pending {
            results.push(json!({"email": email, "status": "already-invited", "invitationId": id, "acceptable": acceptable}));
            continue;
        }
        let (user_id, created_user) = match pending_account(tx, &email)? {
            Ok(account) => account,
            Err(status) => {
                results.push(json!({"email": email, "status": status}));
                continue;
            }
        };
        let challenge = auth.issue_challenge(tx, &user_id, &email, Purpose::Invitation)?;
        let invitation = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO app_invitations (id, email, deployment, inviter, user_id, created_user, state, token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9)",
            params![invitation, email, deployment.id, inviter, user_id, created_user, auth::hash_token(&challenge.token), now, challenge.expires_at],
        )?;
        results.push(json!({"email": email, "status": "invited", "invitationId": invitation, "expiresAt": challenge.expires_at, "acceptable": acceptable, "delivery": "not-sent"}));
        issued.push((invitation, challenge));
    }
    let response = json!({"deploymentId": deployment.id, "policy": deployment.signup_policy.mode, "results": results});
    remember(tx, inviter, key, &request, &response)?;
    Ok(Prepared { response, issued, request })
}

fn resend(
    tx: &rusqlite::Transaction<'_>,
    auth: &AuthService,
    deployment: &Deployment,
    inviter: &str,
    key: &str,
    invitation: &str,
) -> Result<Prepared, Refusal> {
    let request = format!("resend:{invitation}");
    if let Some(response) = replay(tx, inviter, key, &request)? {
        return Ok(Prepared { response, issued: Vec::new(), request });
    }
    let row: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT email, state, token_hash FROM app_invitations WHERE id = ?1",
            [invitation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((email, state, old_hash)) = row else {
        return Err(Refusal::Said("No invitation with that ID on this deployment."));
    };
    if state != "pending" {
        return Err(Refusal::Said("Only a pending invitation can be sent again."));
    }
    let (user_id, created_user) = match pending_account(tx, &email)? {
        Ok(account) => account,
        Err("already-member") => return Err(Refusal::Said("This email already has an active account. Ask the person to sign in.")),
        Err(_) => return Err(Refusal::Said("This account is suspended. Resume it before inviting it again.")),
    };
    // The new challenge replaces the account's open invitation challenge;
    // deleting the recorded one as well covers an account that changed.
    if let Some(old_hash) = old_hash {
        tx.execute("DELETE FROM account_challenges WHERE token_hash = ?1", [&old_hash])?;
    }
    let challenge = auth.issue_challenge(tx, &user_id, &email, Purpose::Invitation)?;
    tx.execute(
        "UPDATE app_invitations SET token_hash = ?1, expires_at = ?2, deployment = ?3, user_id = ?4,
           created_user = MAX(created_user, ?5), delivery = 'not-sent' WHERE id = ?6",
        params![auth::hash_token(&challenge.token), challenge.expires_at, deployment.id, user_id, created_user, invitation],
    )?;
    let response = json!({"invitationId": invitation, "email": email, "status": "invited", "expiresAt": challenge.expires_at, "delivery": "not-sent"});
    remember(tx, inviter, key, &request, &response)?;
    Ok(Prepared { response, issued: vec![(invitation.to_owned(), challenge)], request })
}

fn revoke(tx: &rusqlite::Transaction<'_>, invitation: &str) -> Result<Value, Refusal> {
    let row: Option<(String, String, Option<String>, Option<String>, bool)> = tx
        .query_row(
            "SELECT email, state, token_hash, user_id, created_user FROM app_invitations WHERE id = ?1",
            [invitation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?;
    let Some((email, state, token_hash, user_id, created_user)) = row else {
        return Err(Refusal::Said("No invitation with that ID on this deployment."));
    };
    let revoked = json!({"invitationId": invitation, "email": email, "status": "revoked"});
    match state.as_str() {
        "revoked" => return Ok(revoked),
        "accepted" => return Err(Refusal::Said("This invitation was already accepted. Suspend the account instead.")),
        _ => {}
    }
    if let Some(token_hash) = token_hash {
        tx.execute("DELETE FROM account_challenges WHERE token_hash = ?1", [&token_hash])?;
    }
    tx.execute(
        "UPDATE app_invitations SET state = 'revoked', revoked_at = ?1, token_hash = NULL WHERE id = ?2",
        params![auth::now_seconds(), invitation],
    )?;
    // An account the invitation created, and nobody used, goes with it,
    // unless a signup link is also open for it.
    if let (Some(user_id), true) = (user_id, created_user) {
        tx.execute(
            "DELETE FROM app_users WHERE id = ?1 AND state = 'pending' AND verified_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM account_challenges WHERE user_id = ?1 AND consumed_at IS NULL)",
            [&user_id],
        )?;
    }
    Ok(revoked)
}

fn set_suspended(auth: &AuthService, user_id: &str, suspend: bool) -> Result<Value> {
    let email: Option<String> = auth.read(|db| {
        db.query_row("SELECT email FROM app_users WHERE id = ?1", [user_id], |row| row.get(0))
            .optional()
            .map_err(storage)
    })?;
    let Some(email) = email else {
        anyhow::bail!("No account with that ID on this deployment.");
    };
    let state = if suspend {
        auth.suspend(&email)?;
        "suspended".to_owned()
    } else {
        match auth.resume(&email) {
            Ok((_, state)) => state,
            Err(AuthError::InvalidCredentials) => anyhow::bail!("This account is not suspended."),
            Err(other) => return Err(other.into()),
        }
    };
    Ok(json!({"userId": user_id, "email": email, "state": state}))
}

fn list_users(db: &Connection, state: Option<&str>, cursor: Option<&str>, limit: usize) -> Result<Value, AuthError> {
    let mut query = db
        .prepare(
            "SELECT id, email, state, verified_at, created_at FROM app_users
             WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR email > ?2)
             ORDER BY email LIMIT ?3",
        )
        .map_err(storage)?;
    let mut users = query
        .query_map(params![state, cursor, limit as i64 + 1], |row| {
            let verified_at: Option<i64> = row.get(3)?;
            Ok(json!({
                "userId": row.get::<_, String>(0)?,
                "email": row.get::<_, String>(1)?,
                "state": row.get::<_, String>(2)?,
                "verified": verified_at.is_some(),
                "verifiedAt": verified_at,
                "createdAt": row.get::<_, i64>(4)?,
            }))
        })
        .map_err(storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)?;
    let next = (users.len() > limit).then(|| {
        users.truncate(limit);
        users.last().map(|user| user["email"].clone())
    });
    Ok(json!({"users": users, "nextCursor": next.flatten()}))
}

/// Lists invitations newest first. A pending row whose challenge is gone was
/// carried by a copy or ended by a suspension, and needs a resend; one whose
/// address has an active account needs nothing.
fn list_invitations(db: &Connection, cursor: Option<&str>, limit: usize) -> Result<Value, AuthError> {
    let (before_at, before_id) = cursor
        .and_then(|value| value.split_once(':'))
        .and_then(|(at, id)| at.parse::<i64>().ok().map(|at| (at, id.to_owned())))
        .unwrap_or((i64::MAX, String::new()));
    let now = auth::now_seconds();
    let mut query = db
        .prepare(
            "SELECT i.id, i.email, i.state, i.inviter, i.created_at, i.expires_at, i.accepted_at, i.revoked_at,
                    i.delivery, i.sends, c.token_hash IS NOT NULL, c.expires_at, u.state = 'active'
             FROM app_invitations i
             LEFT JOIN account_challenges c ON c.token_hash = i.token_hash AND c.consumed_at IS NULL
             LEFT JOIN app_users u ON u.email = i.email
             WHERE i.created_at < ?1 OR (i.created_at = ?1 AND i.id < ?2) OR ?2 = ''
             ORDER BY i.created_at DESC, i.id DESC LIMIT ?3",
        )
        .map_err(storage)?;
    let mut rows = query
        .query_map(params![before_at, before_id, limit as i64 + 1], |row| {
            let state: String = row.get(2)?;
            let expires_at: i64 = row.get(5)?;
            let live: bool = row.get(10)?;
            let challenge_expires: Option<i64> = row.get(11)?;
            let member: Option<bool> = row.get(12)?;
            let status = match state.as_str() {
                // The address became an active account another way, such as
                // signing up and verifying while the app was Open.
                "pending" if member == Some(true) => "already-member",
                "pending" if !live => "needs-resend",
                "pending" if challenge_expires.unwrap_or(expires_at) <= now => "expired",
                other => other,
            }
            .to_owned();
            let created_at: i64 = row.get(4)?;
            let id: String = row.get(0)?;
            Ok((
                format!("{created_at}:{id}"),
                json!({
                    "invitationId": id,
                    "email": row.get::<_, String>(1)?,
                    "status": status,
                    "invitedBy": row.get::<_, String>(3)?,
                    "createdAt": created_at,
                    "expiresAt": expires_at,
                    "acceptedAt": row.get::<_, Option<i64>>(6)?,
                    "revokedAt": row.get::<_, Option<i64>>(7)?,
                    "delivery": row.get::<_, String>(8)?,
                    "sends": row.get::<_, i64>(9)?,
                }),
            ))
        })
        .map_err(storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)?;
    let next = (rows.len() > limit).then(|| {
        rows.truncate(limit);
        rows.last().map(|(cursor, _)| cursor.clone())
    });
    Ok(json!({"invitations": rows.into_iter().map(|(_, row)| row).collect::<Vec<_>>(), "nextCursor": next.flatten()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use accounts::{AccountMail, AccountSettings, MemoryMailbox};
    use auth::Verification;

    const APP_HOST: &str = "invite-test.localhost";
    const OTHER_HOST: &str = "invite-other.localhost";
    const OWNER: &str = "owner-1";
    const PASSWORD: &str = "correct horse battery";

    struct Fixture {
        state: AppState,
        mailbox: Arc<MemoryMailbox>,
        root: PathBuf,
        inviter_email: Option<String>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(mode: SignupMode) -> Fixture {
        let root = std::env::temp_dir().join(format!("konae-invitations-{}", Uuid::new_v4()));
        let mut deployments = HashMap::new();
        for host in [APP_HOST, OTHER_HOST] {
            let app_id = Uuid::new_v4().to_string();
            let item = Deployment {
                id: app_id.clone(),
                project_id: app_id,
                project_name: "Invitation test".into(),
                slug: "app".into(),
                url: format!("http://{host}"),
                created_at: now_ms(),
                manifest: json!({"visibility": "private"}),
                hostname: host.into(),
                actions: "{}".into(),
                artifact_version: "v1".into(),
                signup_policy: SignupPolicy::from_input(mode, &[]).unwrap(),
                sign_in_methods: default_sign_in_methods(),
                contact_email: None,
            };
            std::fs::create_dir_all(crate::layout::data(&root, &item.id)).unwrap();
            deployments.insert(host.to_owned(), item);
        }
        let mailbox = Arc::new(MemoryMailbox::default());
        let state = AppState {
            deployments: Arc::new(RwLock::new(deployments)),
            accounts: AccountSettings {
                verification: Verification::Required,
                mail: Some(mailbox.clone() as Arc<dyn AccountMail>),
                queue: None,
                google: Arc::new(crate::google::Google::real()),
            },
            ..AppState::for_test(&root)
        };
        Fixture { state, mailbox, root, inviter_email: None }
    }

    impl Fixture {
        async fn app(&self, host: &str) -> Deployment {
            self.state.deployments.read().await.get(host).cloned().unwrap()
        }

        async fn call(&self, name: &str, arguments: Value) -> Result<Value> {
            let app = self.app(APP_HOST).await;
            run(&self.state, &app, &Inviter { id: OWNER.into(), email: self.inviter_email.clone() }, name, &arguments).await
        }

        async fn invite(&self, emails: &[&str], key: &str) -> Value {
            self.call("app_invitations_create", json!({"deploymentId": "unused", "emails": emails, "idempotencyKey": key}))
                .await
                .unwrap()
        }

        fn sent(&self) -> usize {
            self.mailbox.0.lock().unwrap().len()
        }

        /// The token in the newest message, which must be an invitation.
        fn last_token(&self) -> String {
            let messages = self.mailbox.0.lock().unwrap();
            let message = messages.last().expect("a message was sent");
            assert_eq!(message.purpose, Purpose::Invitation);
            message.link.split_once("#invitation=").unwrap().1.to_owned()
        }

        async fn set_policy(&self, mode: SignupMode, allowlist: &[&str]) {
            let entries: Vec<String> = allowlist.iter().map(|entry| entry.to_string()).collect();
            for item in self.state.deployments.write().await.values_mut() {
                item.signup_policy = SignupPolicy::from_input(mode, &entries).unwrap();
            }
        }

        async fn db(&self) -> Connection {
            let app = self.app(APP_HOST).await;
            store(&self.state, &app).unwrap();
            Connection::open(crate::layout::identity(&self.root, &app.id)).unwrap()
        }

        async fn accounts_named(&self, email: &str) -> Vec<(String, String)> {
            let db = self.db().await;
            let mut query = db.prepare("SELECT id, state FROM app_users WHERE email = ?1").unwrap();
            query.query_map([email], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(Result::unwrap).collect()
        }

        async fn answer(&self, host: &str, token: &str, password: &str) -> Response {
            let body = json!({"token": token, "password": password}).to_string();
            let request = Request::builder()
                .method("POST")
                .uri("/_konae/auth/accept")
                .header(header::HOST, host)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            accept_route(State(self.state.clone()), request).await
        }

        async fn login(&self, email: &str, password: &str) -> StatusCode {
            let body = json!({"email": email, "password": password}).to_string();
            let request = Request::builder()
                .method("POST")
                .uri("/_konae/auth/login")
                .header(header::HOST, APP_HOST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_static(APP_HOST));
            super::super::login(State(self.state.clone()), headers, request).await.status()
        }

        async fn signup(&self, email: &str) -> StatusCode {
            let body = json!({"email": email}).to_string();
            let request = Request::builder()
                .method("POST")
                .uri("/_konae/auth/signup")
                .header(header::HOST, APP_HOST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_static(APP_HOST));
            super::super::signup(State(self.state.clone()), headers, request).await.status()
        }
    }

    fn has_cookie(response: &Response) -> bool {
        response.headers().get(header::SET_COOKIE).is_some()
    }

    /// Records what the queue's worker sends.
    #[derive(Default)]
    struct Recorded(std::sync::Mutex<Vec<mail::OutgoingEmail>>);

    impl mail::Transport for Recorded {
        fn name(&self) -> &'static str {
            "recorded"
        }
        fn send<'a>(&'a self, email: &'a mail::OutgoingEmail) -> std::pin::Pin<Box<dyn std::future::Future<Output = mail::SendOutcome> + Send + 'a>> {
            self.0.lock().unwrap().push(email.clone());
            Box::pin(async { mail::SendOutcome::Accepted { provider_id: None, delivered: false } })
        }
        fn preflight<'a>(&'a self, _sender: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = mail::PreflightResult> + Send + 'a>> {
            Box::pin(async { mail::PreflightResult::Ready })
        }
    }

    #[tokio::test]
    async fn a_queued_invitation_names_the_inviting_owner_escaped() {
        let mut fixture = fixture(SignupMode::Invitation);
        let transport = Arc::new(Recorded::default());
        let queue = Arc::new(
            mail::MailQueue::open(&fixture.root, transport.clone(), "Konae <no-reply@mail.example.test>",
                "a mail secret that is long enough for tests", mail::Limits::default(), std::time::Duration::from_millis(50))
            .unwrap(),
        );
        queue.run_preflight().await;
        fixture.state.accounts = AccountSettings { verification: Verification::Required, mail: None, queue: Some(queue.clone()), google: Arc::new(crate::google::Google::real()) };
        fixture.inviter_email = Some("o'neil&co@example.test".into());

        let answer = fixture.invite(&["ana@example.test"], "inviter-key-1").await;
        assert_eq!(answer["results"][0]["delivery"], "queued");
        let stored: Option<String> = Connection::open(fixture.root.join(mail::MAIL_DATABASE))
            .unwrap()
            .query_row("SELECT inviter FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored.as_deref(), Some("o'neil&co@example.test"), "the job carries the inviter");
        assert_eq!(queue.run_once().await.unwrap(), 1);
        let sent = transport.0.lock().unwrap().clone();
        assert_eq!(sent[0].subject, "o'neil&co@example.test invited you to Invitation test");
        assert!(sent[0].text.starts_with("o'neil&co@example.test invited you to join Invitation test with this email address."));
        assert!(sent[0].html.contains("<p>o&#39;neil&amp;co@example.test invited you to join Invitation test with this email address.</p>"));

        // An inviter with no email, as a Clerk owner, is not named.
        let app = fixture.app(APP_HOST).await;
        run(&fixture.state, &app, &Inviter { id: "clerk-owner".into(), email: None }, "app_invitations_create",
            &json!({"emails": ["ben@example.test"], "idempotencyKey": "inviter-key-2"})).await.unwrap();
        assert_eq!(queue.run_once().await.unwrap(), 1);
        assert_eq!(transport.0.lock().unwrap()[1].subject, "You are invited to Invitation test");
    }

    #[tokio::test]
    async fn an_invited_person_sets_a_password_and_enters_and_an_uninvited_one_cannot_sign_up() {
        let fixture = fixture(SignupMode::Invitation);
        assert_eq!(fixture.signup("stranger@example.test").await, StatusCode::FORBIDDEN);
        assert!(fixture.accounts_named("stranger@example.test").await.is_empty());
        assert_eq!(fixture.sent(), 0);

        let created = fixture.invite(&["Ana@Example.test"], "first-batch").await;
        let result = &created["results"][0];
        assert_eq!(result["email"], "ana@example.test");
        assert_eq!((result["status"].as_str(), result["delivery"].as_str()), (Some("invited"), Some("queued")));
        assert_eq!(created["policy"], "invitation");
        let link = fixture.mailbox.0.lock().unwrap()[0].clone();
        assert!(link.link.starts_with(&format!("http://{APP_HOST}/_konae/invitation#invitation=")));
        assert!(!created.to_string().contains(&fixture.last_token()), "no tool returns a token");
        assert_eq!(fixture.accounts_named("ana@example.test").await[0].1, "pending");
        assert_eq!(fixture.signup("ana@example.test").await, StatusCode::FORBIDDEN, "an invitation is not a signup");

        // Opening the link with GET returns the page and changes nothing; the
        // page learns the email from a read that changes nothing either.
        let page = invitation_page(State(fixture.state.clone()), {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_static(APP_HOST));
            headers
        })
        .await;
        assert_eq!(page.status(), StatusCode::OK);
        let token = fixture.last_token();
        let app = fixture.app(APP_HOST).await;
        let shown = inspect(&fixture.state, &app, &token).await.unwrap();
        assert_eq!(shown, json!({"email": "ana@example.test", "app": "Invitation test", "status": "new-account"}));
        assert_eq!(fixture.accounts_named("ana@example.test").await[0].1, "pending");

        let accepted = fixture.answer(APP_HOST, &token, PASSWORD).await;
        assert_eq!(accepted.status(), StatusCode::OK);
        assert!(has_cookie(&accepted));
        assert_eq!(fixture.accounts_named("ana@example.test").await[0].1, "active");
        assert_eq!(fixture.login("ana@example.test", PASSWORD).await, StatusCode::OK);
        assert_eq!(fixture.answer(APP_HOST, &token, "another password").await.status(), StatusCode::GONE, "a link works once");

        let listed = fixture.call("app_invitations_list", json!({})).await.unwrap();
        assert_eq!(listed["invitations"][0]["status"], "accepted");
        let users = fixture.call("app_users_list", json!({})).await.unwrap();
        assert_eq!(users["users"][0]["verified"], true);
        assert_eq!(users["users"][0]["state"], "active");
    }

    #[tokio::test]
    async fn acceptance_rechecks_the_policy_and_a_refusal_keeps_the_link() {
        let fixture = fixture(SignupMode::Invitation);
        fixture.invite(&["ana@example.test"], "policy-batch").await;
        let token = fixture.last_token();
        fixture.set_policy(SignupMode::Allowlist, &["@other.test"]).await;
        let refused = fixture.answer(APP_HOST, &token, PASSWORD).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "an Allowlist must still list the email");
        assert!(!has_cookie(&refused));
        assert_eq!(fixture.accounts_named("ana@example.test").await[0].1, "pending");
        fixture.set_policy(SignupMode::Allowlist, &["@example.test"]).await;
        assert_eq!(fixture.answer(APP_HOST, &token, PASSWORD).await.status(), StatusCode::OK, "the refused link still works");
    }

    /// The owner chose the invited person, so an invitation is acceptable
    /// under Closed (ADR-0003), from the answer
    /// to `app_invitations_create` to the acceptance. A signup and a
    /// verification stay refused.
    #[tokio::test]
    async fn closed_refuses_a_signup_and_accepts_an_owner_invitation() {
        let fixture = fixture(SignupMode::Closed);
        assert_eq!(fixture.signup("stranger@example.test").await, StatusCode::FORBIDDEN);
        assert!(fixture.accounts_named("stranger@example.test").await.is_empty());

        let created = fixture.invite(&["ana@example.test"], "closed-batch").await;
        assert_eq!(created["policy"], "closed");
        assert_eq!(created["results"][0]["acceptable"], true);
        assert_eq!(fixture.signup("ana@example.test").await, StatusCode::FORBIDDEN, "an invitation is not a signup");
        let token = fixture.last_token();
        let app = fixture.app(APP_HOST).await;
        assert_eq!(inspect(&fixture.state, &app, &token).await.unwrap()["status"], "new-account");

        let accepted = fixture.answer(APP_HOST, &token, PASSWORD).await;
        assert_eq!(accepted.status(), StatusCode::OK);
        assert!(has_cookie(&accepted));
        assert_eq!(fixture.accounts_named("ana@example.test").await[0].1, "active");
        assert_eq!(fixture.login("ana@example.test", PASSWORD).await, StatusCode::OK);
        assert_eq!(fixture.signup("another@example.test").await, StatusCode::FORBIDDEN, "Closed still refuses everyone else");
    }

    #[tokio::test]
    async fn a_suspended_account_is_not_reactivated_by_an_invitation() {
        let fixture = fixture(SignupMode::Open);
        // Suspended before the invitation: nothing is issued.
        fixture.invite(&["held@example.test"], "first-held").await;
        let token = fixture.last_token();
        assert_eq!(fixture.answer(APP_HOST, &token, PASSWORD).await.status(), StatusCode::OK);
        let user_id = fixture.accounts_named("held@example.test").await[0].0.clone();
        fixture.call("app_user_suspend", json!({"userId": user_id})).await.unwrap();
        let sent = fixture.sent();
        let again = fixture.invite(&["held@example.test"], "second-held").await;
        assert_eq!(again["results"][0]["status"], "suspended");
        assert_eq!(fixture.sent(), sent);

        // Suspended while the invitation is open: the suspension ends the link.
        fixture.invite(&["late@example.test"], "late-batch").await;
        let token = fixture.last_token();
        let late_id = fixture.accounts_named("late@example.test").await[0].0.clone();
        fixture.call("app_user_suspend", json!({"userId": late_id})).await.unwrap();
        let refused = fixture.answer(APP_HOST, &token, PASSWORD).await;
        assert!(!has_cookie(&refused));
        assert_eq!(fixture.accounts_named("late@example.test").await[0].1, "suspended");

        // Suspended inside the window between issue and answer, with the link
        // still stored: the transaction reads the state and refuses.
        fixture.invite(&["race@example.test"], "race-batch").await;
        let token = fixture.last_token();
        fixture.db().await.execute("UPDATE app_users SET state = 'suspended' WHERE email = 'race@example.test'", []).unwrap();
        let refused = fixture.answer(APP_HOST, &token, PASSWORD).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert!(!has_cookie(&refused));
        assert_eq!(fixture.accounts_named("race@example.test").await[0].1, "suspended");
    }

    #[tokio::test]
    async fn two_concurrent_acceptances_produce_one_account() {
        let fixture = fixture(SignupMode::Invitation);
        fixture.invite(&["twice@example.test"], "twice-batch").await;
        let token = fixture.last_token();
        let (first, second) = tokio::join!(fixture.answer(APP_HOST, &token, PASSWORD), fixture.answer(APP_HOST, &token, "other password"));
        let mut statuses = [first.status(), second.status()];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::OK, StatusCode::GONE]);
        assert_eq!(fixture.accounts_named("twice@example.test").await.len(), 1);

        // Without the Kernel's identity lock, the store's own transaction
        // still lets only one answer through.
        fixture.invite(&["threads@example.test"], "threads-batch").await;
        let token = fixture.last_token();
        let app = fixture.app(APP_HOST).await;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (auth, token, barrier) = (store(&fixture.state, &app).unwrap(), token.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let chosen = NewPassword::prepare(PASSWORD).unwrap();
                    barrier.wait();
                    accept_in(&auth, &token, chosen, |_| true).is_ok()
                })
            })
            .collect();
        let accepted = handles.into_iter().map(|handle| handle.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(accepted, 1);
        assert_eq!(fixture.accounts_named("threads@example.test").await.len(), 1);
        let consumed: i64 = fixture.db().await
            .query_row("SELECT COUNT(*) FROM account_challenges WHERE purpose = 'invitation' AND consumed_at IS NOT NULL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(consumed, 2, "one consumption per invitation");
    }

    #[tokio::test]
    async fn a_token_from_another_deployment_an_expired_one_or_a_replaced_one_does_nothing() {
        let fixture = fixture(SignupMode::Invitation);
        fixture.invite(&["ana@example.test"], "scope-batch").await;
        let first = fixture.last_token();
        assert_eq!(fixture.answer(OTHER_HOST, &first, PASSWORD).await.status(), StatusCode::GONE, "another Deployment");

        let listed = fixture.call("app_invitations_list", json!({})).await.unwrap();
        let invitation = listed["invitations"][0]["invitationId"].as_str().unwrap().to_owned();
        let resent = fixture.call("app_invitation_resend", json!({"invitationId": invitation, "idempotencyKey": "resend-one"})).await.unwrap();
        assert_eq!(resent["delivery"], "queued");
        let second = fixture.last_token();
        assert_ne!(first, second);
        assert_eq!(fixture.answer(APP_HOST, &first, PASSWORD).await.status(), StatusCode::GONE, "resend invalidates the earlier link");

        fixture.db().await.execute("UPDATE account_challenges SET expires_at = 1 WHERE purpose = 'invitation'", []).unwrap();
        assert_eq!(fixture.answer(APP_HOST, &second, PASSWORD).await.status(), StatusCode::GONE, "expired");
        let listed = fixture.call("app_invitations_list", json!({})).await.unwrap();
        assert_eq!(listed["invitations"][0]["status"], "expired");

        fixture.call("app_invitation_resend", json!({"invitationId": invitation, "idempotencyKey": "resend-two"})).await.unwrap();
        assert_eq!(fixture.answer(APP_HOST, &fixture.last_token(), PASSWORD).await.status(), StatusCode::OK);
        assert!(fixture.call("app_invitation_resend", json!({"invitationId": invitation, "idempotencyKey": "resend-three"})).await.is_err());
    }

    #[tokio::test]
    async fn a_retried_create_with_the_same_key_produces_one_invitation_and_one_email() {
        let fixture = fixture(SignupMode::Invitation);
        let first = fixture.invite(&["ana@example.test", "ANA@example.test", "not an email", "bo@example.test"], "retry-key-1").await;
        let statuses: Vec<_> = first["results"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap().to_owned()).collect();
        assert_eq!(statuses, ["invited", "invalid-email", "invited"]);
        assert_eq!(fixture.sent(), 2);
        let retried = fixture.invite(&["ana@example.test", "ANA@example.test", "not an email", "bo@example.test"], "retry-key-1").await;
        assert_eq!(retried, first, "the retry returns the first answer");
        assert_eq!(fixture.sent(), 2, "and sends nothing");
        let rows: i64 = fixture.db().await.query_row("SELECT COUNT(*) FROM app_invitations", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 2);
        let reused = fixture
            .call("app_invitations_create", json!({"emails": ["cy@example.test"], "idempotencyKey": "retry-key-1"}))
            .await;
        assert!(reused.unwrap_err().to_string().contains("different request"));
        // A new key for a pending recipient sends nothing new either.
        let again = fixture.invite(&["ana@example.test"], "retry-key-2").await;
        assert_eq!(again["results"][0]["status"], "already-invited");
        assert_eq!(fixture.sent(), 2);
    }

    #[tokio::test]
    async fn an_active_account_is_told_to_sign_in_and_keeps_its_password() {
        let fixture = fixture(SignupMode::Open);
        fixture.invite(&["ana@example.test"], "member-batch").await;
        assert_eq!(fixture.answer(APP_HOST, &fixture.last_token(), PASSWORD).await.status(), StatusCode::OK);
        let again = fixture.invite(&["ana@example.test"], "member-again").await;
        assert_eq!(again["results"][0]["status"], "already-member");

        // An account that became active while its invitation was open.
        fixture.invite(&["bo@example.test"], "bo-batch").await;
        let token = fixture.last_token();
        fixture.db().await.execute("UPDATE app_users SET state = 'active' WHERE email = 'bo@example.test'", []).unwrap();
        let app = fixture.app(APP_HOST).await;
        assert_eq!(inspect(&fixture.state, &app, &token).await.unwrap()["status"], "existing-account");
        assert_eq!(fixture.answer(APP_HOST, &token, PASSWORD).await.status(), StatusCode::CONFLICT);
        let listed = fixture.call("app_invitations_list", json!({})).await.unwrap();
        assert_eq!(listed["invitations"][0]["status"], "already-member", "not needs-resend for someone who already entered");
        let hash: String = fixture.db().await
            .query_row("SELECT password_hash FROM app_users WHERE email = 'bo@example.test'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(hash, auth::NO_PASSWORD, "the invitation did not set a password");
    }

    #[tokio::test]
    async fn revoking_ends_the_link_and_removes_the_unused_account() {
        let fixture = fixture(SignupMode::Invitation);
        let created = fixture.invite(&["ana@example.test"], "revoke-batch").await;
        let invitation = created["results"][0]["invitationId"].as_str().unwrap().to_owned();
        let token = fixture.last_token();
        let revoked = fixture.call("app_invitation_revoke", json!({"invitationId": invitation})).await.unwrap();
        assert_eq!(revoked["status"], "revoked");
        assert_eq!(fixture.answer(APP_HOST, &token, PASSWORD).await.status(), StatusCode::GONE);
        assert!(fixture.accounts_named("ana@example.test").await.is_empty());
        assert!(fixture.call("app_invitation_resend", json!({"invitationId": invitation, "idempotencyKey": "after-revoke"})).await.is_err());
        // The address can be invited again.
        assert_eq!(fixture.invite(&["ana@example.test"], "revoke-again").await["results"][0]["status"], "invited");
    }

    #[tokio::test]
    async fn without_mail_nothing_is_created() {
        let mut fixture = fixture(SignupMode::Invitation);
        fixture.state.accounts.mail = None;
        let refused = fixture.call("app_invitations_create", json!({"emails": ["ana@example.test"], "idempotencyKey": "no-mail-1"})).await;
        assert!(refused.unwrap_err().to_string().contains("Email is not configured"));
        assert!(fixture.accounts_named("ana@example.test").await.is_empty());
        let too_many: Vec<String> = (0..=MAX_BATCH).map(|n| format!("p{n}@example.test")).collect();
        fixture.state.accounts.mail = Some(fixture.mailbox.clone() as Arc<dyn AccountMail>);
        let refused = fixture.call("app_invitations_create", json!({"emails": too_many, "idempotencyKey": "too-many-1"})).await;
        assert!(refused.is_err());
        assert_eq!(fixture.sent(), 0);
    }

    #[tokio::test]
    async fn a_copy_carries_recipients_and_history_but_no_working_link() {
        let fixture = fixture(SignupMode::Invitation);
        fixture.invite(&["ana@example.test", "bo@example.test"], "copy-batch").await;
        let first = fixture.mailbox.0.lock().unwrap()[0].link.split_once("#invitation=").unwrap().1.to_owned();
        let pending = fixture.last_token();
        assert_eq!(fixture.answer(APP_HOST, &first, PASSWORD).await.status(), StatusCode::OK);

        // Export the accounts as the export does, then import them into a new Deployment.
        let app = fixture.app(APP_HOST).await;
        let carried = fixture.root.join("carried.sqlite");
        crate::copies::vacuum_into(&crate::layout::identity(&fixture.root, &app.id), &carried).unwrap();
        crate::package::revoke_access(&carried).unwrap();
        let copy = Connection::open(&carried).unwrap();
        let count = |sql: &str| copy.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM app_invitations"), 2, "the history travels");
        assert_eq!(count("SELECT COUNT(*) FROM app_invitations WHERE token_hash IS NOT NULL"), 0, "no token hash travels");
        assert_eq!(count("SELECT COUNT(*) FROM account_challenges"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM app_invitation_requests"), 0);
        drop(copy);

        let imported_id = Uuid::new_v4().to_string();
        let directory = crate::layout::data(&fixture.root, &imported_id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::copy(&carried, crate::layout::identity(&fixture.root, &imported_id)).unwrap();
        let imported = Deployment { id: imported_id, hostname: "invite-imported.localhost".into(), url: "http://invite-imported.localhost".into(), ..app.clone() };
        fixture.state.deployments.write().await.insert(imported.hostname.clone(), imported.clone());
        let answer = |token: String| {
            let state = fixture.state.clone();
            let imported = imported.clone();
            async move { accept(&state, &imported, &token, PASSWORD).await }
        };
        assert!(matches!(answer(pending.clone()).await, Err(AuthError::InvalidChallenge)), "the old link fails at the destination");
        let listed = run(&fixture.state, &imported, &Inviter { id: OWNER.into(), email: None }, "app_invitations_list", &json!({"limit": 1})).await.unwrap();
        let rest = run(&fixture.state, &imported, &Inviter { id: OWNER.into(), email: None }, "app_invitations_list", &json!({"cursor": listed["nextCursor"]})).await.unwrap();
        let mut statuses: Vec<(String, String)> = [&listed, &rest]
            .iter()
            .flat_map(|page| page["invitations"].as_array().unwrap().clone())
            .map(|row| (row["email"].as_str().unwrap().to_owned(), row["status"].as_str().unwrap().to_owned()))
            .collect();
        statuses.sort();
        assert_eq!(statuses, [("ana@example.test".into(), "accepted".into()), ("bo@example.test".into(), "needs-resend".into())]);
        assert_eq!(fixture.login("ana@example.test", PASSWORD).await, StatusCode::OK, "the accepted account keeps its password at the origin");

        // The destination resends with its own link and the recipient enters.
        let bo = rest["invitations"].as_array().unwrap().iter().chain(listed["invitations"].as_array().unwrap())
            .find(|row| row["email"] == "bo@example.test").unwrap()["invitationId"].as_str().unwrap().to_owned();
        let resent = run(&fixture.state, &imported, &Inviter { id: OWNER.into(), email: None }, "app_invitation_resend", &json!({"invitationId": bo, "idempotencyKey": "copy-resend"})).await.unwrap();
        assert_eq!(resent["delivery"], "queued");
        let link = fixture.mailbox.0.lock().unwrap().last().unwrap().link.clone();
        assert!(link.starts_with("http://invite-imported.localhost/_konae/invitation#invitation="));
        assert!(answer(fixture.last_token()).await.is_ok());
    }

    #[tokio::test]
    async fn users_list_pages_and_suspension_round_trips() {
        let fixture = fixture(SignupMode::Open);
        fixture.invite(&["a@example.test", "b@example.test", "c@example.test"], "users-batch").await;
        let first = fixture.call("app_users_list", json!({"limit": 2})).await.unwrap();
        assert_eq!(first["users"].as_array().unwrap().len(), 2);
        let rest = fixture.call("app_users_list", json!({"cursor": first["nextCursor"]})).await.unwrap();
        assert_eq!(rest["users"][0]["email"], "c@example.test");
        assert!(rest["nextCursor"].is_null());
        let pending = fixture.call("app_users_list", json!({"state": "pending"})).await.unwrap();
        assert_eq!(pending["users"].as_array().unwrap().len(), 3);
        let user = first["users"][0]["userId"].as_str().unwrap().to_owned();
        assert_eq!(fixture.call("app_user_suspend", json!({"userId": user})).await.unwrap()["state"], "suspended");
        assert_eq!(fixture.call("app_user_resume", json!({"userId": user})).await.unwrap()["state"], "pending", "never verified");
        assert!(fixture.call("app_user_resume", json!({"userId": user})).await.is_err());
        assert!(fixture.call("app_user_suspend", json!({"userId": "nobody"})).await.is_err());
    }
}
