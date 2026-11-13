use super::*;
use super::runtime::origin;
use axum::Extension;
use axum::extract::{ConnectInfo, Form, Query};
use rusqlite::params;
use sha2::{Digest, Sha256};

pub fn routes() -> Router<Konae> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize_page).post(authorize))
        .route("/oauth/token", post(token))
}

/// Registered clients that never reach consent are deleted after this long.
/// A person registers and consents within minutes, so an hour is plenty.
const UNUSED_CLIENT_TTL_MS: u64 = 60 * 60 * 1000;
/// Clients that reached consent are deleted once they go this long without
/// obtaining a token and hold no live credential.
const IDLE_CLIENT_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Clients that have not reached consent yet. Registration is open to anyone,
/// so this cap bounds what strangers can store; consented clients do not count.
/// At the cap the oldest unconsented client makes room for the new one, so
/// strangers cannot lock registration.
const MAX_UNUSED_CLIENTS: u64 = 1000;

pub(super) fn install(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS oauth_clients(id TEXT PRIMARY KEY, body TEXT NOT NULL, created INTEGER NOT NULL DEFAULT 0, used INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS oauth_codes(hash TEXT PRIMARY KEY, body TEXT NOT NULL, expires INTEGER NOT NULL);")?;
    let columns = db
        .prepare("PRAGMA table_info(oauth_clients)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == "created") {
        // Clients registered before this column existed get a fresh window.
        db.execute_batch(&format!(
            "ALTER TABLE oauth_clients ADD COLUMN created INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE oauth_clients ADD COLUMN used INTEGER NOT NULL DEFAULT 0;
             UPDATE oauth_clients SET created={};",
            now_ms()
        ))?;
    }
    if !columns.iter().any(|name| name == "last_used") {
        // Clients that consented before this column existed count from now.
        db.execute_batch(&format!(
            "ALTER TABLE oauth_clients ADD COLUMN last_used INTEGER NOT NULL DEFAULT 0;
             UPDATE oauth_clients SET last_used={} WHERE used=1;",
            now_ms()
        ))?;
    }
    let credential_columns = db
        .prepare("PRAGMA table_info(credentials)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !credential_columns.iter().any(|name| name == "client") {
        db.execute("ALTER TABLE credentials ADD COLUMN client TEXT", [])?;
    }
    Ok(())
}

pub(super) async fn valid_owner_host(state: &Konae, headers: &HeaderMap) -> bool {
    // The path host serves apps and the home, so it never serves administration,
    // even when it is `localhost` on a local installation.
    if super::path::on_path_host(state, headers) {
        return false;
    }
    let Some(authority) = headers.get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Ok(requested) = url::Url::parse(&format!("http://{authority}")) else {
        return false;
    };
    if !requested.username().is_empty() || requested.password().is_some()
        || requested.path() != "/" || requested.query().is_some() || requested.fragment().is_some()
    {
        return false;
    }
    let Some(host) = requested.host_str() else { return false; };
    let public = url::Url::parse(&state.public_origin).ok();
    let configured = state.mcp_origin.as_deref().and_then(|value| url::Url::parse(value).ok());
    let exact = public.as_ref().is_some_and(|origin| origin.host_str() == Some(host))
        || configured.as_ref().is_some_and(|origin| origin.host_str() == Some(host));
    let local = public.as_ref().and_then(|origin| origin.host_str()).is_some_and(is_local_host)
        && ["localhost", "127.0.0.1", "[::1]", "app.localhost"].contains(&host);
    (exact || local)
        && deployment_for(state, headers).await.is_none()
        && !super::security::from_app_origin(state, headers).await
}

async fn resource_metadata(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !valid_owner_host(&state, &headers).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    let origin = origin(&state, &headers);
    Json(json!({"resource":format!("{origin}/mcp"),"authorization_servers":[origin],"scopes_supported":["read","write","publish","manage_users"],"bearer_methods_supported":["header"]})).into_response()
}

async fn server_metadata(State(state): State<Konae>, headers: HeaderMap) -> Response {
    if !valid_owner_host(&state, &headers).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    let origin = origin(&state, &headers);
    Json(json!({"issuer":origin,"authorization_endpoint":format!("{origin}/oauth/authorize"),"token_endpoint":format!("{origin}/oauth/token"),"registration_endpoint":format!("{origin}/oauth/register"),"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":["read","write","publish","manage_users"],"authorization_response_iss_parameter_supported":true})).into_response()
}

fn valid_redirect(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.scheme() == "https"
            || (url.scheme() == "http"
                && [Some("127.0.0.1"), Some("localhost"), Some("[::1]")].contains(&url.host_str())))
}

