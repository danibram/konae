//! "Continue with Google" for the app accounts of a Deployment.
//!
//! Each app has its own Google OAuth client: the owner sets its client ID
//! and secret as the project secrets `google_client_id` and
//! `google_client_secret`, and registers this app's own callback,
//! `https://<app host>/_littlebox/auth/google/callback`, in the Google
//! console, once for the generated host and once for a verified custom
//! domain. The whole flow stays on the app's host:
//!
//! `/_littlebox/auth/google/start` → Google → `/_littlebox/auth/google/callback`
//!
//! `start` records the flow (its `state`, the ID token's `nonce`, the PKCE
//! verifier, the Deployment and the app origin) and gives the browser a
//! cookie that names it. The callback takes the flow once, only on the same
//! Deployment, origin and browser, exchanges Google's code with the app's
//! client, checks the ID token and opens the app's session.
//!
//! The account is the one that holds the email Google vouched for. Google
//! vouches only for an address it is the authority for (`google_vouches_for`).
//! Signup Policy decides whether an email without an account gets one, in
//! the transaction that creates it.
//!
//! Google's signing keys are fetched only from the fixed JWKS URL, never from
//! a URL a token names, cached by the answer's Cache-Control and fetched again
//! when a token names an unknown key, at most once per `REFETCH_COOLDOWN`, so
//! a stream of fresh flows cannot make the Kernel call Google each time. The
//! keys are Google's, not a client's, so the installation keeps one cache for
//! every app.

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
const MAX_JWKS_BYTES: usize = 64 * 1024;
/// A token that names a key the fresh cache lacks fetches the keys again at
/// most this often. Google publishes a new key well before it signs with it.
const REFETCH_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
pub(crate) const CALLBACK_PATH: &str = "/_littlebox/auth/google/callback";
const START_PATH: &str = "/_littlebox/auth/google/start";

/// The project secrets that hold the app's Google client.
pub(crate) const CLIENT_ID: &str = "google_client_id";
pub(crate) const CLIENT_SECRET: &str = "google_client_secret";

/// The refusal for a Deployment whose owner has not turned Google on, or
/// whose Google client is not set on this installation.
pub(crate) const NOT_ON: &str = "Google sign-in is not turned on for this app.";
/// `sign_in_methods_set` refuses google with this while either secret is unset.
pub(crate) const NEEDS_CLIENT: &str = "Set this app's Google client ID and client secret before turning Google on.";
pub(crate) const NAMES_RESERVED: &str = "google_client_id and google_client_secret hold this app's Google client. Set them with use google.";
pub(crate) const NAMES_ONLY: &str = "A secret with use google is named google_client_id or google_client_secret.";
pub(crate) const ID_LOOKS_LIKE_SECRET: &str = "This looks like the client secret. The client ID ends with .apps.googleusercontent.com.";
const FAILED: &str = "Google sign-in failed. Start again from the sign-in page.";
const UNVERIFIED: &str = "Google has not verified this email address, so it cannot sign in here.";
pub(crate) const NOT_AUTHORITATIVE: &str =
    "This Google account uses an email address Google does not manage, so it cannot sign in here. Sign in with email and password.";
const CHECKS: &str = "Google's answer did not pass its checks. Start again from the sign-in page.";
const STALE: &str = "This Google sign-in expired or was already used. Start again from the sign-in page.";
const OTHER_BROWSER: &str = "This Google sign-in started in another browser. Start again from the sign-in page.";

/// `secret_delete` refuses a Google secret with this while Google is on.
pub(crate) fn still_on(name: &str) -> String {
    format!("Google sign-in is on for this app and needs {name}. Turn Google off before deleting it.")
}

/// Google's fixed endpoints. Only a development build may replace them, with
/// `LITTLEBOX_GOOGLE_TEST_ISSUER` (see `Google::from_env`).
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
}

impl Endpoints {
    fn google() -> Self {
        Self {
            authorize: AUTHORIZE.into(),
            token: TOKEN.into(),
            jwks: JWKS.into(),
            issuers: ISSUERS.iter().map(|issuer| (*issuer).to_owned()).collect(),
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
        }
    }
}

/// Where the installation reaches Google, and Google's signing keys.
pub(crate) struct Google {
    endpoints: Endpoints,
    http: reqwest::Client,
    keys: Mutex<Option<KeyCache>>,
}

/// One app's Google OAuth client, from its project secrets.
pub(crate) struct Client {
    id: String,
    secret: String,
}

struct KeyCache {
    keys: Vec<Jwk>,
    fetched_at: Instant,
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

impl Google {
    /// Google itself.
    pub(crate) fn real() -> Self {
        Self::with(Endpoints::google())
    }

