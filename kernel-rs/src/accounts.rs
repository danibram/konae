//! The account flows that run on top of `auth.rs` for both scopes: signup
//! that waits for verification, recovery, the pages a challenge link opens,
//! the mail seam that carries links, and the operator's suspension command.
//!
//! A challenge link opens a page with GET, which reads nothing from the
//! token and changes nothing. The page posts the token and the new password,
//! and only that POST consumes the token.

use super::*;
use auth::{
    AuthSession, CREATOR_SCOPE, IssuedChallenge, NewPassword, Purpose, Verification,
    deployment_scope,
};

/// How this installation treats new accounts and how it sends their links.
#[derive(Clone)]
pub(crate) struct AccountSettings {
    pub(crate) verification: Verification,
    /// A transport called directly after the challenge commits. Tests use it;
    /// an installation uses `queue`.
    pub(crate) mail: Option<Arc<dyn AccountMail>>,
    /// The persistent queue in `mail.sqlite` (`mail.rs`). When set, every
    /// account email goes through it.
    pub(crate) queue: Option<Arc<mail::MailQueue>>,
    /// Where the Kernel reaches Google. Each app sets its own client.
    pub(crate) google: Arc<google::Google>,
}

impl AccountSettings {
    /// Local development: unverified signup and no mail.
    #[cfg(test)]
    pub(crate) fn local() -> Self {
        Self {
            verification: Verification::Local,
            mail: None,
            queue: None,
            google: Arc::new(google::Google::real()),
        }
    }

    /// Reads `KONAE_ACCOUNT_VERIFICATION` and the mail transport
    /// (`mail::MailQueue::from_env`). Every transport, including
    /// `development`, sends through the queue.
    pub(crate) fn from_env(data_root: &Path, admin_key: &str, public_origin: &str) -> Result<Self> {
        let verification = Verification::from_env().map_err(anyhow::Error::msg)?;
        let queue = mail::MailQueue::from_env(data_root, admin_key)?;
        let google = Arc::new(google::Google::from_env(public_origin)?);
        Ok(Self { verification, mail: None, queue, google })
    }
}

/// One account email: a challenge link for one recipient. The link carries
/// the token in its fragment, so it never reaches a server log or a Referer.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountMessage {
    pub(crate) to: String,
    pub(crate) purpose: Purpose,
    pub(crate) scope: String,
    pub(crate) link: String,
    pub(crate) expires_at: i64,
}

/// Where account emails go. A transport receives each message once, after
/// the challenge is stored; a persistent queue can implement this.
pub(crate) trait AccountMail: Send + Sync {
    fn deliver(&self, message: &AccountMessage) -> Result<()>;
}

/// Writes each message as one JSON line to a file the operator reads. For
/// development and tests only: the file holds live links.
pub(crate) struct DevelopmentMailbox {
    path: PathBuf,
    lock: std::sync::Mutex<()>,
}

impl DevelopmentMailbox {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: std::sync::Mutex::new(()),
        }
    }
}

impl AccountMail for DevelopmentMailbox {
    fn deliver(&self, message: &AccountMessage) -> Result<()> {
        self.append(message)
    }
}

impl DevelopmentMailbox {
    /// Appends one JSON line, an account message or a notice.
    pub(crate) fn append(&self, message: &impl Serialize) -> Result<()> {
        use std::io::Write;
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&self.path)?.write_all(&line)?;
        Ok(())
    }
}

/// Keeps messages in memory so tests can follow the links.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryMailbox(pub(crate) std::sync::Mutex<Vec<AccountMessage>>);

#[cfg(test)]
impl AccountMail for MemoryMailbox {
    fn deliver(&self, message: &AccountMessage) -> Result<()> {
        self.0.lock().unwrap().push(message.clone());
        Ok(())
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/_konae/account", get(app_account_page))
        .route("/_konae/auth/verify", post(app_verify))
        .route("/_konae/auth/reset", post(app_reset))
        .route("/_konae/auth/recover", post(app_recover))
        .route("/_konae/owner/verify", post(creator_verify))
        .route("/_konae/owner/reset", post(creator_reset))
        .route("/_konae/owner/recover", post(creator_recover))
}

/// The answer to a signup or recovery request, the same whether or not an
/// account exists.
const LINK_SENT: &str = "If this email can use the app, a link is on its way. Open it to choose your password.";

/// The answer to a sign-in that failed in a browser that started a signup
/// for the same email on this app. It keeps the 401 and the first sentence.
pub(crate) const SIGNUP_HINT: &str = "Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first.";

/// The cookie a browser keeps after it asks for a signup link, so a sign-in
/// that fails before the link is opened can say why. It holds a hash of the
/// Deployment and the email the person typed, never the email.
fn signup_hint_name(state: &AppState) -> &'static str {
    if state.public_origin.starts_with("https://") { "__Host-konae_signup" } else { "konae_signup" }
}

fn signup_hint_value(deployment: &Deployment, email: &str) -> Option<String> {
    let email = auth::normalize_email(email).ok()?.to_lowercase();
    Some(auth::hash_token(&format!("{}\n{email}", deployment.id)))
}

/// The `Set-Cookie` for an answered signup request. Every answer that says a
/// link is on its way sets it, whether the email is new, pending, active or
/// suspended, so the cookie records only what this browser asked for and
/// says nothing about the account. It lives as long as a verification link.
pub(crate) fn signup_hint_cookie(state: &AppState, deployment: &Deployment, email: &str) -> Option<HeaderValue> {
    let value = signup_hint_value(deployment, email)?;
    let name = signup_hint_name(state);
    let secure = if name.starts_with("__Host-") { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
        auth::VERIFICATION_TTL_SECONDS
    ))
    .ok()
}

/// Whether this browser asked for a signup link for `email` on this app.
/// The hint depends on this alone, never on the account's state, so it tells
/// a person who missed the email what to do and tells someone testing
/// addresses nothing: their own signup request sets the same cookie for an
/// address with an account as for one without.
pub(crate) fn signup_hint_applies(state: &AppState, deployment: &Deployment, headers: &HeaderMap, email: &str) -> bool {
    let Some(expected) = signup_hint_value(deployment, email) else { return false };
    let name = signup_hint_name(state);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| part.trim().strip_prefix(name)?.strip_prefix('='))
        .any(|value| value == expected)
}

/// Opens one Deployment's account store in this installation's mode.
pub(crate) fn app_store(state: &AppState, deployment: &Deployment) -> Result<AuthService, AuthError> {
    AuthService::open_in(
        state
            .data_root
            .join("deployments")
            .join(&deployment.id)
            .join("identity.sqlite"),
        &deployment_scope(&deployment.id),
        state.accounts.verification,
    )
}

/// Opens the creator account store in this installation's mode.
pub(crate) fn creator_store(state: &AppState) -> Result<AuthService, AuthError> {
    AuthService::open_in(
        state.data_root.join("owners.sqlite"),
        CREATOR_SCOPE,
        state.accounts.verification,
    )
}

/// How a challenge's message leaves: the persistent queue, or a direct
/// transport in tests.
#[derive(Clone)]
enum Outbox {
    Queue(Arc<mail::MailQueue>),
    Direct(Arc<dyn AccountMail>),
}

