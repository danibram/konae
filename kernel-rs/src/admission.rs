//! Creator admission: who may create a creator account on this installation.
//!
//! The operator issues invitation codes from the server with the Kernel
//! executable. A code is single use, expires, and is stored in
//! `owners.sqlite` as a SHA-256 hash next to the accounts, so one transaction
//! consumes the code and creates the pending creator.
//!
//! The installation key creates the first creator on an empty store and
//! nothing else. Once any code has been issued it creates no account at all,
//! so a leaked key never opens creator signup. The one allowance is the
//! development key on an installation that permits it: while no code exists
//! it keeps creating local accounts, because local development and the
//! browser tests sign up a fresh owner per run. The Kernel refuses to listen
//! off loopback with that key unless the operator sets
//! `KONAE_ALLOW_DEFAULT_KEY=1`.

use super::*;
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::params;
use sha2::{Digest, Sha256};

/// A code expires after 14 days unless the operator chooses otherwise.
const DEFAULT_TTL_SECONDS: i64 = 14 * 24 * 60 * 60;
const MIN_TTL_SECONDS: i64 = 60 * 60;
const MAX_TTL_SECONDS: i64 = 90 * 24 * 60 * 60;
const MAX_NOTE_CHARS: usize = 200;
/// Crockford's base32 alphabet: no I, L, O or U, so a code read aloud or
/// copied by hand survives.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 25 symbols carry 125 random bits.
const CODE_SYMBOLS: usize = 25;
/// The longest `returnTo` an emailed link carries.
const MAX_RETURN_BYTES: usize = 4096;

pub(crate) const CODE_REFUSED: &str =
    "This invitation code is not valid. It may be mistyped, expired, revoked or already used.";
pub(crate) const CODE_REQUIRED: &str =
    "An invitation code is required to create a creator account. Ask the operator of this installation for one.";
const KEY_REFUSED: &str =
    "A valid installation setup key is required to create the first owner.";

/// How a signup request asks to be admitted.
#[derive(Clone)]
pub(crate) enum Admission {
    /// An invitation code, held as its hash.
    Code(String),
    /// The installation key. `development` is true for the default local key.
    SetupKey { development: bool },
}

/// Which admission the signup forms should ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    SetupKey,
    Code,
}

impl Mode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SetupKey => "setup-key",
            Self::Code => "code",
        }
    }
}

fn install(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS creator_codes (
           id TEXT PRIMARY KEY,
           code_hash TEXT NOT NULL UNIQUE,
           note TEXT NOT NULL DEFAULT '',
           created_at INTEGER NOT NULL,
           expires_at INTEGER NOT NULL,
           revoked_at INTEGER,
           used_at INTEGER,
           creator_id TEXT
         );",
    )
}

fn hash_code(normalized: &str) -> String {
    hex::encode(Sha256::digest(format!("creator-code:{normalized}").as_bytes()))
}

/// Uppercases, drops separators and maps the letters Crockford's base32
/// reads as digits. Returns `None` for anything that cannot be a code.
fn normalize_code(raw: &str) -> Option<String> {
    let mut code = String::with_capacity(CODE_SYMBOLS);
    for character in raw.chars() {
        let symbol = match character.to_ascii_uppercase() {
            '-' | ' ' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !symbol.is_ascii() || !ALPHABET.contains(&(symbol as u8)) || code.len() == CODE_SYMBOLS {
            return None;
        }
        code.push(symbol);
    }
    (code.len() == CODE_SYMBOLS).then_some(code)
}

fn new_code() -> Result<String> {
    let mut bytes = [0_u8; CODE_SYMBOLS];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("The system random source failed."))?;
    // 256 is a multiple of 32, so taking each byte modulo 32 is uniform.
    let symbols: Vec<char> = bytes.iter().map(|byte| ALPHABET[(byte % 32) as usize] as char).collect();
    Ok(symbols.chunks(5).map(|group| group.iter().collect::<String>()).collect::<Vec<_>>().join("-"))
}

fn now_seconds() -> i64 {
    (now_ms() / 1000) as i64
}

fn owners_path(root: &Path) -> PathBuf {
    root.join("owners.sqlite")
}

