//! "Continue with Google" for the app accounts of a Deployment.
//!
//! The installation registers one Google OAuth client, and Google accepts
//! only redirect URIs registered exactly, so every Deployment shares one
//! callback on the Kernel's own host:
//!
//! app host `/_littlebox/auth/google/start` → Google →
//! Kernel host `/_littlebox/auth/google/callback` →
//! app host `/_littlebox/auth/google/finish?code=…`
//!
//! `start` records the flow (its `state`, the ID token's `nonce`, the PKCE
//! verifier, the Deployment and the app origin) and gives the browser a
//! cookie on the app host that names it. The callback never sets an app
//! cookie: it exchanges Google's code, checks the ID token and stores a
//! single-use finish code bound to the Deployment, the app origin and that
//! browser cookie, valid for 60 seconds. `finish` on the app host consumes
//! the code and opens the app's own session, after the Signup Policy has
//! decided in the transaction that creates or links the account.
//!
//! Google's signing keys are fetched only from the fixed JWKS URL in the
//! configuration, never from a URL a token names, cached by the answer's
//! Cache-Control and fetched again once when a token names an unknown key.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, bail, ensure};
use axum::extract::Query;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use rusqlite::params;
use sha2::{Digest, Sha256};

use super::*;
use auth::{IdentityIntent, IdentityOutcome, VerifiedIdentity};

/// A flow between start and callback lives this long.
const FLOW_TTL_SECONDS: i64 = 10 * 60;
/// A finish code lives this long.
const CODE_TTL_SECONDS: i64 = 60;
const MAX_JWKS_BYTES: usize = 64 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const CALLBACK_PATH: &str = "/_littlebox/auth/google/callback";
const START_PATH: &str = "/_littlebox/auth/google/start";
const FINISH_PATH: &str = "/_littlebox/auth/google/finish";

/// The refusal for an installation without a Google client.
pub(crate) const UNAVAILABLE: &str = "This installation has no Google sign-in.";
/// The refusal for a Deployment whose owner has not turned Google on.
const NOT_ON: &str = "Google sign-in is not turned on for this app.";
const FAILED: &str = "Google sign-in failed. Start again from the sign-in page.";
const UNVERIFIED: &str = "Google has not verified this email address, so it cannot sign in here.";
const CHECKS: &str = "Google's answer did not pass its checks. Start again from the sign-in page.";
const STALE: &str = "This Google sign-in expired or was already used. Start again from the sign-in page.";
const OTHER_BROWSER: &str = "This Google sign-in started in another browser. Start again from the sign-in page.";

/// Google's fixed endpoints. Only a development build may replace them, with
/// `LITTLEBOX_GOOGLE_TEST_ISSUER` (see `GoogleConfig::from_env`).
const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const JWKS: &str = "https://www.googleapis.com/oauth2/v3/certs";
/// Google signs ID tokens with either issuer value.
const ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

#[derive(Clone, Debug)]
struct Endpoints {
    authorize: String,
    token: String,
    jwks: String,
    /// The `iss` values a token may carry.
    issuers: Vec<String>,
    /// The issuer stored with a linked identity.
    issuer: String,
}

impl Endpoints {
    fn google() -> Self {
        Self {
            authorize: AUTHORIZE.into(),
            token: TOKEN.into(),
            jwks: JWKS.into(),
            issuers: ISSUERS.iter().map(|issuer| (*issuer).to_owned()).collect(),
            issuer: ISSUERS[0].into(),
        }
    }

    /// A local stand-in for Google at `base`, for development and tests.
    fn fake(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            authorize: format!("{base}/authorize"),
            token: format!("{base}/token"),
            jwks: format!("{base}/jwks"),
            issuers: vec![base.to_owned()],
            issuer: base.to_owned(),
        }
    }
}

/// The installation's Google OAuth client.
pub(crate) struct GoogleConfig {
    client_id: String,
    client_secret: String,
    /// The Kernel's public origin, which the callback answers on.
    kernel_origin: String,
    endpoints: Endpoints,
    http: reqwest::Client,
    keys: Mutex<Option<KeyCache>>,
}

struct KeyCache {
    keys: Vec<Jwk>,
    fresh_until: Instant,
}

#[derive(Clone, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "use")]
    usage: Option<String>,
    n: String,
    e: String,
}