fn mail(state: &AppState) -> Result<Outbox, AuthError> {
    match (&state.accounts.queue, &state.accounts.mail) {
        (Some(queue), _) => Ok(Outbox::Queue(queue.clone())),
        (None, Some(mail)) => Ok(Outbox::Direct(mail.clone())),
        (None, None) => Err(AuthError::MailUnavailable),
    }
}

/// Who a challenge's message is for and where its link opens.
struct Delivery {
    email: String,
    client: Option<std::net::IpAddr>,
    scope: String,
    creator: Option<String>,
    app_name: String,
    /// The inviting owner's account email, for an invitation.
    inviter: Option<String>,
    page: String,
}

/// Issues a challenge with `issue` and gets its message out. Through the
/// queue, the limits run first and the job commits with the challenge;
/// a refusal stores neither. Returns whether a challenge was issued.
async fn issue_and_send(
    outbox: Outbox,
    delivery: Delivery,
    issue: impl FnOnce(Option<&dyn auth::ChallengeOutbox>) -> Result<Option<IssuedChallenge>, AuthError> + Send + 'static,
) -> Result<bool, AuthError> {
    match outbox {
        Outbox::Queue(queue) => {
            let recipient = delivery.email.trim().to_lowercase();
            let admitted = queue.clone();
            let (scope, creator) = (delivery.scope.clone(), delivery.creator.clone());
            blocking(move || {
                admitted.admit(&mail::Admission {
                    recipient: &recipient,
                    client: delivery.client,
                    scope: &scope,
                    creator: creator.as_deref(),
                })
            })
            .await?;
            let worker = queue.clone();
            let issued = blocking(move || {
                let outbox = queue.outbox(
                    &delivery.scope,
                    delivery.creator.as_deref(),
                    &delivery.app_name,
                    delivery.inviter.as_deref(),
                    &delivery.page,
                );
                issue(Some(&outbox))
            })
            .await?;
            worker.wake();
            Ok(issued.is_some())
        }
        Outbox::Direct(mail) => {
            let issued = blocking(move || issue(None)).await?;
            match issued {
                Some(challenge) => send(&*mail, challenge, &delivery.page).map(|()| true),
                None => Ok(false),
            }
        }
    }
}