/// Whether a redirect lands on the machine that runs the browser, as a native
/// client's does (RFC 8252). `valid_redirect` allows plain http only there.
fn is_loopback(redirect: &str) -> bool {
    url::Url::parse(redirect).is_ok_and(|url| {
        url.scheme() == "http"
            && [Some("127.0.0.1"), Some("localhost"), Some("[::1]")].contains(&url.host_str())
    })
}

/// How long a token lasts. One sent to a loopback redirect stays on the
/// owner's machine and lasts 24 hours, as one from `konae login` does. Any
/// other lasts an hour, and so does every token under Clerk, as in token_create.
fn token_lifetime_ms(redirect: &str) -> Result<u64> {
    let clerk = super::clerk::ClerkConfig::from_env()?.is_some();
    Ok(if is_loopback(redirect) && !clerk { 86_400_000 } else { 3_600_000 })
}

async fn register(
    State(state): State<Konae>,
    peer: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Json(a): Json<Value>,
) -> Response {
    if !valid_owner_host(&state, &headers).await || !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let client = limits::registration_key(state.guards.client_ip(peer.map(|info| info.0.0), &headers));
    if let Err(wait) = state.guards.registrations.check(&client) {
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"rate-limited","error_description":format!("This address may register {} clients per hour. Try again later.", limits::REGISTRATIONS)})),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
        return response;
    }
    let result = (|| -> Result<Value> {
        let redirects = a["redirect_uris"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("redirect_uris is required."))?;
        anyhow::ensure!(
            !redirects.is_empty()
                && redirects.len() <= 5
                && redirects
                    .iter()
                    .all(|v| v.as_str().is_some_and(valid_redirect)),
            "Invalid redirect URI."
        );
        anyhow::ensure!(
            a["token_endpoint_auth_method"].as_str().unwrap_or("none") == "none",
            "Only public PKCE clients are supported."
        );
        let db = mcp::store(&state)?;
        expire_clients(&db)?;
        make_room_for_client(&db, MAX_UNUSED_CLIENTS)?;
        let id = Uuid::new_v4().to_string();
        let body = json!({"client_id":id,"redirect_uris":redirects,"client_name":a["client_name"].as_str().unwrap_or("MCP client").chars().take(100).collect::<String>(),"token_endpoint_auth_method":"none","grant_types":["authorization_code"],"response_types":["code"]});
        db.execute(
            "INSERT INTO oauth_clients(id,body,created) VALUES(?1,?2,?3)",
            params![id, body.to_string(), now_ms()],
        )?;
        Ok(body)
    })();
    match result {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => oauth_error("invalid_client_metadata", &e.to_string()),
    }
}

/// Deletes the oldest unconsented clients until fewer than `cap` remain.
fn make_room_for_client(db: &Connection, cap: u64) -> Result<()> {
    db.execute(
        "DELETE FROM oauth_clients WHERE id IN (
           SELECT id FROM oauth_clients WHERE used=0 ORDER BY created, rowid
           LIMIT max(0, (SELECT COUNT(*) FROM oauth_clients WHERE used=0) - ?1 + 1))",
        [cap],
    )?;
    Ok(())
}

