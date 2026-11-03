use anyhow::{Context, Result, ensure};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{AuthSession, AuthUser};
use crate::base64url;

pub const SESSION_TTL_SECONDS: i64 = 3600;

#[derive(Clone)]
pub struct ClerkConfig {
    issuer: String,
    authorized_origins: Vec<String>,
    beta_subjects: Vec<String>,
    keys: Vec<Jwk>,
    pub publishable_key: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    alg: String,
    #[serde(rename = "use")]
    usage: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
    #[serde(default)]
    crit: Vec<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    sid: String,
    azp: String,
    exp: i64,
    nbf: i64,
    iat: i64,
    #[serde(default)]
    sts: Option<String>,
}

impl ClerkConfig {
    /// Any Clerk setting enables hosted mode. Partial configuration fails closed.
    pub fn from_env() -> Result<Option<Self>> {
        let names = [
            "KONAE_CLERK_ISSUER",
            "KONAE_CLERK_JWKS_FILE",
            "KONAE_CLERK_AUTHORIZED_ORIGINS",
            "KONAE_CLERK_BETA_SUBJECTS",
            "KONAE_CLERK_PUBLISHABLE_KEY",
        ];
        if names.iter().all(|name| std::env::var_os(name).is_none()) {
            return Ok(None);
        }
        let get = |name| std::env::var(name).with_context(|| format!("Missing {name}."));
        let path = get(names[1])?;
        let metadata = std::fs::metadata(&path).context("Cannot read Clerk JWKS file.")?;
        ensure!(metadata.len() <= 65536, "Clerk JWKS file exceeds 64 KiB.");
        let mut config = Self::new(
            get(names[0])?,
            serde_json::from_str(&get(names[2])?)?,
            serde_json::from_str(&get(names[3])?)?,
            &std::fs::read_to_string(path)?,
        )?;
        config.publishable_key = std::env::var(names[4]).ok();
        if let Some(key) = &config.publishable_key {
            ensure!(
                key.starts_with("pk_test_") || key.starts_with("pk_live_"),
                "Invalid Clerk publishable key."
            );
            ensure!(
                key.len() <= 1024
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'='),
                "Invalid Clerk publishable key."
            );
        }
        Ok(Some(config))
    }

    fn new(
        issuer: String,
        authorized_origins: Vec<String>,
        beta_subjects: Vec<String>,
        jwks: &str,
    ) -> Result<Self> {
        let parsed = url::Url::parse(&issuer)?;
        ensure!(
            parsed.scheme() == "https"
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && issuer == parsed.origin().ascii_serialization(),
            "Clerk issuer must be an HTTPS origin without a trailing slash."
        );
        ensure!(
            !authorized_origins.is_empty() && authorized_origins.len() <= 32,
            "Configure authorized Clerk origins."
        );
        for origin in &authorized_origins {
            let parsed = url::Url::parse(origin)?;
            let local = parsed
                .host_str()
                .is_some_and(|h| ["localhost", "127.0.0.1", "[::1]"].contains(&h));
            ensure!(
                (parsed.scheme() == "https" || parsed.scheme() == "http" && local)
                    && origin == &parsed.origin().ascii_serialization()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.path() == "/"
                    && parsed.query().is_none()
                    && parsed.fragment().is_none(),
                "Clerk authorized origins must be exact HTTPS origins or local HTTP origins."
            );
        }
        ensure!(
            beta_subjects.len() <= 10000 && beta_subjects.iter().all(|s| valid_id(s, "user_")),
            "Invalid beta subject list."
        );
        #[derive(Deserialize)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        let keys: Vec<Jwk> = serde_json::from_str::<Jwks>(jwks)?.keys;
        ensure!(
            !keys.is_empty() && keys.len() <= 16,
            "Configure one to sixteen Clerk signing keys."
        );
        let mut seen = std::collections::HashSet::new();
        for key in &keys {
            ensure!(
                key.kty == "RSA"
                    && key.alg == "RS256"
                    && key.usage == "sig"
                    && !key.kid.is_empty()
                    && key.kid.len() <= 256
                    && seen.insert(key.kid.clone()),
                "Invalid or duplicate Clerk signing key."
            );
            let modulus = base64url(&key.n)?;
            let exponent = base64url(&key.e)?;
            ensure!(
                (256..=1024).contains(&modulus.len())
                    && modulus[0] != 0
                    && !exponent.is_empty()
                    && exponent.len() <= 8,
                "Invalid Clerk RSA key size."
            );
        }
        Ok(Self {
            issuer,
            authorized_origins,
            beta_subjects,
            keys,
            publishable_key: None,
        })
    }

    fn verify(&self, token: &str, request_origin: &str, now: i64) -> Result<Claims> {
        ensure!(token.len() <= 16384, "Clerk token exceeds limit.");
        ensure!(
            self.authorized_origins
                .iter()
                .any(|origin| origin == request_origin),
            "Unauthorized Clerk request origin."
        );
        let parts: Vec<_> = token.split('.').collect();
        ensure!(parts.len() == 3, "Invalid Clerk token.");
        let header: JwtHeader = serde_json::from_slice(&base64url(parts[0])?)?;
        ensure!(
            header.alg == "RS256" && header.crit.is_empty(),
            "Unsupported Clerk signing algorithm or critical header."
        );
        let key = self
            .keys
            .iter()
            .find(|key| key.kid == header.kid)
            .context("Unknown Clerk signing key.")?;
        RsaPublicKeyComponents {
            n: base64url(&key.n)?,
            e: base64url(&key.e)?,
        }
        .verify(
            &RSA_PKCS1_2048_8192_SHA256,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &base64url(parts[2])?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid Clerk signature."))?;
        let claims: Claims = serde_json::from_slice(&base64url(parts[1])?)?;
        ensure!(
            claims.iss == self.issuer && claims.azp == request_origin,
            "Clerk issuer or authorized party does not match."
        );
        ensure!(
            claims.exp > now
                && claims.nbf <= now
                && claims.iat <= now
                && claims.exp > claims.iat
                && claims.nbf < claims.exp
                && claims.exp.saturating_sub(claims.iat) <= 300,
            "Clerk token is expired, premature, or exceeds five minutes."
        );
        ensure!(
            valid_id(&claims.sub, "user_")
                && valid_id(&claims.sid, "sess_")
                && claims
                    .sts
                    .as_deref()
                    .is_none_or(|status| status == "active"),
            "Invalid or pending Clerk session."
        );
        ensure!(
            self.beta_subjects.contains(&claims.sub),
            "This account has not been admitted to the beta."
        );
        Ok(claims)
    }
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn install(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS creator_identities(
        owner TEXT PRIMARY KEY, provider TEXT NOT NULL, subject TEXT NOT NULL,
        suspended INTEGER NOT NULL DEFAULT 0 CHECK(suspended IN (0,1)),
        UNIQUE(provider,subject));
        CREATE TABLE IF NOT EXISTS creator_sessions(
        hash TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES creator_identities(owner),
        expires INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS creator_sessions_expiry ON creator_sessions(expires);",
    )?;
    Ok(())
}

pub fn exchange(
    db: &mut Connection,
    config: &ClerkConfig,
    token: &str,
    request_origin: &str,
    now: i64,
) -> Result<AuthSession> {
    let claims = config.verify(token, request_origin, now)?;
    install(db)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let provider = format!("clerk:{}", config.issuer);
    tx.execute("INSERT INTO creator_identities(owner,provider,subject) VALUES(?1,?2,?3) ON CONFLICT(provider,subject) DO NOTHING", params![Uuid::new_v4().to_string(), provider, claims.sub])?;
    let (owner, suspended): (String, bool) = tx.query_row(
        "SELECT owner,suspended FROM creator_identities WHERE provider=?1 AND subject=?2",
        params![provider, claims.sub],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(!suspended, "This creator is suspended.");
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = now
        .checked_add(SESSION_TTL_SECONDS)
        .context("Invalid session time.")?;
    tx.execute("DELETE FROM creator_sessions WHERE expires<=?1", [now])?;
    tx.execute(
        "INSERT INTO creator_sessions(hash,owner,expires) VALUES(?1,?2,?3)",
        params![digest(&token), owner, expires_at],
    )?;
    tx.commit()?;
    Ok(AuthSession {
        token,
        user: user(owner, claims.sub),
        expires_at,
    })
}

pub fn owner_allowed(db: &Connection, config: &ClerkConfig, owner: &str) -> Result<bool> {
    install(db)?;
    let subject: Option<String> = db
        .query_row(
            "SELECT subject FROM creator_identities WHERE owner=?1 AND provider=?2 AND suspended=0",
            params![owner, format!("clerk:{}", config.issuer)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(subject.is_some_and(|subject| config.beta_subjects.contains(&subject)))
}

pub fn authenticate(
    db: &Connection,
    config: &ClerkConfig,
    token: Option<&str>,
    now: i64,
) -> Result<Option<AuthUser>> {
    install(db)?;
    let Some(token) =
        token.filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return Ok(None);
    };
    let record: Option<(String, String)> = db.query_row("SELECT i.owner,i.subject FROM creator_sessions s JOIN creator_identities i ON i.owner=s.owner WHERE s.hash=?1 AND s.expires>?2 AND i.suspended=0 AND i.provider=?3", params![digest(token), now, format!("clerk:{}", config.issuer)], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    Ok(record
        .filter(|(_, subject)| config.beta_subjects.contains(subject))
        .map(|(owner, subject)| user(owner, subject)))
}

pub fn revoke(db: &Connection, token: Option<&str>) -> Result<()> {
    install(db)?;
    if let Some(token) = token.filter(|token| token.len() <= 128) {
        db.execute(
            "DELETE FROM creator_sessions WHERE hash=?1",
            [digest(token)],
        )?;
    }
    Ok(())
}

/// The operator suspends a creator without deleting projects or app users.
pub fn suspend(db: &mut Connection, owner: &str) -> Result<()> {
    install(db)?;
    let tx = db.transaction()?;
    ensure!(
        tx.execute(
            "UPDATE creator_identities SET suspended=1 WHERE owner=?1",
            [owner]
        )? == 1,
        "Creator not found."
    );
    tx.execute("DELETE FROM creator_sessions WHERE owner=?1", [owner])?;
    tx.commit()?;
    Ok(())
}

/// Runs a local maintenance command before the Kernel starts listening.
pub fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("creator-suspend") {
        return Ok(false);
    }
    ensure!(
        args.len() == 2 && valid_id(&args[1], "user_"),
        "Usage: konae creator-suspend <Clerk user subject>"
    );
    let issuer = std::env::var("KONAE_CLERK_ISSUER")
        .context("Set KONAE_CLERK_ISSUER before suspending a hosted creator.")?;
    let root = std::path::PathBuf::from(
        std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()),
    );
    let mut db = Connection::open_with_flags(
        root.join("owners.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let owner: String = db
        .query_row(
            "SELECT owner FROM creator_identities WHERE provider=?1 AND subject=?2",
            params![format!("clerk:{issuer}"), args[1]],
            |row| row.get(0),
        )
        .optional()?
        .context("Creator not found for this issuer and subject.")?;
    suspend(&mut db, &owner)?;
    // Suspension is durable even if cleanup in the MCP database fails.
    let mcp_path = root.join("mcp.sqlite");
    if mcp_path.exists() {
        let mut mcp =
            Connection::open_with_flags(mcp_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        mcp.busy_timeout(std::time::Duration::from_secs(5))?;
        revoke_delegations(&mut mcp, &owner)?;
    }
    println!(
        "Suspended creator {owner}. Projects and app accounts remain available to the installation operator."
    );
    Ok(true)
}

fn revoke_delegations(db: &mut Connection, owner: &str) -> Result<()> {
    let tx = db.transaction()?;
    tx.execute("DELETE FROM credentials WHERE owner=?1", [owner])?;
    let has_codes: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='oauth_codes')",
        [],
        |row| row.get(0),
    )?;
    if has_codes {
        tx.execute(
            "DELETE FROM oauth_codes WHERE json_extract(body,'$.owner')=?1",
            [owner],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
fn user(owner: String, subject: String) -> AuthUser {
    AuthUser {
        user_id: owner,
        name: subject,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::{
        rand::SystemRandom,
        signature::{RSA_PKCS1_SHA256, RsaKeyPair},
    };
    use serde_json::{Value, json};

    const NOW: i64 = 1_800_000_000;
    const ORIGIN: &str = "https://editor.example.test";
    const ISSUER: &str = "https://example.clerk.accounts.dev";

    fn config() -> ClerkConfig {
        ClerkConfig::new(
            ISSUER.into(),
            vec![ORIGIN.into()],
            vec!["user_alice".into(), "user_bob".into()],
            include_str!("../../test-fixtures/clerk/jwks.json"),
        )
        .unwrap()
    }

    fn claims() -> Value {
        json!({"iss":ISSUER,"sub":"user_alice","sid":"sess_test","azp":ORIGIN,"iat":NOW-5,"nbf":NOW-5,"exp":NOW+55})
    }
    fn encode(bytes: &[u8]) -> String {
        crate::encode_base64(bytes)
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_")
    }
    fn signed(header: Value, claims: Value) -> String {
        let key =
            RsaKeyPair::from_pkcs8(include_bytes!("../../test-fixtures/clerk/test-key.pk8")).unwrap();
        let message = format!(
            "{}.{}",
            encode(header.to_string().as_bytes()),
            encode(claims.to_string().as_bytes())
        );
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{message}.{}", encode(&signature))
    }
    fn token(claims: Value) -> String {
        signed(json!({"alg":"RS256","kid":"local-test"}), claims)
    }

    #[test]
    fn verifies_signature_and_required_claims() {
        let config = config();
        assert_eq!(
            config.verify(&token(claims()), ORIGIN, NOW).unwrap().sub,
            "user_alice"
        );
        for (field, value) in [
            ("iss", json!("https://attacker.example")),
            ("azp", json!("https://app.example.test")),
            ("exp", json!(NOW)),
            ("nbf", json!(NOW + 1)),
            ("iat", json!(NOW + 1)),
            ("sub", json!("user_uninvited")),
            ("sid", json!("")),
            ("sts", json!("pending")),
            ("exp", json!(NOW + 600)),
        ] {
            let mut claims = claims();
            claims[field] = value;
            assert!(
                config.verify(&token(claims), ORIGIN, NOW).is_err(),
                "accepted {field}"
            );
        }
        for field in ["exp", "nbf", "iat", "sub", "sid", "azp", "iss"] {
            let mut claims = claims();
            claims.as_object_mut().unwrap().remove(field);
            assert!(
                config.verify(&token(claims), ORIGIN, NOW).is_err(),
                "missing {field}"
            );
        }
        assert!(
            config
                .verify(&token(claims()), "https://app.example.test", NOW)
                .is_err()
        );
        for header in [
            json!({"alg":"none","kid":"local-test"}),
            json!({"alg":"HS256","kid":"local-test"}),
            json!({"alg":"RS256","kid":"unknown"}),
            json!({"alg":"RS256","kid":"local-test","crit":["b64"]}),
        ] {
            assert!(
                config
                    .verify(&signed(header, claims()), ORIGIN, NOW)
                    .is_err()
            );
        }
        let good = token(claims());
        let mut parts: Vec<String> = good.split('.').map(str::to_owned).collect();
        let mut forged = claims();
        forged["sub"] = json!("user_bob");
        parts[1] = encode(forged.to_string().as_bytes());
        assert!(config.verify(&parts.join("."), ORIGIN, NOW).is_err());
        assert!(
            config
                .verify(&format!("{good}.extra"), ORIGIN, NOW)
                .is_err()
        );
        assert!(config.verify(&"x".repeat(16385), ORIGIN, NOW).is_err());
    }

    #[test]
    fn mapping_is_idempotent_and_never_links_email() {
        let mut db = Connection::open_in_memory().unwrap();
        let config = config();
        let mut alice = claims();
        alice["email"] = json!("same@example.test");
        let first = exchange(&mut db, &config, &token(alice.clone()), ORIGIN, NOW).unwrap();
        let second = exchange(&mut db, &config, &token(alice.clone()), ORIGIN, NOW).unwrap();
        assert_eq!(first.user.user_id, second.user.user_id);
        assert_ne!(first.token, second.token);
        alice["sub"] = json!("user_bob");
        let bob = exchange(&mut db, &config, &token(alice), ORIGIN, NOW).unwrap();
        assert_ne!(first.user.user_id, bob.user.user_id);
        assert_eq!(
            db.query_row("SELECT count(*) FROM creator_identities", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let stored: String = db
            .query_row("SELECT hash FROM creator_sessions LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_ne!(stored, first.token);
        assert!(db.prepare("SELECT * FROM app_users").is_err());
    }

    #[test]
    fn sessions_expire_revoke_and_obey_beta_removal_and_suspension() {
        let mut db = Connection::open_in_memory().unwrap();
        let mut config = config();
        let session = exchange(&mut db, &config, &token(claims()), ORIGIN, NOW).unwrap();
        assert!(
            authenticate(&db, &config, Some(&session.token), NOW)
                .unwrap()
                .is_some()
        );
        assert!(
            authenticate(&db, &config, Some(&session.token), session.expires_at)
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate(&db, &config, Some(&token(claims())), NOW)
                .unwrap()
                .is_none()
        );
        config.beta_subjects.clear();
        assert!(
            authenticate(&db, &config, Some(&session.token), NOW)
                .unwrap()
                .is_none()
        );
        assert!(!owner_allowed(&db, &config, &session.user.user_id).unwrap());
        assert!(exchange(&mut db, &config, &token(claims()), ORIGIN, NOW).is_err());
        config.beta_subjects.push("user_alice".into());
        revoke(&db, Some(&session.token)).unwrap();
        assert!(
            authenticate(&db, &config, Some(&session.token), NOW)
                .unwrap()
                .is_none()
        );
        let session = exchange(&mut db, &config, &token(claims()), ORIGIN, NOW).unwrap();
        suspend(&mut db, &session.user.user_id).unwrap();
        assert!(
            authenticate(&db, &config, Some(&session.token), NOW)
                .unwrap()
                .is_none()
        );
        assert!(!owner_allowed(&db, &config, &session.user.user_id).unwrap());
        assert!(exchange(&mut db, &config, &token(claims()), ORIGIN, NOW).is_err());
    }

    #[test]
    fn configuration_rejects_unsafe_origins_and_ambiguous_keys() {
        let jwks = include_str!("../../test-fixtures/clerk/jwks.json");
        for issuer in [
            "http://issuer.example",
            "https://issuer.example/",
            "https://issuer.example/path",
            "https://user@issuer.example",
        ] {
            assert!(ClerkConfig::new(issuer.into(), vec![ORIGIN.into()], vec![], jwks).is_err());
        }
        for origin in [
            "http://editor.example",
            "https://editor.example/",
            "https://editor.example/path",
            "null",
            "*",
        ] {
            assert!(ClerkConfig::new(ISSUER.into(), vec![origin.into()], vec![], jwks).is_err());
        }
        let empty_beta =
            ClerkConfig::new(ISSUER.into(), vec![ORIGIN.into()], vec![], jwks).unwrap();
        assert!(empty_beta.verify(&token(claims()), ORIGIN, NOW).is_err());
        let mut duplicate: Value = serde_json::from_str(jwks).unwrap();
        let key = duplicate["keys"][0].clone();
        duplicate["keys"].as_array_mut().unwrap().push(key);
        assert!(
            ClerkConfig::new(
                ISSUER.into(),
                vec![ORIGIN.into()],
                vec![],
                &duplicate.to_string()
            )
            .is_err()
        );
        for malformed in ["A", "AA=", "AA+", "AB", "AA\n"] {
            assert!(base64url(malformed).is_err());
        }
    }

    #[test]
    fn suspension_cleanup_removes_only_that_owners_delegations() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE credentials(hash TEXT,owner TEXT); CREATE TABLE oauth_codes(hash TEXT,body TEXT);
            INSERT INTO credentials VALUES('a','alice'),('b','bob');
            INSERT INTO oauth_codes VALUES('a','{\"owner\":\"alice\"}'),('b','{\"owner\":\"bob\"}');").unwrap();
        revoke_delegations(&mut db, "alice").unwrap();
        assert_eq!(
            db.query_row("SELECT owner FROM credentials", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "bob"
        );
        assert_eq!(
            db.query_row("SELECT hash FROM oauth_codes", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "b"
        );
    }
}