/// Reads the admission a signup body asks for. A non-empty `code` wins over
/// `setupKey`. A wrong key is refused here, with a message that names the
/// key only while the key can still create the first creator.
pub(crate) fn read(state: &AppState, body: &Value) -> std::result::Result<Admission, AuthError> {
    let code = body["code"].as_str().unwrap_or("");
    if !code.trim().is_empty() {
        return normalize_code(code)
            .map(|code| Admission::Code(hash_code(&code)))
            .ok_or(AuthError::AdmissionRefused(CODE_REFUSED));
    }
    let key = body["setupKey"].as_str().unwrap_or("");
    if !key.is_empty() && crate::digest(key.as_bytes()) == crate::digest(state.admin_key.as_bytes()) {
        return Ok(Admission::SetupKey { development: state.admin_key == DEFAULT_ADMIN_KEY });
    }
    Err(AuthError::AdmissionRefused(match mode(state) {
        Mode::SetupKey => KEY_REFUSED,
        Mode::Code => CODE_REQUIRED,
    }))
}

/// The check that runs inside the signup transaction. It receives the
/// creator the request created or re-linked, or `None` when the email
/// already has an active or suspended account. A code is consumed in every
/// accepted case, so its holder cannot use it to probe for existing
/// accounts; it records a creator only when one was created or re-linked.
pub(crate) fn admit(
    admission: Admission,
) -> impl FnOnce(&Connection, Option<&str>) -> std::result::Result<(), AuthError> + Send + 'static {
    move |connection, creator| {
        install(connection)?;
        match &admission {
            Admission::Code(hash) => {
                let now = now_seconds();
                let used = connection.execute(
                    "UPDATE creator_codes SET used_at = ?1, creator_id = ?2
                     WHERE code_hash = ?3 AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
                    params![now, creator, hash],
                )?;
                if used != 1 {
                    return Err(AuthError::AdmissionRefused(CODE_REFUSED));
                }
            }
            Admission::SetupKey { development } => {
                let codes: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM creator_codes)", [], |row| row.get(0))?;
                if codes {
                    return Err(AuthError::AdmissionRefused(CODE_REQUIRED));
                }
                if *development {
                    return Ok(());
                }
                // A pending first creator may sign up again to get a new link.
                let others: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM app_users WHERE id IS NOT ?1",
                    [creator],
                    |row| row.get(0),
                )?;
                if creator.is_none() || others > 0 {
                    return Err(AuthError::AdmissionRefused(CODE_REQUIRED));
                }
            }
        }
        Ok(())
    }
}

/// `setup-key` while the installation key can still create a creator;
/// `code` once any code exists or, for a configured key, any creator exists.
pub(crate) fn mode(state: &AppState) -> Mode {
    let development = state.admin_key == DEFAULT_ADMIN_KEY;
    match store_counts(&state.data_root) {
        Ok((0, _)) if development => Mode::SetupKey,
        Ok((0, 0)) => Mode::SetupKey,
        // An unreadable store asks for a code, the stricter form.
        _ => Mode::Code,
    }
}

/// Codes ever issued and accounts that exist, without creating the store.
fn store_counts(root: &Path) -> rusqlite::Result<(i64, i64)> {
    let path = owners_path(root);
    if !path.exists() {
        return Ok((0, 0));
    }
    let connection = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let count = |table: &str| -> rusqlite::Result<i64> {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(0);
        }
        connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
    };
    Ok((count("creator_codes")?, count("app_users")?))
}

/// The OAuth consent path a verification link sends the creator back to.
/// Only `/oauth/authorize?...` on this host is carried; anything else is
/// dropped, so an emailed link never redirects elsewhere.
pub(crate) fn return_to(body: &Value) -> Option<String> {
    let value = body["returnTo"].as_str()?;
    (value.starts_with("/oauth/authorize?")
        && value.len() <= MAX_RETURN_BYTES
        && !value.chars().any(|character| character.is_control() || character == '#'))
    .then(|| value.to_owned())
}