/// The creator who owns a Deployment's project, for the per-creator limit.
pub(crate) fn project_creator(state: &AppState, project_id: &str) -> Option<String> {
    let path = state.data_root.join("mcp.sqlite");
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection
        .query_row("SELECT owner FROM projects WHERE id = ?1", [project_id], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
}

fn app_delivery(state: &AppState, deployment: &Deployment, email: &str, client: std::net::IpAddr) -> Delivery {
    Delivery {
        email: email.to_owned(),
        client: Some(client),
        scope: deployment_scope(&deployment.id),
        creator: Some(project_creator(state, &deployment.project_id).unwrap_or_else(|| "operator".into())),
        app_name: deployment.project_name.clone(),
        inviter: None,
        page: format!("{}/_konae/account", super::domain::app_url(state, deployment)),
    }
}

fn creator_delivery(email: &str, client: std::net::IpAddr, page: &str) -> Delivery {
    Delivery {
        email: email.to_owned(),
        client: Some(client),
        scope: CREATOR_SCOPE.to_owned(),
        creator: None,
        app_name: "Konae".into(),
        inviter: None,
        page: page.to_owned(),
    }
}

/// Whether this installation can send account email at all. Tools that
/// create something a message must follow check this before writing.
pub(crate) fn mail_ready(state: &AppState) -> Result<(), AuthError> {
    mail(state).map(|_| ())
}

/// Gets out the message for a challenge another module issued in its own
/// transaction, such as an invitation. It goes through the same outbox,
/// budgets and transport as signup and recovery. `page` is where the link
/// opens; the link is `page#<purpose>=<token>`. Owner tools pass no client,
/// so only the creator, app and installation budgets apply.
pub(crate) async fn deliver_issued(
    state: &AppState,
    deployment: &Deployment,
    challenge: IssuedChallenge,
    page: String,
    client: Option<std::net::IpAddr>,
    inviter: Option<String>,
) -> Result<(), AuthError> {
    let mut delivery = app_delivery(state, deployment, &challenge.email, std::net::Ipv4Addr::UNSPECIFIED.into());
    delivery.client = client;
    delivery.page = page;
    delivery.inviter = inviter;
    let auth = app_store(state, deployment)?;
    issue_and_send(mail(state)?, delivery, move |outbox| {
        if let Some(outbox) = outbox {
            auth.record_issued(outbox, &challenge)?;
        }
        Ok(Some(challenge))
    })
    .await
    .map(|_| ())
}

fn send(mail: &dyn AccountMail, challenge: IssuedChallenge, page: &str) -> Result<(), AuthError> {
    let message = AccountMessage {
        link: format!("{page}#{}={}", challenge.purpose.as_str(), challenge.token),
        to: challenge.email,
        purpose: challenge.purpose,
        scope: challenge.scope,
        expires_at: challenge.expires_at,
    };
    mail.deliver(&message).map_err(|error_value| {
        eprintln!("Account email for {} failed: {error_value}", message.scope);
        AuthError::MailUnavailable
    })
}

fn current_deployment_admits(deployments: &HashMap<String, Deployment>, id: &str, email: &str) -> bool {
    deployments
        .values()
        .find(|current| current.id == id)
        .is_some_and(|current| current.signup_policy.allows(email))
}

/// Starts a signup on an app that requires verification. The current signup
/// policy is checked under the identity lock, like an unverified signup.
pub(crate) async fn signup_app(
    state: &AppState,
    deployment: &Deployment,
    email: &str,
    client: std::net::IpAddr,
) -> Result<(), AuthError> {
    let email = auth::normalize_email(email)?;
    let mail = mail(state)?;
    if !deployment.signup_policy.allows(&email) {
        return Err(AuthError::SignupClosed);
    }
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    if !current_deployment_admits(&*state.deployments.read().await, &deployment.id, &email) {
        return Err(AuthError::SignupClosed);
    }
    let auth = app_store(state, deployment)?;
    let delivery = app_delivery(state, deployment, &email, client);
    issue_and_send(mail, delivery, move |outbox| auth.request_signup_with(&email, outbox, |_, _| Ok(()))).await?;
    Ok(())
}

/// Starts a recovery on an app. Unknown, suspended and malformed emails get
/// the same answer as real ones.
pub(crate) async fn recover_app(
    state: &AppState,
    deployment: &Deployment,
    email: &str,
    client: std::net::IpAddr,
) -> Result<(), AuthError> {
    let mail = mail(state)?;
    let email = email.to_owned();
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let auth = app_store(state, deployment)?;
    let delivery = app_delivery(state, deployment, &email, client);
    issue_and_send(mail, delivery, move |outbox| auth.request_recovery_with(&email, outbox)).await?;
    Ok(())
}

/// Answers an app challenge. Verification rechecks the Deployment's current
/// signup policy inside the transaction that would activate the account.
pub(crate) async fn complete_app(
    state: &AppState,
    deployment: &Deployment,
    purpose: Purpose,
    token: &str,
    password: &str,
) -> Result<AuthSession, AuthError> {
    let password = password.to_owned();
    let chosen = blocking(move || NewPassword::prepare(&password)).await?;
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let deployments = state.deployments.read().await.clone();
    let id = deployment.id.clone();
    let auth = app_store(state, deployment)?;
    let token = token.to_owned();
    let session = blocking(move || {
        auth.complete_challenge(purpose, &token, chosen, |email| {
            current_deployment_admits(&deployments, &id, email)
        })
    })
    .await?;
    // An answered verification link activates an account; a recovery signs one in.
    let operation = if purpose == Purpose::Recovery { super::usage::Operation::Login } else { super::usage::Operation::Signup };
    super::usage::count(&state.data_root, &deployment.id, operation);
    Ok(session)
}

/// Starts a creator signup on an installation that requires verification.
/// The admission, an invitation code or the installation key, is checked and
/// consumed in the transaction that creates the pending creator. `page` is
/// the sign-in page the link opens. `client` feeds the mail limits.
pub(crate) async fn signup_creator(
    state: &AppState,
    email: &str,
    page: &str,
    admission: admission::Admission,
    client: std::net::IpAddr,
) -> Result<Option<String>, AuthError> {
    let email = auth::normalize_email(email)?;
    let mail = mail(state)?;
    let _guard = database_lock(state, "owner-auth").await;
    let auth = creator_store(state)?;
    let lookup = email.clone();
    let delivery = creator_delivery(&email, client, page);
    let issue = move |outbox: Option<&dyn auth::ChallengeOutbox>| auth.request_signup_with(&lookup, outbox, admission::admit(admission));
    if !issue_and_send(mail, delivery, issue).await? {
        return Ok(None);
    }
    let path = state.data_root.join("owners.sqlite");
    let user_id = Connection::open(path)
        .and_then(|db| {
            db.query_row("SELECT id FROM app_users WHERE email = ?1 COLLATE NOCASE", [&email], |row| row.get(0))
        })
        .map_err(AuthError::Storage)?;
    Ok(Some(user_id))
}

async fn complete_creator(state: &AppState, purpose: Purpose, token: &str, password: &str) -> Result<AuthSession, AuthError> {
    let password = password.to_owned();
    let chosen = blocking(move || NewPassword::prepare(&password)).await?;
    let _guard = database_lock(state, "owner-auth").await;
    let auth = creator_store(state)?;
    let token = token.to_owned();
    blocking(move || auth.complete_challenge(purpose, &token, chosen, |_| true)).await
}

#[derive(Deserialize)]
struct ChallengeAnswer {
    token: String,
    password: String,
}

#[derive(Deserialize)]
struct RecoveryRequest {
    email: String,
}

/// Reads a small JSON body after the origin check and the rate limit.
pub(crate) async fn read_body<T: serde::de::DeserializeOwned>(
    state: &AppState,
    request: Request,
    scope: &str,
    key: impl FnOnce(&T) -> String,
) -> std::result::Result<(T, std::net::IpAddr), Response> {
    let headers = request.headers().clone();
    if !same_origin(&headers) {
        return Err(error(StatusCode::FORBIDDEN, "Cross-origin account request rejected."));
    }
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    let bytes = axum::body::to_bytes(request.into_body(), 4096)
        .await
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Account request is too large."))?;
    let body: T = serde_json::from_slice(&bytes)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Account request is missing a field."))?;
    let client = state.guards.client_ip(peer, &headers);
    state
        .guards
        .auth_attempt(scope, client, &key(&body))
        .map_err(too_many_attempts)?;
    Ok((body, client))
}

pub(crate) fn link_sent() -> Response {
    let mut response = (
        StatusCode::ACCEPTED,
        Json(json!({"result": {"status": "link-sent", "message": LINK_SENT}})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The `Set-Cookie` value that carries an app session to the browser, under
/// the name `app_cookie_name` chose for the request.
pub(crate) fn app_session_cookie(cookie_name: &str, session: &AuthSession) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{cookie_name}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800{}",
        session.token,
        super::cookie_secure(cookie_name)
    ))
    .unwrap()
}

/// The app session cookie and body for a session just issued.
pub(crate) fn app_session_response(cookie_name: &str, session: AuthSession) -> Response {
    let cookie = app_session_cookie(cookie_name, &session);
    let mut response =
        Json(json!({"result": {"user": session.user, "expiresAt": session.expires_at}})).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Runs one runtime MCP account tool for the app on this host. Returns the
/// tool result and, when the tool signed someone in, the session cookie.
pub(crate) async fn runtime_tool(
    state: &AppState,
    headers: &HeaderMap,
    client: std::net::IpAddr,
    name: &str,
    arguments: &Value,
) -> Result<(Value, Option<HeaderValue>)> {
    let deployment = deployment_for(state, headers)
        .await
        .ok_or_else(|| anyhow::anyhow!("Deployment not found."))?;
    let text = |key: &str| arguments[key].as_str().unwrap_or("").to_owned();
    let limit_key = if name == "auth_signup" || name == "auth_recover" { text("email") } else { "account-link".into() };
    if state.guards.auth_attempt(&deployment.id, client, &limit_key).is_err() {
        anyhow::bail!("Too many sign-in attempts. Try again later.");
    }
    let signed_in = |session: AuthSession| {
        let cookie = app_session_cookie(&app_cookie_name(state, &deployment.id, headers), &session);
        (json!({"user": session.user, "expiresAt": session.expires_at}), Some(cookie))
    };
    let sent = || (json!({"status": "link-sent", "message": LINK_SENT}), None);
    let outcome = match name {
        "auth_signup" if state.accounts.verification == Verification::Required => {
            let hint = signup_hint_cookie(state, &deployment, &text("email"));
            signup_app(state, &deployment, &text("email"), client)
                .await
                .map(|()| (json!({"status": "link-sent", "message": LINK_SENT}), hint))
        }
        "auth_signup" => signup_app_unverified(state, &deployment, &text("email"), &text("password"))
            .await
            .map(signed_in),
        "auth_recover" => recover_app(state, &deployment, &text("email"), client).await.map(|()| sent()),
        "auth_verify" => complete_app(state, &deployment, Purpose::Verification, &text("token"), &text("password"))
            .await
            .map(signed_in),
        "auth_reset" => complete_app(state, &deployment, Purpose::Recovery, &text("token"), &text("password"))
            .await
            .map(signed_in),
        _ => anyhow::bail!("Unknown account tool."),
    };
    outcome.map_err(|error_value| anyhow::anyhow!(error_value.to_string()))
}

/// A signup on an installation in local mode: the account is active at once.
async fn signup_app_unverified(
    state: &AppState,
    deployment: &Deployment,
    email: &str,
    password: &str,
) -> Result<AuthSession, AuthError> {
    let (email, password) = (email.to_owned(), password.to_owned());
    let account = blocking(move || auth::NewAccount::prepare(&email, &password)).await?;
    let requested = account.email().to_owned();
    if !deployment.signup_policy.allows(&requested) {
        return Err(AuthError::SignupClosed);
    }
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    if !current_deployment_admits(&*state.deployments.read().await, &deployment.id, &requested) {
        return Err(AuthError::SignupClosed);
    }
    let auth = app_store(state, deployment)?;
    let session = blocking(move || auth.create_account(account)).await?;
    super::usage::count(&state.data_root, &deployment.id, super::usage::Operation::Signup);
    Ok(session)
}

async fn app_account_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if deployment_for(&state, &headers).await.is_none() {
        return error(StatusCode::NOT_FOUND, "Konae deployment not found.");
    }
    html_page(APP_ACCOUNT_PAGE)
}

async fn app_verify(State(state): State<AppState>, request: Request) -> Response {
    app_complete(state, request, Purpose::Verification).await
}

async fn app_reset(State(state): State<AppState>, request: Request) -> Response {
    app_complete(state, request, Purpose::Recovery).await
}

async fn app_complete(state: AppState, request: Request, purpose: Purpose) -> Response {
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let cookie_name = app_cookie_name(&state, &deployment.id, request.headers());
    let answer: ChallengeAnswer = match read_body(&state, request, &deployment.id, |_| "account-link".into()).await {
        Ok((answer, _)) => answer,
        Err(response) => return response,
    };
    match complete_app(&state, &deployment, purpose, &answer.token, &answer.password).await {
        Ok(session) => app_session_response(&cookie_name, session),
        Err(error_value) => auth_error(error_value),
    }
}

async fn app_recover(State(state): State<AppState>, request: Request) -> Response {
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let (input, client): (RecoveryRequest, _) =
        match read_body(&state, request, &deployment.id, |input: &RecoveryRequest| input.email.clone()).await {
            Ok(input) => input,
            Err(response) => return response,
        };
    match recover_app(&state, &deployment, &input.email, client).await {
        Ok(()) => link_sent(),
        Err(error_value) => auth_error(error_value),
    }
}

async fn creator_verify(State(state): State<AppState>, request: Request) -> Response {
    creator_complete(state, request, Purpose::Verification).await
}

async fn creator_reset(State(state): State<AppState>, request: Request) -> Response {
    creator_complete(state, request, Purpose::Recovery).await
}

async fn creator_complete(state: AppState, request: Request, purpose: Purpose) -> Response {
    if !creator_host(&state, request.headers()).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    let answer: ChallengeAnswer = match read_body(&state, request, "owner", |_| "account-link".into()).await {
        Ok((answer, _)) => answer,
        Err(response) => return response,
    };
    match complete_creator(&state, purpose, &answer.token, &answer.password).await {
        Ok(session) => mcp::owner_session_response(&state, session),
        Err(error_value) => auth_error(error_value),
    }
}

async fn creator_recover(State(state): State<AppState>, request: Request) -> Response {
    if !creator_host(&state, request.headers()).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    let origin = oauth::origin(&state, request.headers());
    let (input, client): (RecoveryRequest, _) =
        match read_body(&state, request, "owner", |input: &RecoveryRequest| input.email.clone()).await {
            Ok(input) => input,
            Err(response) => return response,
        };
    let result = async {
        let mail = mail(&state)?;
        let _guard = database_lock(&state, "owner-auth").await;
        let auth = creator_store(&state)?;
        let email = input.email.clone();
        let delivery = creator_delivery(&email, client, &origin);
        issue_and_send(mail, delivery, move |outbox| auth.request_recovery_with(&email, outbox)).await?;
        Ok::<(), AuthError>(())
    }
    .await;
    match result {
        Ok(()) => link_sent(),
        Err(error_value) => auth_error(error_value),
    }
}

/// Creator accounts answer only on the editor host and only without the
/// optional hosted identity adapter, which keeps its own accounts.
async fn creator_host(state: &AppState, headers: &HeaderMap) -> bool {
    oauth::valid_owner_host(state, headers).await && matches!(clerk::ClerkConfig::from_env(), Ok(None))
}

fn html_page(body: impl Into<Body>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Into::<Body>::into(body),
    )
        .into_response()
}

