use crate::*;

pub async fn session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if !modules::is_on(&deployment.modules, "auth") {
        return error(StatusCode::NOT_FOUND, "Authentication is not enabled for this app.");
    }
    match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(Some(user)) => Json(json!({"result": user})).into_response(),
        Ok(None) => Json(
            json!({"result": {"userId": "anonymous", "name": "Anonymous", "role": "anonymous"}}),
        )
        .into_response(),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
pub struct AuthInput {
    pub email: String,


    #[serde(default)]
    pub password: String,
}

pub async fn signup(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, true).await
}

pub async fn login(State(state): State<AppState>, headers: HeaderMap, request: Request) -> Response {
    auth_request(&state, &headers, request, false).await
}

pub async fn auth_request(
    state: &AppState,
    headers: &HeaderMap,
    request: Request,
    signup: bool,
) -> Response {
    if !same_origin(headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(state, headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    if !modules::is_on(&deployment.modules, "auth") {
        return error(StatusCode::NOT_FOUND, "Authentication is not enabled for this app.");
    }
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    let bytes = match axum::body::to_bytes(request.into_body(), 4096).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Authentication request is too large.",
            );
        }
    };
    let input: AuthInput = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Authentication request must include email and password.",
            );
        }
    };
    let client = state.guards.client_ip(peer, headers);
    if let Err(wait) = state.guards.auth_attempt(&deployment.id, client, &input.email) {
        return too_many_attempts(wait);
    }
    if signup && state.accounts.verification == auth::Verification::Required {
        return match accounts::signup_app(state, &deployment, &input.email, client).await {
            Ok(()) => {
                let mut response = accounts::link_sent();
                if let Some(cookie) = accounts::signup_hint_cookie(state, &deployment, &input.email) {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                }
                response
            }
            Err(error_value) => auth_error(error_value),
        };
    }
    if signup {


        if let Err(error_value) = auth::normalize_email(&input.email)
            .and_then(|_| auth::validate_password(&input.password))
        {
            return auth_error(error_value);
        }
        if !deployment.signup_policy.allows(&input.email) {
            return error(StatusCode::FORBIDDEN, SIGNUP_REFUSED);
        }
    }



    let identity = format!("identity:{}", deployment.id);
    let (email, password) = (input.email, input.password);
    let failed_email = email.clone();
    let result = if signup {
        let requested = email.clone();
        match blocking(move || auth::NewAccount::prepare(&email, &password)).await {
            Ok(account) => {
                let _identity_guard = database_lock(state, &identity).await;


                let allowed = state
                    .deployments
                    .read()
                    .await
                    .values()
                    .find(|current| current.id == deployment.id)
                    .map(|current| current.signup_policy.allows(&requested));
                match allowed {
                    None => return error(StatusCode::NOT_FOUND, "Deployment not found."),
                    Some(false) => return error(StatusCode::FORBIDDEN, SIGNUP_REFUSED),
                    Some(true) => {}
                }
                let auth = accounts::app_store(state, &deployment);
                blocking(move || auth?.create_account(account)).await
            }
            Err(error_value) => Err(error_value),
        }
    } else {
        let lookup = {
            let _identity_guard = database_lock(state, &identity).await;
            let (auth, email, password) = (accounts::app_store(state, &deployment), email.clone(), password.clone());
            blocking(move || auth?.find_login(&email, &password)).await
        };
        match lookup {
            Ok(stored) => match blocking(move || auth::check_password(&password, stored)).await {
                Ok(user) => {
                    let _identity_guard = database_lock(state, &identity).await;
                    let auth = accounts::app_store(state, &deployment);
                    blocking(move || auth?.start_session(user)).await
                }
                Err(error_value) => Err(error_value),
            },
            Err(error_value) => Err(error_value),
        }
    };
    if !signup && matches!(result, Err(AuthError::InvalidCredentials)) {
        state.guards.auth_failed(&deployment.id, client, &failed_email);
        if accounts::signup_hint_applies(state, &deployment, headers, &failed_email) {
            return error(StatusCode::UNAUTHORIZED, accounts::signup_hint(state));
        }
    }
    match result {
        Ok(session) => {
            let operation = if signup { usage::Operation::Signup } else { usage::Operation::Login };
            usage::count(&state.data_root, &deployment.id, operation);
            let cookie_name = app_cookie_name(state, &deployment.id, headers);
            accounts::app_session_response(&cookie_name, session)
        }


        Err(AuthError::AccountPending) => {
            let invited = accounts::invitation_pending(state, &deployment, &failed_email).await;
            error(StatusCode::FORBIDDEN, &accounts::pending_message(state, invited))
        }
        Err(error_value) => auth_error(error_value),
    }
}

/// Runs blocking authentication work off the async runtime.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AuthError> + Send + 'static,
) -> Result<T, AuthError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(AuthError::PasswordHash))
}

/// The answer when a login or signup exceeds its rate limit.
pub fn too_many_attempts(wait: std::time::Duration) -> Response {
    let mut response = error(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many sign-in attempts. Try again later.",
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
    response
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let token = headers.get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).map(str::to_owned)
        .or_else(|| cookie_value(&headers, &app_cookie_name(&state, &deployment.id, &headers)));
    let _identity_guard = database_lock(&state, &format!("identity:{}", deployment.id)).await;
    let auth = match open_auth(&state, &deployment) {
        Ok(auth) => auth,
        Err(error_value) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
        }
    };
    if let Err(error_value) = auth.logout(token.as_deref()) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &error_value.to_string());
    }
    let mut response = Json(json!({"result": {"ok": true}})).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cleared_app_cookie(&state, &deployment.id, &headers));
    response
}

/// The `Set-Cookie` value that removes the app session cookie from the browser.
pub fn cleared_app_cookie(state: &AppState, deployment_id: &str, headers: &HeaderMap) -> HeaderValue {
    let name = app_cookie_name(state, deployment_id, headers);
    HeaderValue::from_str(&format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}", cookie_secure(&name)))
        .unwrap()
}

/// `; Secure` for a name with a prefix that requires it.
pub fn cookie_secure(name: &str) -> &'static str {
    if name.starts_with("__Host-") || name.starts_with("__Secure-") { "; Secure" } else { "" }
}

pub fn auth_error(error_value: AuthError) -> Response {
    let status = match error_value {
        AuthError::InvalidEmail | AuthError::WeakPassword => StatusCode::BAD_REQUEST,
        AuthError::EmailTaken | AuthError::AccountExists => StatusCode::CONFLICT,
        AuthError::InvitedEmailDiffers => StatusCode::FORBIDDEN,
        AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::AccountPending
        | AuthError::AccountSuspended
        | AuthError::SignupClosed
        | AuthError::AdmissionRefused(_) => {
            StatusCode::FORBIDDEN
        }
        AuthError::InvalidChallenge => StatusCode::GONE,
        AuthError::MailUnavailable | AuthError::MailBlocked(_) => StatusCode::SERVICE_UNAVAILABLE,
        AuthError::MailLimit(_) => StatusCode::TOO_MANY_REQUESTS,
        AuthError::VerificationRequired
        | AuthError::Storage(_)
        | AuthError::PasswordHash => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &error_value.to_string())
}