/// The page a creator verification link opens, with the consent path when
/// the signup started from an MCP client.
pub(crate) fn login_page(origin: &str, return_to: Option<&str>) -> String {
    match return_to {
        Some(target) => format!(
            "{origin}/_konae/owner/login?{}",
            url::form_urlencoded::Serializer::new(String::new()).append_pair("returnTo", target).finish()
        ),
        None => format!("{origin}/_konae/owner/login"),
    }
}

// ---------------------------------------------------------------------------
// Operator commands

/// One issued code, as `creator-code-list` shows it.
#[derive(Debug, PartialEq, Eq)]
struct CodeRow {
    id: String,
    state: &'static str,
    note: String,
    created_at: i64,
    expires_at: i64,
    creator: Option<(String, String)>,
}

/// Runs `creator-code-create`, `creator-code-list` or `creator-code-revoke`
/// against `KONAE_DATA_DIR`. Returns false when the arguments name
/// another command.
pub fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.first().is_some_and(|name| name.starts_with("creator-code-")) {
        return Ok(false);
    }
    let root = PathBuf::from(env("KONAE_DATA_DIR", "/data"));
    println!("{}", run(&root, &args, now_seconds())?);
    Ok(true)
}

const USAGE: &str = "Usage: konae-kernel creator-code-create [--expires-in 14d] [--note TEXT] | creator-code-list | creator-code-revoke <id>";

fn run(root: &Path, args: &[String], now: i64) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("creator-code-create") => {
            let mut ttl = DEFAULT_TTL_SECONDS;
            let mut note = String::new();
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                let value = rest.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value. {USAGE}"))?;
                match flag.as_str() {
                    "--expires-in" => ttl = parse_duration(value)?,
                    "--note" => note = value.clone(),
                    _ => anyhow::bail!("Unknown option {flag}. {USAGE}"),
                }
            }
            anyhow::ensure!(
                note.chars().count() <= MAX_NOTE_CHARS && !note.chars().any(char::is_control),
                "A note holds up to {MAX_NOTE_CHARS} characters on one line."
            );
            let (id, code, expires_at) = create(root, &note, ttl, now)?;
            Ok(format!(
                "Invitation code, shown once: {code}\nID: {id}\nExpires: {}\nIt creates one creator account. The setup key no longer creates accounts on this installation.",
                timestamp(expires_at)
            ))
        }
        Some("creator-code-list") => {
            anyhow::ensure!(args.len() == 1, "{USAGE}");
            let rows = list(root, now)?;
            if rows.is_empty() {
                return Ok("No invitation codes have been issued.".into());
            }
            let mut out = String::from("ID\tSTATE\tCREATED\tEXPIRES\tCREATOR\tNOTE");
            for row in rows {
                let creator = row
                    .creator
                    .map(|(email, state)| format!("{email} ({state})"))
                    .unwrap_or_else(|| "-".into());
                out.push_str(&format!(
                    "\n{}\t{}\t{}\t{}\t{}\t{}",
                    row.id,
                    row.state,
                    timestamp(row.created_at),
                    timestamp(row.expires_at),
                    creator,
                    row.note
                ));
            }
            Ok(out)
        }
        Some("creator-code-revoke") => {
            anyhow::ensure!(args.len() == 2, "{USAGE}");
            revoke(root, &args[1], now)
        }
        _ => anyhow::bail!("{USAGE}"),
    }
}

/// Opens the creator store, creating it on a fresh installation so a code
/// can be issued before anyone has signed up.
fn open_store(root: &Path) -> Result<Connection> {
    std::fs::create_dir_all(root)?;
    // Installs the schema without applying a verification mode, so a shell
    // without the Kernel's KONAE_ACCOUNT_VERIFICATION changes nothing.
    AuthService::open(owners_path(root))?;
    let connection = Connection::open(owners_path(root))?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    install(&connection)?;
    Ok(connection)
}