/// The page an app's account link opens. It reads the token from the
/// fragment and posts it with the new password; GET does nothing else.
const APP_ACCOUNT_PAGE: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Your account</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}form,label{display:grid;gap:8px}form{gap:16px;margin-top:24px}input,button{font:inherit;border-radius:8px;padding:11px 12px}input{width:100%;border:1px solid #c9ced6}button{border:0;background:#273449;color:white;cursor:pointer}#error{min-height:1.5em;margin:0;color:#a32727}[hidden]{display:none}@media (prefers-color-scheme:dark){body{background:#14161a;color:#e8eaed}main{background:#1d2026;border-color:#2e333b;box-shadow:none}p{color:#a9b0bc}input{background:#14161a;color:#e8eaed;border-color:#3a404a}input[readonly]{background:#23272e;color:#e8eaed}button{background:#e8eaed;color:#14161a}button+button{background:#2a2f37;color:#e8eaed}#error{color:#ff8a8a}a{color:#8ab4ff}}</style><link rel="icon" href="data:,">
<main><h1 id="title">Reset your password</h1><p id="note">Enter the email of your account. If it can use this app, we send a link to choose a new password.</p>
<form id="recover"><label>Email <input name="email" type="email" required autocomplete="email"></label><button>Send link</button></form>
<form id="answer" hidden><label>New password <input name="password" type="password" minlength="8" required autocomplete="new-password"></label><button id="confirm">Confirm</button></form>
<p id="error" role="alert"></p><p><a id="login" href="/_konae/login">Back to sign in</a></p></main>
<script>
const match=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);const prefix=match?'/p/'+match[1]:'';const at=path=>prefix+path;document.querySelector('#login').href=at('/_konae/login');
const params=new URLSearchParams(location.hash.slice(1));const purpose=params.has('verification')?'verification':params.has('recovery')?'recovery':null;const token=purpose&&params.get(purpose);
const $=s=>document.querySelector(s);
if(token){history.replaceState(null,'',location.pathname);$('#recover').hidden=true;$('#answer').hidden=false;$('#title').textContent=purpose==='verification'?'Confirm your email':'Choose a new password';$('#note').textContent=purpose==='verification'?'Choose the password for your account. Confirming activates it.':'Your other sessions end when you confirm.';}
async function post(path,body){const r=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});const b=await r.json().catch(()=>({}));if(!r.ok)throw Error(b.error||'Request failed.');return {status:r.status,body:b};}
$('#recover').addEventListener('submit',async e=>{e.preventDefault();try{const r=await post(at('/_konae/auth/recover'),{email:new FormData(e.currentTarget).get('email')});$('#note').textContent=r.body.result.message;$('#error').textContent='';}catch(x){$('#error').textContent=x.message}});
$('#answer').addEventListener('submit',async e=>{e.preventDefault();try{await post(at(purpose==='verification'?'/_konae/auth/verify':'/_konae/auth/reset'),{token,password:new FormData(e.currentTarget).get('password')});location.replace(at('/'));}catch(x){$('#error').textContent=x.message}});
</script></html>"#;

/// Suspends or resumes a local creator account from the server, before the
/// Kernel starts listening:
/// `konae-kernel creator-account-suspend <email>` and
/// `konae-kernel creator-account-resume <email>`. Suspension ends the
/// creator's sessions, open links and MCP credentials; projects and app
/// accounts stay. Returns false when the arguments name another command.
pub fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let suspend = match args.first().map(String::as_str) {
        Some("creator-account-suspend") => true,
        Some("creator-account-resume") => false,
        _ => return Ok(false),
    };
    anyhow::ensure!(
        args.len() == 2,
        "Usage: konae-kernel creator-account-suspend <email> | creator-account-resume <email>"
    );
    let root = PathBuf::from(env("KONAE_DATA_DIR", "/data"));
    println!("{}", run_operator_command(&root, suspend, &args[1])?);
    Ok(true)
}