    /// Google, unless `LITTLEBOX_GOOGLE_TEST_ISSUER` points the endpoints at
    /// a local fake issuer. A release build refuses to start with it set, and
    /// a development build reads it only with a local HTTP public origin.
    pub(crate) fn from_env(public_origin: &str) -> Result<Self> {
        for retired in ["LITTLEBOX_GOOGLE_CLIENT_ID", "LITTLEBOX_GOOGLE_CLIENT_SECRET"] {
            if std::env::var(retired).is_ok_and(|value| !value.is_empty()) {
                eprintln!("{retired} is no longer read. Each app's owner sets the app's own Google client in Auth > Users.");
            }
        }
        let Some(base) = std::env::var("LITTLEBOX_GOOGLE_TEST_ISSUER").ok().filter(|v| !v.is_empty()) else {
            return Ok(Self::real());
        };
        if !cfg!(debug_assertions) {
            bail!("LITTLEBOX_GOOGLE_TEST_ISSUER works only in a development build. Unset it.");
        }
        ensure!(
            local_http(public_origin) && local_http(&base),
            "LITTLEBOX_GOOGLE_TEST_ISSUER needs a local HTTP public origin and a local HTTP issuer."
        );
        Ok(Self::with(Endpoints::fake(&base)))
    }

    fn with(endpoints: Endpoints) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("an HTTP client");
        Self { endpoints, http, keys: Mutex::new(None) }
    }

    /// Endpoints at a fake issuer at `base`, for tests.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str) -> Self {
        Self::with(Endpoints::fake(base))
    }

    /// Checks an ID token: its RS256 signature by a key from the JWKS URL,
    /// Google as issuer, this app's client as audience, an unexpired time,
    /// the nonce the flow set, and an email Google verified and vouches for.
    async fn verify(&self, token: &str, client_id: &str, nonce: &str, now: i64) -> std::result::Result<VerifiedIdentity, &'static str> {
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
            Value::String(aud) => aud == client_id,
            Value::Array(list) => {
                list.iter().any(|aud| aud.as_str() == Some(client_id)) && claims["azp"].as_str() == Some(client_id)
            }
            _ => false,
        };
        let exp = claims["exp"].as_i64().unwrap_or(0);
        let iat = claims["iat"].as_i64().unwrap_or(i64::MAX);
        if !self.endpoints.issuers.iter().any(|allowed| allowed == issuer)
            || !audience_ok
            || exp <= now
            || iat > now + 300
            || claims["nonce"].as_str() != Some(nonce)
        {
            return Err(CHECKS);
        }
        // Google documents a boolean; its samples show the string "true".
        let verified = claims["email_verified"] == Value::Bool(true) || claims["email_verified"] == "true";
        let email = claims["email"].as_str().unwrap_or("");
        if !verified || auth::normalize_email(email).is_err() {
            return Err(UNVERIFIED);
        }
        if !google_vouches_for(email, claims["hd"].as_str()) {
            return Err(NOT_AUTHORITATIVE);
        }
        Ok(VerifiedIdentity { email: email.to_owned() })
    }

    /// The key a token names: from the cache while it is fresh, else from
    /// one fetch of the JWKS URL. An unknown key on a fresh cache fetches
    /// again only when the last fetch is older than `REFETCH_COOLDOWN`.
    async fn key(&self, kid: &str) -> Option<Jwk> {
        {
            let cache = self.keys.lock().unwrap();
            if let Some(cache) = cache.as_ref().filter(|cache| cache.fresh_until > Instant::now()) {
                if let Some(key) = cache.keys.iter().find(|key| key.kid == kid) {
                    return Some(key.clone());
                }
                if cache.fetched_at.elapsed() < REFETCH_COOLDOWN {
                    return None;
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
        let fetched_at = Instant::now();
        Ok(KeyCache { keys, fetched_at, fresh_until: fetched_at + Duration::from_secs(max_age) })
    }

    /// Exchanges an authorization code for its ID token with the app's client.
    async fn exchange(&self, client: &Client, code: &str, verifier: &str, redirect_uri: &str) -> Result<String> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", client.id.as_str()),
                ("client_secret", client.secret.as_str()),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
            ])
            .send()
            .await?;
        ensure!(response.status().is_success(), "Google's token endpoint answered {}.", response.status());
        let body: Value = serde_json::from_slice(&read_capped(response, MAX_TOKEN_RESPONSE_BYTES).await?)?;
        body["id_token"].as_str().map(str::to_owned).context("Google answered without an ID token.")
    }
}

/// Whether Google is the authority for this address, so its word proves who
/// holds it. Google manages every `@gmail.com` address, and a Workspace
/// domain names itself in the `hd` claim. A Google account made with an
/// outside address only proves that someone once read mail there, and
/// Google's own docs warn that `email` alone is not a unique, permanent
/// identifier, so such an address does not sign in to an account.
fn google_vouches_for(email: &str, hosted_domain: Option<&str>) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else { return false };
    domain.eq_ignore_ascii_case("gmail.com") || hosted_domain.is_some_and(|hd| hd.eq_ignore_ascii_case(domain))
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

/// The app's Google client, when the owner set both its ID and its secret.
fn client(state: &AppState, deployment: &Deployment) -> Option<Client> {
    match client_state(state, deployment) {
        (Some(id), Some(secret)) => Some(Client { id, secret }),
        _ => None,
    }
}

/// The app's client ID and secret, each when set. Values stay in the Kernel.
fn client_state(state: &AppState, deployment: &Deployment) -> (Option<String>, Option<String>) {
    mcp::store(state)
        .and_then(|db| http::google_client(&db, &deployment.project_id))
        .unwrap_or((None, None))
}

/// Whether the app's client ID and client secret are each set.
pub(crate) fn client_set(state: &AppState, deployment: &Deployment) -> (bool, bool) {
    let (id, secret) = client_state(state, deployment);
    (id.is_some(), secret.is_some())
}