impl GoogleConfig {
    /// Reads `LITTLEBOX_GOOGLE_CLIENT_ID`, `LITTLEBOX_GOOGLE_CLIENT_SECRET`
    /// and `LITTLEBOX_PUBLIC_ORIGIN`. None when neither Google setting is
    /// set; a partial setting refuses to start. `LITTLEBOX_GOOGLE_TEST_ISSUER`
    /// points the endpoints at a local fake issuer. A release build refuses
    /// to start with it set, and a development build reads it only for a
    /// local HTTP public origin.
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let id = std::env::var("LITTLEBOX_GOOGLE_CLIENT_ID").ok().filter(|v| !v.is_empty());
        let secret = std::env::var("LITTLEBOX_GOOGLE_CLIENT_SECRET").ok().filter(|v| !v.is_empty());
        let fake = std::env::var("LITTLEBOX_GOOGLE_TEST_ISSUER").ok().filter(|v| !v.is_empty());
        if fake.is_some() && !cfg!(debug_assertions) {
            bail!("LITTLEBOX_GOOGLE_TEST_ISSUER works only in a development build. Unset it.");
        }
        let (id, secret) = match (id, secret) {
            (None, None) => return Ok(None),
            (Some(id), Some(secret)) => (id, secret),
            _ => bail!("Set both LITTLEBOX_GOOGLE_CLIENT_ID and LITTLEBOX_GOOGLE_CLIENT_SECRET, or neither."),
        };
        let origin = std::env::var("LITTLEBOX_PUBLIC_ORIGIN")
            .context("Google sign-in needs LITTLEBOX_PUBLIC_ORIGIN, the origin its callback is registered on.")?;
        let endpoints = match fake {
            Some(base) => {
                ensure!(
                    local_http(&origin) && local_http(&base),
                    "LITTLEBOX_GOOGLE_TEST_ISSUER needs a local HTTP public origin and a local HTTP issuer."
                );
                Endpoints::fake(&base)
            }
            None => Endpoints::google(),
        };
        Self::new(id, secret, &origin, endpoints).map(Some)
    }

    fn new(client_id: String, client_secret: String, origin: &str, endpoints: Endpoints) -> Result<Self> {
        ensure!(
            !client_id.is_empty() && client_id.len() <= 256 && client_id.bytes().all(|b| b.is_ascii_graphic()),
            "LITTLEBOX_GOOGLE_CLIENT_ID is not a client ID."
        );
        ensure!(client_secret.len() <= 256, "LITTLEBOX_GOOGLE_CLIENT_SECRET is too long.");
        let parsed = url::Url::parse(origin).context("LITTLEBOX_PUBLIC_ORIGIN is not a URL.")?;
        let kernel_origin = parsed.origin().ascii_serialization();
        ensure!(
            (parsed.scheme() == "https" || local_http(origin))
                && kernel_origin == origin.trim_end_matches('/'),
            "Google sign-in needs LITTLEBOX_PUBLIC_ORIGIN to be an HTTPS origin, or a local HTTP one."
        );
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { client_id, client_secret, kernel_origin, endpoints, http, keys: Mutex::new(None) })
    }

    /// A configuration against a fake issuer at `base`, for tests.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, kernel_origin: &str) -> Self {
        Self::new("test-client".into(), "test-secret".into(), kernel_origin, Endpoints::fake(base)).unwrap()
    }

    fn redirect_uri(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.kernel_origin)
    }

    /// Checks an ID token: its RS256 signature by a key from the JWKS URL,
    /// Google as issuer, this client as audience, an unexpired time, the
    /// nonce the flow set, and a verified email.
    async fn verify(&self, token: &str, nonce: &str, now: i64) -> std::result::Result<VerifiedIdentity, &'static str> {
        let parts: Vec<&str> = token.split('.').collect();
        if token.len() > 16384 || parts.len() != 3 {
            return Err(CHECKS);
        }
        #[derive(Deserialize)]
        struct Header {
            alg: String,
            kid: String,
            #[serde(default)]
            crit: Vec<String>,
        }
        let header: Header = clerk::base64url(parts[0])
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or(CHECKS)?;
        if header.alg != "RS256" || !header.crit.is_empty() {
            return Err(CHECKS);
        }
        let key = self.key(&header.kid).await.ok_or(CHECKS)?;
        let (n, e, signature) = match (clerk::base64url(&key.n), clerk::base64url(&key.e), clerk::base64url(parts[2])) {
            (Ok(n), Ok(e), Ok(signature)) => (n, e, signature),
            _ => return Err(CHECKS),
        };
        RsaPublicKeyComponents { n, e }
            .verify(&RSA_PKCS1_2048_8192_SHA256, format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .map_err(|_| CHECKS)?;
        let claims: Value = clerk::base64url(parts[1])
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or(CHECKS)?;
        let issuer = claims["iss"].as_str().unwrap_or("");
        let audience_ok = match &claims["aud"] {
            Value::String(aud) => *aud == self.client_id,
            Value::Array(list) => {
                list.iter().any(|aud| aud.as_str() == Some(self.client_id.as_str()))
                    && claims["azp"].as_str() == Some(self.client_id.as_str())
            }
            _ => false,
        };
        let exp = claims["exp"].as_i64().unwrap_or(0);
        let iat = claims["iat"].as_i64().unwrap_or(i64::MAX);
        let subject = claims["sub"].as_str().unwrap_or("");
        if !self.endpoints.issuers.iter().any(|allowed| allowed == issuer)
            || !audience_ok
            || exp <= now
            || iat > now + 300
            || claims["nonce"].as_str() != Some(nonce)
            || subject.is_empty()
            || subject.len() > 255
            || !subject.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(CHECKS);
        }
        // Google documents a boolean; its samples show the string "true".
        let verified = claims["email_verified"] == Value::Bool(true) || claims["email_verified"] == "true";
        let email = claims["email"].as_str().unwrap_or("");
        if !verified || auth::normalize_email(email).is_err() {
            return Err(UNVERIFIED);
        }
        Ok(VerifiedIdentity {
            issuer: self.endpoints.issuer.clone(),
            subject: subject.to_owned(),
            email: email.to_owned(),
        })
    }

    /// The key a token names: from the cache while it is fresh, else from
    /// one fetch of the JWKS URL.
    async fn key(&self, kid: &str) -> Option<Jwk> {
        {
            let cache = self.keys.lock().unwrap();
            if let Some(cache) = cache.as_ref().filter(|cache| cache.fresh_until > Instant::now()) {
                if let Some(key) = cache.keys.iter().find(|key| key.kid == kid) {
                    return Some(key.clone());
                }
            }
        }
        let fetched = self.fetch_keys().await.ok()?;
        let key = fetched.keys.iter().find(|key| key.kid == kid).cloned();
        *self.keys.lock().unwrap() = Some(fetched);
        key
    }

    async fn fetch_keys(&self) -> Result<KeyCache> {
        let response = self.http.get(&self.endpoints.jwks).send().await?;
        ensure!(response.status().is_success(), "Google keys answered {}.", response.status());
        let max_age = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value.split(',').find_map(|part| part.trim().strip_prefix("max-age=")?.parse::<u64>().ok())
            })
            .unwrap_or(3600)
            .clamp(60, 24 * 3600);
        let bytes = read_capped(response, MAX_JWKS_BYTES).await?;
        #[derive(Deserialize)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        let keys: Vec<Jwk> = serde_json::from_slice::<Jwks>(&bytes)?
            .keys
            .into_iter()
            .filter(|key| {
                key.kty == "RSA"
                    && key.alg.as_deref().is_none_or(|alg| alg == "RS256")
                    && key.usage.as_deref().is_none_or(|usage| usage == "sig")
            })
            .take(16)
            .collect();
        Ok(KeyCache { keys, fresh_until: Instant::now() + Duration::from_secs(max_age) })
    }

    /// Exchanges an authorization code for its ID token.
    async fn exchange(&self, code: &str, verifier: &str) -> Result<String> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", self.redirect_uri().as_str()),
                ("code_verifier", verifier),
            ])
            .send()
            .await?;
        ensure!(response.status().is_success(), "Google's token endpoint answered {}.", response.status());
        let body: Value = serde_json::from_slice(&read_capped(response, MAX_TOKEN_RESPONSE_BYTES).await?)?;
        body["id_token"].as_str().map(str::to_owned).context("Google answered without an ID token.")
    }
}

async fn read_capped(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        bytes.extend_from_slice(&chunk);
        ensure!(bytes.len() <= cap, "The answer exceeds {cap} bytes.");
    }
    Ok(bytes)
}

fn local_http(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str().is_some_and(|host| host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host.ends_with(".localhost"))
    })
}

/// Whether this Deployment's sign-in page offers Google: the owner turned it
/// on and the installation has a client.
pub(crate) fn offered(state: &AppState, deployment: &Deployment) -> bool {
    state.accounts.google.is_some() && deployment.sign_in_methods.contains(&SignInMethod::Google)
}

// ---------------------------------------------------------------------------
// Flow store

