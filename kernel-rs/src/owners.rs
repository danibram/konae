//! Owner accounts kept in `owners.sqlite`: signup through an invitation
//! code or the setup key, the verification and recovery links, and the
//! operator's suspension command. They use the same account core and email
//! queue as app accounts, under their own scope.

use super::*;
use super::accounts::{ChallengeAnswer, Delivery, RecoveryRequest, issue_and_send, link_sent, mail, read_body};
use auth::{AuthSession, NewPassword, Purpose, Verification};

/// The scope of owner accounts in `owners.sqlite`. App accounts use
/// `deployment:<id>`.
pub(crate) const CREATOR_SCOPE: &str = "creator";

pub fn routes() -> Router<Konae> {
    Router::new()
        .route("/_konae/owner/verify", post(creator_verify))
        .route("/_konae/owner/reset", post(creator_reset))
        .route("/_konae/owner/recover", post(creator_recover))
}

/// Opens the creator account store in this installation's mode.
pub(crate) fn creator_store(state: &Konae) -> Result<AuthService, AuthError> {
    AuthService::open_in(
        state.data_root.join("owners.sqlite"),
        CREATOR_SCOPE,
        state.owner_verification,
    )
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


/// Starts a creator signup on an installation that requires verification.
/// The admission, an invitation code or the installation key, is checked and
/// consumed in the transaction that creates the pending creator. `page` is
/// the sign-in page the link opens. `client` feeds the mail limits.
pub(crate) async fn signup_creator(
    state: &Konae,
    email: &str,
    page: &str,
    admission: super::admission::Admission,
    client: std::net::IpAddr,
) -> Result<Option<String>, AuthError> {
    let email = auth::normalize_email(email)?;
    let mail = mail(state)?;
    let _guard = database_lock(state, "owner-auth").await;
    let auth = creator_store(state)?;
    let lookup = email.clone();
    let delivery = creator_delivery(&email, client, page);
    let issue = move |outbox: Option<&dyn auth::ChallengeOutbox>| auth.request_signup_with(&lookup, outbox, super::admission::admit(admission));
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


async fn complete_creator(state: &Konae, purpose: Purpose, token: &str, password: &str) -> Result<AuthSession, AuthError> {
    let password = password.to_owned();
    let chosen = blocking(move || NewPassword::prepare(&password)).await?;
    let _guard = database_lock(state, "owner-auth").await;
    let auth = creator_store(state)?;
    let token = token.to_owned();
    blocking(move || auth.complete_challenge(purpose, &token, chosen, |_| true)).await
}


async fn creator_verify(State(state): State<Konae>, request: Request) -> Response {
    creator_complete(state, request, Purpose::Verification).await
}


async fn creator_reset(State(state): State<Konae>, request: Request) -> Response {
    creator_complete(state, request, Purpose::Recovery).await
}


async fn creator_complete(state: Konae, request: Request, purpose: Purpose) -> Response {
    if !creator_host(&state, request.headers()).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    let answer: ChallengeAnswer = match read_body(&state, request, "owner", |_| "account-link".into()).await {
        Ok((answer, _)) => answer,
        Err(response) => return response,
    };
    match complete_creator(&state, purpose, &answer.token, &answer.password).await {
        Ok(session) => super::mcp::owner_session_response(&state, session),
        Err(error_value) => auth_error(error_value),
    }
}


async fn creator_recover(State(state): State<Konae>, request: Request) -> Response {
    if !creator_host(&state, request.headers()).await {
        return error(StatusCode::FORBIDDEN, "Owner authentication is unavailable on this origin.");
    }
    let origin = super::runtime::origin(&state, request.headers());
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
async fn creator_host(state: &Konae, headers: &HeaderMap) -> bool {
    super::oauth::valid_owner_host(state, headers).await && matches!(super::clerk::ClerkConfig::from_env(), Ok(None))
}


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
    use super::super::accounts::tests::{APP_HOST, Fixture, PASSWORD, app_answer, app_signup, cookie_of, fixture, host_headers, request, rpc_json};

    /// An app's link does nothing as an owner link.
    #[tokio::test]
    async fn an_app_link_does_nothing_as_an_owner_link() {
        let fixture = fixture(Verification::Required, true);
        app_signup(&fixture, APP_HOST, "person@example.test").await;
        let (_, token) = fixture.last_link();
        let creator = creator_verify(
            State(Konae::for_test(fixture.state.clone())),
            request("POST", "localhost", "/_konae/owner/verify", json!({"token": token, "password": PASSWORD})),
        )
        .await;
        assert_eq!(creator.status(), StatusCode::GONE);
        assert_eq!(fixture.app_state_of(APP_HOST, "person@example.test").await, "pending");
    }

    async fn owner(fixture: &Fixture, path: &str, body: Value) -> Response {
        let request = request("POST", "localhost", path, body);
        match path {
            "/_konae/owner/signup" => mcp::owner_signup(State(Konae::for_test(fixture.state.clone())), request).await,
            "/_konae/owner/login" => mcp::owner_login(State(Konae::for_test(fixture.state.clone())), request).await,
            "/_konae/owner/verify" => creator_verify(State(Konae::for_test(fixture.state.clone())), request).await,
            "/_konae/owner/reset" => creator_reset(State(Konae::for_test(fixture.state.clone())), request).await,
            "/_konae/owner/recover" => creator_recover(State(Konae::for_test(fixture.state.clone())), request).await,
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
        let session = mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(Some(&cookie), None)).unwrap().unwrap();
        // The editor's first check answers 200 either way, so a signed-out visit logs no error.
        let signed_out = mcp::owner_session(State(Konae::for_test(fixture.state.clone())), owner_headers(None, None)).await;
        assert_eq!(signed_out.status(), StatusCode::OK);
        assert_eq!(rpc_json(signed_out).await, json!({"signedIn": false}));
        let signed_in = mcp::owner_session(State(Konae::for_test(fixture.state.clone())), owner_headers(Some(&cookie), None)).await;
        assert_eq!(signed_in.status(), StatusCode::OK);
        assert_eq!(rpc_json(signed_in).await, json!({"signedIn": true, "ownerId": session.id, "displayName": "creator@example.test"}));
        assert_eq!(owner(&fixture, "/_konae/owner/login", login.clone()).await.status(), StatusCode::OK);

        // A delegated MCP credential for this creator.
        let bearer = "creator-bearer-token";
        mcp::store(&Konae::for_test(fixture.state.clone())).unwrap().execute(
            "INSERT INTO credentials(hash, owner, scopes, projects, expires) VALUES (?1, ?2, '[\"read\"]', '[]', ?3)",
            rusqlite::params![crate::digest(bearer.as_bytes()), session.id, now_ms() + 60_000],
        ).unwrap();
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(None, Some(bearer))).unwrap().is_some());

        // A credential that survives suspension is still refused, because
        // every use reads the creator's state.
        creator_store(&Konae::for_test(fixture.state.clone())).unwrap().suspend("creator@example.test").unwrap();
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(None, Some(bearer))).is_err());
        creator_store(&Konae::for_test(fixture.state.clone())).unwrap().resume("creator@example.test").unwrap();
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(None, Some(bearer))).unwrap().is_some());

        // The operator suspends the creator from the server.
        let message = run_operator_command(&fixture.root, true, "Creator@Example.test").unwrap();
        assert!(message.contains(&session.id));
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(Some(&cookie), None)).unwrap().is_none());
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(None, Some(bearer))).unwrap().is_none());
        let credentials: i64 = mcp::store(&Konae::for_test(fixture.state.clone())).unwrap()
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
        assert!(mcp::authenticate_owner(&Konae::for_test(fixture.state.clone()), &owner_headers(Some(&first), None)).unwrap().is_none());
        assert_eq!(owner(&fixture, "/_konae/owner/reset", json!({"token": token, "password": "third password"})).await.status(), StatusCode::GONE);
    }

}