/// Whether this Deployment's sign-in page offers Google: the owner turned it
/// on and set the app's client on this installation.
pub(crate) fn offered(state: &AppState, deployment: &Deployment) -> bool {
    deployment.sign_in_methods.contains(&SignInMethod::Google) && client(state, deployment).is_some()
}

/// The redirect URIs the owner registers in the Google console: the
/// generated host's callback and, once verified, the custom domain's. The
/// start step sends one of these and no other.
pub(crate) fn redirect_uris(state: &AppState, deployment: &Deployment) -> Vec<String> {
    let mut hosts = vec![deployment.hostname.clone()];
    if let Some(record) = domain::read_record(state, &deployment.id).filter(|record| record.verified) {
        hosts.push(record.hostname);
    }
    hosts.into_iter().map(|host| format!("{}{CALLBACK_PATH}", deployment_url(&state.public_origin, &host))).collect()
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
           intent TEXT NOT NULL,
           intent_value TEXT NOT NULL DEFAULT '',
           expires_at INTEGER NOT NULL
         );
         DROP TABLE IF EXISTS google_codes;",
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
    Router::new().route(START_PATH, get(start_get).post(start_post)).route(CALLBACK_PATH, get(callback))
}

/// Starts a sign-in. A navigation, so it answers with a redirect to Google.
async fn start_get(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let peer = request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|info| info.0);
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    };
    match begin(&state, &deployment, &headers, peer, IdentityIntent::SignIn).await {
        Ok((location, cookie)) => {
            let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err((status, message)) => login_link_page(status, &message),
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
    let client = match client(state, deployment) {
        Some(client) if deployment.sign_in_methods.contains(&SignInMethod::Google) => client,
        _ => return Err((StatusCode::NOT_FOUND, NOT_ON.into())),
    };
    let ip = state.guards.client_ip(peer, headers);
    if let Err(wait) = state.guards.auth_attempt(&deployment.id, ip, "google") {
        return Err((StatusCode::TOO_MANY_REQUESTS, format!("Too many sign-in attempts. Try again in {} seconds.", wait.as_secs().max(1))));
    }
    // Google sends the person back only to a URI the owner registered, so
    // the flow starts only on a host whose callback is one of them.
    let origin = app_origin(state, headers).ok_or((StatusCode::BAD_REQUEST, FAILED.to_owned()))?;
    let redirect_uri = format!("{origin}{CALLBACK_PATH}");
    if !redirect_uris(state, deployment).contains(&redirect_uri) {
        return Err((StatusCode::BAD_REQUEST, FAILED.into()));
    }
    let (flow_state, browser, nonce, verifier) = (random_hex(), random_hex(), random_hex(), random_hex());
    let (intent_name, intent_value) = match &intent {
        IdentityIntent::SignIn => ("signin", String::new()),
        IdentityIntent::Invitation { token_hash } => ("invitation", token_hash.clone()),
    };
    let now = auth::now_seconds();
    let stored = (|| -> Result<()> {
        let db = open(state)?;
        db.execute("DELETE FROM google_flows WHERE expires_at <= ?1", [now])?;
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
    let mut location = url::Url::parse(&state.accounts.google.endpoints.authorize).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, FAILED.to_owned()))?;
    location
        .query_pairs_mut()
        .append_pair("client_id", &client.id)
        .append_pair("redirect_uri", &redirect_uri)
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

/// A flow the callback took.
struct Flow {
    nonce: String,
    verifier: String,
    intent: IdentityIntent,
}

/// Takes a flow once, on the Deployment and origin it started on, in the
/// browser that started it. A flow that fails a check stays, so a request
/// on another host cannot burn it.
fn take_flow(db: &Connection, flow_state: &str, deployment_id: &str, origin: &str, browser: Option<&str>, now: i64) -> std::result::Result<Flow, &'static str> {
    let row: Option<(String, String, String, String, String, String, String, i64)> = db
        .query_row(
            "SELECT browser_hash, deployment_id, app_origin, nonce, verifier, intent, intent_value, expires_at
             FROM google_flows WHERE state_hash = ?1",
            [digest(flow_state)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((browser_hash, flow_deployment, flow_origin, nonce, verifier, intent, intent_value, expires_at)) = row else {
        return Err(STALE);
    };
    if expires_at <= now || flow_deployment != deployment_id || flow_origin != origin {
        return Err(STALE);
    }
    if browser.map(digest).as_deref() != Some(browser_hash.as_str()) {
        return Err(OTHER_BROWSER);
    }
    let taken = db.execute("DELETE FROM google_flows WHERE state_hash = ?1", [digest(flow_state)]).map_err(|_| FAILED)?;
    if taken != 1 {
        return Err(STALE);
    }
    let intent = match intent.as_str() {
        "invitation" => IdentityIntent::Invitation { token_hash: intent_value },
        _ => IdentityIntent::SignIn,
    };
    Ok(Flow { nonce, verifier, intent })
}

/// Google's redirect, on the app's own host. Arrives as a cross-site
/// navigation, so it does not ask for same-origin headers; the single-use
/// state and this browser's flow cookie are its proof.
async fn callback(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    let Some(deployment) = deployment_for(&state, &headers).await else {
        return error(StatusCode::NOT_FOUND, "Littlebox deployment not found.");
    };
    let mut response = callback_in(&state, &deployment, &headers, query).await;
    response.headers_mut().append(header::SET_COOKIE, browser_cookie(&state, "", 0));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

async fn callback_in(state: &AppState, deployment: &Deployment, headers: &HeaderMap, query: CallbackQuery) -> Response {
    let Some(origin) = app_origin(state, headers) else {
        return login_link_page(StatusCode::BAD_REQUEST, FAILED);
    };
    let Some(flow_state) = query.state.filter(|value| value.len() <= 128) else {
        return login_link_page(StatusCode::BAD_REQUEST, FAILED);
    };
    let now = auth::now_seconds();
    let browser = cookie_value(headers, cookie_name(state));
    let flow = open(state)
        .map_err(|_| FAILED)
        .and_then(|db| take_flow(&db, &flow_state, &deployment.id, &origin, browser.as_deref(), now));
    let flow = match flow {
        Ok(flow) => flow,
        Err(message) => return login_link_page(StatusCode::GONE, message),
    };
    if query.error.is_some() {
        return login_link_page(StatusCode::FORBIDDEN, "Google sign-in was cancelled.");
    }
    let Some(code) = query.code.filter(|code| !code.is_empty() && code.len() <= 2048) else {
        return login_link_page(StatusCode::BAD_REQUEST, FAILED);
    };
    // The owner may have turned Google off or changed the client since start.
    let client = match client(state, deployment) {
        Some(client) if deployment.sign_in_methods.contains(&SignInMethod::Google) => client,
        _ => return login_link_page(StatusCode::NOT_FOUND, NOT_ON),
    };
    let google = state.accounts.google.clone();
    let token = match google.exchange(&client, &code, &flow.verifier, &format!("{origin}{CALLBACK_PATH}")).await {
        Ok(token) => token,
        Err(error_value) => {
            eprintln!("Google sign-in exchange failed: {error_value}");
            return login_link_page(StatusCode::BAD_GATEWAY, FAILED);
        }
    };
    let identity = match google.verify(&token, &client.id, &flow.nonce, now).await {
        Ok(identity) => identity,
        Err(message) => return login_link_page(StatusCode::FORBIDDEN, message),
    };
    match sign_in(state, deployment, flow.intent, identity).await {
        Ok(signed) => {
            let operation = if signed.outcome == IdentityOutcome::Created { usage::Operation::Signup } else { usage::Operation::Login };
            usage::count(&state.data_root, &deployment.id, operation);
            let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
            response.headers_mut().insert(header::SET_COOKIE, accounts::app_session_cookie(state, &signed.session));
            response
        }
        Err(error_value) => {
            let status = match &error_value {
                AuthError::Storage(_) | AuthError::PasswordHash => StatusCode::INTERNAL_SERVER_ERROR,
                AuthError::InvalidChallenge => StatusCode::GONE,
                AuthError::AccountExists => StatusCode::CONFLICT,
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
    intent: IdentityIntent,
    identity: VerifiedIdentity,
) -> std::result::Result<auth::IdentitySignIn, AuthError> {
    let _identity_guard = database_lock(state, &format!("identity:{}", deployment.id)).await;
    let auth = invitations::store(state, deployment)?;
    let deployments = state.deployments.clone();
    let (new_id, invited_id) = (deployment.id.clone(), deployment.id.clone());
    let deployments_too = deployments.clone();
    blocking(move || {
        auth.sign_in_with_identity(
            &identity,
            &intent,
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
    .await
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
    const APP_CLIENT: &str = "1001-practice.apps.googleusercontent.com";
    const OTHER_CLIENT: &str = "1002-other.apps.googleusercontent.com";
    const CLIENT_SECRET_VALUE: &str = "GOCSPX-test-secret-value";

    /// What the fake issuer hands out for one authorization code.
    #[derive(Clone)]
    struct Grant {
        claims: Value,
        challenge: String,
        redirect_uri: String,
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
                        if form.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET_VALUE)
                            || form.get("redirect_uri") != Some(&grant.redirect_uri)
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

    fn set_client(state: &AppState, project: &str, client_id: &str) {
        let db = mcp::store(state).unwrap();
        http::secrets_call(&db, project, None, "secret_set", &json!({"name": CLIENT_ID, "value": client_id, "use": "google"})).unwrap();
        http::secrets_call(&db, project, None, "secret_set", &json!({"name": CLIENT_SECRET, "value": CLIENT_SECRET_VALUE})).unwrap();
    }

    async fn fixture(mode: SignupMode) -> Fixture {
        let (fake, base) = fake_google().await;
        let root = std::env::temp_dir().join(format!("littlebox-google-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
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
            files: crate::file_store::Files::local(&root),
            copies: Default::default(),
            admin_key: "test-admin".into(),
            accounts: AccountSettings {
                verification: auth::Verification::Required,
                mail: None,
                queue: None,
                google: Arc::new(Google::for_test(&base)),
            },
            public_origin: KERNEL.into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        };
        let fixture = Fixture { state, fake, base, root };
        set_client(&fixture.state, &fixture.deployment(APP).await.project_id, APP_CLIENT);
        set_client(&fixture.state, &fixture.deployment(OTHER).await.project_id, OTHER_CLIENT);
        fixture
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

    /// The Google person a test signs in as.
    struct Person {
        subject: &'static str,
        email: &'static str,
        hd: Option<&'static str>,
    }

    /// A Gmail address: Google is its authority.
    const ANA: Person = Person { subject: "100000000000000000001", email: "ana@gmail.com", hd: None };
    /// A Workspace address: the `hd` claim names its domain.
    const BEA: Person = Person { subject: "100000000000000000002", email: "bea@practice.example", hd: Some("practice.example") };
    /// A Google account made with an outside address.
    const CARL: Person = Person { subject: "100000000000000000003", email: "carl@outside.example", hd: None };

    impl Fixture {
        async fn deployment(&self, host: &str) -> Deployment {
            self.state.deployments.read().await.get(host.split(':').next().unwrap()).cloned().unwrap()
        }

        fn identity(&self, deployment_id: &str) -> Connection {
            Connection::open(self.root.join("deployments").join(deployment_id).join("identity.sqlite")).unwrap()
        }

        /// Starts a flow on `host` and returns the browser cookie and the
        /// authorization URL's query.
        async fn start(&self, host: &str) -> (String, HashMap<String, String>) {
            let request = Request::builder().uri(START_PATH).header(header::HOST, host).body(Body::empty()).unwrap();
            let response = start_get(State(self.state.clone()), request).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{}", text(response).await);
            let url = url::Url::parse(&location(&response)).unwrap();
            assert!(url.as_str().starts_with(&format!("{}/authorize?", self.base)));
            let cookie = set_cookies(&response).into_iter().find(|c| c.starts_with("littlebox_google=")).unwrap();
            (cookie, url.query_pairs().into_owned().collect())
        }

        /// Plays Google: grants a code for `person` with the claims `edit`
        /// leaves, then follows the redirect to the callback on `host` in the
        /// browser that holds `cookie`.
        async fn google(&self, host: &str, cookie: &str, query: &HashMap<String, String>, person: &Person, kid: &'static str, edit: impl FnOnce(&mut Value)) -> Response {
            let now = auth::now_seconds();
            let mut claims = json!({
                "iss": self.base, "aud": query["client_id"], "azp": query["client_id"], "sub": person.subject,
                "email": person.email, "email_verified": true, "nonce": query["nonce"], "iat": now, "exp": now + 3600,
            });
            if let Some(hd) = person.hd {
                claims["hd"] = json!(hd);
            }
            edit(&mut claims);
            let code = format!("code-{}", Uuid::new_v4());
            let grant = Grant { claims, challenge: query["code_challenge"].clone(), redirect_uri: query["redirect_uri"].clone(), kid };
            self.fake.grants.lock().unwrap().insert(code.clone(), grant);
            let callback_query = CallbackQuery { code: Some(code), state: Some(query["state"].clone()), error: None };
            callback(State(self.state.clone()), headers(host, Some(cookie)), Query(callback_query)).await
        }

        /// One whole sign-in. Returns the callback's response.
        async fn sign_in(&self, host: &str, person: &Person) -> Response {
            let (cookie, query) = self.start(host).await;
            self.google(host, &cookie, &query, person, "local-test", |_| {}).await
        }

        async fn session_user(&self, host: &str, response: &Response) -> Value {
            let cookie = set_cookies(response).into_iter().find(|c| c.starts_with("littlebox_app_session=")).expect("a session cookie");
            let response = session(State(self.state.clone()), headers(host, Some(&cookie))).await;
            serde_json::from_str::<Value>(&text(response).await).unwrap()["result"].clone()
        }

        async fn password_account(&self, host: &str, email: &str) -> String {
            let deployment = self.deployment(host).await;
            let store = AuthService::open_in(
                self.root.join("deployments").join(&deployment.id).join("identity.sqlite"),
                &auth::deployment_scope(&deployment.id),
                auth::Verification::Local,
            )
            .unwrap();
            let session = store.create_account(auth::NewAccount::prepare(email, "correct horse battery").unwrap()).unwrap();
            // A password account on an installation that requires verification proved its email.
            self.identity(&deployment.id).execute("UPDATE app_users SET verified_at = 1 WHERE email = ?1", [email]).unwrap();
            session.user.user_id
        }

        fn users(&self, deployment_id: &str) -> i64 {
            self.identity(deployment_id).query_row("SELECT COUNT(*) FROM app_users", [], |row| row.get(0)).unwrap_or(0)
        }
    }

    #[tokio::test]
    async fn a_person_signs_in_with_google_on_the_apps_own_host_and_gets_one_stable_account() {
        let fixture = fixture(SignupMode::Open).await;
        let (cookie, query) = fixture.start(APP).await;
        // The app's own client and the app host's own callback.
        assert_eq!(query["client_id"], APP_CLIENT);
        assert_eq!(query["redirect_uri"], format!("http://{APP}{CALLBACK_PATH}"));
        let first = fixture.google(APP, &cookie, &query, &ANA, "local-test", |_| {}).await;
        assert_eq!(first.status(), StatusCode::SEE_OTHER, "{}", text(first).await);
        assert_eq!(location(&first), "/");
        assert!(set_cookies(&first).contains(&"littlebox_google=".to_owned()), "the callback clears the flow cookie");
        let user = fixture.session_user(APP, &first).await;
        assert_eq!(user["name"], ANA.email);
        assert_eq!(user["role"], "member");
        let again = fixture.sign_in(APP, &ANA).await;
        assert_eq!(fixture.session_user(APP, &again).await["userId"], user["userId"]);
        // The account has a password hash nobody chose: no password opens it.
        let app = fixture.deployment(APP).await;
        let (verified, hash): (Option<i64>, String) = fixture
            .identity(&app.id)
            .query_row("SELECT verified_at, password_hash FROM app_users", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert!(verified.is_some());
        assert!(hash.starts_with("pbkdf2-sha256$600000$") && hash != auth::NO_PASSWORD);
        // Accounts stay on their Deployment.
        assert_eq!(fixture.users(&fixture.deployment(OTHER).await.id), 0);
    }

    #[tokio::test]
    async fn the_redirect_uris_shown_to_the_owner_are_the_ones_sent_and_no_other_host_starts() {
        let fixture = fixture(SignupMode::Open).await;
        let app = fixture.deployment(APP).await;
        let shown = redirect_uris(&fixture.state, &app);
        assert_eq!(shown, vec![format!("http://{APP}{CALLBACK_PATH}")]);
        let (_, query) = fixture.start(APP).await;
        assert!(shown.contains(&query["redirect_uri"]));
        // A verified custom domain adds its own callback, and starts there send it.
        let record = domain::Record { hostname: "practice.example.com".into(), token: "t".into(), verified: true, verified_at: Some(1), created_at: 1 };
        std::fs::write(fixture.root.join("deployments").join(&app.id).join(domain::RECORD_FILE), serde_json::to_vec(&record).unwrap()).unwrap();
        let shown = redirect_uris(&fixture.state, &app);
        assert_eq!(shown[1], format!("http://practice.example.com:8790{CALLBACK_PATH}"));
        // A host whose callback the owner was not shown does not start a flow.
        let request = Request::builder().uri(START_PATH).header(header::HOST, "practice.localhost:9999").body(Body::empty()).unwrap();
        let refused = start_get(State(fixture.state.clone()), request).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert!(text(refused).await.contains(&escape(FAILED)));
    }

    #[tokio::test]
    async fn a_forged_expired_foreign_mismatched_or_unverified_token_is_refused() {
        let mut fixture = fixture(SignupMode::Open).await;
        let cases: Vec<(&str, &'static str, Box<dyn FnOnce(&mut Value)>)> = vec![
            ("unknown key", "someone-else", Box::new(|_| {})),
            ("expired", "local-test", Box::new(|c| c["exp"] = json!(auth::now_seconds() - 1))),
            ("another app's client", "local-test", Box::new(|c| { c["aud"] = json!(OTHER_CLIENT); c["azp"] = json!(OTHER_CLIENT); })),
            ("wrong issuer", "local-test", Box::new(|c| c["iss"] = json!("https://issuer.example"))),
            ("wrong nonce", "local-test", Box::new(|c| c["nonce"] = json!("not-the-nonce"))),
            ("unverified email", "local-test", Box::new(|c| c["email_verified"] = json!(false))),
        ];
        for (case, kid, edit) in cases {
            // Each case is a new person's first try, not one client's sixth.
            fixture.state.guards = Arc::new(limits::Guards::new(Vec::new()));
            let (cookie, query) = fixture.start(APP).await;
            let response = fixture.google(APP, &cookie, &query, &ANA, kid, edit).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{case}");
            let body = text(response).await;
            let expected = if case == "unverified email" { UNVERIFIED } else { CHECKS };
            assert!(body.contains(&escape(expected)), "{case}: {body}");
        }
        // A token whose payload was changed after signing fails the signature.
        let (_, query) = fixture.start(APP).await;
        let token = signed("local-test", &json!({"iss": fixture.base, "aud": APP_CLIENT, "sub": "1", "email": ANA.email, "email_verified": true, "nonce": query["nonce"], "iat": 0, "exp": i64::MAX}));
        let parts: Vec<&str> = token.split('.').collect();
        let forged = format!("{}.{}.{}", parts[0], encode(br#"{"iss":"x"}"#), parts[2]);
        let google = fixture.state.accounts.google.clone();
        assert_eq!(google.verify(&forged, APP_CLIENT, &query["nonce"], auth::now_seconds()).await, Err(CHECKS));
        assert!(google.verify(&token, APP_CLIENT, &query["nonce"], auth::now_seconds()).await.is_ok());
        assert_eq!(google.verify(&token, OTHER_CLIENT, &query["nonce"], auth::now_seconds()).await, Err(CHECKS));
        // Keys are cached, and the unknown kid did not fetch them again within the cooldown.
        assert_eq!(fixture.fake.key_fetches.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.users(&fixture.deployment(APP).await.id), 0);
    }

    #[tokio::test]
    async fn an_unknown_key_fetches_googles_keys_again_at_most_once_a_minute() {
        let fixture = fixture(SignupMode::Open).await;
        let google = fixture.state.accounts.google.clone();
        let fetches = || fixture.fake.key_fetches.load(Ordering::SeqCst);
        assert!(google.key("local-test").await.is_some());
        assert_eq!(fetches(), 1);
        for _ in 0..5 {
            assert!(google.key("rotated-in").await.is_none());
        }
        assert_eq!(fetches(), 1, "fresh flows naming an unknown key do not reach Google each time");
        assert!(google.key("local-test").await.is_some());
        // A minute later an unknown key fetches once more, then waits again.
        let backdate = |google: &Google| {
            let mut cache = google.keys.lock().unwrap();
            let cache = cache.as_mut().unwrap();
            cache.fetched_at = Instant::now() - REFETCH_COOLDOWN - Duration::from_secs(1);
        };
        backdate(&google);
        assert!(google.key("rotated-in").await.is_none());
        assert!(google.key("rotated-in").await.is_none());
        assert_eq!(fetches(), 2);
        // A stale cache fetches whatever the cooldown says.
        google.keys.lock().unwrap().as_mut().unwrap().fresh_until = Instant::now() - Duration::from_secs(1);
        assert!(google.key("local-test").await.is_some());
        assert_eq!(fetches(), 3);
    }

    #[tokio::test]
    async fn google_proves_only_an_address_it_manages() {
        let fixture = fixture(SignupMode::Open).await;
        // A Google account made with an outside address is refused.
        let outside = fixture.sign_in(APP, &CARL).await;
        assert_eq!(outside.status(), StatusCode::FORBIDDEN);
        assert!(text(outside).await.contains(&escape(NOT_AUTHORITATIVE)));
        // So is a Workspace claim for another domain.
        let (cookie, query) = fixture.start(APP).await;
        let foreign = fixture.google(APP, &cookie, &query, &BEA, "local-test", |c| c["hd"] = json!("other.example")).await;
        assert!(text(foreign).await.contains(&escape(NOT_AUTHORITATIVE)));
        assert_eq!(fixture.users(&fixture.deployment(APP).await.id), 0);
        // A Workspace address whose hd names its domain signs in.
        let workspace = fixture.sign_in(APP, &BEA).await;
        assert_eq!(workspace.status(), StatusCode::SEE_OTHER, "{}", text(workspace).await);
        assert_eq!(fixture.session_user(APP, &workspace).await["name"], BEA.email);
        assert!(google_vouches_for("Ana@GMAIL.com", None) && !google_vouches_for("ana@googlemail.example", None));
    }

    #[tokio::test]
    async fn the_string_true_counts_as_a_verified_email() {
        let fixture = fixture(SignupMode::Open).await;
        let (cookie, query) = fixture.start(APP).await;
        let callback = fixture.google(APP, &cookie, &query, &ANA, "local-test", |c| c["email_verified"] = json!("true")).await;
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn a_flow_works_once_only_on_its_deployment_origin_and_browser_and_only_until_it_expires() {
        let fixture = fixture(SignupMode::Open).await;
        let (cookie, query) = fixture.start(APP).await;
        // Another Deployment, another origin of this one, or another browser: refused, and the flow stays.
        let other = fixture.google(OTHER, &cookie, &query, &ANA, "local-test", |_| {}).await;
        assert_eq!(other.status(), StatusCode::GONE);
        assert!(text(other).await.contains(&escape(STALE)));
        let port = fixture.google("practice.localhost:9999", &cookie, &query, &ANA, "local-test", |_| {}).await;
        assert_eq!(port.status(), StatusCode::GONE);
        let stranger = fixture.google(APP, "littlebox_google=someone-else", &query, &ANA, "local-test", |_| {}).await;
        assert!(text(stranger).await.contains(&escape(OTHER_BROWSER)));
        // The right one works once.
        assert_eq!(fixture.google(APP, &cookie, &query, &ANA, "local-test", |_| {}).await.status(), StatusCode::SEE_OTHER);
        let replay = fixture.google(APP, &cookie, &query, &ANA, "local-test", |_| {}).await;
        assert_eq!(replay.status(), StatusCode::GONE);
        // An expired flow is refused.
        let (cookie, query) = fixture.start(APP).await;
        open(&fixture.state).unwrap().execute("UPDATE google_flows SET expires_at = ?1", [auth::now_seconds() - 1]).unwrap();
        assert_eq!(fixture.google(APP, &cookie, &query, &ANA, "local-test", |_| {}).await.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn each_signup_policy_admits_and_refuses_with_the_password_paths_words() {
        // Open creates; Allowlist admits listed emails only; Closed and Invitation only create nobody.
        let open = fixture(SignupMode::Open).await;
        assert_eq!(open.sign_in(APP, &ANA).await.status(), StatusCode::SEE_OTHER);
        let list = fixture(SignupMode::Allowlist).await;
        assert_eq!(list.sign_in(APP, &BEA).await.status(), StatusCode::SEE_OTHER);
        for mode in [SignupMode::Allowlist, SignupMode::Closed, SignupMode::Invitation] {
            let fixture = fixture(mode).await;
            let refused = fixture.sign_in(APP, &ANA).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{mode:?}");
            assert!(text(refused).await.contains(SIGNUP_REFUSED), "{mode:?}");
        }
        // Closed still admits an account that exists.
        let closed = fixture(SignupMode::Open).await;
        assert_eq!(closed.sign_in(APP, &ANA).await.status(), StatusCode::SEE_OTHER);
        for item in closed.state.deployments.write().await.values_mut() {
            item.signup_policy = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        }
        assert_eq!(closed.sign_in(APP, &ANA).await.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn a_password_account_is_signed_in_by_google_with_the_same_email() {
        let fixture = fixture(SignupMode::Closed).await;
        let user_id = fixture.password_account(APP, "Ana@Gmail.com").await;
        let signed = fixture.sign_in(APP, &ANA).await;
        assert_eq!(signed.status(), StatusCode::SEE_OTHER, "{}", text(signed).await);
        assert_eq!(fixture.session_user(APP, &signed).await["userId"], user_id);
        assert_eq!(fixture.users(&fixture.deployment(APP).await.id), 1);
    }

    #[tokio::test]
    async fn a_package_moved_to_another_client_still_signs_the_same_person_in() {
        let fixture = fixture(SignupMode::Open).await;
        let first = fixture.sign_in(APP, &ANA).await;
        let user = fixture.session_user(APP, &first).await;
        // The package carries identity.sqlite; the destination app has its
        // own client, and Google gives this person another subject there.
        let (app, other) = (fixture.deployment(APP).await, fixture.deployment(OTHER).await);
        let moved = fixture.root.join("deployments").join(&other.id).join("identity.sqlite");
        fixture.identity(&app.id).execute("VACUUM INTO ?1", [moved.to_string_lossy().as_ref()]).unwrap();
        for item in fixture.state.deployments.write().await.values_mut() {
            item.signup_policy = SignupPolicy::from_input(SignupMode::Closed, &[]).unwrap();
        }
        let (cookie, query) = fixture.start(OTHER).await;
        assert_eq!(query["client_id"], OTHER_CLIENT);
        let there = fixture.google(OTHER, &cookie, &query, &ANA, "local-test", |c| c["sub"] = json!("a-different-subject")).await;
        assert_eq!(there.status(), StatusCode::SEE_OTHER, "{}", text(there).await);
        assert_eq!(fixture.session_user(OTHER, &there).await["userId"], user["userId"]);
    }

    /// Creates a pending account and its invitation as `app_invitations_create`
    /// does, and returns the token.
    async fn invite(fixture: &Fixture, email: &str) -> String {
        let deployment = fixture.deployment(APP).await;
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
        fixture.google(APP, &cookie, &query, person, "local-test", |_| {}).await
    }

    #[tokio::test]
    async fn an_invitation_accepted_with_google_needs_the_invited_email_and_activates_the_account() {
        let fixture = fixture(SignupMode::Invitation).await;
        let token = invite(&fixture, BEA.email).await;
        // The invited person must use the Google account of the invited email.
        let wrong = accept_with_google(&fixture, &token, &ANA).await;
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        assert!(text(wrong).await.contains("This invitation is for another email."));
        // Signing in from the sign-in page does not accept an invitation.
        let pending = fixture.sign_in(APP, &BEA).await;
        assert_eq!(pending.status(), StatusCode::FORBIDDEN);
        let accepted = accept_with_google(&fixture, &token, &BEA).await;
        assert_eq!(accepted.status(), StatusCode::SEE_OTHER, "{}", text(accepted).await);
        let app = fixture.deployment(APP).await;
        let (state, invitation): (String, String) = fixture
            .identity(&app.id)
            .query_row(
                "SELECT u.state, i.state FROM app_users u JOIN app_invitations i ON i.user_id = u.id WHERE u.email = ?1",
                [BEA.email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), invitation.as_str()), ("active", "accepted"));
        // The link worked once; later sign-ins match the email.
        assert_eq!(accept_with_google(&fixture, &token, &BEA).await.status(), StatusCode::GONE);
        assert_eq!(fixture.sign_in(APP, &BEA).await.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn an_app_without_google_turned_on_or_without_its_client_offers_none() {
        let fixture = fixture(SignupMode::Open).await;
        let start = |state: AppState| async move {
            let request = Request::builder().uri(START_PATH).header(header::HOST, APP).body(Body::empty()).unwrap();
            start_get(State(state), request).await
        };
        for item in fixture.state.deployments.write().await.values_mut() {
            item.sign_in_methods = vec![SignInMethod::Password];
        }
        let response = start(fixture.state.clone()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(text(response).await.contains(&escape(NOT_ON)));
        // On, but the client secret is not set here, as after an import.
        for item in fixture.state.deployments.write().await.values_mut() {
            item.sign_in_methods = vec![SignInMethod::Password, SignInMethod::Google];
        }
        let app = fixture.deployment(APP).await;
        mcp::store(&fixture.state).unwrap().execute("DELETE FROM project_secrets WHERE project = ?1 AND name = ?2", [&app.project_id, CLIENT_SECRET]).unwrap();
        assert!(!offered(&fixture.state, &app));
        assert_eq!(client_set(&fixture.state, &app), (true, false));
        assert!(text(start(fixture.state.clone()).await).await.contains(&escape(NOT_ON)));
    }

    #[test]
    fn the_test_issuer_needs_local_http() {
        assert!(local_http("http://localhost:8790") && local_http("http://127.0.0.1:9") && local_http("http://app.localhost"));
        assert!(!local_http("https://localhost") && !local_http("http://example.com") && !local_http("http://localhost.example.com"));
        assert_eq!(Google::real().endpoints.jwks, "https://www.googleapis.com/oauth2/v3/certs");
    }
}