/// Deletes clients that never completed consent within an hour of registering,
/// and consented clients idle for 30 days that hold no live credential.
fn expire_clients(db: &Connection) -> Result<()> {
    let now = now_ms();
    db.execute(
        "DELETE FROM oauth_clients WHERE used=0 AND created<=?1",
        [now.saturating_sub(UNUSED_CLIENT_TTL_MS)],
    )?;
    db.execute(
        "DELETE FROM oauth_clients WHERE used=1 AND max(created,last_used)<=?1
         AND NOT EXISTS (SELECT 1 FROM credentials WHERE credentials.client=oauth_clients.id AND credentials.expires>?2)",
        params![now.saturating_sub(IDLE_CLIENT_TTL_MS), now],
    )?;
    Ok(())
}

fn authorization(
    state: &Konae,
    headers: &HeaderMap,
    a: &HashMap<String, String>,
) -> Result<Value> {
    let get = |key: &str| a.get(key).map(String::as_str).unwrap_or("");
    anyhow::ensure!(
        get("response_type") == "code" && get("code_challenge_method") == "S256",
        "Authorization requires code flow and S256 PKCE."
    );
    anyhow::ensure!(
        get("code_challenge").len() == 43
            && get("code_challenge")
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "Invalid PKCE challenge."
    );
    let body: String = mcp::store(state)?.query_row(
        "SELECT body FROM oauth_clients WHERE id=?1",
        [get("client_id")],
        |r| r.get(0),
    )?;
    let client: Value = serde_json::from_str(&body)?;
    anyhow::ensure!(
        client["redirect_uris"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(get("redirect_uri"))),
        "Redirect URI does not match client registration."
    );
    anyhow::ensure!(
        get("resource") == format!("{}/mcp", origin(state, headers)),
        "Resource must match this MCP endpoint."
    );
    let scopes = get("scope").split_whitespace().collect::<Vec<_>>();
    // export_data downloads accounts and data, so only a token that stays on
    // the owner's machine may hold it.
    let loopback = is_loopback(get("redirect_uri"));
    anyhow::ensure!(
        !scopes.is_empty()
            && scopes.iter().all(|s| {
                ["read", "write", "publish", "manage_users"].contains(s)
                    || (loopback && *s == "export_data")
            }),
        "Invalid scope. Request one or more of read, write, publish and manage_users, separated by spaces. Only a client that redirects to this machine, such as konae login --browser --export-data, may ask for export_data."
    );
    anyhow::ensure!(get("state").len() <= 2048, "State is too large.");
    Ok(client)
}

/// Where an owner signs in on this installation: the editor's page when
/// Clerk holds owner identity, Konae's own page otherwise.
pub(super) fn owner_login_path() -> &'static str {
    if super::clerk::ClerkConfig::from_env().ok().flatten().is_some() { "/platform-access" } else { "/_konae/owner/login" }
}