fn open(state: &AppState) -> Result<Connection> {
    let db = Connection::open(state.data_root.join("google.sqlite"))?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS google_flows (
           state_hash TEXT PRIMARY KEY,
           browser_hash TEXT NOT NULL,
           deployment_id TEXT NOT NULL,
           app_origin TEXT NOT NULL,
           nonce TEXT NOT NULL,
           verifier TEXT NOT NULL,
           intent TEXT NOT NULL CHECK (intent IN ('signin', 'connect', 'invitation')),
           intent_value TEXT NOT NULL DEFAULT '',
           expires_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS google_codes (
           code_hash TEXT PRIMARY KEY,
           browser_hash TEXT NOT NULL,
           deployment_id TEXT NOT NULL,
           app_origin TEXT NOT NULL,
           intent TEXT NOT NULL,
           intent_value TEXT NOT NULL,
           issuer TEXT NOT NULL,
           subject TEXT NOT NULL,
           email TEXT NOT NULL,
           expires_at INTEGER NOT NULL,
           consumed_at INTEGER
         );",
    )?;
    Ok(db)
}

fn random_hex() -> String {
    let mut bytes = [0_u8; 32];
    SystemRandom::new().fill(&mut bytes).expect("system randomness");
    hex::encode(bytes)
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn base64url(bytes: &[u8]) -> String {
    encode_base64(bytes).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

/// The origin the browser used for this app host.
fn app_origin(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = if state.public_origin.starts_with("https://") { "https" } else { "http" };
    let parsed = url::Url::parse(&format!("{scheme}://{host}")).ok()?;
    (parsed.path() == "/" && parsed.username().is_empty() && parsed.query().is_none())
        .then(|| parsed.origin().ascii_serialization())
}

fn cookie_name(state: &AppState) -> &'static str {
    if state.public_origin.starts_with("https://") { "__Host-littlebox_google" } else { "littlebox_google" }
}

fn browser_cookie(state: &AppState, value: &str, max_age: i64) -> HeaderValue {
    let name = cookie_name(state);
    let secure = if name.starts_with("__Host-") { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")).unwrap()
}

// ---------------------------------------------------------------------------
// Routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(START_PATH, get(start_get).post(start_post))
        .route(CALLBACK_PATH, get(callback))
        .route(FINISH_PATH, get(finish))
}

#[derive(Deserialize)]
struct StartQuery {
    #[serde(default)]
    intent: Option<String>,
}

/// Starts a sign-in, or with `?intent=connect` links Google to the signed-in
/// account. A navigation, so it answers with a redirect to Google.
async fn start_get(State(state): State<AppState>, Query(query): Query<StartQuery>, request: Request) -> Response {
    let headers = request.headers().clone();
    let peer = request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|info| info.0);
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    };
    let intent = match query.intent.as_deref() {
        None | Some("signin") => IdentityIntent::SignIn,
        Some("connect") => match authenticate_deployment(&state, &deployment, &headers) {
            Ok(Some(user)) => IdentityIntent::Connect { user_id: user.user_id },
            Ok(None) => return page(StatusCode::UNAUTHORIZED, "Sign in to this app first, then connect Google."),
            Err(response) => return response,
        },
        Some(_) => return page(StatusCode::BAD_REQUEST, FAILED),
    };
    match begin(&state, &deployment, &headers, peer, intent).await {
        Ok((location, cookie)) => {
            let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err((status, message)) => page(status, &message),
    }
}

#[derive(Deserialize)]
struct InvitationStart {
    invitation: String,
}

/// Starts an invitation answer with Google. The invitation page posts the
/// token from its fragment, so it never sits in a URL, and navigates to the
/// location this returns.
async fn start_post(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "Cross-origin auth request rejected.");
    }
    let peer = request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|info| info.0);
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Deployment not found.");
    };
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 4096).await else {
        return error(StatusCode::BAD_REQUEST, "Authentication request is too large.");
    };
    let Ok(input) = serde_json::from_slice::<InvitationStart>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "Send the invitation token.");
    };
    let intent = IdentityIntent::Invitation { token_hash: auth::hash_token(&input.invitation) };
    match begin(&state, &deployment, &headers, peer, intent).await {
        Ok((location, cookie)) => {
            let mut response = Json(json!({"result": {"location": location}})).into_response();
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err((status, message)) => error(status, &message),
    }
}

/// Records a flow and returns Google's authorization URL and the cookie
/// that ties the flow to this browser.
async fn begin(
    state: &AppState,
    deployment: &Deployment,
    headers: &HeaderMap,
    peer: Option<std::net::SocketAddr>,
    intent: IdentityIntent,
) -> std::result::Result<(String, HeaderValue), (StatusCode, String)> {
    let Some(config) = state.accounts.google.clone() else {
        return Err((StatusCode::NOT_FOUND, UNAVAILABLE.into()));
    };
    if !deployment.sign_in_methods.contains(&SignInMethod::Google) {
        return Err((StatusCode::NOT_FOUND, NOT_ON.into()));
    }
    let client = state.guards.client_ip(peer, headers);
    if let Err(wait) = state.guards.auth_attempt(&deployment.id, client, "google") {
        return Err((StatusCode::TOO_MANY_REQUESTS, format!("Too many sign-in attempts. Try again in {} seconds.", wait.as_secs().max(1))));
    }
    let origin = app_origin(state, headers).ok_or((StatusCode::BAD_REQUEST, FAILED.to_owned()))?;
    let (flow_state, browser, nonce, verifier) = (random_hex(), random_hex(), random_hex(), random_hex());
    let (intent_name, intent_value) = match &intent {
        IdentityIntent::SignIn => ("signin", String::new()),
        IdentityIntent::Connect { user_id } => ("connect", user_id.clone()),
        IdentityIntent::Invitation { token_hash } => ("invitation", token_hash.clone()),
    };
    let now = auth::now_seconds();
    let stored = (|| -> Result<()> {
        let db = open(state)?;
        db.execute("DELETE FROM google_flows WHERE expires_at <= ?1", [now])?;
        db.execute("DELETE FROM google_codes WHERE expires_at <= ?1", [now])?;
        db.execute(
            "INSERT INTO google_flows (state_hash, browser_hash, deployment_id, app_origin, nonce, verifier, intent, intent_value, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![digest(&flow_state), digest(&browser), deployment.id, origin, nonce, verifier, intent_name, intent_value, now + FLOW_TTL_SECONDS],
        )?;
        Ok(())
    })();
    if let Err(error_value) = stored {
        eprintln!("Google sign-in could not record a flow: {error_value}");
        return Err((StatusCode::INTERNAL_SERVER_ERROR, FAILED.into()));
    }
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    let mut location = url::Url::parse(&config.endpoints.authorize).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, FAILED.to_owned()))?;
    location
        .query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri())
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email")
        .append_pair("state", &flow_state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    Ok((location.into(), browser_cookie(state, &browser, FLOW_TTL_SECONDS)))
}