fn run_operator_command(root: &Path, suspend: bool, email: &str) -> Result<String> {
    let owners = root.join("owners.sqlite");
    anyhow::ensure!(owners.exists(), "No creator accounts exist in {}.", root.display());
    let verification = Verification::from_env().map_err(anyhow::Error::msg)?;
    let auth = AuthService::open_in(&owners, CREATOR_SCOPE, verification)?;
    let not_found = || anyhow::anyhow!("No {}creator account has that email.", if suspend { "" } else { "suspended " });
    if !suspend {
        let (id, state) = auth.resume(email).map_err(|_| not_found())?;
        return Ok(format!("Resumed creator {id}. The account is {state}; it signs in again to get a session."));
    }
    let id = auth.suspend(email).map_err(|_| not_found())?;
    // The account is suspended even if this cleanup fails, and a delegated
    // credential checks the account's state on every use.
    let mcp_path = root.join("mcp.sqlite");
    if mcp_path.exists() {
        let mut mcp = Connection::open_with_flags(mcp_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        mcp.busy_timeout(std::time::Duration::from_secs(5))?;
        let transaction = mcp.transaction()?;
        transaction.execute("DELETE FROM credentials WHERE owner = ?1", [&id])?;
        let has_codes: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'oauth_codes')",
            [],
            |row| row.get(0),
        )?;
        if has_codes {
            transaction.execute("DELETE FROM oauth_codes WHERE json_extract(body, '$.owner') = ?1", [&id])?;
        }
        transaction.commit()?;
    }
    super::recovery::record_offline(root, super::recovery::Entry::new("creator", None, &id, "suspended"))?;
    Ok(format!(
        "Suspended creator {id}. Projects and app accounts remain available to the installation operator."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_HOST: &str = "accounts-test.localhost";
    const OTHER_HOST: &str = "other-app.localhost";
    const PASSWORD: &str = "correct horse battery";

    fn deployment(host: &str) -> Deployment {
        Deployment {
            id: Uuid::new_v4().to_string(),
            project_id: Uuid::new_v4().to_string(),
            project_name: "Accounts test".into(),
            url: format!("http://{host}"),
            created_at: now_ms(),
            manifest: json!({"visibility": "public"}),
            hostname: host.into(),
            actions: "{}".into(),
            artifact_version: None,
            database_model: None,
            signup_policy: SignupPolicy::default(),
            sign_in_methods: default_sign_in_methods(),
        }
    }

    struct Fixture {
        state: AppState,
        mailbox: Arc<MemoryMailbox>,
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(verification: Verification, with_mail: bool) -> Fixture {
        let root = std::env::temp_dir().join(format!("konae-accounts-{}", Uuid::new_v4()));
        let mut deployments = HashMap::new();
        for host in [APP_HOST, OTHER_HOST] {
            let item = deployment(host);
            std::fs::create_dir_all(root.join("deployments").join(&item.id)).unwrap();
            deployments.insert(host.to_owned(), item);
        }
        let mailbox = Arc::new(MemoryMailbox::default());
        let state = AppState {
            deployments: Arc::new(RwLock::new(deployments)),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            files: crate::file_store::Files::local(&root),
            data_root: root.clone(),
            copies: Default::default(),
            admin_key: "test-admin".into(),
            accounts: AccountSettings {
                verification,
                mail: with_mail.then(|| mailbox.clone() as Arc<dyn AccountMail>),
                queue: None,
                google: Arc::new(google::Google::real()),
            },
            public_origin: "http://localhost".into(),
            mcp_origin: None,
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
        };
        Fixture { state, mailbox, root }
    }

    impl Fixture {
        async fn app(&self, host: &str) -> Deployment {
            self.state.deployments.read().await.get(host).cloned().unwrap()
        }

        /// The token of the newest message, and its purpose.
        fn last_link(&self) -> (Purpose, String) {
            let messages = self.mailbox.0.lock().unwrap();
            let message = messages.last().expect("a message was sent");
            let token = message.link.split_once('=').unwrap().1.to_owned();
            (message.purpose, token)
        }

        fn sent(&self) -> usize {
            self.mailbox.0.lock().unwrap().len()
        }

        async fn app_state_of(&self, host: &str, email: &str) -> String {
            let app = self.app(host).await;
            Connection::open(self.root.join("deployments").join(app.id).join("identity.sqlite"))
                .unwrap()
                .query_row("SELECT state FROM app_users WHERE email = ?1", [email], |row| row.get(0))
                .unwrap()
        }

        async fn set_policy(&self, mode: SignupMode) {
            for item in self.state.deployments.write().await.values_mut() {
                item.signup_policy = SignupPolicy::from_input(mode, &[]).unwrap();
            }
        }
    }

    fn request(method: &str, host: &str, path: &str, body: Value) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn host_headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        headers
    }

    fn cookie_of(response: &Response) -> Option<String> {
        response
            .headers()
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().split(';').next().unwrap().to_owned())
    }

    async fn app_signup(fixture: &Fixture, host: &str, email: &str) -> Response {
        let body = json!({"email": email, "password": PASSWORD});
        super::super::signup(State(fixture.state.clone()), host_headers(host), request("POST", host, "/_konae/auth/signup", body)).await
    }

    async fn app_login(fixture: &Fixture, host: &str, email: &str, password: &str) -> Response {
        let body = json!({"email": email, "password": password});
        super::super::login(State(fixture.state.clone()), host_headers(host), request("POST", host, "/_konae/auth/login", body)).await
    }

    async fn app_answer(fixture: &Fixture, host: &str, path: &str, token: &str, password: &str) -> Response {
        let body = json!({"token": token, "password": password});
        let request = request("POST", host, path, body);
        if path.ends_with("/verify") {
            app_verify(State(fixture.state.clone()), request).await
        } else {
            app_reset(State(fixture.state.clone()), request).await
        }
    }

    async fn session_user(fixture: &Fixture, host: &str, cookie: &str) -> Option<AuthUser> {
        let app = fixture.app(host).await;
        let mut headers = host_headers(host);
        headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        authenticate_deployment(&fixture.state, &app, &headers).ok().flatten()
    }

    #[tokio::test]
    async fn app_signup_waits_for_verification_and_a_get_changes_nothing() {
        let fixture = fixture(Verification::Required, true);
        let login = super::super::login_page(State(fixture.state.clone()), host_headers(APP_HOST)).await;
        let login = String::from_utf8(axum::body::to_bytes(login.into_body(), 1 << 16).await.unwrap().to_vec()).unwrap();
        assert!(login.contains("formnovalidate") && login.contains("a link to choose your password"));
        let response = app_signup(&fixture, APP_HOST, "person@example.test").await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(cookie_of(&response).is_some_and(|cookie| cookie.starts_with("konae_signup=")), "a pending account gets no session, only the signup hint");
        let (purpose, token) = fixture.last_link();
        assert_eq!(purpose, Purpose::Verification);
        let link = fixture.mailbox.0.lock().unwrap()[0].link.clone();
        assert!(link.starts_with(&format!("http://{APP_HOST}/_konae/account#verification=")));
        assert_eq!(fixture.app_state_of(APP_HOST, "person@example.test").await, "pending");
        // The signup password was not stored, and the pending account cannot sign in.
        assert_eq!(app_login(&fixture, APP_HOST, "person@example.test", PASSWORD).await.status(), StatusCode::UNAUTHORIZED);

        // A mail scanner opening the link gets the form and nothing else. The
        // page handler receives no URI, so a token in the query cannot reach
        // it, and `routes()` binds the answer routes to POST only.
        let page = app_account_page(State(fixture.state.clone()), host_headers(APP_HOST)).await;
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.headers().get(header::SET_COOKIE).is_none());
        let body = String::from_utf8(axum::body::to_bytes(page.into_body(), 1 << 16).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("method:'POST'") && !body.contains(&token));
        assert_eq!(fixture.app_state_of(APP_HOST, "person@example.test").await, "pending");

        let verified = app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, "the chosen password").await;
        assert_eq!(verified.status(), StatusCode::OK);
        let cookie = cookie_of(&verified).unwrap();
        assert_eq!(session_user(&fixture, APP_HOST, &cookie).await.unwrap().name, "person@example.test");
        assert_eq!(fixture.app_state_of(APP_HOST, "person@example.test").await, "active");
        let again = app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, "another password").await;
        assert_eq!(again.status(), StatusCode::GONE, "a link works once");
        assert_eq!(app_login(&fixture, APP_HOST, "person@example.test", "the chosen password").await.status(), StatusCode::OK);

        // Signing up again with an active email answers the same and sends nothing.
        let repeated = app_signup(&fixture, APP_HOST, "person@example.test").await;
        assert_eq!(repeated.status(), StatusCode::ACCEPTED);
        assert_eq!(fixture.sent(), 1);
    }

    async fn app_login_with(fixture: &Fixture, host: &str, cookie: &str, email: &str, password: &str) -> (StatusCode, String) {
        let mut headers = host_headers(host);
        headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        let body = json!({"email": email, "password": password});
        let response = super::super::login(State(fixture.state.clone()), headers, request("POST", host, "/_konae/auth/login", body)).await;
        let status = response.status();
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap()).unwrap();
        (status, body["error"].as_str().unwrap_or_default().to_owned())
    }

    #[tokio::test]
    async fn a_sign_in_before_the_link_hints_only_in_the_browser_that_asked_and_never_by_account_state() {
        let mut fixture = fixture(Verification::Required, true);
        let plain = (StatusCode::UNAUTHORIZED, "Email or password is incorrect.".to_owned());
        let hinted = (StatusCode::UNAUTHORIZED, SIGNUP_HINT.to_owned());

        // Ana asks for a link and signs in before opening it, in the same browser.
        let started = app_signup(&fixture, APP_HOST, "ana@example.test").await;
        assert_eq!(started.status(), StatusCode::ACCEPTED);
        let set = started.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().to_owned();
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains(&format!("Max-Age={}", auth::VERIFICATION_TTL_SECONDS)));
        assert!(!set.contains("ana@example.test"), "the cookie holds a hash, never the email");
        let hint = cookie_of(&started).unwrap();
        assert_eq!(app_login_with(&fixture, APP_HOST, &hint, "ana@example.test", PASSWORD).await, hinted);
        assert_eq!(app_login_with(&fixture, APP_HOST, &hint, "ANA@example.test", PASSWORD).await, hinted, "the email matches as typed in any case");
        // Another browser, another email, or another app gets the plain answer.
        assert_eq!(app_login(&fixture, APP_HOST, "ana@example.test", PASSWORD).await.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(app_login_with(&fixture, APP_HOST, "other=1", "ana@example.test", PASSWORD).await, plain);
        assert_eq!(app_login_with(&fixture, APP_HOST, &hint, "ben@example.test", PASSWORD).await, plain);
        assert_eq!(app_login_with(&fixture, OTHER_HOST, &hint, "ana@example.test", PASSWORD).await, plain);

        // Someone testing addresses learns nothing. A signup request for an
        // address with an active account answers the same 202 and sets the
        // same kind of cookie, and a wrong password then gets the same hint
        // a pending account gets.
        // Every request here comes from one test address; start its rate limits over.
        fixture.state.guards = Arc::new(Guards::new(Vec::new()));
        let first = app_signup(&fixture, APP_HOST, "dora@example.test").await;
        let token = fixture.last_link().1;
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, "dora's real password").await.status(), StatusCode::OK);
        let probe = app_signup(&fixture, APP_HOST, "dora@example.test").await;
        assert_eq!(probe.status(), StatusCode::ACCEPTED);
        let probe_cookie = cookie_of(&probe).unwrap();
        assert_eq!(probe_cookie, cookie_of(&first).unwrap(), "the cookie depends on the app and the email, not on the account");
        assert_eq!(app_login_with(&fixture, APP_HOST, &probe_cookie, "dora@example.test", "a guessed password").await, hinted);
        let fresh = app_signup(&fixture, APP_HOST, "nobody@example.test").await;
        assert_eq!(app_login_with(&fixture, APP_HOST, &cookie_of(&fresh).unwrap(), "nobody@example.test", "a guessed password").await, hinted);
        // The right password still signs in with the cookie.
        assert_eq!(app_login_with(&fixture, APP_HOST, &probe_cookie, "dora@example.test", "dora's real password").await.0, StatusCode::OK);

        // The runtime endpoint's auth_signup sets the same cookie.
        fixture.state.guards = Arc::new(Guards::new(Vec::new()));
        let over_mcp = rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_signup", "arguments": {"email": "cara@example.test"}})).await;
        let cara = cookie_of(&over_mcp).unwrap();
        assert!(cara.starts_with("konae_signup="));
        assert_eq!(app_login_with(&fixture, APP_HOST, &cara, "cara@example.test", PASSWORD).await, hinted);
    }

    #[tokio::test]
    async fn verification_rechecks_the_policy_and_a_refusal_keeps_the_link() {
        let fixture = fixture(Verification::Required, true);
        assert_eq!(app_signup(&fixture, APP_HOST, "person@example.test").await.status(), StatusCode::ACCEPTED);
        let (_, token) = fixture.last_link();
        fixture.set_policy(SignupMode::Closed).await;
        let closed = app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await;
        assert_eq!(closed.status(), StatusCode::FORBIDDEN);
        assert!(cookie_of(&closed).is_none());
        assert_eq!(app_signup(&fixture, APP_HOST, "late@example.test").await.status(), StatusCode::FORBIDDEN);
        fixture.set_policy(SignupMode::Open).await;
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_link_does_nothing_on_another_app_the_creator_scope_or_another_purpose() {
        let fixture = fixture(Verification::Required, true);
        app_signup(&fixture, APP_HOST, "person@example.test").await;
        let (_, token) = fixture.last_link();
        assert_eq!(app_answer(&fixture, OTHER_HOST, "/_konae/auth/verify", &token, PASSWORD).await.status(), StatusCode::GONE);
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/reset", &token, PASSWORD).await.status(), StatusCode::GONE);
        let creator = creator_verify(
            State(fixture.state.clone()),
            request("POST", "localhost", "/_konae/owner/verify", json!({"token": token, "password": PASSWORD})),
        )
        .await;
        assert_eq!(creator.status(), StatusCode::GONE);
        assert_eq!(fixture.app_state_of(APP_HOST, "person@example.test").await, "pending");
        // A weak password is refused before the token is touched.
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, "short").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn recovery_answers_the_same_for_everyone_and_ends_earlier_sessions() {
        let fixture = fixture(Verification::Required, true);
        app_signup(&fixture, APP_HOST, "person@example.test").await;
        let (_, token) = fixture.last_link();
        let first = cookie_of(&app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await).unwrap();
        let recover = |email: &'static str| {
            let state = fixture.state.clone();
            async move {
                let response = app_recover(State(state), request("POST", APP_HOST, "/_konae/auth/recover", json!({"email": email}))).await;
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
                (status, body)
            }
        };
        let unknown = recover("nobody@example.test").await;
        assert_eq!(fixture.sent(), 1, "an unknown email gets no message");
        let known = recover("person@example.test").await;
        assert_eq!(unknown, known, "the answer does not reveal whether the account exists");
        assert_eq!(known.0, StatusCode::ACCEPTED);
        let (purpose, token) = fixture.last_link();
        assert_eq!(purpose, Purpose::Recovery);
        let reset = app_answer(&fixture, APP_HOST, "/_konae/auth/reset", &token, "a new password").await;
        assert_eq!(reset.status(), StatusCode::OK);
        let fresh = cookie_of(&reset).unwrap();
        assert!(session_user(&fixture, APP_HOST, &first).await.is_none());
        assert!(session_user(&fixture, APP_HOST, &fresh).await.is_some());
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/reset", &token, "third password").await.status(), StatusCode::GONE);
        assert_eq!(app_login(&fixture, APP_HOST, "person@example.test", PASSWORD).await.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_suspended_app_user_loses_every_session_and_gets_no_new_one() {
        let fixture = fixture(Verification::Required, true);
        app_signup(&fixture, APP_HOST, "person@example.test").await;
        let (_, token) = fixture.last_link();
        let cookie = cookie_of(&app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await).unwrap();
        let app = fixture.app(APP_HOST).await;
        app_store(&fixture.state, &app).unwrap().suspend("person@example.test").unwrap();
        assert!(session_user(&fixture, APP_HOST, &cookie).await.is_none());
        assert_eq!(app_login(&fixture, APP_HOST, "person@example.test", PASSWORD).await.status(), StatusCode::FORBIDDEN);
        let sent = fixture.sent();
        app_recover(State(fixture.state.clone()), request("POST", APP_HOST, "/_konae/auth/recover", json!({"email": "person@example.test"}))).await;
        assert_eq!(fixture.sent(), sent, "a suspended account gets no recovery link");
    }

    #[tokio::test]
    async fn without_mail_signup_and_recovery_explain_why_and_existing_accounts_still_sign_in() {
        let local = fixture(Verification::Local, false);
        let session = app_signup(&local, APP_HOST, "person@example.test").await;
        assert_eq!(session.status(), StatusCode::OK, "local mode keeps unverified signup");
        let app = local.app(APP_HOST).await;
        let mut state = local.state.clone();
        state.accounts = AccountSettings { verification: Verification::Required, mail: None, queue: None, google: Arc::new(google::Google::real()) };
        let required = Fixture { state, mailbox: local.mailbox.clone(), root: std::env::temp_dir().join("konae-accounts-unused") };
        assert_eq!(app_signup(&required, APP_HOST, "new@example.test").await.status(), StatusCode::SERVICE_UNAVAILABLE);
        let recover = app_recover(State(required.state.clone()), request("POST", APP_HOST, "/_konae/auth/recover", json!({"email": "person@example.test"}))).await;
        assert_eq!(recover.status(), StatusCode::SERVICE_UNAVAILABLE);
        // The local account never verified, so required mode moved it to pending.
        assert_eq!(app_login(&required, APP_HOST, "person@example.test", PASSWORD).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(local.app_state_of(APP_HOST, "person@example.test").await, "pending");
        let _ = app;
    }

    async fn rpc(fixture: &Fixture, host: &str, cookie: Option<&str>, method: &str, params: Value) -> Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/_konae/mcp")
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        mcp::runtime(State(fixture.state.clone()), builder.body(Body::from(body.to_string())).unwrap()).await
    }

    async fn rpc_json(response: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn runtime_mcp_account_tools_work_before_sign_in_on_a_private_app() {
        let fixture = fixture(Verification::Required, true);
        for item in fixture.state.deployments.write().await.values_mut() {
            item.manifest = json!({"visibility": "private"});
        }
        // Signed out on a private app: no tool list and no other tool.
        assert_eq!(rpc(&fixture, APP_HOST, None, "tools/list", json!({})).await.status(), StatusCode::UNAUTHORIZED);
        let refused = rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "actions_invoke", "arguments": {"name": "x"}})).await;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

        let signup = rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_signup", "arguments": {"email": "person@example.test"}})).await;
        assert!(cookie_of(&signup).is_some_and(|cookie| cookie.starts_with("konae_signup=")), "no session, only the signup hint");
        let body = rpc_json(signup).await;
        assert_eq!(body["result"]["structuredContent"]["result"]["status"], "link-sent", "{body}");
        assert!(!body.to_string().contains(&fixture.last_link().1), "no tool returns a token");
        let (_, token) = fixture.last_link();
        let verify = rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_verify", "arguments": {"token": token, "password": PASSWORD}})).await;
        let cookie = cookie_of(&verify).unwrap();
        let body = rpc_json(verify).await;
        assert_eq!(body["result"]["isError"], false, "{body}");
        assert!(!body.to_string().contains(cookie.split('=').nth(1).unwrap()), "the session token stays in the cookie");
        let again = rpc_json(rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_verify", "arguments": {"token": token, "password": PASSWORD}})).await).await;
        assert_eq!(again["result"]["isError"], true);
        let session = rpc_json(rpc(&fixture, APP_HOST, Some(&cookie), "tools/call", json!({"name": "auth_session", "arguments": {}})).await).await;
        assert_eq!(session["result"]["structuredContent"]["result"]["name"], "person@example.test");

        rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_recover", "arguments": {"email": "person@example.test"}})).await;
        let (purpose, token) = fixture.last_link();
        assert_eq!(purpose, Purpose::Recovery);
        let reset = rpc(&fixture, APP_HOST, None, "tools/call", json!({"name": "auth_reset", "arguments": {"token": token, "password": "a new password"}})).await;
        assert!(cookie_of(&reset).is_some());
        assert!(session_user(&fixture, APP_HOST, &cookie).await.is_none(), "reset ends the earlier session");
    }

    async fn owner(fixture: &Fixture, path: &str, body: Value) -> Response {
        let request = request("POST", "localhost", path, body);
        match path {
            "/_konae/owner/signup" => mcp::owner_signup(State(fixture.state.clone()), request).await,
            "/_konae/owner/login" => mcp::owner_login(State(fixture.state.clone()), request).await,
            "/_konae/owner/verify" => creator_verify(State(fixture.state.clone()), request).await,
            "/_konae/owner/reset" => creator_reset(State(fixture.state.clone()), request).await,
            "/_konae/owner/recover" => creator_recover(State(fixture.state.clone()), request).await,
            _ => unreachable!(),
        }
    }

    fn owner_headers(cookie: Option<&str>, bearer: Option<&str>) -> HeaderMap {
        let mut headers = host_headers("localhost");
        if let Some(cookie) = cookie {
            headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        }
        if let Some(bearer) = bearer {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {bearer}")).unwrap());
        }
        headers
    }

    #[tokio::test]
    async fn a_creator_signs_up_verifies_enters_and_a_suspended_one_cannot() {
        let mut fixture = fixture(Verification::Required, true);
        let login = json!({"email": "creator@example.test", "password": PASSWORD});
        let refused = owner(&fixture, "/_konae/owner/signup", json!({"email": "creator@example.test", "setupKey": "wrong"})).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let signup = owner(&fixture, "/_konae/owner/signup", json!({"email": "creator@example.test", "setupKey": "test-admin"})).await;
        assert_eq!(signup.status(), StatusCode::ACCEPTED);
        assert!(cookie_of(&signup).is_none());
        let link = fixture.mailbox.0.lock().unwrap()[0].clone();
        assert_eq!(link.scope, CREATOR_SCOPE);
        assert!(link.link.starts_with("http://localhost/_konae/owner/login#verification="));
        assert_eq!(owner(&fixture, "/_konae/owner/login", login.clone()).await.status(), StatusCode::UNAUTHORIZED);
        let (_, token) = fixture.last_link();
        // A creator token does nothing on an app.
        assert_eq!(app_answer(&fixture, APP_HOST, "/_konae/auth/verify", &token, PASSWORD).await.status(), StatusCode::GONE);
        let verified = owner(&fixture, "/_konae/owner/verify", json!({"token": token, "password": PASSWORD})).await;
        assert_eq!(verified.status(), StatusCode::OK);
        let cookie = cookie_of(&verified).unwrap();
        let session = mcp::authenticate_owner(&fixture.state, &owner_headers(Some(&cookie), None)).unwrap().unwrap();
        // The editor's first check answers 200 either way, so a signed-out visit logs no error.
        let signed_out = mcp::owner_session(State(fixture.state.clone()), owner_headers(None, None)).await;
        assert_eq!(signed_out.status(), StatusCode::OK);
        assert_eq!(rpc_json(signed_out).await, json!({"signedIn": false}));
        let signed_in = mcp::owner_session(State(fixture.state.clone()), owner_headers(Some(&cookie), None)).await;
        assert_eq!(signed_in.status(), StatusCode::OK);
        assert_eq!(rpc_json(signed_in).await, json!({"signedIn": true, "ownerId": session.id, "displayName": "creator@example.test"}));
        assert_eq!(owner(&fixture, "/_konae/owner/login", login.clone()).await.status(), StatusCode::OK);

        // A delegated MCP credential for this creator.
        let bearer = "creator-bearer-token";
        mcp::store(&fixture.state).unwrap().execute(
            "INSERT INTO credentials(hash, owner, scopes, projects, expires) VALUES (?1, ?2, '[\"read\"]', '[]', ?3)",
            rusqlite::params![mcp::digest(bearer.as_bytes()), session.id, now_ms() + 60_000],
        ).unwrap();
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(None, Some(bearer))).unwrap().is_some());

        // A credential that survives suspension is still refused, because
        // every use reads the creator's state.
        creator_store(&fixture.state).unwrap().suspend("creator@example.test").unwrap();
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(None, Some(bearer))).is_err());
        creator_store(&fixture.state).unwrap().resume("creator@example.test").unwrap();
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(None, Some(bearer))).unwrap().is_some());

        // The operator suspends the creator from the server.
        let message = run_operator_command(&fixture.root, true, "Creator@Example.test").unwrap();
        assert!(message.contains(&session.id));
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(Some(&cookie), None)).unwrap().is_none());
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(None, Some(bearer))).unwrap().is_none());
        let credentials: i64 = mcp::store(&fixture.state).unwrap()
            .query_row("SELECT COUNT(*) FROM credentials WHERE owner = ?1", [&session.id], |row| row.get(0))
            .unwrap();
        assert_eq!(credentials, 0);
        assert_eq!(owner(&fixture, "/_konae/owner/login", login.clone()).await.status(), StatusCode::FORBIDDEN);
        let sent = fixture.sent();
        owner(&fixture, "/_konae/owner/recover", json!({"email": "creator@example.test"})).await;
        assert_eq!(fixture.sent(), sent, "a suspended creator gets no recovery link");

        assert!(run_operator_command(&fixture.root, false, "nobody@example.test").is_err());
        run_operator_command(&fixture.root, false, "creator@example.test").unwrap();
        // This test has used up the per-email sign-in limit; start a fresh window.
        fixture.state.guards = Arc::new(Guards::new(Vec::new()));
        assert_eq!(owner(&fixture, "/_konae/owner/login", login).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn creator_recovery_resets_the_password_and_ends_earlier_sessions() {
        let fixture = fixture(Verification::Required, true);
        owner(&fixture, "/_konae/owner/signup", json!({"email": "creator@example.test", "setupKey": "test-admin"})).await;
        let (_, token) = fixture.last_link();
        let first = cookie_of(&owner(&fixture, "/_konae/owner/verify", json!({"token": token, "password": PASSWORD})).await).unwrap();
        let unknown = owner(&fixture, "/_konae/owner/recover", json!({"email": "nobody@example.test"})).await.status();
        let known = owner(&fixture, "/_konae/owner/recover", json!({"email": "creator@example.test"})).await.status();
        assert_eq!((unknown, known), (StatusCode::ACCEPTED, StatusCode::ACCEPTED));
        let (purpose, token) = fixture.last_link();
        assert_eq!(purpose, Purpose::Recovery);
        assert_eq!(owner(&fixture, "/_konae/owner/verify", json!({"token": token, "password": PASSWORD})).await.status(), StatusCode::GONE);
        assert_eq!(owner(&fixture, "/_konae/owner/reset", json!({"token": token, "password": "a new password"})).await.status(), StatusCode::OK);
        assert!(mcp::authenticate_owner(&fixture.state, &owner_headers(Some(&first), None)).unwrap().is_none());
        assert_eq!(owner(&fixture, "/_konae/owner/reset", json!({"token": token, "password": "third password"})).await.status(), StatusCode::GONE);
    }

    #[test]
    fn the_development_mailbox_appends_one_line_per_message() {
        let path = std::env::temp_dir().join(format!("konae-mailbox-{}.jsonl", Uuid::new_v4()));
        let mailbox = DevelopmentMailbox::new(path.clone());
        let message = AccountMessage {
            to: "person@example.test".into(),
            purpose: Purpose::Recovery,
            scope: CREATOR_SCOPE.into(),
            link: "http://localhost/_konae/owner/login#recovery=abc".into(),
            expires_at: 1,
        };
        mailbox.deliver(&message).unwrap();
        mailbox.deliver(&message).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["purpose"], "recovery");
        assert_eq!(lines[0]["link"], message.link);
        let _ = std::fs::remove_file(path);
    }
}