async fn authorize_page(
    State(state): State<Konae>,
    headers: HeaderMap,
    Query(a): Query<HashMap<String, String>>,
) -> Response {
    if !valid_owner_host(&state, &headers).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    let client = match authorization(&state, &headers, &a) {
        Ok(c) => c,
        Err(e) => return oauth_error("invalid_request", &e.to_string()),
    };
    let owner = match mcp::authenticate_owner(&state, &headers) {
        Ok(Some(owner)) if headers.get(header::AUTHORIZATION).is_none() => owner,
        _ => {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(&a)
                .finish();
            let target = format!("/oauth/authorize?{query}");
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("returnTo", &target)
                .finish();
            let login = owner_login_path();
            return (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, format!("{login}?{query}"))],
            )
                .into_response();
        }
    };
    let csrf = crate::digest(format!("{}:{}", state.admin_key, owner.id).as_bytes());
    let fields = a
        .iter()
        .map(|(key, value)| {
            format!(
                "<input type=hidden name=\"{}\" value=\"{}\">",
                escape(key),
                escape(value)
            )
        })
        .collect::<String>();
    // authorization() already matched redirect_uri against the registration.
    let destination = url::Url::parse(a.get("redirect_uri").map(String::as_str).unwrap_or(""))
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            })
        })
        .unwrap_or_default();
    // Chromium applies form-action to the whole redirect chain after Allow.
    // Cursor's registered callback returns a 308 to another origin, which an
    // origin-specific policy blocks. default-src 'none' keeps scripts out;
    // the POST target and redirect_uri stay server-chosen.
    let pub_origin = origin(&state, &headers);
    let lifetime = match token_lifetime_ms(a.get("redirect_uri").map(String::as_str).unwrap_or("")) {
        Ok(86_400_000) => "24 hours",
        Ok(_) => "one hour",
        Err(e) => return oauth_error("server_error", &e.to_string()),
    };
    let export = if a["scope"].split_whitespace().any(|scope| scope == "export_data") {
        "<p>It includes export_data, which downloads each app's accounts, rows and files.</p>"
    } else {
        ""
    };
    let body = format!(
        "<!doctype html><html lang=en><meta charset=utf-8><title>Authorize Konae access</title><main><h1>Allow access for {destination}?</h1><p>Approving sends an access token to <strong>{destination}</strong>. Only continue if you started this sign-in from an app you trust at that address.</p><p>The client calls itself \"{}\". Clients register themselves, so Konae has not verified this name.</p><p>The token grants {} access to <strong>all of your projects</strong> for {lifetime}. You can revoke it on your owner account page in the editor.</p>{export}<form method=post action=\"{authorize}\">{fields}<input type=hidden name=csrf value=\"{csrf}\"><button>Allow access</button></form><a href=/_konae/owner/login>Cancel</a></main></html>",
        escape(client["client_name"].as_str().unwrap()),
        escape(a.get("scope").unwrap()),
        destination = escape(&destination),
        authorize = escape(&format!("{pub_origin}/oauth/authorize")),
    );
    let csp = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".into()),
            (header::CACHE_CONTROL, "no-store".into()),
            (header::CONTENT_SECURITY_POLICY, csp),
        ],
        body,
    )
        .into_response()
}

async fn authorize(
    State(state): State<Konae>,
    headers: HeaderMap,
    Form(a): Form<HashMap<String, String>>,
) -> Response {
    if !same_origin(&headers)
        || !valid_owner_host(&state, &headers).await
        || headers.get(header::AUTHORIZATION).is_some()
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let result = (|| -> Result<String> {
        authorization(&state, &headers, &a)?;
        let owner = mcp::authenticate_owner(&state, &headers)?
            .ok_or_else(|| anyhow::anyhow!("Sign in first."))?;
        anyhow::ensure!(
            a.get("csrf")
                == Some(&crate::digest(
                    format!("{}:{}", state.admin_key, owner.id).as_bytes()
                )),
            "Invalid consent token."
        );
        let code = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut body = serde_json::to_value(&a)?;
        body["owner"] = json!(owner.id);
        let mut db = mcp::store(&state)?;
        issue_code(&mut db, a.get("client_id").map(String::as_str).unwrap_or(""), &code, &body)?;
        let mut redirect = url::Url::parse(a.get("redirect_uri").unwrap())?;
        redirect
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", a.get("state").map(String::as_str).unwrap_or(""))
            .append_pair("iss", &origin(&state, &headers));
        Ok(redirect.to_string())
    })();
    match result {
        Ok(uri) => (StatusCode::SEE_OTHER, [(header::LOCATION, uri)]).into_response(),
        Err(e) => oauth_error("invalid_request", &e.to_string()),
    }
}