fn create(root: &Path, note: &str, ttl: i64, now: i64) -> Result<(String, String, i64)> {
    let connection = open_store(root)?;
    let id = format!("code_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let code = new_code()?;
    let expires_at = now + ttl;
    connection.execute(
        "INSERT INTO creator_codes (id, code_hash, note, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, hash_code(&normalize_code(&code).expect("generated codes are well formed")), note, now, expires_at],
    )?;
    Ok((id, code, expires_at))
}

fn list(root: &Path, now: i64) -> Result<Vec<CodeRow>> {
    let connection = open_store(root)?;
    let mut statement = connection.prepare(
        "SELECT c.id, c.note, c.created_at, c.expires_at, c.revoked_at, c.used_at, u.email, u.state
         FROM creator_codes c LEFT JOIN app_users u ON u.id = c.creator_id
         ORDER BY c.created_at, c.id",
    )?;
    let rows = statement
        .query_map([], |row| {
            let revoked: Option<i64> = row.get(4)?;
            let used: Option<i64> = row.get(5)?;
            let expires_at: i64 = row.get(3)?;
            let email: Option<String> = row.get(6)?;
            let account_state: Option<String> = row.get(7)?;
            Ok(CodeRow {
                id: row.get(0)?,
                note: row.get(1)?,
                created_at: row.get(2)?,
                expires_at,
                state: if used.is_some() {
                    "used"
                } else if revoked.is_some() {
                    "revoked"
                } else if expires_at <= now {
                    "expired"
                } else {
                    "open"
                },
                creator: email.zip(account_state),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn revoke(root: &Path, id: &str, now: i64) -> Result<String> {
    let connection = open_store(root)?;
    let record: Option<(Option<i64>, Option<i64>, Option<String>)> = connection
        .query_row(
            "SELECT c.revoked_at, c.used_at, u.email FROM creator_codes c
             LEFT JOIN app_users u ON u.id = c.creator_id WHERE c.id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match record {
        None => anyhow::bail!("No invitation code has the ID {id}."),
        Some((_, Some(_), Some(email))) => anyhow::bail!(
            "Code {id} was already used by {email}. Suspend that creator with creator-account-suspend instead."
        ),
        Some((_, Some(_), None)) => anyhow::bail!("Code {id} was already used and created no account."),
        Some((Some(_), _, _)) => Ok(format!("Code {id} is already revoked.")),
        Some((None, None, _)) => {
            connection.execute(
                "UPDATE creator_codes SET revoked_at = ?1 WHERE id = ?2 AND used_at IS NULL",
                params![now, id],
            )?;
            Ok(format!("Revoked code {id}. It no longer creates an account."))
        }
    }
}

/// Reads `36h` or `14d`, between one hour and 90 days.
fn parse_duration(value: &str) -> Result<i64> {
    let invalid = || anyhow::anyhow!("--expires-in takes hours or days, such as 36h or 14d, from 1h to 90d.");
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let amount: i64 = number.parse().map_err(|_| invalid())?;
    let seconds = match unit {
        "h" => amount.checked_mul(60 * 60),
        "d" => amount.checked_mul(24 * 60 * 60),
        _ => None,
    }
    .ok_or_else(invalid)?;
    anyhow::ensure!((MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&seconds), invalid());
    Ok(seconds)
}

/// A UTC time as `2026-11-20 14:00Z`.
fn timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let in_day = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}Z", in_day / 3600, in_day / 60 % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use accounts::{AccountMail, AccountSettings, MemoryMailbox};
    use auth::{NewPassword, Purpose, Verification};

    const KEY: &str = "configured-installation-key";
    const PASSWORD: &str = "correct horse battery";

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

    fn fixture(verification: Verification, key: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("konae-admission-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mailbox = Arc::new(MemoryMailbox::default());
        let state = AppState {
            admin_key: key.into(),
            accounts: AccountSettings {
                verification,
                mail: Some(mailbox.clone() as Arc<dyn AccountMail>),
                queue: None,
                google: Arc::new(crate::google::Google::real()),
            },
            ..AppState::for_test(&root)
        };
        Fixture { state, mailbox, root }
    }

    impl Fixture {
        /// Issues a code as the operator command does, `ttl` seconds from now.
        fn code(&self, ttl: i64) -> (String, String) {
            let (id, code, _) = create(&self.root, "test", ttl, now_seconds()).unwrap();
            (id, code)
        }

        fn cli(&self, args: &[&str]) -> Result<String> {
            let args: Vec<String> = args.iter().map(|value| value.to_string()).collect();
            run(&self.root, &args, now_seconds())
        }

        async fn signup(&self, body: Value) -> Response {
            let request = Request::builder()
                .method("POST")
                .uri("/_konae/owner/signup")
                .header(header::HOST, "localhost")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            // A fresh rate-limit window per call, so the sign-in limits
            // never decide a test.
            let mut state = self.state.clone();
            state.guards = Arc::new(Guards::new(Vec::new()));
            mcp::owner_signup(State(state), request).await
        }

        fn accounts(&self) -> i64 {
            store_counts(&self.root).unwrap().1
        }

        fn last_link(&self) -> String {
            self.mailbox.0.lock().unwrap().last().expect("a message was sent").link.clone()
        }

        fn sent(&self) -> usize {
            self.mailbox.0.lock().unwrap().len()
        }

        fn verify(&self, link: &str) -> std::result::Result<auth::AuthSession, AuthError> {
            let token = link.split_once("#verification=").unwrap().1;
            accounts::creator_store(&self.state).unwrap().complete_challenge(
                Purpose::Verification,
                token,
                NewPassword::prepare(PASSWORD).unwrap(),
                |_| true,
            )
        }
    }

    async fn message(response: Response) -> (StatusCode, String) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value["error"].as_str().unwrap_or("").to_owned())
    }

    #[test]
    fn codes_are_25_crockford_symbols_and_typing_slips_still_match() {
        let code = new_code().unwrap();
        assert_eq!(code.len(), 29);
        assert_eq!(code.matches('-').count(), 4);
        let normalized = normalize_code(&code).unwrap();
        assert_eq!(normalize_code(&code.to_lowercase().replace('-', " ")).as_deref(), Some(normalized.as_str()));
        assert_eq!(normalize_code("0000O-11111-IIIII-LLLLL-22222").as_deref(), Some("0000011111111111111122222"));
        assert!(normalize_code("short").is_none());
        assert!(normalize_code(&format!("{code}A")).is_none());
        assert!(normalize_code("UUUUU-UUUUU-UUUUU-UUUUU-UUUUU").is_none(), "U is not in the alphabet");
        assert_ne!(new_code().unwrap(), code);
    }

    #[test]
    fn expiry_options_take_hours_or_days_within_bounds() {
        assert_eq!(parse_duration("36h").unwrap(), 36 * 3600);
        assert_eq!(parse_duration("14d").unwrap(), DEFAULT_TTL_SECONDS);
        for refused in ["0h", "91d", "2w", "d", "", "-1d", "99999999999999999d"] {
            assert!(parse_duration(refused).is_err(), "{refused}");
        }
        assert_eq!(timestamp(0), "1970-01-01 00:00Z");
        assert_eq!(timestamp(1_795_000_000), "2026-11-18 11:06Z");
    }

    #[test]
    fn the_operator_creates_lists_and_revokes_codes() {
        let fixture = fixture(Verification::Local, KEY);
        assert_eq!(fixture.cli(&["creator-code-list"]).unwrap(), "No invitation codes have been issued.");
        let created = fixture
            .cli(&["creator-code-create", "--expires-in", "2d", "--note", "For Ada"])
            .unwrap();
        assert!(created.starts_with("Invitation code, shown once: "));
        assert!(created.ends_with("The setup key no longer creates accounts on this installation."), "{created}");
        let id = created.lines().nth(1).unwrap().trim_start_matches("ID: ").to_owned();
        let listed = fixture.cli(&["creator-code-list"]).unwrap();
        let row = listed.lines().nth(1).unwrap();
        assert!(row.starts_with(&format!("{id}\topen\t")), "{row}");
        assert!(row.ends_with("\t-\tFor Ada"), "{row}");
        assert!(!listed.contains(created.lines().next().unwrap().rsplit(' ').next().unwrap()), "the list never shows the code");

        assert!(fixture.cli(&["creator-code-revoke", &id]).unwrap().starts_with("Revoked code"));
        assert!(fixture.cli(&["creator-code-revoke", &id]).unwrap().contains("already revoked"));
        assert!(fixture.cli(&["creator-code-list"]).unwrap().contains(&format!("{id}\trevoked\t")));
        assert!(fixture.cli(&["creator-code-revoke", "code_unknown"]).is_err());
        assert!(fixture.cli(&["creator-code-create", "--expires-in"]).is_err());
        assert!(fixture.cli(&["creator-code-create", "--colour", "red"]).is_err());
        assert!(fixture.cli(&["creator-code-create", "--note", "two\nlines"]).is_err());
        assert!(fixture.cli(&["creator-code-list", "extra"]).is_err());

        // A code issued with a past clock lists as expired.
        create(&fixture.root, "", MIN_TTL_SECONDS, now_seconds() - 2 * MIN_TTL_SECONDS).unwrap();
        assert!(fixture.cli(&["creator-code-list"]).unwrap().contains("\texpired\t"));
    }

    #[test]
    fn operator_commands_leave_the_kernels_verification_mode_alone() {
        let fixture = fixture(Verification::Required, KEY);
        accounts::creator_store(&fixture.state).unwrap();
        // The operator's shell does not set KONAE_ACCOUNT_VERIFICATION.
        fixture.cli(&["creator-code-create"]).unwrap();
        fixture.cli(&["creator-code-list"]).unwrap();
        let recorded: String = Connection::open(owners_path(&fixture.root))
            .unwrap()
            .query_row("SELECT value FROM account_meta WHERE key = 'verification'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(recorded, "required");
    }

    #[tokio::test]
    async fn the_installation_key_creates_only_the_first_creator() {
        let fixture = fixture(Verification::Local, KEY);
        assert_eq!(mode(&fixture.state), Mode::SetupKey);
        let (status, text) = message(fixture.signup(json!({"email": "first@example.test", "password": PASSWORD, "setupKey": "wrong"})).await).await;
        assert_eq!((status, text.as_str()), (StatusCode::FORBIDDEN, KEY_REFUSED));
        let first = fixture.signup(json!({"email": "first@example.test", "password": PASSWORD, "setupKey": KEY})).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(mode(&fixture.state), Mode::Code);

        // The key is now refused, and the refusal does not mention it.
        let (status, text) = message(fixture.signup(json!({"email": "second@example.test", "password": PASSWORD, "setupKey": KEY})).await).await;
        assert_eq!((status, text.as_str()), (StatusCode::FORBIDDEN, CODE_REQUIRED));
        let (_, text) = message(fixture.signup(json!({"email": "second@example.test", "password": PASSWORD, "setupKey": "wrong"})).await).await;
        assert_eq!(text, CODE_REQUIRED);
        assert_eq!(fixture.accounts(), 1, "a refused key created no account");
    }

    #[tokio::test]
    async fn once_a_code_exists_the_installation_key_creates_no_account() {
        // Even on an empty store: issuing a code ends the key's signup path.
        let fixture = fixture(Verification::Required, KEY);
        let (id, _) = fixture.code(DEFAULT_TTL_SECONDS);
        assert_eq!(mode(&fixture.state), Mode::Code);
        let (status, text) = message(fixture.signup(json!({"email": "first@example.test", "setupKey": KEY})).await).await;
        assert_eq!((status, text.as_str()), (StatusCode::FORBIDDEN, CODE_REQUIRED));
        assert!(!text.to_lowercase().contains("key"));
        assert_eq!((fixture.accounts(), fixture.sent()), (0, 0));
        // Revoking every code does not reopen it.
        fixture.cli(&["creator-code-revoke", &id]).unwrap();
        assert_eq!(fixture.signup(json!({"email": "first@example.test", "setupKey": KEY})).await.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_development_key_keeps_creating_local_owners_until_a_code_exists() {
        let fixture = fixture(Verification::Local, DEFAULT_ADMIN_KEY);
        for email in ["one@example.test", "two@example.test"] {
            let body = json!({"email": email, "password": PASSWORD, "setupKey": DEFAULT_ADMIN_KEY});
            assert_eq!(fixture.signup(body).await.status(), StatusCode::OK);
        }
        assert_eq!(mode(&fixture.state), Mode::SetupKey);
        fixture.code(DEFAULT_TTL_SECONDS);
        assert_eq!(mode(&fixture.state), Mode::Code);
        let body = json!({"email": "three@example.test", "password": PASSWORD, "setupKey": DEFAULT_ADMIN_KEY});
        assert_eq!(message(fixture.signup(body).await).await, (StatusCode::FORBIDDEN, CODE_REQUIRED.to_owned()));
        assert_eq!(fixture.accounts(), 2);
    }

    #[tokio::test]
    async fn a_pending_first_creator_may_ask_the_key_for_a_new_link() {
        let fixture = fixture(Verification::Required, KEY);
        let body = json!({"email": "first@example.test", "setupKey": KEY});
        assert_eq!(fixture.signup(body.clone()).await.status(), StatusCode::ACCEPTED);
        let lost = fixture.last_link();
        assert_eq!(fixture.signup(body).await.status(), StatusCode::ACCEPTED);
        assert_eq!((fixture.accounts(), fixture.sent()), (1, 2));
        assert!(matches!(fixture.verify(&lost), Err(AuthError::InvalidChallenge)), "the newer link replaces the lost one");
        fixture.verify(&fixture.last_link()).unwrap();
        let other = json!({"email": "second@example.test", "setupKey": KEY});
        assert_eq!(fixture.signup(other).await.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_code_admits_one_creator_who_verifies_without_the_key() {
        let fixture = fixture(Verification::Required, KEY);
        // An operator who bootstrapped with the key, then opened admission.
        fixture.signup(json!({"email": "operator@example.test", "setupKey": KEY})).await;
        fixture.verify(&fixture.last_link()).unwrap();
        let (id, code) = fixture.code(DEFAULT_TTL_SECONDS);

        let signup = fixture.signup(json!({"email": "ada@example.test", "code": code.to_lowercase()})).await;
        assert_eq!(signup.status(), StatusCode::ACCEPTED);
        let link = fixture.last_link();
        assert!(link.starts_with("http://localhost/_konae/owner/login#verification="), "{link}");
        let listed = fixture.cli(&["creator-code-list"]).unwrap();
        assert!(listed.contains(&format!("{id}\tused\t")), "{listed}");
        assert!(listed.contains("ada@example.test (pending)"), "{listed}");
        let session = fixture.verify(&link).unwrap();
        assert_eq!(session.user.name, "ada@example.test");
        assert!(fixture.cli(&["creator-code-list"]).unwrap().contains("ada@example.test (active)"));
        assert!(fixture.cli(&["creator-code-revoke", &id]).unwrap_err().to_string().contains("creator-account-suspend"));

        // The same code is refused for anyone else, and creates nothing.
        let again = fixture.signup(json!({"email": "eve@example.test", "code": code})).await;
        assert_eq!(message(again).await, (StatusCode::FORBIDDEN, CODE_REFUSED.to_owned()));
        assert_eq!(fixture.accounts(), 2);
    }

    #[tokio::test]
    async fn a_code_in_local_mode_creates_an_active_creator_at_once() {
        let fixture = fixture(Verification::Local, KEY);
        let (_, code) = fixture.code(DEFAULT_TTL_SECONDS);
        let signup = fixture.signup(json!({"email": "ada@example.test", "password": PASSWORD, "code": code})).await;
        assert_eq!(signup.status(), StatusCode::OK);
        assert!(signup.headers().get(header::SET_COOKIE).is_some());
        // A taken email rolls the transaction back and keeps the code usable.
        let (_, other) = fixture.code(DEFAULT_TTL_SECONDS);
        let taken = fixture.signup(json!({"email": "ada@example.test", "password": PASSWORD, "code": other})).await;
        assert_eq!(taken.status(), StatusCode::CONFLICT);
        let next = fixture.signup(json!({"email": "bob@example.test", "password": PASSWORD, "code": other})).await;
        assert_eq!(next.status(), StatusCode::OK);
        // Without a valid code, a taken email and a new one get the same refusal.
        for email in ["ada@example.test", "nobody@example.test"] {
            let guessed = fixture.signup(json!({"email": email, "password": PASSWORD, "code": "00000-00000-00000-00000-00000"})).await;
            assert_eq!(message(guessed).await, (StatusCode::FORBIDDEN, CODE_REFUSED.to_owned()), "{email}");
        }
    }

    #[tokio::test]
    async fn expired_revoked_unknown_and_malformed_codes_are_refused() {
        let fixture = fixture(Verification::Required, KEY);
        let (_, expired, _) = create(&fixture.root, "", MIN_TTL_SECONDS, now_seconds() - MIN_TTL_SECONDS - 1).unwrap();
        let (revoked_id, revoked) = fixture.code(DEFAULT_TTL_SECONDS);
        fixture.cli(&["creator-code-revoke", &revoked_id]).unwrap();
        let unknown = new_code().unwrap();
        for code in [expired.as_str(), revoked.as_str(), unknown.as_str(), "not-a-code"] {
            let response = fixture.signup(json!({"email": "eve@example.test", "code": code})).await;
            assert_eq!(message(response).await, (StatusCode::FORBIDDEN, CODE_REFUSED.to_owned()), "{code}");
        }
        assert_eq!((fixture.accounts(), fixture.sent()), (0, 0));
        // A code wins over a key sent with it, so a bad code is not rescued by the key.
        let (_, text) = message(fixture.signup(json!({"email": "eve@example.test", "code": revoked, "setupKey": KEY})).await).await;
        assert_eq!(text, CODE_REFUSED);
    }

    #[tokio::test]
    async fn a_code_spent_on_an_existing_email_reveals_nothing_and_cannot_be_reused() {
        let fixture = fixture(Verification::Required, KEY);
        let (_, first) = fixture.code(DEFAULT_TTL_SECONDS);
        fixture.signup(json!({"email": "ada@example.test", "code": first})).await;
        fixture.verify(&fixture.last_link()).unwrap();
        let sent = fixture.sent();
        let (id, probe) = fixture.code(DEFAULT_TTL_SECONDS);
        let response = fixture.signup(json!({"email": "ada@example.test", "code": probe})).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED, "the same answer as a new email");
        assert_eq!(fixture.sent(), sent, "an active creator gets no link");
        assert!(fixture.cli(&["creator-code-list"]).unwrap().contains(&format!("{id}\tused\t")));
        let reuse = fixture.signup(json!({"email": "new@example.test", "code": probe})).await;
        assert_eq!(reuse.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn two_concurrent_signups_with_one_code_admit_one_creator() {
        let fixture = fixture(Verification::Required, KEY);
        let (_, code) = fixture.code(DEFAULT_TTL_SECONDS);
        let (a, b) = tokio::join!(
            fixture.signup(json!({"email": "a@example.test", "code": code})),
            fixture.signup(json!({"email": "b@example.test", "code": code})),
        );
        let mut statuses = [a.status(), b.status()];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::ACCEPTED, StatusCode::FORBIDDEN]);
        assert_eq!((fixture.accounts(), fixture.sent()), (1, 1));
    }

    #[tokio::test]
    async fn the_link_carries_only_an_oauth_consent_path_back() {
        let fixture = fixture(Verification::Required, KEY);
        let consent = "/oauth/authorize?client_id=abc&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcb&state=s";
        let (_, code) = fixture.code(DEFAULT_TTL_SECONDS);
        fixture.signup(json!({"email": "ada@example.test", "code": code, "returnTo": consent})).await;
        let link = fixture.last_link();
        let parsed = url::Url::parse(&link).unwrap();
        assert_eq!(parsed.path(), "/_konae/owner/login");
        let back: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
        assert_eq!(back, vec![("returnTo".to_owned(), consent.to_owned())]);
        assert!(parsed.fragment().unwrap().starts_with("verification="));
        // The verified creator holds the session the consent page asks for.
        let session = fixture.verify(&link).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost"));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{}={}", mcp::owner_cookie_name(&fixture.state), session.token)).unwrap(),
        );
        assert!(mcp::authenticate_owner(&fixture.state, &headers).unwrap().is_some());

        for foreign in ["https://evil.example/oauth/authorize?x", "//evil.example/", "/projects", "/oauth/authorize?a#b", "/oauth/authorize?\nx"] {
            assert_eq!(return_to(&json!({"returnTo": foreign})), None, "{foreign}");
        }
        assert_eq!(return_to(&json!({"returnTo": format!("/oauth/authorize?{}", "a".repeat(MAX_RETURN_BYTES))})), None);
        assert_eq!(login_page("http://localhost", None), "http://localhost/_konae/owner/login");
    }
}