#[derive(Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Google's redirect, on the Kernel's own host. Exchanges the code, checks
/// the ID token and hands the app host a finish code. Sets no app cookie.
async fn callback(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    let Some(config) = state.accounts.google.clone() else {
        return page(StatusCode::NOT_FOUND, UNAVAILABLE);
    };
    if app_origin(&state, &headers).as_deref() != Some(config.kernel_origin.as_str()) {
        return page(StatusCode::NOT_FOUND, "Google sign-in answers only on the Kernel's own host.");
    }
    let Some(flow_state) = query.state.filter(|value| value.len() <= 128) else {
        return page(StatusCode::BAD_REQUEST, FAILED);
    };
    let now = auth::now_seconds();
    // One use: the flow row goes whatever happens next.
    let flow = (|| -> Result<Option<(String, String, String, String, String, String, String)>> {
        let db = open(&state)?;
        let row = db
            .query_row(
                "DELETE FROM google_flows WHERE state_hash = ?1 AND expires_at > ?2
                 RETURNING browser_hash, deployment_id, app_origin, nonce, verifier, intent, intent_value",
                params![digest(&flow_state), now],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )
            .optional()?;
        Ok(row)
    })();
    let Ok(Some((browser_hash, deployment_id, origin, nonce, verifier, intent, intent_value))) = flow else {
        return page(StatusCode::GONE, STALE);
    };
    let back = format!("{origin}/_littlebox/login");
    if query.error.is_some() {
        return page_with_link(StatusCode::FORBIDDEN, "Google sign-in was cancelled.", &back);
    }
    let Some(code) = query.code.filter(|code| !code.is_empty() && code.len() <= 2048) else {
        return page_with_link(StatusCode::BAD_REQUEST, FAILED, &back);
    };
    let token = match config.exchange(&code, &verifier).await {
        Ok(token) => token,
        Err(error_value) => {
            eprintln!("Google sign-in exchange failed: {error_value}");
            return page_with_link(StatusCode::BAD_GATEWAY, FAILED, &back);
        }
    };
    let identity = match config.verify(&token, &nonce, now).await {
        Ok(identity) => identity,
        Err(message) => return page_with_link(StatusCode::FORBIDDEN, message, &back),
    };
    let finish_code = random_hex();
    let stored = open(&state).and_then(|db| {
        db.execute(
            "INSERT INTO google_codes (code_hash, browser_hash, deployment_id, app_origin, intent, intent_value, issuer, subject, email, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![digest(&finish_code), browser_hash, deployment_id, origin, intent, intent_value, identity.issuer, identity.subject, identity.email, now + CODE_TTL_SECONDS],
        )?;
        Ok(())
    });
    if stored.is_err() {
        return page_with_link(StatusCode::INTERNAL_SERVER_ERROR, FAILED, &back);
    }
    let location = format!("{origin}{FINISH_PATH}?code={finish_code}");
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

#[derive(Deserialize)]
struct FinishQuery {
    #[serde(default)]
    code: String,
}

/// A finish code that passed its checks.
struct Finished {
    intent: IdentityIntent,
    identity: VerifiedIdentity,
}

/// Consumes a finish code on the Deployment and origin it was issued for,
/// in the browser that started the flow. A code that fails a check stays
/// unused, so a request on another host cannot burn it.
fn consume(db: &Connection, code: &str, deployment_id: &str, origin: &str, browser: Option<&str>, now: i64) -> std::result::Result<Finished, &'static str> {
    let row: Option<(String, String, String, String, String, String, String, String, i64, Option<i64>)> = db
        .query_row(
            "SELECT browser_hash, deployment_id, app_origin, intent, intent_value, issuer, subject, email, expires_at, consumed_at
             FROM google_codes WHERE code_hash = ?1",
            [digest(code)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((browser_hash, code_deployment, code_origin, intent, intent_value, issuer, subject, email, expires_at, consumed_at)) = row else {
        return Err(STALE);
    };
    if consumed_at.is_some() || expires_at <= now || code_deployment != deployment_id || code_origin != origin {
        return Err(STALE);
    }
    if browser.map(digest).as_deref() != Some(browser_hash.as_str()) {
        return Err(OTHER_BROWSER);
    }
    let taken = db
        .execute("UPDATE google_codes SET consumed_at = ?1 WHERE code_hash = ?2 AND consumed_at IS NULL", params![now, digest(code)])
        .map_err(|_| FAILED)?;
    if taken != 1 {
        return Err(STALE);
    }
    let intent = match intent.as_str() {
        "connect" => IdentityIntent::Connect { user_id: intent_value },
        "invitation" => IdentityIntent::Invitation { token_hash: intent_value },
        _ => IdentityIntent::SignIn,
    };
    Ok(Finished { intent, identity: VerifiedIdentity { issuer, subject, email } })
}

/// The app host's end of the flow. Arrives as a cross-site navigation from
/// Google's redirect chain, so it does not ask for same-origin headers; the
/// single-use code and this browser's flow cookie are its proof.
async fn finish(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<FinishQuery>) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    };
    let mut response = finish_in(&state, &deployment, &headers, &query.code).await;
    response.headers_mut().append(header::SET_COOKIE, browser_cookie(&state, "", 0));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

async fn finish_in(state: &AppState, deployment: &Deployment, headers: &HeaderMap, code: &str) -> Response {
    if !offered(state, deployment) {
        return login_link_page(StatusCode::NOT_FOUND, if state.accounts.google.is_none() { UNAVAILABLE } else { NOT_ON });
    }
    let Some(origin) = app_origin(state, headers) else {
        return login_link_page(StatusCode::BAD_REQUEST, FAILED);
    };
    let browser = cookie_value(headers, cookie_name(state));
    let finished = open(state)
        .map_err(|_| FAILED)
        .and_then(|db| consume(&db, code, &deployment.id, &origin, browser.as_deref(), auth::now_seconds()));
    let finished = match finished {
        Ok(finished) => finished,
        Err(message) => return login_link_page(StatusCode::GONE, message),
    };
    if let IdentityIntent::Connect { user_id } = &finished.intent {
        // The account that started the connection must still be the one
        // signed in on this browser.
        match authenticate_deployment(state, deployment, headers) {
            Ok(Some(user)) if &user.user_id == user_id => {}
            Ok(_) => return login_link_page(StatusCode::UNAUTHORIZED, "Sign in to this app first, then connect Google."),
            Err(response) => return response,
        }
    }
    match sign_in(state, deployment, finished).await {
        Ok((Some(session), outcome)) => {
            let operation = if outcome == IdentityOutcome::Created { usage::Operation::Signup } else { usage::Operation::Login };
            usage::count(&state.data_root, &deployment.id, operation);
            let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
            response.headers_mut().insert(header::SET_COOKIE, accounts::app_session_cookie(state, &session));
            response
        }
        Ok((None, _)) => (StatusCode::SEE_OTHER, [(header::LOCATION, "/_littlebox/account")]).into_response(),
        Err(error_value) => {
            let status = match &error_value {
                AuthError::Storage(_) | AuthError::PasswordHash => StatusCode::INTERNAL_SERVER_ERROR,
                AuthError::InvalidChallenge => StatusCode::GONE,
                AuthError::EmailHasAccount | AuthError::IdentityTaken | AuthError::AccountExists => StatusCode::CONFLICT,
                _ => StatusCode::FORBIDDEN,
            };
            login_link_page(status, &error_value.to_string())
        }
    }
}

/// Runs the account decision under the identity lock, with the Deployment's
/// current Signup Policy read inside the transaction, as the password path does.
async fn sign_in(
    state: &AppState,
    deployment: &Deployment,
    finished: Finished,
) -> std::result::Result<(Option<auth::AuthSession>, IdentityOutcome), AuthError> {
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let auth = invitations::store(state, deployment)?;
    let deployments = state.deployments.clone();
    let (new_id, invited_id) = (deployment.id.clone(), deployment.id.clone());
    let deployments_too = deployments.clone();
    let signed = blocking(move || {
        auth.sign_in_with_identity(
            &finished.identity,
            &finished.intent,
            |email| {
                deployments
                    .blocking_read()
                    .values()
                    .find(|current| current.id == new_id)
                    .is_some_and(|current| current.signup_policy.allows(email))
            },
            |email| {
                deployments_too
                    .blocking_read()
                    .values()
                    .find(|current| current.id == invited_id)
                    .is_some_and(|current| invitations::admits_invitation(&current.signup_policy, email))
            },
            invitations::mark_accepted,
        )
    })
    .await?;
    Ok((signed.session, signed.outcome))
}

// ---------------------------------------------------------------------------
// Pages

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const PAGE_HEAD: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Sign in with Google</title><style>*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:#f4f5f7;color:#20232a;font:16px system-ui,sans-serif}main{width:min(100%,380px);padding:32px;background:white;border:1px solid #e1e4e8;border-radius:12px;box-shadow:0 12px 36px #1f293714}h1{margin:0 0 8px;font-size:24px}p{color:#586174;line-height:1.5}@media (prefers-color-scheme:dark){body{background:#14161a;color:#e8eaed}main{background:#1d2026;border-color:#2e333b;box-shadow:none}p{color:#a9b0bc}a{color:#8ab4ff}}</style><link rel="icon" href="data:,"><main><h1>Sign in with Google</h1><p role="alert">"#;

fn html(status: StatusCode, body: String) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn page(status: StatusCode, message: &str) -> Response {
    html(status, format!("{PAGE_HEAD}{}</p></main></html>", escape(message)))
}

fn page_with_link(status: StatusCode, message: &str, link: &str) -> Response {
    html(status, format!("{PAGE_HEAD}{}</p><p><a href=\"{}\">Back to sign in</a></p></main></html>", escape(message), escape(link)))
}

fn login_link_page(status: StatusCode, message: &str) -> Response {
    page_with_link(status, message, "/_littlebox/login")
}

/// The sign-in page's Google button, as a link: a navigation is not bound by
/// the page's form-action or connect-src.
pub(crate) const BUTTON: &str = r#"<a class="google" href="/_littlebox/auth/google/start">Continue with Google</a>"#;
pub(crate) const BUTTON_STYLE: &str = ".google{display:block;text-align:center;text-decoration:none;font:inherit;border-radius:8px;padding:11px 12px;border:1px solid #c9ced6;color:#20232a;margin-top:16px}@media (prefers-color-scheme:dark){.google{color:#e8eaed;border-color:#3a404a}}";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use accounts::AccountSettings;
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};

    const KERNEL: &str = "http://localhost:8790";
    const APP: &str = "practice.localhost:8790";
    const OTHER: &str = "other.localhost:8790";

    /// What the fake issuer hands out for one authorization code.
    #[derive(Clone)]
    struct Grant {
        claims: Value,
        challenge: String,
        kid: &'static str,
    }

    #[derive(Clone, Default)]
    struct Fake {
        grants: Arc<Mutex<HashMap<String, Grant>>>,
        key_fetches: Arc<AtomicUsize>,
    }

    fn encode(bytes: &[u8]) -> String {
        base64url(bytes)
    }

    fn signed(kid: &str, claims: &Value) -> String {
        let key = RsaKeyPair::from_pkcs8(include_bytes!("../test-fixtures/clerk/test-key.pk8")).unwrap();
        let message = format!(
            "{}.{}",
            encode(json!({"alg": "RS256", "kid": kid}).to_string().as_bytes()),
            encode(claims.to_string().as_bytes())
        );
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), message.as_bytes(), &mut signature).unwrap();
        format!("{message}.{}", encode(&signature))
    }

    /// A local stand-in for Google's token and JWKS endpoints. The token
    /// endpoint checks the client secret, the redirect URI and PKCE as
    /// Google does, and answers once per code.
    async fn fake_google() -> (Fake, String) {
        let fake = Fake::default();
        let jwks_fake = fake.clone();
        let token_fake = fake.clone();
        let router = Router::new()
            .route(
                "/jwks",
                get(move || {
                    let fake = jwks_fake.clone();
                    async move {
                        fake.key_fetches.fetch_add(1, Ordering::SeqCst);
                        ([(header::CACHE_CONTROL, "public, max-age=300")], include_str!("../test-fixtures/clerk/jwks.json"))
                    }
                }),
            )
            .route(
                "/token",
                post(move |body: String| {
                    let fake = token_fake.clone();
                    async move {
                        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
                        let grant = fake.grants.lock().unwrap().remove(form.get("code").map(String::as_str).unwrap_or(""));
                        let Some(grant) = grant else {
                            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"})));
                        };
                        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                        if form.get("client_secret").map(String::as_str) != Some("test-secret")
                            || form.get("redirect_uri").map(String::as_str) != Some(&format!("{KERNEL}{CALLBACK_PATH}"))
                            || base64url(&Sha256::digest(verifier.as_bytes())) != grant.challenge
                        {
                            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_request"})));
                        }
                        (StatusCode::OK, Json(json!({"id_token": signed(grant.kid, &grant.claims)})))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (fake, base)
    }

    struct Fixture {
        state: AppState,
        fake: Fake,
        base: String,
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn fixture(mode: SignupMode) -> Fixture {
        let (fake, base) = fake_google().await;
        let root = std::env::temp_dir().join(format!("littlebox-google-{}", Uuid::new_v4()));
        let mut deployments = HashMap::new();
        for host in [APP, OTHER] {
            let hostname = host.split(':').next().unwrap();
            let item = Deployment {
                id: Uuid::new_v4().to_string(),
                project_id: Uuid::new_v4().to_string(),
                project_name: "Practice".into(),
                url: format!("http://{host}"),
                created_at: now_ms(),
                manifest: json!({"visibility": "private"}),
                hostname: hostname.into(),
                actions: "{}".into(),
                artifact_version: None,
                database_model: None,
                signup_policy: SignupPolicy::from_input(mode, &["@practice.example".into()]).unwrap(),
                sign_in_methods: vec![SignInMethod::Password, SignInMethod::Google],
            };
            std::fs::create_dir_all(root.join("deployments").join(&item.id)).unwrap();
            deployments.insert(hostname.to_owned(), item);
        }
        let state = AppState {
            deployments: Arc::new(RwLock::new(deployments)),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.clone(),
            copies: Default::default(),
            admin_key: "test-admin".into(),
            accounts: AccountSettings {
                verification: auth::Verification::Required,
                mail: None,
                queue: None,
                google: Some(Arc::new(GoogleConfig::for_test(&base, KERNEL))),
            },
            public_origin: KERNEL.into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        };
        Fixture { state, fake, base, root }
    }

    fn headers(host: &str, cookie: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        if let Some(cookie) = cookie {
            headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        }
        headers
    }

    fn location(response: &Response) -> String {
        response.headers().get(header::LOCATION).expect("a redirect").to_str().unwrap().to_owned()
    }

    fn set_cookies(response: &Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().split(';').next().unwrap().to_owned())
            .collect()
    }

    async fn text(response: Response) -> String {
        String::from_utf8(axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap()
    }

    /// The Google person a test signs in as, and how their token differs
    /// from a good one.
    struct Person {
        subject: &'static str,
        email: &'static str,
    }

    const ANA: Person = Person { subject: "100000000000000000001", email: "ana@practice.example" };
    const BEN: Person = Person { subject: "100000000000000000002", email: "ben@elsewhere.example" };

    impl Fixture {
        async fn id(&self, host: &str) -> String {
            self.state.deployments.read().await.get(host.split(':').next().unwrap()).unwrap().id.clone()
        }

        fn identity(&self, host_id: &str) -> Connection {
            Connection::open(self.root.join("deployments").join(host_id).join("identity.sqlite")).unwrap()
        }

        /// Starts a flow on `host` and returns the browser cookie and the
        /// authorization URL's query.
        async fn start(&self, host: &str, intent: Option<&str>, session: Option<&str>) -> (String, HashMap<String, String>) {
            let request = Request::builder()
                .uri(START_PATH)
                .header(header::HOST, host)
                .header(header::COOKIE, session.unwrap_or(""))
                .body(Body::empty())
                .unwrap();
            let query = StartQuery { intent: intent.map(str::to_owned) };
            let response = start_get(State(self.state.clone()), Query(query), request).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{}", text(response).await);
            let url = url::Url::parse(&location(&response)).unwrap();
            assert!(url.as_str().starts_with(&format!("{}/authorize?", self.base)));
            let cookie = set_cookies(&response).into_iter().find(|c| c.starts_with("littlebox_google=")).unwrap();
            (cookie, url.query_pairs().into_owned().collect())
        }

        /// Plays Google: grants a code for `person` with the claims `edit`
        /// leaves, then follows the redirect to the Kernel's callback.
        async fn google(&self, query: &HashMap<String, String>, person: &Person, kid: &'static str, edit: impl FnOnce(&mut Value)) -> Response {
            assert_eq!(query["redirect_uri"], format!("{KERNEL}{CALLBACK_PATH}"));
            let now = auth::now_seconds();
            let mut claims = json!({
                "iss": self.base, "aud": "test-client", "azp": "test-client", "sub": person.subject,
                "email": person.email, "email_verified": true, "nonce": query["nonce"], "iat": now, "exp": now + 3600,
            });
            edit(&mut claims);
            let code = format!("code-{}", Uuid::new_v4());
            self.fake.grants.lock().unwrap().insert(code.clone(), Grant { claims, challenge: query["code_challenge"].clone(), kid });
            let callback_query = CallbackQuery { code: Some(code), state: Some(query["state"].clone()), error: None };
            callback(State(self.state.clone()), headers("localhost:8790", None), Query(callback_query)).await
        }

        async fn finish(&self, host: &str, cookie: &str, finish_url: &str) -> Response {
            let code = url::Url::parse(finish_url).unwrap().query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
            finish(State(self.state.clone()), headers(host, Some(cookie)), Query(FinishQuery { code })).await
        }

        /// One whole sign-in. Returns the finish response.
        async fn sign_in(&self, host: &str, person: &Person) -> Response {
            let (cookie, query) = self.start(host, None, None).await;
            let callback = self.google(&query, person, "local-test", |_| {}).await;
            assert_eq!(callback.status(), StatusCode::SEE_OTHER, "{}", text(callback).await);
            self.finish(host, &cookie, &location(&callback)).await
        }

        async fn session_user(&self, host: &str, response: &Response) -> Value {
            let cookie = set_cookies(response).into_iter().find(|c| c.starts_with("littlebox_app_session=")).expect("a session cookie");
            let response = session(State(self.state.clone()), headers(host, Some(&cookie))).await;
            serde_json::from_str::<Value>(&text(response).await).unwrap()["result"].clone()
        }

        async fn password_account(&self, host: &str, email: &str) -> String {
            let deployment = self.state.deployments.read().await.get(host.split(':').next().unwrap()).cloned().unwrap();
            let store = AuthService::open_in(
                self.root.join("deployments").join(&deployment.id).join("identity.sqlite"),
                &auth::deployment_scope(&deployment.id),
                auth::Verification::Local,
            )
            .unwrap();
            let session = store.create_account(auth::NewAccount::prepare(email, "correct horse battery").unwrap()).unwrap();
            // A password account on an installation that requires verification proved its email.
            self.identity(&deployment.id).execute("UPDATE app_users SET verified_at = 1 WHERE email = ?1", [email]).unwrap();
            format!("littlebox_app_session={}", session.token)
        }
    }

    #[tokio::test]
    async fn a_person_signs_in_with_google_and_gets_one_stable_account() {
        let fixture = fixture(SignupMode::Open).await;
        let first = fixture.sign_in(APP, &ANA).await;
        assert_eq!(first.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&first), "/");
        let user = fixture.session_user(APP, &first).await;
        assert_eq!(user["name"], ANA.email);
        assert_eq!(user["role"], "member");
        assert_eq!(user["methods"], json!(["google"]));
        let again = fixture.sign_in(APP, &ANA).await;
        assert_eq!(fixture.session_user(APP, &again).await["userId"], user["userId"]);
        // The account has a password hash nobody chose: no password opens it.
        let id = fixture.id(APP).await;
        let (verified, hash): (Option<i64>, String) = fixture
            .identity(&id)
            .query_row("SELECT verified_at, password_hash FROM app_users", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert!(verified.is_some());
        assert!(hash.starts_with("pbkdf2-sha256$600000$") && hash != auth::NO_PASSWORD);
        // Linked identities stay on their Deployment.
        let other = fixture.id(OTHER).await;
        assert_eq!(fixture.identity(&other).query_row("SELECT COUNT(*) FROM app_users", [], |row| row.get::<_, i64>(0)).unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn a_forged_expired_foreign_mismatched_or_unverified_token_is_refused() {
        let mut fixture = fixture(SignupMode::Open).await;
        let cases: Vec<(&str, &'static str, Box<dyn FnOnce(&mut Value)>)> = vec![
            ("unknown key", "someone-else", Box::new(|_| {})),
            ("expired", "local-test", Box::new(|c| c["exp"] = json!(auth::now_seconds() - 1))),
            ("wrong audience", "local-test", Box::new(|c| c["aud"] = json!("another-client"))),
            ("wrong issuer", "local-test", Box::new(|c| c["iss"] = json!("https://issuer.example"))),
            ("wrong nonce", "local-test", Box::new(|c| c["nonce"] = json!("not-the-nonce"))),
            ("unverified email", "local-test", Box::new(|c| c["email_verified"] = json!(false))),
        ];
        for (case, kid, edit) in cases {
            // Each case is a new person's first try, not one client's sixth.
            fixture.state.guards = Arc::new(limits::Guards::new(Vec::new()));
            let (_, query) = fixture.start(APP, None, None).await;
            let response = fixture.google(&query, &ANA, kid, edit).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{case}");
            let body = text(response).await;
            let expected = if case == "unverified email" { UNVERIFIED } else { CHECKS };
            assert!(body.contains(&escape(expected)), "{case}: {body}");
        }
        // A token whose payload was changed after signing fails the signature.
        let (_, query) = fixture.start(APP, None, None).await;
        let token = signed("local-test", &json!({"iss": fixture.base, "aud": "test-client", "sub": "1", "email": ANA.email, "email_verified": true, "nonce": query["nonce"], "iat": 0, "exp": i64::MAX}));
        let parts: Vec<&str> = token.split('.').collect();
        let forged = format!("{}.{}.{}", parts[0], encode(br#"{"iss":"x"}"#), parts[2]);
        let config = fixture.state.accounts.google.clone().unwrap();
        assert_eq!(config.verify(&forged, &query["nonce"], auth::now_seconds()).await, Err(CHECKS));
        assert!(config.verify(&token, &query["nonce"], auth::now_seconds()).await.is_ok());
        // Keys are cached; an unknown kid fetched them once more per attempt.
        assert!(fixture.fake.key_fetches.load(Ordering::SeqCst) <= 3);
        let id = fixture.id(APP).await;
        assert_eq!(fixture.identity(&id).query_row("SELECT COUNT(*) FROM app_users", [], |row| row.get::<_, i64>(0)).unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn the_string_true_counts_as_a_verified_email() {
        let fixture = fixture(SignupMode::Open).await;
        let (cookie, query) = fixture.start(APP, None, None).await;
        let callback = fixture.google(&query, &ANA, "local-test", |c| c["email_verified"] = json!("true")).await;
        assert_eq!(fixture.finish(APP, &cookie, &location(&callback)).await.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn a_finish_code_works_once_only_on_its_deployment_origin_and_browser_and_only_for_60_seconds() {
        let fixture = fixture(SignupMode::Open).await;
        let (cookie, query) = fixture.start(APP, None, None).await;
        let callback = fixture.google(&query, &ANA, "local-test", |_| {}).await;
        let finish_url = location(&callback);
        assert!(finish_url.starts_with("http://practice.localhost:8790/_littlebox/auth/google/finish?code="));
        assert!(set_cookies(&callback).is_empty(), "the callback sets no cookie");
        // Another Deployment, another origin of this one, or another browser: refused, and the code stays.
        let other = fixture.finish(OTHER, &cookie, &finish_url).await;
        assert_eq!(other.status(), StatusCode::GONE);
        assert!(text(other).await.contains(&escape(STALE)));
        let port = fixture.finish("practice.localhost:9999", &cookie, &finish_url).await;
        assert_eq!(port.status(), StatusCode::GONE);
        let stranger = fixture.finish(APP, "littlebox_google=someone-else", &finish_url).await;
        assert!(text(stranger).await.contains(&escape(OTHER_BROWSER)));
        // The right one works once.
        assert_eq!(fixture.finish(APP, &cookie, &finish_url).await.status(), StatusCode::SEE_OTHER);
        let replay = fixture.finish(APP, &cookie, &finish_url).await;
        assert_eq!(replay.status(), StatusCode::GONE);
        // A code past 60 seconds is refused.
        let (cookie, query) = fixture.start(APP, None, None).await;
        let callback = fixture.google(&query, &ANA, "local-test", |_| {}).await;
        let db = open(&fixture.state).unwrap();
        let expires: i64 = db.query_row("SELECT MAX(expires_at) FROM google_codes", [], |row| row.get(0)).unwrap();
        assert!(expires <= auth::now_seconds() + CODE_TTL_SECONDS);
        db.execute("UPDATE google_codes SET expires_at = ?1 WHERE consumed_at IS NULL", [auth::now_seconds() - 1]).unwrap();
        assert_eq!(fixture.finish(APP, &cookie, &location(&callback)).await.status(), StatusCode::GONE);
        // A state answers once: Google's redirect replayed finds no flow.
        let replayed = callback_again(&fixture, &query).await;
        assert_eq!(replayed.status(), StatusCode::GONE);
    }

    async fn callback_again(fixture: &Fixture, query: &HashMap<String, String>) -> Response {
        let callback_query = CallbackQuery { code: Some("any".into()), state: Some(query["state"].clone()), error: None };
        callback(State(fixture.state.clone()), headers("localhost:8790", None), Query(callback_query)).await
    }

    #[tokio::test]
    async fn the_callback_answers_only_on_the_kernel_host() {
        let fixture = fixture(SignupMode::Open).await;
        let (_, query) = fixture.start(APP, None, None).await;
        let callback_query = CallbackQuery { code: Some("x".into()), state: Some(query["state"].clone()), error: None };
        let response = callback(State(fixture.state.clone()), headers(APP, None), Query(callback_query)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn each_signup_policy_admits_and_refuses_with_the_password_paths_words() {
        // Open creates; Allowlist admits listed emails only; Closed and Invitation only create nobody.
        let open = fixture(SignupMode::Open).await;
        assert_eq!(open.sign_in(APP, &BEN).await.status(), StatusCode::SEE_OTHER);
        let list = fixture(SignupMode::Allowlist).await;
        assert_eq!(list.sign_in(APP, &ANA).await.status(), StatusCode::SEE_OTHER);
        for mode in [SignupMode::Allowlist, SignupMode::Closed, SignupMode::Invitation] {
            let fixture = fixture(mode).await;
            let refused = fixture.sign_in(APP, &BEN).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{mode:?}");
            assert!(text(refused).await.contains(SIGNUP_REFUSED), "{mode:?}");
        }
        // Closed still admits an account that is already linked.
        let closed = fixture(SignupMode::Open).await;
        assert_eq!(closed.sign_in(APP, &BEN).await.status(), StatusCode::SEE_OTHER);
        for item in closed.state.deployments.write().await.values_mut() {
            item.signup_policy = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        }
        assert_eq!(closed.sign_in(APP, &BEN).await.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn an_email_with_an_account_is_refused_until_that_account_connects_google() {
        let fixture = fixture(SignupMode::Open).await;
        let password_session = fixture.password_account(APP, ANA.email).await;
        let refused = fixture.sign_in(APP, &ANA).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert!(text(refused).await.contains("An account with this email exists. Sign in with your password, then connect Google."));
        // Signed in with the password, the person connects Google.
        let (cookie, query) = fixture.start(APP, Some("connect"), Some(&password_session)).await;
        let callback = fixture.google(&query, &ANA, "local-test", |_| {}).await;
        let both = format!("{cookie}; {password_session}");
        let connected = fixture.finish(APP, &both, &location(&callback)).await;
        assert_eq!(location(&connected), "/_littlebox/account");
        let google = fixture.sign_in(APP, &ANA).await;
        let user = fixture.session_user(APP, &google).await;
        assert_eq!(user["methods"], json!(["password", "google"]));
        // The same Google account cannot be connected to a second account.
        let ben_session = fixture.password_account(APP, BEN.email).await;
        let (cookie, query) = fixture.start(APP, Some("connect"), Some(&ben_session)).await;
        let callback = fixture.google(&query, &ANA, "local-test", |_| {}).await;
        let taken = fixture.finish(APP, &format!("{cookie}; {ben_session}"), &location(&callback)).await;
        assert_eq!(taken.status(), StatusCode::CONFLICT);
        // Connecting needs the session that started it.
        let (cookie, query) = fixture.start(APP, Some("connect"), Some(&ben_session)).await;
        let callback = fixture.google(&query, &BEN, "local-test", |_| {}).await;
        assert_eq!(fixture.finish(APP, &cookie, &location(&callback)).await.status(), StatusCode::UNAUTHORIZED);
    }

    /// Creates a pending account and its invitation as `app_invitations_create`
    /// does, and returns the token.
    async fn invite(fixture: &Fixture, email: &str) -> String {
        let deployment = fixture.state.deployments.read().await.get(APP.split(':').next().unwrap()).cloned().unwrap();
        let store = invitations::store(&fixture.state, &deployment).unwrap();
        let user_id = Uuid::new_v4().to_string();
        store
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO app_users (id, email, password_hash, role, created_at, state) VALUES (?1, ?2, ?3, 'member', 0, 'pending')",
                    params![user_id, email, auth::NO_PASSWORD],
                )?;
                let issued = store.issue_challenge(tx, &user_id, email, auth::Purpose::Invitation)?;
                tx.execute(
                    "INSERT INTO app_invitations (id, email, deployment, inviter, user_id, created_user, state, token_hash, created_at, expires_at)
                     VALUES (?1, ?2, ?3, 'owner', ?4, 1, 'pending', ?5, 0, ?6)",
                    params![Uuid::new_v4().to_string(), email, deployment.id, user_id, auth::hash_token(&issued.token), issued.expires_at],
                )?;
                Ok(issued.token)
            })
            .unwrap()
    }

    async fn accept_with_google(fixture: &Fixture, token: &str, person: &Person) -> Response {
        let request = Request::builder()
            .method("POST")
            .uri(START_PATH)
            .header(header::HOST, APP)
            .body(Body::from(json!({"invitation": token}).to_string()))
            .unwrap();
        let started = start_post(State(fixture.state.clone()), request).await;
        assert_eq!(started.status(), StatusCode::OK);
        let cookie = set_cookies(&started).into_iter().next().unwrap();
        let body: Value = serde_json::from_str(&text(started).await).unwrap();
        let url = url::Url::parse(body["result"]["location"].as_str().unwrap()).unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let callback = fixture.google(&query, person, "local-test", |_| {}).await;
        fixture.finish(APP, &cookie, &location(&callback)).await
    }

    #[tokio::test]
    async fn an_invitation_accepted_with_google_needs_the_invited_email_and_activates_the_account() {
        let fixture = fixture(SignupMode::Invitation).await;
        let token = invite(&fixture, ANA.email).await;
        // The invited person must use the Google account of the invited email.
        let wrong = accept_with_google(&fixture, &token, &BEN).await;
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        assert!(text(wrong).await.contains("This invitation is for another email."));
        // Signing in from the sign-in page does not accept an invitation.
        let pending = fixture.sign_in(APP, &ANA).await;
        assert_eq!(pending.status(), StatusCode::FORBIDDEN);
        let accepted = accept_with_google(&fixture, &token, &ANA).await;
        assert_eq!(accepted.status(), StatusCode::SEE_OTHER, "{}", text(accepted).await);
        let id = fixture.id(APP).await;
        let (state, invitation): (String, String) = fixture
            .identity(&id)
            .query_row(
                "SELECT u.state, i.state FROM app_users u JOIN app_invitations i ON i.user_id = u.id WHERE u.email = ?1",
                [ANA.email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), invitation.as_str()), ("active", "accepted"));
        // The link worked once; later sign-ins go through the linked identity.
        assert_eq!(accept_with_google(&fixture, &token, &ANA).await.status(), StatusCode::CONFLICT);
        assert_eq!(fixture.sign_in(APP, &ANA).await.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn a_deployment_without_google_turned_on_or_an_installation_without_a_client_offers_none() {
        let mut fixture = fixture(SignupMode::Open).await;
        for item in fixture.state.deployments.write().await.values_mut() {
            item.sign_in_methods = vec![SignInMethod::Password];
        }
        let request = Request::builder().uri(START_PATH).header(header::HOST, APP).body(Body::empty()).unwrap();
        let response = start_get(State(fixture.state.clone()), Query(StartQuery { intent: None }), request).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(text(response).await.contains(NOT_ON));
        for item in fixture.state.deployments.write().await.values_mut() {
            item.sign_in_methods = vec![SignInMethod::Password, SignInMethod::Google];
        }
        fixture.state.accounts.google = None;
        let deployment = fixture.state.deployments.read().await.values().next().cloned().unwrap();
        assert!(!offered(&fixture.state, &deployment));
        let request = Request::builder().uri(START_PATH).header(header::HOST, APP).body(Body::empty()).unwrap();
        let response = start_get(State(fixture.state.clone()), Query(StartQuery { intent: None }), request).await;
        assert!(text(response).await.contains(UNAVAILABLE));
    }

    #[test]
    fn the_test_issuer_is_refused_outside_local_http() {
        assert!(local_http("http://localhost:8790") && local_http("http://127.0.0.1:9") && local_http("http://app.localhost"));
        assert!(!local_http("https://localhost") && !local_http("http://example.com") && !local_http("http://localhost.example.com"));
        assert!(GoogleConfig::new("id".into(), "s".into(), "http://example.com", Endpoints::google()).is_err());
        assert!(GoogleConfig::new("id".into(), "s".into(), "https://kernel.example.com/path", Endpoints::google()).is_err());
        let config = GoogleConfig::new("id".into(), "s".into(), "https://kernel.example.com", Endpoints::google()).unwrap();
        assert_eq!(config.redirect_uri(), "https://kernel.example.com/_littlebox/auth/google/callback");
        assert_eq!(config.endpoints.jwks, "https://www.googleapis.com/oauth2/v3/certs");
    }
}