/// Marks the client as consented and stores the code in one transaction.
/// The client may have expired since the consent page loaded; then no code is
/// issued.
fn issue_code(db: &mut Connection, client_id: &str, code: &str, body: &Value) -> Result<()> {
    let tx = db.transaction()?;
    tx.execute("DELETE FROM oauth_codes WHERE expires<=?1", [now_ms()])?;
    let updated = tx.execute(
        "UPDATE oauth_clients SET used=1,last_used=?2 WHERE id=?1",
        params![client_id, now_ms()],
    )?;
    anyhow::ensure!(
        updated == 1,
        "This client is no longer registered. Start the connection again from your MCP client."
    );
    tx.execute(
        "INSERT INTO oauth_codes(hash,body,expires) VALUES(?1,?2,?3)",
        params![crate::digest(code.as_bytes()), body.to_string(), now_ms() + 120_000],
    )?;
    tx.commit()?;
    Ok(())
}

async fn token(
    State(state): State<Konae>,
    headers: HeaderMap,
    Form(a): Form<HashMap<String, String>>,
) -> Response {
    if !valid_owner_host(&state, &headers).await || !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let result = (|| -> Result<Value> {
        let get = |key: &str| a.get(key).map(String::as_str).unwrap_or("");
        anyhow::ensure!(
            get("grant_type") == "authorization_code",
            "Only authorization_code grants are supported."
        );
        let mut db = mcp::store(&state)?;
        let tx = db.transaction()?;
        let body: String = tx.query_row(
            "SELECT body FROM oauth_codes WHERE hash=?1 AND expires>?2",
            params![crate::digest(get("code").as_bytes()), now_ms()],
            |r| r.get(0),
        )?;
        let code: Value = serde_json::from_str(&body)?;
        if let Some(config) = super::clerk::ClerkConfig::from_env()? {
            let owners = Connection::open(state.data_root.join("owners.sqlite"))?;
            anyhow::ensure!(super::clerk::owner_allowed(&owners, &config, code["owner"].as_str().unwrap_or(""))?, "Creator is not admitted.");
        } else {
            anyhow::ensure!(super::owners::creator_store(&state)?.is_active(code["owner"].as_str().unwrap_or(""))?, "Creator is not active.");
        }
        anyhow::ensure!(
            code["client_id"] == get("client_id")
                && code["redirect_uri"] == get("redirect_uri")
                && code["resource"] == get("resource")
                && get("resource") == format!("{}/mcp", origin(&state, &headers)),
            "Code binding does not match the request."
        );
        let verifier = get("code_verifier");
        anyhow::ensure!(
            (43..=128).contains(&verifier.len())
                && verifier
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c)),
            "Invalid PKCE verifier."
        );
        let challenge = encode_base64(&Sha256::digest(verifier.as_bytes()))
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");
        anyhow::ensure!(
            code["code_challenge"] == challenge,
            "PKCE verification failed."
        );
        let token = format!("lb_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let scopes = code["scope"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>();
        let lifetime = token_lifetime_ms(get("redirect_uri"))?;
        tx.execute("INSERT INTO credentials(hash,owner,scopes,projects,expires,audience,client) VALUES(?1,?2,?3,'[]',?4,?5,?6)",params![crate::digest(token.as_bytes()),code["owner"].as_str().unwrap(),serde_json::to_string(&scopes)?,now_ms()+lifetime,get("resource"),get("client_id")])?;
        tx.execute(
            "UPDATE oauth_clients SET last_used=?2 WHERE id=?1",
            params![get("client_id"), now_ms()],
        )?;
        tx.execute(
            "DELETE FROM oauth_codes WHERE hash=?1",
            [crate::digest(get("code").as_bytes())],
        )?;
        tx.commit()?;
        Ok(
            json!({"access_token":token,"token_type":"Bearer","expires_in":lifetime / 1000,"scope":scopes.join(" ")}),
        )
    })();
    let mut response = match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => oauth_error("invalid_grant", &e.to_string()),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn oauth_error(kind: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":kind,"error_description":description})),
    )
        .into_response()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn consent_allows_cursor_redirect_without_form_action() {
        use super::super::accounts::tests::{cookie_of, fixture, request, PASSWORD};
        use tower::ServiceExt;

        let fixture = fixture(auth::Verification::Local, false);
        let state = Konae::for_test(fixture.state.clone());
        let signup = mcp::owner_signup(
            State(state.clone()),
            request("POST", "localhost", "/_konae/owner/signup", json!({
                "email": "owner@example.test", "password": PASSWORD, "setupKey": "test-admin"
            })),
        ).await;
        assert_eq!(signup.status(), StatusCode::OK);
        let cookie = cookie_of(&signup).unwrap();
        // The registered www callback returns a 308 to the cursor.com origin.
        let callback = "https://www.cursor.com/agents/mcp/oauth/callback";
        let client = json!({"client_name":"Cursor", "redirect_uris":[callback]});
        mcp::store(&state).unwrap().execute(
            "INSERT INTO oauth_clients(id,body,created) VALUES (?1,?2,?3)",
            params!["cursor", client.to_string(), now_ms()],
        ).unwrap();
        let args = [
            ("client_id", "cursor"), ("redirect_uri", callback), ("response_type", "code"),
            ("code_challenge_method", "S256"),
            ("code_challenge", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("scope", "read"), ("resource", "http://localhost/mcp"), ("state", "cursor-state"),
        ];
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(args).finish();
        let router = routes().with_state(state);
        let get = Request::builder().uri(format!("/oauth/authorize?{query}"))
            .header(header::HOST, "localhost").header(header::COOKIE, &cookie)
            .body(Body::empty()).unwrap();
        let consent = router.clone().oneshot(get).await.unwrap();
        assert_eq!(consent.status(), StatusCode::OK);
        let csp = consent.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(!csp.contains("form-action"));
        for directive in ["default-src 'none'", "frame-ancestors 'none'", "base-uri 'none'"] {
            assert!(csp.contains(directive));
        }
        let html = String::from_utf8(
            axum::body::to_bytes(consent.into_body(), usize::MAX).await.unwrap().to_vec(),
        ).unwrap();
        let csrf = html.split("name=csrf value=\"").nth(1).unwrap().split('"').next().unwrap();
        let form = format!("{query}&csrf={csrf}");
        let changed_form = form.replace(
            "www.cursor.com%2Fagents", "cursor.com%2Fagents",
        );
        let post = |body: String| Request::builder().method("POST").uri("/oauth/authorize")
            .header(header::HOST, "localhost").header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body)).unwrap();
        assert_eq!(router.clone().oneshot(post(changed_form)).await.unwrap().status(), StatusCode::BAD_REQUEST);
        let response = router.oneshot(post(form)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(format!("{}{}", location.origin().ascii_serialization(), location.path()), callback);
        assert!(location.query_pairs().any(|(key, value)| key == "state" && value == "cursor-state"));
        assert!(location.query_pairs().any(|(key, value)| key == "code" && !value.is_empty()));
    }

    #[tokio::test]
    async fn a_loopback_client_gets_a_day_long_token_and_may_ask_for_export_data() {
        use super::super::accounts::tests::{cookie_of, fixture, request, PASSWORD};
        use tower::ServiceExt;

        let fixture = fixture(auth::Verification::Local, false);
        let state = Konae::for_test(fixture.state.clone());
        let signup = mcp::owner_signup(
            State(state.clone()),
            request("POST", "localhost", "/_konae/owner/signup", json!({
                "email": "owner@example.test", "password": PASSWORD, "setupKey": "test-admin"
            })),
        ).await;
        let cookie = cookie_of(&signup).unwrap();
        let loopback = "http://127.0.0.1:49152/callback";
        let remote = "https://client.example/callback";
        for (id, redirect) in [("cli", loopback), ("remote", remote)] {
            mcp::store(&state).unwrap().execute(
                "INSERT INTO oauth_clients(id,body,created) VALUES (?1,?2,?3)",
                params![id, json!({"client_name":id, "redirect_uris":[redirect]}).to_string(), now_ms()],
            ).unwrap();
        }
        let verifier = "v".repeat(43);
        let challenge = encode_base64(&Sha256::digest(verifier.as_bytes()))
            .trim_end_matches('=').replace('+', "-").replace('/', "_");
        let query = |client: &str, redirect: &str| url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("client_id", client), ("redirect_uri", redirect), ("response_type", "code"),
                ("code_challenge_method", "S256"), ("code_challenge", &challenge),
                ("scope", "read export_data"), ("resource", "http://localhost/mcp"), ("state", "s"),
            ]).finish();
        let router = routes().with_state(state.clone());
        let page = |query: String| Request::builder().uri(format!("/oauth/authorize?{query}"))
            .header(header::HOST, "localhost").header(header::COOKIE, &cookie)
            .body(Body::empty()).unwrap();
        let refused = router.clone().oneshot(page(query("remote", remote))).await.unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "only a loopback client may ask for export_data");

        let consent = router.clone().oneshot(page(query("cli", loopback))).await.unwrap();
        assert_eq!(consent.status(), StatusCode::OK);
        let html = String::from_utf8(
            axum::body::to_bytes(consent.into_body(), usize::MAX).await.unwrap().to_vec(),
        ).unwrap();
        assert!(html.contains("for 24 hours") && html.contains("downloads each app's accounts"), "{html}");
        let csrf = html.split("name=csrf value=\"").nth(1).unwrap().split('"').next().unwrap();
        let allowed = router.clone().oneshot(Request::builder().method("POST").uri("/oauth/authorize")
            .header(header::HOST, "localhost").header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("{}&csrf={csrf}", query("cli", loopback)))).unwrap()).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::SEE_OTHER);
        let location = url::Url::parse(allowed.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let code = location.query_pairs().find(|(key, _)| key == "code").unwrap().1.into_owned();
        let exchange = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "authorization_code"), ("code", &code), ("client_id", "cli"),
                ("redirect_uri", loopback), ("resource", "http://localhost/mcp"), ("code_verifier", &verifier),
            ]).finish();
        let token = router.oneshot(Request::builder().method("POST").uri("/oauth/token")
            .header(header::HOST, "localhost")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(exchange)).unwrap()).await.unwrap();
        assert_eq!(token.status(), StatusCode::OK);
        let token: Value = serde_json::from_slice(&axum::body::to_bytes(token.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(token["expires_in"], 86_400);
        assert_eq!(token["scope"], "read export_data");
        let expires: u64 = mcp::store(&state).unwrap()
            .query_row("SELECT expires FROM credentials WHERE client='cli'", [], |r| r.get(0)).unwrap();
        assert!(expires > now_ms() + 86_000_000);
    }

    #[test]
    fn unused_clients_expire_so_registration_never_locks_out() {
        let path = std::env::temp_dir().join(format!("konae-oauth-{}.sqlite", Uuid::new_v4()));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);
            CREATE TABLE oauth_clients(id TEXT PRIMARY KEY, body TEXT NOT NULL); INSERT INTO oauth_clients VALUES ('legacy','{}');").unwrap();
        install(&db).unwrap();
        install(&db).unwrap();
        let created: u64 = db
            .query_row("SELECT created FROM oauth_clients WHERE id='legacy'", [], |r| r.get(0))
            .unwrap();
        assert!(created > 0, "legacy clients get a fresh window instead of instant deletion");

        db.execute("INSERT INTO oauth_clients(id,body,created,used) VALUES('stale','{}',1,0)", []).unwrap();
        db.execute("INSERT INTO oauth_clients(id,body,created,used,last_used) VALUES('trusted','{}',1,1,?1)", [now_ms()]).unwrap();
        expire_clients(&db).unwrap();
        let mut ids = db
            .prepare("SELECT id FROM oauth_clients ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        ids.sort();
        assert_eq!(ids, vec!["legacy", "trusted"]);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn consented_clients_expire_after_30_idle_days_without_live_credentials() {
        let path = std::env::temp_dir().join(format!("konae-oauth-idle-{}.sqlite", Uuid::new_v4()));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);").unwrap();
        install(&db).unwrap();
        let now = now_ms();
        let old = now - IDLE_CLIENT_TTL_MS - 1;
        let recent = now - IDLE_CLIENT_TTL_MS / 2;
        for (id, last_used) in [("idle", old), ("active", recent), ("holding", old)] {
            db.execute(
                "INSERT INTO oauth_clients(id,body,created,used,last_used) VALUES(?1,'{}',?2,1,?3)",
                params![id, old, last_used],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO credentials(hash,owner,scopes,projects,expires,client) VALUES('h','o','[]','[]',?1,'holding')",
            [now + 60_000],
        )
        .unwrap();
        expire_clients(&db).unwrap();
        let mut ids = db
            .prepare("SELECT id FROM oauth_clients")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        ids.sort();
        assert_eq!(ids, vec!["active", "holding"]);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_full_registry_evicts_the_oldest_unconsented_client() {
        let path = std::env::temp_dir().join(format!("konae-oauth-cap-{}.sqlite", Uuid::new_v4()));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);").unwrap();
        install(&db).unwrap();
        let now = now_ms();
        for (id, created, used) in [("consented", 1, 1), ("oldest", now - 3, 0), ("middle", now - 2, 0), ("newest", now - 1, 0)] {
            db.execute(
                "INSERT INTO oauth_clients(id,body,created,used,last_used) VALUES(?1,'{}',?2,?3,?4)",
                params![id, created, used, now],
            )
            .unwrap();
        }
        make_room_for_client(&db, 3).unwrap();
        let ids = |db: &Connection| {
            let mut ids = db
                .prepare("SELECT id FROM oauth_clients")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            ids.sort();
            ids
        };
        assert_eq!(ids(&db), vec!["consented", "middle", "newest"], "room for one more at a cap of 3");
        make_room_for_client(&db, 10).unwrap();
        assert_eq!(ids(&db).len(), 3, "below the cap nothing is evicted");

        db.execute("INSERT INTO oauth_clients(id,body,created,used) VALUES('hour-old','{}',?1,0)", [now - UNUSED_CLIENT_TTL_MS - 1]).unwrap();
        expire_clients(&db).unwrap();
        assert!(!ids(&db).contains(&"hour-old".to_owned()), "unconsented clients expire after an hour");
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn consent_for_an_expired_client_issues_no_code() {
        let path = std::env::temp_dir().join(format!("konae-oauth-code-{}.sqlite", Uuid::new_v4()));
        let mut db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE credentials(hash TEXT PRIMARY KEY, owner TEXT NOT NULL, scopes TEXT NOT NULL, projects TEXT NOT NULL, expires INTEGER NOT NULL, audience TEXT);").unwrap();
        install(&db).unwrap();
        db.execute("INSERT INTO oauth_clients(id,body,created,used) VALUES('live','{}',?1,0)", [now_ms()]).unwrap();
        issue_code(&mut db, "live", "code-1", &json!({})).unwrap();
        let used: i64 = db.query_row("SELECT used FROM oauth_clients WHERE id='live'", [], |r| r.get(0)).unwrap();
        assert_eq!(used, 1);

        let error = issue_code(&mut db, "gone", "code-2", &json!({})).unwrap_err();
        assert!(error.to_string().contains("no longer registered"), "{error}");
        let codes: u64 = db.query_row("SELECT COUNT(*) FROM oauth_codes", [], |r| r.get(0)).unwrap();
        assert_eq!(codes, 1, "only the live client got a code");
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn registrations_are_limited_per_address_and_only_unused_clients_count_toward_the_cap() {
        let guards = Guards::new(Vec::new());
        for _ in 0..limits::REGISTRATIONS {
            assert!(guards.registrations.check("192.0.2.1").is_ok());
        }
        assert!(guards.registrations.check("192.0.2.1").is_err());
        assert!(guards.registrations.check("192.0.2.2").is_ok());
    }
}
