//! Account email: the persistent queue in `mail.sqlite`, the worker that
//! drains it, the transports (Resend, SMTP and the development mailbox),
//! Resend's delivery events, the sender preflight and the send limits.
//!
//! A challenge and its job commit in one transaction: the identity
//! connection attaches `mail.sqlite` and `JobOutbox::record` inserts the job
//! before the challenge commits (`auth::ChallengeOutbox`). The job keeps the
//! link sealed with AES-256-GCM until the provider accepts it, then drops it.
//! The worker sends a job only while its challenge is still open, so a link
//! that was replaced, used or expired is never mailed.

use std::{future::Future, net::IpAddr, pin::Pin, sync::Mutex as SyncMutex, time::Duration};

use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    hkdf, hmac,
    rand::{SecureRandom, SystemRandom},
};

use rusqlite::params;
use sha2::Digest;

use super::*;
use accounts::{AccountMail, AccountMessage, DevelopmentMailbox};
use auth::{AuthError, ChallengeOutbox, IssuedChallenge, Purpose};

pub(crate) const MAIL_DATABASE: &str = "mail.sqlite";
/// How long a claimed job stays with the worker before another pass may
/// claim it again. A Kernel stopped mid-send leaves the lease to expire.
const LEASE_MS: i64 = 2 * 60 * 1000;
/// Retries after the first attempt fail. The backoff doubles from 30
/// seconds and stops at an hour, so the last attempt runs about two hours
/// after the first, inside Resend's 24-hour idempotency window.
const MAX_ATTEMPTS: i64 = 8;
const FIRST_BACKOFF_MS: i64 = 30_000;
const MAX_BACKOFF_MS: i64 = 60 * 60 * 1000;
/// A job whose challenge is not visible yet is younger than this when the
/// two databases' commits are still landing; it waits instead of failing.
const COMMIT_GRACE_MS: i64 = 60_000;
/// Jobs claimed per worker pass.
const BATCH: i64 = 20;
/// Accepted age of a Resend event's `svix-timestamp`, in either direction.
/// Svix's own libraries use five minutes.
const WEBHOOK_TOLERANCE_SECONDS: i64 = 5 * 60;
/// How often the worker checks a refused sender again.
const PREFLIGHT_RETRY_MS: i64 = 10 * 60 * 1000;
const DEFAULT_RESEND_API: &str = "https://api.resend.com";
const DAY_MS: i64 = 86_400_000;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Configuration

/// Send limits. Each is a count over a sliding window.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Limits {
    pub(crate) recipient_per_minute: u32,
    pub(crate) recipient_per_day: u32,
    pub(crate) creator_per_day: u32,
    pub(crate) app_per_day: u32,
    pub(crate) installation_per_day: u32,
    pub(crate) client_per_hour: u32,
}

impl Default for Limits {
    /// The launch plan's values. The plan names no per-app or per-client
    /// value; an app gets its creator's daily allowance, and one client
    /// address may ask for 20 emails an hour.
    fn default() -> Self {
        Self {
            recipient_per_minute: 3,
            recipient_per_day: 10,
            creator_per_day: 100,
            app_per_day: 100,
            installation_per_day: 1000,
            client_per_hour: 20,
        }
    }
}

impl Limits {
    fn from_env() -> Result<Self> {
        let defaults = Self::default();
        let read = |name: &str, default: u32| -> Result<u32> {
            match std::env::var(name).as_deref() {
                Err(_) | Ok("") => Ok(default),
                Ok(value) => value
                    .parse::<u32>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| anyhow::anyhow!("{name}={value} is not a positive whole number.")),
            }
        };
        Ok(Self {
            recipient_per_minute: read("LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_MINUTE", defaults.recipient_per_minute)?,
            recipient_per_day: read("LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_DAY", defaults.recipient_per_day)?,
            creator_per_day: read("LITTLEBOX_MAIL_LIMIT_CREATOR_PER_DAY", defaults.creator_per_day)?,
            app_per_day: read("LITTLEBOX_MAIL_LIMIT_APP_PER_DAY", defaults.app_per_day)?,
            installation_per_day: read("LITTLEBOX_MAIL_LIMIT_INSTALLATION_PER_DAY", defaults.installation_per_day)?,
            client_per_hour: read("LITTLEBOX_MAIL_LIMIT_CLIENT_PER_HOUR", defaults.client_per_hour)?,
        })
    }
}

/// The key that seals links at rest. `LITTLEBOX_MAIL_SECRET` when set;
/// otherwise derived from `LITTLEBOX_ADMIN_KEY`, which every installation
/// reachable from another machine already keeps secret. The development
/// default key is public, so it never seals anything.
fn mail_secret(admin_key: &str) -> Result<String> {
    if let Some(secret) = std::env::var("LITTLEBOX_MAIL_SECRET").ok().filter(|value| !value.is_empty()) {
        anyhow::ensure!(secret.len() >= 32, "LITTLEBOX_MAIL_SECRET must be at least 32 characters.");
        return Ok(secret);
    }
    anyhow::ensure!(
        admin_key != DEFAULT_ADMIN_KEY,
        "Email needs a secret to encrypt queued links. Set LITTLEBOX_MAIL_SECRET, or set LITTLEBOX_ADMIN_KEY to a secret value."
    );
    Ok(admin_key.to_owned())
}

struct LinkKey(LessSafeKey);

impl LinkKey {
    /// HKDF-SHA256 with a fixed salt and label, so the admin key itself
    /// never encrypts anything and rotating either secret is one variable.
    fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"littlebox-mail-v1").extract(secret.as_bytes());
        let okm = prk.expand(&[b"account link"], &AES_256_GCM).expect("HKDF output fits AES-256");
        Self(LessSafeKey::new(UnboundKey::from(okm)))
    }

    /// Seals `link` for job `id`. The job ID is the associated data, so a
    /// sealed link moved to another row does not open.
    fn seal(&self, id: &str, link: &str) -> Result<(Vec<u8>, Vec<u8>), AuthError> {
        let mut nonce = [0_u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| AuthError::PasswordHash)?;
        let mut sealed = link.as_bytes().to_vec();
        self.0
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(id.as_bytes()), &mut sealed)
            .map_err(|_| AuthError::PasswordHash)?;
        Ok((nonce.to_vec(), sealed))
    }

    fn open(&self, id: &str, nonce: &[u8], sealed: &[u8]) -> Option<String> {
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut buffer = sealed.to_vec();
        let plain = self.0.open_in_place(nonce, Aad::from(id.as_bytes()), &mut buffer).ok()?;
        String::from_utf8(plain.to_vec()).ok()
    }
}

// ---------------------------------------------------------------------------
// Transports

/// One email ready for a transport. `id` is the job ID and the provider's
/// idempotency key, so a retry of the same job is the same request.
#[derive(Clone, Debug)]
pub(crate) struct OutgoingEmail {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) subject: String,
    pub(crate) text: String,
    pub(crate) html: String,
    pub(crate) account: AccountMessage,
}

/// What a transport did with one email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    /// The provider took the message. `delivered` is true when the transport
    /// is itself the inbox, as the development mailbox is.
    Accepted { provider_id: Option<String>, delivered: bool },
    /// Try again later: the provider is down, busy or rate limited.
    Retry(String),
    /// The provider refused this message; retrying sends the same refusal.
    Rejected(String),
    /// The provider refused the sender or the credentials. Sending stops for
    /// every job until a preflight passes again.
    SenderRefused(String),
}

/// The result of checking that a transport may send as its sender.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreflightResult {
    Ready,
    Refused(String),
}

pub(crate) trait Transport: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, email: &'a OutgoingEmail) -> BoxFuture<'a, SendOutcome>;
    fn preflight<'a>(&'a self, sender: &'a str) -> BoxFuture<'a, PreflightResult>;
    /// The secret that signs this provider's delivery events, if it sends any.
    fn webhook_secret(&self) -> Option<&[u8]> {
        None
    }
}

/// The development mailbox as a transport: each message is one line of
/// `mailbox.jsonl`, and writing it counts as delivery.
struct DevelopmentTransport(DevelopmentMailbox);

impl Transport for DevelopmentTransport {
    fn name(&self) -> &'static str {
        "development"
    }

    fn send<'a>(&'a self, email: &'a OutgoingEmail) -> BoxFuture<'a, SendOutcome> {
        Box::pin(async move {
            match self.0.deliver(&email.account) {
                Ok(()) => SendOutcome::Accepted { provider_id: None, delivered: true },
                Err(error_value) => SendOutcome::Retry(format!("mailbox write failed: {error_value}")),
            }
        })
    }

    fn preflight<'a>(&'a self, _sender: &'a str) -> BoxFuture<'a, PreflightResult> {
        Box::pin(async { PreflightResult::Ready })
    }
}

/// Resend's HTTP API: `POST /emails` with an `Idempotency-Key` header, and
/// `GET /domains` for the preflight.
/// <https://resend.com/docs/api-reference/emails/send-email>
pub(crate) struct ResendTransport {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    domains_key: Option<String>,
    webhook_secret: Option<Vec<u8>>,
}

impl ResendTransport {
    pub(crate) fn new(api_url: &str, api_key: &str, domains_key: Option<String>, webhook_secret: Option<Vec<u8>>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            api_url: api_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
            domains_key,
            webhook_secret,
        })
    }
}

/// The `name` and `message` of a Resend error body.
fn resend_error(body: &str) -> (String, String) {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let text = |key: &str| value[key].as_str().unwrap_or("").chars().take(200).collect::<String>();
    (text("name"), text("message"))
}

impl Transport for ResendTransport {
    fn name(&self) -> &'static str {
        "resend"
    }

    fn send<'a>(&'a self, email: &'a OutgoingEmail) -> BoxFuture<'a, SendOutcome> {
        Box::pin(async move {
            let body = json!({
                "from": email.from,
                "to": [email.account.to],
                "subject": email.subject,
                "text": email.text,
                "html": email.html,
            });
            let response = match self
                .client
                .post(format!("{}/emails", self.api_url))
                .bearer_auth(&self.api_key)
                .header("Idempotency-Key", &email.id)
                .json(&body)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error_value) => return SendOutcome::Retry(format!("Resend unreachable: {}", error_value.without_url())),
            };
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            if (200..300).contains(&status) {
                let id = serde_json::from_str::<Value>(&text).ok().and_then(|value| value["id"].as_str().map(str::to_owned));
                return match id {
                    Some(id) => SendOutcome::Accepted { provider_id: Some(id), delivered: false },
                    None => SendOutcome::Retry("Resend answered without an email ID".into()),
                };
            }
            let (name, message) = resend_error(&text);
            match (status, name.as_str()) {
                (401, _) => SendOutcome::SenderRefused(format!("Resend refused the API key ({name})")),
                // An unverified domain answers 403 validation_error.
                // https://resend.com/docs/api-reference/errors
                (403, _) => SendOutcome::SenderRefused(format!("Resend refused the sender: {message}")),
                (409, "concurrent_idempotent_requests") | (429, _) => SendOutcome::Retry(format!("Resend {status} {name}")),
                (status, _) if status >= 500 => SendOutcome::Retry(format!("Resend {status} {name}")),
                _ => SendOutcome::Rejected(format!("Resend {status} {name}")),
            }
        })
    }

    /// Passes when the sender's domain is `verified` in this Resend account.
    /// A key with `sending_access` cannot list domains, so it cannot pass on
    /// its own; `LITTLEBOX_RESEND_DOMAINS_KEY` supplies one that can.
    fn preflight<'a>(&'a self, sender: &'a str) -> BoxFuture<'a, PreflightResult> {
        Box::pin(async move {
            let Some(domain) = sender_domain(sender) else {
                return PreflightResult::Refused("LITTLEBOX_MAIL_FROM has no domain.".into());
            };
            let key = self.domains_key.as_deref().unwrap_or(&self.api_key);
            let response = match self.client.get(format!("{}/domains", self.api_url)).bearer_auth(key).send().await {
                Ok(response) => response,
                Err(error_value) => {
                    return PreflightResult::Refused(format!(
                        "Resend could not be reached to check the sender domain: {}",
                        error_value.without_url()
                    ));
                }
            };
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            if status == 401 || status == 403 {
                return PreflightResult::Refused(
                    "The Resend key cannot read domains, so the sender domain cannot be checked. Set LITTLEBOX_RESEND_DOMAINS_KEY to a full-access key.".into(),
                );
            }
            if !(200..300).contains(&status) {
                return PreflightResult::Refused(format!("Resend answered {status} when listing domains."));
            }
            let listed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let found = listed["data"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|item| item["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(&domain)));
            match found.and_then(|item| item["status"].as_str()) {
                Some("verified") => PreflightResult::Ready,
                Some(status) => PreflightResult::Refused(format!(
                    "The sender domain {domain} is {status} in Resend. Verify it there, then check again."
                )),
                None => PreflightResult::Refused(format!("The sender domain {domain} is not a domain of this Resend account.")),
            }
        })
    }

    fn webhook_secret(&self) -> Option<&[u8]> {
        self.webhook_secret.as_deref()
    }
}

/// How an SMTP connection is encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SmtpTls {
    /// Plain connection upgraded with STARTTLS, which is required.
    StartTls,
    /// TLS from the first byte, usually on port 465.
    Implicit,
    /// No encryption, accepted only for a loopback host, for tests.
    None,
}

/// SMTP through `lettre`. The server receives a stable `Message-ID` per job.
/// SMTP has no idempotency key, so a Kernel stopped between the server's
/// answer and the queue's record sends that one message twice.
pub(crate) struct SmtpTransport {
    mailer: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
}

impl SmtpTransport {
    pub(crate) fn new(host: &str, port: u16, tls: SmtpTls, credentials: Option<(String, String)>) -> Result<Self> {
        use lettre::{AsyncSmtpTransport, Tokio1Executor, transport::smtp::authentication::Credentials};
        let mut builder = match tls {
            SmtpTls::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?,
            SmtpTls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?,
            SmtpTls::None => {
                anyhow::ensure!(
                    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
                    "LITTLEBOX_SMTP_TLS=none is only accepted for a loopback host."
                );
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            }
        };
        builder = builder.port(port).timeout(Some(Duration::from_secs(30)));
        if let Some((user, password)) = credentials {
            builder = builder.credentials(Credentials::new(user, password));
        }
        Ok(Self { mailer: builder.build() })
    }
}

impl Transport for SmtpTransport {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn send<'a>(&'a self, email: &'a OutgoingEmail) -> BoxFuture<'a, SendOutcome> {
        use lettre::{AsyncTransport, Message, message::MultiPart};
        Box::pin(async move {
            let domain = sender_domain(&email.from).unwrap_or_else(|| "littlebox.invalid".into());
            let (Ok(from), Ok(to)) = (email.from.parse(), email.account.to.parse()) else {
                return SendOutcome::Rejected("an address is not valid for SMTP".into());
            };
            let message = match Message::builder()
                .from(from)
                .to(to)
                .subject(&email.subject)
                .message_id(Some(format!("<{}@{domain}>", email.id)))
                .multipart(MultiPart::alternative_plain_html(email.text.clone(), email.html.clone()))
            {
                Ok(message) => message,
                Err(error_value) => return SendOutcome::Rejected(format!("message could not be built: {error_value}")),
            };
            match self.mailer.send(message).await {
                Ok(response) => SendOutcome::Accepted {
                    provider_id: response.first_line().map(|line| line.chars().take(200).collect()),
                    delivered: false,
                },
                Err(error_value) => {
                    let code = error_value.status().map(|code| code.to_string()).unwrap_or_default();
                    if code == "530" || code == "535" {
                        SendOutcome::SenderRefused(format!("the SMTP server refused the credentials ({code})"))
                    } else if error_value.is_permanent() {
                        SendOutcome::Rejected(format!("SMTP {code}"))
                    } else {
                        SendOutcome::Retry(format!("SMTP {}", if code.is_empty() { "connection failed".into() } else { code }))
                    }
                }
            }
        })
    }

    /// Connects, negotiates TLS and authenticates. Whether the server lets
    /// this account send as the sender address is the provider's rule; a
    /// refusal there shows as failed jobs.
    fn preflight<'a>(&'a self, _sender: &'a str) -> BoxFuture<'a, PreflightResult> {
        Box::pin(async move {
            match self.mailer.test_connection().await {
                Ok(true) => PreflightResult::Ready,
                Ok(false) => PreflightResult::Refused("The SMTP server did not answer the connection check.".into()),
                Err(error_value) => PreflightResult::Refused(format!("The SMTP connection check failed: {error_value}")),
            }
        })
    }
}

fn sender_domain(sender: &str) -> Option<String> {
    let address = sender.rsplit_once('<').map_or(sender, |(_, rest)| rest.trim_end_matches('>'));
    let domain = address.trim().rsplit_once('@')?.1.trim().to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

/// Reads `LITTLEBOX_MAIL_TRANSPORT` and the variables of the chosen
/// transport. Unset means no transport.
fn transport_from_env(data_root: &Path) -> Result<Option<(Arc<dyn Transport>, String)>> {
    let required = |name: &str| -> Result<String> {
        std::env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("{name} is required for this mail transport."))
    };
    let optional = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let transport: Arc<dyn Transport> = match std::env::var("LITTLEBOX_MAIL_TRANSPORT").as_deref() {
        Err(_) | Ok("") => return Ok(None),
        Ok("development") => Arc::new(DevelopmentTransport(DevelopmentMailbox::new(data_root.join("mailbox.jsonl")))),
        Ok("resend") => {
            let secret = match optional("LITTLEBOX_RESEND_WEBHOOK_SECRET") {
                Some(value) => Some(webhook_key(&value)?),
                None => None,
            };
            Arc::new(ResendTransport::new(
                &optional("LITTLEBOX_RESEND_API_URL").unwrap_or_else(|| DEFAULT_RESEND_API.into()),
                &required("LITTLEBOX_RESEND_API_KEY")?,
                optional("LITTLEBOX_RESEND_DOMAINS_KEY"),
                secret,
            )?)
        }
        Ok("smtp") => {
            let tls = match optional("LITTLEBOX_SMTP_TLS").as_deref() {
                None | Some("starttls") => SmtpTls::StartTls,
                Some("implicit") => SmtpTls::Implicit,
                Some("none") => SmtpTls::None,
                Some(other) => anyhow::bail!("LITTLEBOX_SMTP_TLS={other} is not valid. Use starttls or implicit."),
            };
            let default_port = match tls {
                SmtpTls::Implicit => "465",
                SmtpTls::StartTls => "587",
                SmtpTls::None => "25",
            };
            let port = optional("LITTLEBOX_SMTP_PORT").unwrap_or_else(|| default_port.into()).parse::<u16>()?;
            let credentials = match (optional("LITTLEBOX_SMTP_USERNAME"), optional("LITTLEBOX_SMTP_PASSWORD")) {
                (Some(user), Some(password)) => Some((user, password)),
                (None, None) => None,
                _ => anyhow::bail!("Set both LITTLEBOX_SMTP_USERNAME and LITTLEBOX_SMTP_PASSWORD, or neither."),
            };
            Arc::new(SmtpTransport::new(&required("LITTLEBOX_SMTP_HOST")?, port, tls, credentials)?)
        }
        Ok(other) => anyhow::bail!("LITTLEBOX_MAIL_TRANSPORT={other} is not a known transport. Use resend, smtp or development."),
    };
    let from = match transport.name() {
        "development" => optional("LITTLEBOX_MAIL_FROM").unwrap_or_else(|| "Littlebox <littlebox@localhost>".into()),
        _ => required("LITTLEBOX_MAIL_FROM")?,
    };
    anyhow::ensure!(
        sender_domain(&from).is_some() && !from.contains(['\r', '\n']),
        "LITTLEBOX_MAIL_FROM must be an address such as 'Littlebox <no-reply@example.com>'."
    );
    Ok(Some((transport, from)))
}

/// The HMAC key of a Svix signing secret: the base64 after `whsec_`.
/// <https://docs.svix.com/receiving/verifying-payloads/how-manual>
fn webhook_key(secret: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let encoded = secret.strip_prefix("whsec_").unwrap_or(secret);
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("LITTLEBOX_RESEND_WEBHOOK_SECRET is not a whsec_ signing secret."))
}

// ---------------------------------------------------------------------------
// Templates

/// Subject, plain text and HTML for one account email. Only the app name
/// and the link vary. The app name is the Deployment's, escaped for HTML and
/// stripped of line breaks; the link is built by the Kernel from the
/// Deployment's validated origin. No agent-supplied text reaches a message.
pub(crate) fn render(purpose: Purpose, app_name: &str, link: &str) -> (String, String, String) {
    let name: String = app_name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    let name = if name.is_empty() { "Littlebox".to_owned() } else { name };
    let (subject, lead, action, expiry, ignore) = match purpose {
        Purpose::Verification => (
            format!("Confirm your email for {name}"),
            format!("Someone asked to create an account on {name} with this email address."),
            "Choose your password and confirm the account",
            "The link works once and expires in 24 hours.",
            "If you did not ask for this, ignore this email. No account becomes active without this link.",
        ),
        Purpose::Recovery => (
            format!("Reset your password for {name}"),
            format!("Someone asked to reset the password of your account on {name}."),
            "Choose a new password",
            "The link works once and expires in 30 minutes. Your other sessions end when you confirm.",
            "If you did not ask for this, ignore this email. Your password stays the same.",
        ),
        Purpose::Invitation => (
            format!("You are invited to {name}"),
            format!("You are invited to join {name} with this email address."),
            "Accept the invitation",
            "The link works once and expires in 7 days.",
            "If you do not expect this invitation, ignore this email.",
        ),
    };
    let text = format!("{lead}\n\n{action}:\n{link}\n\n{expiry}\n\n{ignore}\n");
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;line-height:1.5;color:#20232a\"><p>{}</p><p><a href=\"{}\">{}</a></p><p>{}</p><p style=\"color:#586174\">{}</p></body></html>",
        escape_html(&lead),
        escape_html(link),
        escape_html(action),
        escape_html(expiry),
        escape_html(ignore),
    );
    (subject, text, html)
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

// ---------------------------------------------------------------------------
// The queue

/// Where the preflight stands. Sending runs only in `Ready`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preflight {
    pub(crate) state: &'static str,
    pub(crate) reason: Option<String>,
    pub(crate) checked_at: Option<i64>,
}

/// Who an email is for and who asks for it, checked before a challenge is
/// issued so a refusal stores nothing.
pub(crate) struct Admission<'a> {
    /// The normalized recipient address.
    pub(crate) recipient: &'a str,
    pub(crate) client: IpAddr,
    /// `creator` or `deployment:<id>`.
    pub(crate) scope: &'a str,
    /// The creator who owns the app, when the email belongs to an app.
    pub(crate) creator: Option<&'a str>,
}

pub(crate) struct MailQueue {
    path: PathBuf,
    data_root: PathBuf,
    key: LinkKey,
    transport: Arc<dyn Transport>,
    from: String,
    limits: Limits,
    preflight: SyncMutex<Preflight>,
    wake: tokio::sync::Notify,
    interval: Duration,
}

/// A job claimed by the worker.
struct Claimed {
    id: String,
    purpose: String,
    recipient: String,
    scope: String,
    app_name: String,
    token_hash: String,
    nonce: Option<Vec<u8>>,
    sealed: Option<Vec<u8>>,
    link_expires_at: i64,
    attempts: i64,
    created_at: i64,
}

fn now() -> i64 {
    now_ms() as i64
}

fn purpose_named(name: &str) -> Option<Purpose> {
    match name {
        "verification" => Some(Purpose::Verification),
        "recovery" => Some(Purpose::Recovery),
        "invitation" => Some(Purpose::Invitation),
        _ => None,
    }
}

fn hashed(kind: &str, value: &str) -> String {
    hex::encode(sha2::Sha256::digest(format!("{kind}:{value}").as_bytes()))
}

fn storage(error_value: rusqlite::Error) -> AuthError {
    AuthError::Storage(error_value)
}

impl MailQueue {
    /// Builds the queue from the environment, or None when no transport is
    /// configured.
    pub(crate) fn from_env(data_root: &Path, admin_key: &str) -> Result<Option<Arc<Self>>> {
        let Some((transport, from)) = transport_from_env(data_root)? else {
            return Ok(None);
        };
        let interval = std::env::var("LITTLEBOX_MAIL_WORKER_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(5000);
        let queue = Self::open(
            data_root,
            transport,
            &from,
            &mail_secret(admin_key)?,
            Limits::from_env()?,
            Duration::from_millis(interval.max(50)),
        )?;
        Ok(Some(Arc::new(queue)))
    }

    pub(crate) fn open(
        data_root: &Path,
        transport: Arc<dyn Transport>,
        from: &str,
        secret: &str,
        limits: Limits,
        interval: Duration,
    ) -> Result<Self> {
        let queue = Self {
            path: data_root.join(MAIL_DATABASE),
            data_root: data_root.to_path_buf(),
            key: LinkKey::derive(secret),
            transport,
            from: from.to_owned(),
            limits,
            preflight: SyncMutex::new(Preflight { state: "checking", reason: None, checked_at: None }),
            wake: tokio::sync::Notify::new(),
            interval,
        };
        install(&queue.connection()?)?;
        Ok(queue)
    }

    fn connection(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open(&self.path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Ok(connection)
    }

    pub(crate) fn preflight(&self) -> Preflight {
        self.preflight.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn set_preflight(&self, result: PreflightResult) {
        let next = match result {
            PreflightResult::Ready => Preflight { state: "ready", reason: None, checked_at: Some(now()) },
            PreflightResult::Refused(reason) => {
                eprintln!("Email sending is off: {reason}");
                Preflight { state: "refused", reason: Some(reason), checked_at: Some(now()) }
            }
        };
        *self.preflight.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    /// Checks the sender with the transport and records the result.
    pub(crate) async fn run_preflight(&self) -> Preflight {
        let result = self.transport.preflight(&self.from).await;
        let ready = result == PreflightResult::Ready;
        self.set_preflight(result);
        if ready {
            self.wake.notify_one();
        }
        self.preflight()
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Refuses a request that would exceed a limit, or that this
    /// installation cannot send now. On success it counts the request
    /// against the recipient and the client address, whether or not a
    /// message follows, so the answer never depends on whether an account
    /// exists. The creator, app and installation limits count jobs.
    pub(crate) fn admit(&self, admission: &Admission<'_>) -> Result<(), AuthError> {
        let preflight = self.preflight();
        if preflight.state != "ready" {
            return Err(AuthError::MailBlocked(
                "This installation cannot send email right now, so it cannot send account links. Try again later.".into(),
            ));
        }
        let mut connection = self.connection().map_err(storage)?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage)?;
        let at = now();
        transaction.execute("DELETE FROM requests WHERE at <= ?1", [at - DAY_MS]).map_err(storage)?;
        let recipient = hashed("recipient", admission.recipient);
        let client = hashed("client", &admission.client.to_string());
        let requests = |dimension: &str, key: &str, since: i64| -> Result<u32, AuthError> {
            transaction
                .query_row(
                    "SELECT COUNT(*) FROM requests WHERE dimension = ?1 AND key = ?2 AND at > ?3",
                    params![dimension, key, since],
                    |row| row.get(0),
                )
                .map_err(storage)
        };
        let limits = &self.limits;
        if requests("recipient", &recipient, at - 60_000)? >= limits.recipient_per_minute {
            return Err(AuthError::MailLimit("Too many emails to this address. Wait a minute and try again.".into()));
        }
        if requests("recipient", &recipient, at - DAY_MS)? >= limits.recipient_per_day {
            return Err(AuthError::MailLimit(
                "This address has received the most account emails allowed today. Try again tomorrow.".into(),
            ));
        }
        if requests("client", &client, at - 3_600_000)? >= limits.client_per_hour {
            return Err(AuthError::MailLimit("Too many email requests from this network. Try again later.".into()));
        }
        check_job_budget(&transaction, "main", limits, admission.scope, admission.creator, at)?;
        for (dimension, key) in [("recipient", &recipient), ("client", &client)] {
            transaction
                .execute("INSERT INTO requests(dimension, key, at) VALUES (?1, ?2, ?3)", params![dimension, key, at])
                .map_err(storage)?;
        }
        transaction.commit().map_err(storage)
    }

    /// The outbox that records one challenge's job in the identity
    /// transaction. `page` is the account page the link opens, built from
    /// the Deployment's validated origin or the editor's.
    pub(crate) fn outbox<'a>(&'a self, scope: &'a str, creator: Option<&'a str>, app_name: &'a str, page: &'a str) -> JobOutbox<'a> {
        JobOutbox { queue: self, scope, creator, app_name, page }
    }

    // -- Worker -------------------------------------------------------------

    /// Sends the jobs that are due. Returns how many reached the provider.
    pub(crate) async fn run_once(&self) -> Result<usize> {
        if self.preflight().state != "ready" {
            return Ok(0);
        }
        let claimed = self.claim()?;
        let mut sent = 0;
        for job in claimed {
            if self.process(job).await? {
                sent += 1;
            }
            if self.preflight().state != "ready" {
                break;
            }
        }
        Ok(sent)
    }

    fn claim(&self) -> Result<Vec<Claimed>> {
        let connection = self.connection()?;
        let at = now();
        let mut statement = connection.prepare(
            "UPDATE jobs SET leased_until = ?1, attempts = attempts + 1, updated_at = ?2
             WHERE id IN (
               SELECT id FROM jobs WHERE state = 'pending' AND next_attempt_at <= ?2
                 AND (leased_until IS NULL OR leased_until < ?2)
               ORDER BY next_attempt_at LIMIT ?3)
             RETURNING id, purpose, recipient, scope, app_name, token_hash, link_nonce, link_sealed,
                       link_expires_at, attempts, created_at",
        )?;
        let rows = statement
            .query_map(params![at + LEASE_MS, at, BATCH], |row| {
                Ok(Claimed {
                    id: row.get(0)?,
                    purpose: row.get(1)?,
                    recipient: row.get(2)?,
                    scope: row.get(3)?,
                    app_name: row.get(4)?,
                    token_hash: row.get(5)?,
                    nonce: row.get(6)?,
                    sealed: row.get(7)?,
                    link_expires_at: row.get(8)?,
                    attempts: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The identity database that holds a scope's challenges.
    fn identity_path(&self, scope: &str) -> Option<PathBuf> {
        if scope == auth::CREATOR_SCOPE {
            return Some(self.data_root.join("owners.sqlite"));
        }
        let id = scope.strip_prefix("deployment:")?;
        (!id.is_empty() && !id.contains(['/', '\\', '.'])).then(|| self.data_root.join("deployments").join(id).join("identity.sqlite"))
    }

    /// Whether the job's challenge is still open: not used, replaced,
    /// revoked by a suspension or expired.
    fn challenge_open(&self, job: &Claimed) -> Result<bool> {
        let Some(path) = self.identity_path(&job.scope).filter(|path| path.is_file()) else {
            return Ok(false);
        };
        let connection = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_challenges WHERE token_hash = ?1 AND scope = ?2
               AND consumed_at IS NULL AND expires_at > ?3)",
            params![job.token_hash, job.scope, now() / 1000],
            |row| row.get(0),
        )?)
    }

    /// Runs one claimed job. Returns true when the provider accepted it.
    async fn process(&self, job: Claimed) -> Result<bool> {
        if job.link_expires_at * 1000 <= now() {
            self.finish_failed(&job.id, "link-expired")?;
            return Ok(false);
        }
        if !self.challenge_open(&job)? {
            if now() - job.created_at < COMMIT_GRACE_MS {
                self.release(&job.id, 5_000, false, None)?;
            } else {
                self.finish_failed(&job.id, "superseded")?;
            }
            return Ok(false);
        }
        let link = match (&job.nonce, &job.sealed) {
            (Some(nonce), Some(sealed)) => self.key.open(&job.id, nonce, sealed),
            _ => None,
        };
        let (Some(link), Some(purpose)) = (link, purpose_named(&job.purpose)) else {
            self.finish_failed(&job.id, "secret-unavailable")?;
            return Ok(false);
        };
        let (subject, text, html) = render(purpose, &job.app_name, &link);
        let email = OutgoingEmail {
            id: job.id.clone(),
            from: self.from.clone(),
            subject,
            text,
            html,
            account: AccountMessage {
                to: job.recipient.clone(),
                purpose,
                scope: job.scope.clone(),
                link,
                expires_at: job.link_expires_at,
            },
        };
        match self.transport.send(&email).await {
            SendOutcome::Accepted { provider_id, delivered } => {
                self.record_accepted(&job.id, provider_id.as_deref(), delivered)?;
                Ok(true)
            }
            SendOutcome::Retry(reason) => {
                eprintln!("Email job {} will retry: {reason}", job.id);
                if job.attempts >= MAX_ATTEMPTS {
                    self.finish_failed(&job.id, "provider-unavailable")?;
                } else {
                    let backoff = (FIRST_BACKOFF_MS << (job.attempts - 1).clamp(0, 20)).min(MAX_BACKOFF_MS);
                    self.release(&job.id, backoff, true, Some("provider-unavailable"))?;
                }
                Ok(false)
            }
            SendOutcome::Rejected(reason) => {
                eprintln!("Email job {} was rejected: {reason}", job.id);
                self.finish_failed(&job.id, "rejected")?;
                Ok(false)
            }
            SendOutcome::SenderRefused(reason) => {
                // The job did not fail; the installation's sender did. It
                // waits, with this attempt uncounted, for a passing preflight.
                self.release(&job.id, 0, false, Some("sender-refused"))?;
                self.set_preflight(PreflightResult::Refused(reason));
                Ok(false)
            }
        }
    }

    /// Returns a claimed job to pending after `delay_ms`. `counted` keeps
    /// the attempt the claim added.
    fn release(&self, id: &str, delay_ms: i64, counted: bool, error_kind: Option<&str>) -> Result<()> {
        let at = now();
        self.connection()?.execute(
            "UPDATE jobs SET leased_until = NULL, next_attempt_at = ?1, updated_at = ?2,
               attempts = attempts - ?3, error_kind = COALESCE(?4, error_kind)
             WHERE id = ?5 AND state = 'pending'",
            params![at + delay_ms, at, if counted { 0 } else { 1 }, error_kind, id],
        )?;
        Ok(())
    }

    /// Ends a job as failed and drops its link.
    fn finish_failed(&self, id: &str, kind: &str) -> Result<()> {
        self.connection()?.execute(
            "UPDATE jobs SET state = 'failed', error_kind = ?1, link_nonce = NULL, link_sealed = NULL,
               leased_until = NULL, updated_at = ?2
             WHERE id = ?3 AND state = 'pending'",
            params![kind, now(), id],
        )?;
        Ok(())
    }

    /// Records the provider's acceptance, drops the link, then applies any
    /// provider event that arrived before the ID was known.
    fn record_accepted(&self, id: &str, provider_id: Option<&str>, delivered: bool) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE jobs SET state = ?1, provider_id = ?2, link_nonce = NULL, link_sealed = NULL,
               leased_until = NULL, updated_at = ?3
             WHERE id = ?4 AND state = 'pending'",
            params![if delivered { "delivered" } else { "accepted" }, provider_id, now(), id],
        )?;
        if let Some(provider_id) = provider_id {
            let events = {
                let mut statement = transaction.prepare("SELECT type FROM provider_events WHERE email_id = ?1")?;
                statement.query_map([provider_id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for event in events {
                advance(&transaction, provider_id, &event)?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    // -- Status -------------------------------------------------------------

    fn counts(&self) -> Result<Value> {
        let connection = self.connection()?;
        let mut counts = json!({"pending": 0, "accepted": 0, "delivered": 0, "failed": 0});
        let mut statement = connection.prepare("SELECT state, COUNT(*) FROM jobs GROUP BY state")?;
        for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
            let (state, count) = row?;
            counts[state] = json!(count);
        }
        let last: Option<(String, i64, String)> = connection
            .query_row(
                "SELECT error_kind, updated_at, purpose FROM jobs WHERE error_kind IS NOT NULL ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        Ok(json!({
            "depth": counts["pending"],
            "jobs": counts,
            "lastFailure": last.map(|(kind, at, purpose)| json!({"kind": kind, "at": at, "purpose": purpose})),
        }))
    }

    /// The unauthenticated summary on `/_littlebox/health`: no address, no
    /// provider text.
    pub(crate) fn health(&self) -> Value {
        let preflight = self.preflight();
        let queue = self.counts().unwrap_or_else(|_| json!({"depth": null}));
        json!({
            "transport": self.transport.name(),
            "sending": preflight.state == "ready",
            "preflight": preflight.state,
            "depth": queue["depth"],
            "lastFailure": queue["lastFailure"],
        })
    }

    /// The operator's view for the `mail_status` tool.
    pub(crate) fn status(&self) -> Result<Value> {
        let preflight = self.preflight();
        let queue = self.counts()?;
        Ok(json!({
            "transport": self.transport.name(),
            "from": self.from,
            "sending": preflight.state == "ready",
            "preflight": preflight,
            "queue": queue,
            "limits": self.limits,
            "providerEvents": self.transport.webhook_secret().is_some(),
        }))
    }
}

/// The `mail_status` admin tool. Without a transport it says so and how to
/// configure one.
pub(crate) async fn status_tool(state: &AppState, refresh: bool) -> Result<Value> {
    let Some(queue) = state.accounts.queue.clone() else {
        return Ok(json!({
            "transport": null,
            "sending": false,
            "reason": "No mail transport is configured. Set LITTLEBOX_MAIL_TRANSPORT to resend, smtp or development.",
        }));
    };
    if refresh {
        queue.run_preflight().await;
    }
    tokio::task::spawn_blocking(move || queue.status()).await?
}

/// Refuses when the creator, the app or the installation has used its daily
/// allowance of jobs. `schema` is `main` in the queue's own connection and
/// `outbox` inside an identity transaction.
fn check_job_budget(
    connection: &Connection,
    schema: &str,
    limits: &Limits,
    scope: &str,
    creator: Option<&str>,
    at: i64,
) -> Result<(), AuthError> {
    let since = at - DAY_MS;
    let count = |filter: &str, value: Option<&str>| -> Result<u32, AuthError> {
        let sql = format!("SELECT COUNT(*) FROM {schema}.jobs WHERE created_at > ?1 {filter}");
        match value {
            Some(value) => connection.query_row(&sql, params![since, value], |row| row.get(0)),
            None => connection.query_row(&sql, params![since], |row| row.get(0)),
        }
        .map_err(storage)
    };
    if count("", None)? >= limits.installation_per_day {
        return Err(AuthError::MailLimit(
            "This installation has sent its daily allowance of account emails. Try again tomorrow.".into(),
        ));
    }
    if let Some(creator) = creator {
        if count("AND creator = ?2", Some(creator))? >= limits.creator_per_day {
            return Err(AuthError::MailLimit(
                "The apps of this creator have sent their daily allowance of account emails. Try again tomorrow.".into(),
            ));
        }
    }
    if scope.starts_with("deployment:") && count("AND scope = ?2", Some(scope))? >= limits.app_per_day {
        return Err(AuthError::MailLimit("This app has sent its daily allowance of account emails. Try again tomorrow.".into()));
    }
    Ok(())
}

/// Moves a job forward on a provider event. States only advance:
/// pending, then accepted, then delivered or failed. A late `email.sent`
/// after `email.delivered` changes nothing, and the first final state wins.
fn advance(connection: &Connection, provider_id: &str, event: &str) -> rusqlite::Result<usize> {
    let (state, kind) = match event {
        "email.sent" => ("accepted", None),
        "email.delivered" => ("delivered", None),
        "email.bounced" => ("failed", Some("bounced")),
        "email.failed" => ("failed", Some("provider-failed")),
        "email.suppressed" => ("failed", Some("suppressed")),
        _ => return Ok(0),
    };
    let rank = |state: &str| match state {
        "accepted" => 1,
        _ => 2,
    };
    connection.execute(
        "UPDATE jobs SET state = ?1, error_kind = COALESCE(?2, error_kind), link_nonce = NULL, link_sealed = NULL,
           updated_at = ?3
         WHERE provider_id = ?4
           AND (CASE state WHEN 'pending' THEN 0 WHEN 'accepted' THEN 1 ELSE 2 END) < ?5",
        params![state, kind, now(), provider_id, rank(state)],
    )
}

pub(crate) fn install(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS jobs (
           id TEXT PRIMARY KEY,
           purpose TEXT NOT NULL CHECK (purpose IN ('verification', 'invitation', 'recovery')),
           recipient TEXT NOT NULL,
           scope TEXT NOT NULL,
           creator TEXT,
           app_name TEXT NOT NULL,
           token_hash TEXT NOT NULL,
           link_nonce BLOB,
           link_sealed BLOB,
           link_expires_at INTEGER NOT NULL,
           state TEXT NOT NULL CHECK (state IN ('pending', 'accepted', 'delivered', 'failed')),
           attempts INTEGER NOT NULL DEFAULT 0,
           next_attempt_at INTEGER NOT NULL,
           leased_until INTEGER,
           provider_id TEXT UNIQUE,
           error_kind TEXT,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS jobs_due ON jobs(state, next_attempt_at);
         CREATE INDEX IF NOT EXISTS jobs_created ON jobs(created_at);
         CREATE INDEX IF NOT EXISTS jobs_recipient ON jobs(scope, recipient, purpose, state);
         CREATE TABLE IF NOT EXISTS requests (
           dimension TEXT NOT NULL,
           key TEXT NOT NULL,
           at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS requests_window ON requests(dimension, key, at);
         CREATE TABLE IF NOT EXISTS provider_events (
           id TEXT PRIMARY KEY,
           type TEXT NOT NULL,
           email_id TEXT,
           received_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS provider_events_email ON provider_events(email_id);",
    )
}

/// Records one challenge's job inside the identity transaction.
pub(crate) struct JobOutbox<'a> {
    queue: &'a MailQueue,
    scope: &'a str,
    creator: Option<&'a str>,
    app_name: &'a str,
    page: &'a str,
}

impl ChallengeOutbox for JobOutbox<'_> {
    fn database(&self) -> &Path {
        &self.queue.path
    }

    /// Rechecks the job budgets, ends the recipient's earlier pending job of
    /// the same purpose (its challenge was just replaced) and inserts the job.
    fn record(&self, transaction: &Connection, challenge: &IssuedChallenge, token_hash: &str) -> Result<(), AuthError> {
        if challenge.scope != self.scope {
            return Err(AuthError::InvalidChallenge);
        }
        let at = now();
        check_job_budget(transaction, "outbox", &self.queue.limits, self.scope, self.creator, at)?;
        transaction
            .execute(
                "UPDATE outbox.jobs SET state = 'failed', error_kind = 'superseded', link_nonce = NULL,
                   link_sealed = NULL, leased_until = NULL, updated_at = ?1
                 WHERE scope = ?2 AND recipient = ?3 AND purpose = ?4 AND state = 'pending'",
                params![at, self.scope, challenge.email, challenge.purpose.as_str()],
            )
            .map_err(storage)?;
        let id = Uuid::new_v4().to_string();
        let link = format!("{}#{}={}", self.page, challenge.purpose.as_str(), challenge.token);
        let (nonce, sealed) = self.queue.key.seal(&id, &link)?;
        transaction
            .execute(
                "INSERT INTO outbox.jobs (id, purpose, recipient, scope, creator, app_name, token_hash, link_nonce,
                   link_sealed, link_expires_at, state, attempts, next_attempt_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', 0, ?11, ?11, ?11)",
                params![
                    id,
                    challenge.purpose.as_str(),
                    challenge.email,
                    self.scope,
                    self.creator,
                    self.app_name,
                    token_hash,
                    nonce,
                    sealed,
                    challenge.expires_at,
                    at,
                ],
            )
            .map_err(storage)?;
        Ok(())
    }
}

/// Runs the worker on its interval and whenever a job is enqueued. Sending
/// happens on this task, never in a request handler.
pub(crate) fn spawn_worker(queue: Arc<MailQueue>) {
    tokio::spawn(async move {
        queue.run_preflight().await;
        loop {
            if let Err(error_value) = queue.run_once().await {
                eprintln!("Email worker pass failed: {error_value}");
            }
            let preflight = queue.preflight();
            if preflight.state == "refused" && preflight.checked_at.is_some_and(|at| now() - at > PREFLIGHT_RETRY_MS) {
                queue.run_preflight().await;
            }
            tokio::select! {
                _ = queue.wake.notified() => {}
                _ = tokio::time::sleep(queue.interval) => {}
            }
        }
    });
}

/// Ends the pending jobs of a restored copy: their links belong to
/// challenges the restore invalidates. Returns how many it ended.
pub(crate) fn end_pending_after_restore(path: &Path) -> Result<usize> {
    let connection = Connection::open(path)?;
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'jobs')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    Ok(connection.execute(
        "UPDATE jobs SET state = 'failed', error_kind = 'restored', link_nonce = NULL, link_sealed = NULL,
           leased_until = NULL, updated_at = ?1
         WHERE state = 'pending'",
        [now()],
    )?)
}

// ---------------------------------------------------------------------------
// Provider events

pub fn routes() -> Router<AppState> {
    Router::new().route("/_littlebox/mail/resend-events", post(resend_events))
}

/// Checks a Svix signature: HMAC-SHA256 over `{id}.{timestamp}.{body}` with
/// the decoded secret, compared in constant time against each `v1,`
/// signature in the header, with the timestamp within five minutes.
pub(crate) fn verify_svix(secret: &[u8], id: &str, timestamp: &str, signatures: &str, body: &[u8], now_seconds: i64) -> bool {
    use base64::Engine;
    let Ok(sent) = timestamp.parse::<i64>() else {
        return false;
    };
    if (now_seconds - sent).abs() > WEBHOOK_TOLERANCE_SECONDS || id.is_empty() {
        return false;
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut signed = Vec::with_capacity(id.len() + timestamp.len() + body.len() + 2);
    signed.extend_from_slice(id.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(timestamp.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);
    signatures.split_whitespace().any(|entry| {
        entry
            .strip_prefix("v1,")
            .and_then(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded).ok())
            .is_some_and(|signature| hmac::verify(&key, &signed, &signature).is_ok())
    })
}

/// Records one Resend event. Answers 404 when this installation takes no
/// events, 401 on a bad signature, and 200 for a duplicate, which changes
/// nothing. The handler touches `mail.sqlite` only, so no event can
/// activate an account.
async fn resend_events(State(state): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let Some(queue) = state.accounts.queue.clone() else {
        return error(StatusCode::NOT_FOUND, "This installation does not take email events.");
    };
    let Some(secret) = queue.transport.webhook_secret() else {
        return error(StatusCode::NOT_FOUND, "This installation does not take email events.");
    };
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or("");
    let id = header("svix-id").to_owned();
    if !verify_svix(secret, &id, header("svix-timestamp"), header("svix-signature"), &body, now() / 1000) {
        return error(StatusCode::UNAUTHORIZED, "Email event signature rejected.");
    }
    let event: Value = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Email event is not JSON."),
    };
    let kind = event["type"].as_str().unwrap_or("").to_owned();
    let email_id = event["data"]["email_id"].as_str().map(str::to_owned);
    let result = tokio::task::spawn_blocking(move || -> Result<bool> {
        let mut connection = queue.connection()?;
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO provider_events(id, type, email_id, received_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO NOTHING",
            params![id, kind, email_id, now()],
        )?;
        if inserted == 1 {
            if let Some(email_id) = &email_id {
                advance(&transaction, email_id, &kind)?;
            }
        }
        transaction.commit()?;
        Ok(inserted == 1)
    })
    .await;
    match result {
        Ok(Ok(fresh)) => Json(json!({"ok": true, "duplicate": !fresh})).into_response(),
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "Email event could not be stored."),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        net::Ipv4Addr,
        sync::atomic::{AtomicU16, Ordering},
    };

    const SECRET: &str = "a mail secret that is long enough for tests";
    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));

    /// A transport that answers from a script and records what it sent.
    struct Scripted {
        sent: SyncMutex<Vec<OutgoingEmail>>,
        outcomes: SyncMutex<VecDeque<SendOutcome>>,
        preflight: SyncMutex<PreflightResult>,
    }

    impl Scripted {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                sent: SyncMutex::new(Vec::new()),
                outcomes: SyncMutex::new(VecDeque::new()),
                preflight: SyncMutex::new(PreflightResult::Ready),
            })
        }
        fn then(&self, outcome: SendOutcome) {
            self.outcomes.lock().unwrap().push_back(outcome);
        }
        fn sent(&self) -> Vec<OutgoingEmail> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl Transport for Scripted {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn send<'a>(&'a self, email: &'a OutgoingEmail) -> BoxFuture<'a, SendOutcome> {
            Box::pin(async move {
                let outcome = self.outcomes.lock().unwrap().pop_front().unwrap_or(SendOutcome::Accepted {
                    provider_id: Some(format!("provider-{}", email.id)),
                    delivered: false,
                });
                if matches!(outcome, SendOutcome::Accepted { .. }) {
                    self.sent.lock().unwrap().push(email.clone());
                }
                outcome
            })
        }
        fn preflight<'a>(&'a self, _sender: &'a str) -> BoxFuture<'a, PreflightResult> {
            Box::pin(async move { self.preflight.lock().unwrap().clone() })
        }
        fn webhook_secret(&self) -> Option<&[u8]> {
            Some(b"webhook test secret")
        }
    }

    struct Fixture {
        state: AppState,
        root: PathBuf,
        app: Deployment,
        other: Deployment,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn deployment(host: &str, project: &str) -> Deployment {
        Deployment {
            id: Uuid::new_v4().to_string(),
            project_id: project.into(),
            project_name: "Tasks <b>& co</b>".into(),
            url: format!("http://{host}"),
            created_at: now_ms(),
            manifest: json!({"visibility": "public"}),
            hostname: host.into(),
            actions: "{}".into(),
            artifact_version: None,
            signup_policy: SignupPolicy::default(),
        }
    }

    fn queue_in(root: &Path, transport: Arc<dyn Transport>, limits: Limits) -> Arc<MailQueue> {
        Arc::new(MailQueue::open(root, transport, "Littlebox <no-reply@mail.example.test>", SECRET, limits, Duration::from_millis(50)).unwrap())
    }

    async fn fixture_with(transport: Arc<dyn Transport>, limits: Limits) -> Fixture {
        let root = std::env::temp_dir().join(format!("littlebox-mail-{}", Uuid::new_v4()));
        let app = deployment("tasks.localhost", "project-a");
        let other = deployment("notes.localhost", "project-b");
        let mut deployments = HashMap::new();
        for item in [&app, &other] {
            std::fs::create_dir_all(root.join("deployments").join(&item.id)).unwrap();
            deployments.insert(item.hostname.clone(), item.clone());
        }
        // Both projects belong to one creator.
        let projects = Connection::open(root.join("mcp.sqlite")).unwrap();
        projects
            .execute_batch(
                "CREATE TABLE projects(id TEXT PRIMARY KEY, owner TEXT NOT NULL, body TEXT NOT NULL);
                 INSERT INTO projects VALUES ('project-a', 'creator-1', '{}'), ('project-b', 'creator-1', '{}');",
            )
            .unwrap();
        let queue = queue_in(&root, transport, limits);
        queue.run_preflight().await;
        let state = AppState {
            deployments: Arc::new(RwLock::new(deployments)),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.clone(),
            copies: Default::default(),
            admin_key: "test-admin".into(),
            accounts: accounts::AccountSettings {
                verification: auth::Verification::Required,
                mail: None,
                queue: Some(queue),
            },
            public_origin: "http://localhost".into(),
            host_suffix: "localhost".into(),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        };
        Fixture { state, root, app, other }
    }

    async fn fixture(transport: Arc<dyn Transport>) -> Fixture {
        fixture_with(transport, Limits::default()).await
    }

    impl Fixture {
        fn queue(&self) -> Arc<MailQueue> {
            self.state.accounts.queue.clone().unwrap()
        }
        async fn signup(&self, app: &Deployment, email: &str) -> Result<(), AuthError> {
            accounts::signup_app(&self.state, app, email, CLIENT).await
        }
        fn db(&self) -> Connection {
            Connection::open(self.root.join(MAIL_DATABASE)).unwrap()
        }
        fn jobs(&self) -> Vec<(String, String, i64, Option<String>, Option<String>, bool)> {
            let db = self.db();
            let mut statement = db
                .prepare("SELECT id, state, attempts, provider_id, error_kind, link_sealed IS NOT NULL FROM jobs ORDER BY created_at, rowid")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        }
        fn challenges(&self, app: &Deployment) -> i64 {
            Connection::open(self.root.join("deployments").join(&app.id).join("identity.sqlite"))
                .unwrap()
                .query_row("SELECT COUNT(*) FROM account_challenges", [], |row| row.get(0))
                .unwrap()
        }
        fn account_state(&self, app: &Deployment, email: &str) -> Option<String> {
            Connection::open(self.root.join("deployments").join(&app.id).join("identity.sqlite"))
                .unwrap()
                .query_row("SELECT state FROM app_users WHERE email = ?1", [email], |row| row.get(0))
                .optional()
                .unwrap()
        }
        /// Makes every pending job due now.
        fn due_now(&self) {
            self.db().execute("UPDATE jobs SET next_attempt_at = 0 WHERE state = 'pending'", []).unwrap();
        }
    }

    fn token_of(email: &OutgoingEmail) -> String {
        email.account.link.split_once('=').unwrap().1.to_owned()
    }

    #[tokio::test]
    async fn a_signup_queues_one_sealed_job_and_the_worker_sends_it() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        let jobs = fixture.jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].1.as_str(), jobs[0].5), ("pending", true));
        // The link is sealed at rest: no stored column holds the token.
        let raw: Vec<u8> = fixture.db().query_row("SELECT link_sealed FROM jobs", [], |row| row.get(0)).unwrap();
        assert_eq!(fixture.challenges(&fixture.app), 1);

        assert_eq!(fixture.queue().run_once().await.unwrap(), 1);
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        let token = token_of(&sent[0]);
        assert!(!raw.windows(token.len()).any(|window| window == token.as_bytes()), "the token is not stored in clear");
        assert!(sent[0].account.link.starts_with("http://tasks.localhost/_littlebox/account#verification="));
        assert_eq!(sent[0].id, jobs[0].0, "the job ID is the idempotency key");
        assert_eq!(sent[0].subject, "Confirm your email for Tasks <b>& co</b>");
        assert!(sent[0].html.contains("Tasks &lt;b&gt;&amp; co&lt;/b&gt;") && !sent[0].html.contains("<b>"));
        let jobs = fixture.jobs();
        assert_eq!((jobs[0].1.as_str(), jobs[0].3.clone(), jobs[0].5), ("accepted", Some(format!("provider-{}", jobs[0].0)), false));
        // The link from the email verifies the account.
        let session = accounts::complete_app(&fixture.state, &fixture.app, Purpose::Verification, &token, "correct horse battery").await;
        assert!(session.is_ok());
        assert_eq!(fixture.queue().run_once().await.unwrap(), 0, "an accepted job is not sent again");
    }

    #[tokio::test]
    async fn a_refused_outbox_rolls_back_the_challenge() {
        let transport = Scripted::new();
        let limits = Limits { installation_per_day: 1, ..Limits::default() };
        let fixture = fixture_with(transport, limits).await;
        fixture.signup(&fixture.app, "first@example.test").await.unwrap();
        // Past the admission check, the outbox rechecks the budget inside the
        // identity transaction; its refusal leaves no account and no challenge.
        let queue = fixture.queue();
        let auth = accounts::app_store(&fixture.state, &fixture.app).unwrap();
        let scope = auth::deployment_scope(&fixture.app.id);
        let outbox = queue.outbox(&scope, Some("creator-1"), "Tasks", "http://tasks.localhost/_littlebox/account");
        let refused = auth.request_signup_with("second@example.test", Some(&outbox));
        assert!(matches!(refused, Err(AuthError::MailLimit(_))));
        assert_eq!(fixture.account_state(&fixture.app, "second@example.test"), None);
        assert_eq!(fixture.challenges(&fixture.app), 1);
        assert_eq!(fixture.jobs().len(), 1);
    }

    #[tokio::test]
    async fn a_resend_supersedes_the_earlier_job_and_an_expired_link_is_not_sent() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        let jobs = fixture.jobs();
        assert_eq!((jobs[0].1.as_str(), jobs[0].4.as_deref(), jobs[0].5), ("failed", Some("superseded"), false));
        assert_eq!(jobs[1].1, "pending");
        fixture.queue().run_once().await.unwrap();
        assert_eq!(transport.sent().len(), 1, "only the newest link is mailed");

        // A job whose link expired before the worker reached it fails unsent.
        fixture.signup(&fixture.other, "late@example.test").await.unwrap();
        fixture.db().execute("UPDATE jobs SET link_expires_at = 1 WHERE state = 'pending'", []).unwrap();
        fixture.queue().run_once().await.unwrap();
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(fixture.jobs()[2].4.as_deref(), Some("link-expired"));
    }

    #[tokio::test]
    async fn a_job_whose_challenge_is_gone_is_not_sent() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        // The account is suspended before the worker runs: its challenge ends.
        accounts::app_store(&fixture.state, &fixture.app).unwrap().suspend("person@example.test").unwrap();
        fixture.db().execute("UPDATE jobs SET created_at = created_at - ?1", [COMMIT_GRACE_MS + 1]).unwrap();
        fixture.queue().run_once().await.unwrap();
        assert!(transport.sent().is_empty());
        assert_eq!(fixture.jobs()[0].4.as_deref(), Some("superseded"));
    }

    #[tokio::test]
    async fn a_provider_outage_keeps_the_job_and_retries_with_backoff() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        transport.then(SendOutcome::Retry("Resend 503 service_unavailable".into()));
        fixture.queue().run_once().await.unwrap();
        let (_, state, attempts, _, kind, sealed) = fixture.jobs()[0].clone();
        assert_eq!((state.as_str(), attempts, kind.as_deref(), sealed), ("pending", 1, Some("provider-unavailable"), true));
        let next: i64 = fixture.db().query_row("SELECT next_attempt_at FROM jobs", [], |row| row.get(0)).unwrap();
        assert!(next > now() + FIRST_BACKOFF_MS - 5_000, "the retry waits");
        assert_eq!(fixture.queue().run_once().await.unwrap(), 0, "not before its time");
        assert_eq!(fixture.account_state(&fixture.app, "person@example.test").as_deref(), Some("pending"), "no one is activated");
        fixture.due_now();
        assert_eq!(fixture.queue().run_once().await.unwrap(), 1);
        assert_eq!(fixture.jobs()[0].1, "accepted");

        // Retries are bounded: the last allowed attempt ends the job as failed.
        fixture.signup(&fixture.other, "second@example.test").await.unwrap();
        for _ in 0..MAX_ATTEMPTS {
            transport.then(SendOutcome::Retry("Resend 500 application_error".into()));
            fixture.due_now();
            fixture.queue().run_once().await.unwrap();
        }
        let last = fixture.jobs()[1].clone();
        assert_eq!((last.1.as_str(), last.2, last.4.as_deref(), last.5), ("failed", MAX_ATTEMPTS, Some("provider-unavailable"), false));
        let health = fixture.queue().health();
        assert_eq!(health["lastFailure"]["kind"], "provider-unavailable");
        assert!(!health.to_string().contains("example.test"), "health names no address: {health}");
    }

    #[tokio::test]
    async fn an_unverified_sender_stops_sending_and_enqueues_nothing() {
        let transport = Scripted::new();
        *transport.preflight.lock().unwrap() = PreflightResult::Refused("The sender domain mail.example.test is pending in Resend.".into());
        let fixture = fixture(transport.clone()).await;
        let refused = fixture.signup(&fixture.app, "person@example.test").await;
        assert!(matches!(refused, Err(AuthError::MailBlocked(_))), "{refused:?}");
        assert!(fixture.jobs().is_empty());
        assert_eq!(fixture.challenges(&fixture.app), 0);
        let recover = accounts::recover_app(&fixture.state, &fixture.app, "person@example.test", CLIENT).await;
        assert!(matches!(recover, Err(AuthError::MailBlocked(_))));
        let status = status_tool(&fixture.state, false).await.unwrap();
        assert_eq!(status["sending"], false);
        assert_eq!(status["preflight"]["state"], "refused");
        assert!(status["preflight"]["reason"].as_str().unwrap().contains("pending in Resend"));

        // Once the domain verifies, a refresh enables sending.
        *transport.preflight.lock().unwrap() = PreflightResult::Ready;
        assert_eq!(status_tool(&fixture.state, true).await.unwrap()["sending"], true);
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();

        // A provider that refuses the sender at send time parks the queue:
        // the job stays pending with its attempt uncounted.
        transport.then(SendOutcome::SenderRefused("Resend refused the sender".into()));
        fixture.queue().run_once().await.unwrap();
        let job = fixture.jobs()[0].clone();
        assert_eq!((job.1.as_str(), job.2, job.4.as_deref()), ("pending", 0, Some("sender-refused")));
        assert_eq!(fixture.queue().preflight().state, "refused");
        assert_eq!(fixture.queue().run_once().await.unwrap(), 0);
        assert!(matches!(fixture.signup(&fixture.other, "new@example.test").await, Err(AuthError::MailBlocked(_))));
    }

    #[tokio::test]
    async fn the_recipient_limits_refuse_visibly_and_the_same_for_unknown_addresses() {
        let transport = Scripted::new();
        let fixture = fixture(transport).await;
        // Three per minute, whatever happens to the request: two signups and
        // a recovery for the now pending account.
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        fixture.signup(&fixture.other, "person@example.test").await.unwrap();
        accounts::recover_app(&fixture.state, &fixture.app, "person@example.test", CLIENT).await.unwrap();
        let jobs = fixture.jobs().len();
        let refused = fixture.signup(&fixture.app, "Person@Example.test").await;
        assert!(matches!(&refused, Err(AuthError::MailLimit(message)) if message.contains("this address")), "{refused:?}");
        assert_eq!(fixture.jobs().len(), jobs, "a refusal enqueues nothing");
        // An address with no account reaches the limit after the same number
        // of requests, so the refusal says nothing about who has an account.
        for _ in 0..3 {
            accounts::recover_app(&fixture.state, &fixture.app, "nobody@example.test", CLIENT).await.unwrap();
        }
        let unknown = accounts::recover_app(&fixture.state, &fixture.app, "nobody@example.test", CLIENT).await;
        assert!(matches!(unknown, Err(AuthError::MailLimit(_))));
        assert_eq!(fixture.jobs().len(), jobs);
    }

    #[tokio::test]
    async fn the_daily_recipient_creator_app_installation_and_client_limits_refuse() {
        async fn refused_at(limits: Limits, emails: &[&str], on_other: bool) -> (usize, String) {
            let fixture = fixture_with(Scripted::new(), limits).await;
            for (index, email) in emails.iter().enumerate() {
                let app = if on_other && index % 2 == 1 { &fixture.other } else { &fixture.app };
                if let Err(error_value) = fixture.signup(app, email).await {
                    let jobs = fixture.jobs().len();
                    assert_eq!(jobs, index, "nothing was enqueued for the refused request");
                    return (index, error_value.to_string());
                }
                // Clear the per-minute window between requests.
                fixture.db().execute("UPDATE requests SET at = at - 61000", []).unwrap();
            }
            panic!("no limit refused");
        }
        let distinct = ["a@example.test", "b@example.test", "c@example.test", "d@example.test"];
        let base = Limits { recipient_per_minute: 100, ..Limits::default() };
        let (at, message) = refused_at(Limits { recipient_per_day: 2, ..base.clone() }, &["same@example.test"; 4], false).await;
        assert_eq!(at, 2);
        assert!(message.contains("today"), "{message}");
        let (at, message) = refused_at(Limits { app_per_day: 2, ..base.clone() }, &distinct, false).await;
        assert_eq!(at, 2);
        assert!(message.contains("This app"), "{message}");
        // Two apps of one creator share the creator's allowance.
        let (at, message) = refused_at(Limits { creator_per_day: 3, ..base.clone() }, &distinct, true).await;
        assert_eq!(at, 3);
        assert!(message.contains("creator"), "{message}");
        let (at, message) = refused_at(Limits { installation_per_day: 2, ..base.clone() }, &distinct, true).await;
        assert_eq!(at, 2);
        assert!(message.contains("installation"), "{message}");
        let (at, message) = refused_at(Limits { client_per_hour: 3, ..base.clone() }, &distinct, true).await;
        assert_eq!(at, 3);
        assert!(message.contains("network"), "{message}");
    }

    #[tokio::test]
    async fn a_pending_job_survives_a_restart_and_a_restarted_worker_does_not_send_twice() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        // The Kernel stops before the worker runs. A new queue on the same
        // data directory, as after a restart, finds the job and sends it.
        let restarted = queue_in(&fixture.root, transport.clone(), Limits::default());
        restarted.run_preflight().await;
        assert_eq!(restarted.run_once().await.unwrap(), 1);
        assert_eq!(transport.sent().len(), 1);
        let again = queue_in(&fixture.root, transport.clone(), Limits::default());
        again.run_preflight().await;
        assert_eq!(again.run_once().await.unwrap(), 0);
        assert_eq!(transport.sent().len(), 1, "an accepted job is never sent again");

        // A job claimed by a worker that stopped mid-send stays leased until
        // the lease runs out, so a second worker does not send it alongside.
        fixture.signup(&fixture.other, "second@example.test").await.unwrap();
        let claimed = again.claim().unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(again.run_once().await.unwrap(), 0, "a leased job is not claimed twice");
        fixture.db().execute("UPDATE jobs SET leased_until = 1 WHERE state = 'pending'", []).unwrap();
        assert_eq!(again.run_once().await.unwrap(), 1);
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].id, claimed[0].id, "the retry carries the same idempotency key");
    }

    #[tokio::test]
    async fn the_worker_runs_on_its_own_task() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        spawn_worker(fixture.queue());
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        for _ in 0..100 {
            if !transport.sent().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(fixture.jobs()[0].1, "accepted");
    }

    // -- Provider events ------------------------------------------------------

    fn sign(secret: &[u8], id: &str, timestamp: i64, body: &str) -> String {
        use base64::Engine;
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
        let tag = hmac::sign(&key, format!("{id}.{timestamp}.{body}").as_bytes());
        format!("v1,{}", base64::engine::general_purpose::STANDARD.encode(tag.as_ref()))
    }

    async fn event(fixture: &Fixture, id: &str, kind: &str, email_id: &str, signature: Option<String>) -> (StatusCode, Value) {
        let body = json!({"type": kind, "created_at": "2026-11-06T10:00:00.000Z", "data": {"email_id": email_id}}).to_string();
        let timestamp = now() / 1000;
        let mut headers = HeaderMap::new();
        headers.insert("svix-id", HeaderValue::from_str(id).unwrap());
        headers.insert("svix-timestamp", HeaderValue::from_str(&timestamp.to_string()).unwrap());
        let signature = signature.unwrap_or_else(|| sign(b"webhook test secret", id, timestamp, &body));
        headers.insert("svix-signature", HeaderValue::from_str(&signature).unwrap());
        let response = resend_events(State(fixture.state.clone()), headers, axum::body::Bytes::from(body)).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[test]
    fn svix_signatures_are_checked_over_id_timestamp_and_raw_body() {
        let secret = b"0123456789abcdef";
        let body = br#"{"type":"email.delivered"}"#;
        let good = sign(secret, "msg_1", 1_000_000, std::str::from_utf8(body).unwrap());
        assert!(verify_svix(secret, "msg_1", "1000000", &good, body, 1_000_000));
        assert!(verify_svix(secret, "msg_1", "1000000", &format!("v1,AAAA {good}"), body, 1_000_010), "any listed signature may match");
        assert!(!verify_svix(b"another secret!!", "msg_1", "1000000", &good, body, 1_000_000), "wrong secret");
        assert!(!verify_svix(secret, "msg_2", "1000000", &good, body, 1_000_000), "another message ID");
        assert!(!verify_svix(secret, "msg_1", "1000000", &good, br#"{"type":"email.bounced"}"#, 1_000_000), "altered body");
        assert!(!verify_svix(secret, "msg_1", "1000000", &good, body, 1_000_000 + WEBHOOK_TOLERANCE_SECONDS + 1), "too old");
        assert!(!verify_svix(secret, "msg_1", "1000000", &good.replace("v1,", "v2,"), body, 1_000_000), "unknown version");
        assert!(!verify_svix(secret, "msg_1", "1000000", "", body, 1_000_000), "unsigned");
        use base64::Engine;
        let decoded = webhook_key(&format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode(secret))).unwrap();
        assert_eq!(decoded, secret);
    }

    #[tokio::test]
    async fn provider_events_are_signed_idempotent_ordered_and_never_activate_an_account() {
        let transport = Scripted::new();
        let fixture = fixture(transport.clone()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        fixture.queue().run_once().await.unwrap();
        let provider_id = fixture.jobs()[0].3.clone().unwrap();

        let (status, _) = event(&fixture, "msg_unsigned", "email.delivered", &provider_id, Some(String::new())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let forged = sign(b"not the secret", "msg_forged", now() / 1000, "{}");
        assert_eq!(event(&fixture, "msg_forged", "email.delivered", &provider_id, Some(forged)).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(fixture.jobs()[0].1, "accepted", "a rejected event changes nothing");

        let (status, body) = event(&fixture, "msg_1", "email.delivered", &provider_id, None).await;
        assert_eq!((status, body["duplicate"].clone()), (StatusCode::OK, json!(false)));
        assert_eq!(fixture.jobs()[0].1, "delivered");
        // A replay of the same event is recorded once and changes nothing.
        let updated: i64 = fixture.db().query_row("SELECT updated_at FROM jobs", [], |row| row.get(0)).unwrap();
        let (status, body) = event(&fixture, "msg_1", "email.delivered", &provider_id, None).await;
        assert_eq!((status, body["duplicate"].clone()), (StatusCode::OK, json!(true)));
        let after: i64 = fixture.db().query_row("SELECT updated_at FROM jobs", [], |row| row.get(0)).unwrap();
        assert_eq!(updated, after);
        // Out of order: a late `email.sent` and a later bounce do not undo delivery.
        event(&fixture, "msg_0", "email.sent", &provider_id, None).await;
        event(&fixture, "msg_2", "email.bounced", &provider_id, None).await;
        assert_eq!(fixture.jobs()[0].1, "delivered");
        let events: i64 = fixture.db().query_row("SELECT COUNT(*) FROM provider_events", [], |row| row.get(0)).unwrap();
        assert_eq!(events, 3);
        // Delivery is not verification.
        assert_eq!(fixture.account_state(&fixture.app, "person@example.test").as_deref(), Some("pending"));

        // An event that arrives before the worker records the provider ID
        // applies once it does.
        fixture.signup(&fixture.other, "second@example.test").await.unwrap();
        let job = fixture.jobs()[1].0.clone();
        event(&fixture, "msg_early", "email.bounced", &format!("provider-{job}"), None).await;
        fixture.queue().run_once().await.unwrap();
        let second = fixture.jobs()[1].clone();
        assert_eq!((second.1.as_str(), second.4.as_deref()), ("failed", Some("bounced")));
    }

    #[tokio::test]
    async fn the_event_route_is_closed_without_a_queue() {
        let fixture = fixture(Scripted::new()).await;
        let mut state = fixture.state.clone();
        state.accounts.queue = None;
        let response = resend_events(State(state), HeaderMap::new(), axum::body::Bytes::from_static(b"{}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- Resend over HTTP ------------------------------------------------------

    /// A local stand-in for Resend: `POST /emails` keeps one email per
    /// `Idempotency-Key` and answers with the status in `fail` when set;
    /// `GET /domains` lists one domain with `domain_status`.
    #[derive(Clone, Default)]
    struct FakeResend {
        requests: Arc<SyncMutex<Vec<(String, Value)>>>,
        emails: Arc<SyncMutex<HashMap<String, String>>>,
        fail: Arc<AtomicU16>,
        domain_status: Arc<SyncMutex<String>>,
        domains_auth: Arc<AtomicU16>,
    }

    async fn fake_resend() -> (FakeResend, String) {
        let fake = FakeResend::default();
        *fake.domain_status.lock().unwrap() = "verified".into();
        let emails = {
            let fake = fake.clone();
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let fake = fake.clone();
                async move {
                    assert_eq!(headers.get(header::AUTHORIZATION).unwrap(), "Bearer re_test_key");
                    let key = headers.get("idempotency-key").unwrap().to_str().unwrap().to_owned();
                    fake.requests.lock().unwrap().push((key.clone(), body));
                    let status = fake.fail.load(Ordering::SeqCst);
                    if status != 0 {
                        let name = if status == 403 { "validation_error" } else { "service_unavailable" };
                        return (StatusCode::from_u16(status).unwrap(), Json(json!({"statusCode": status, "name": name, "message": "The mail.example.test domain is not verified."})));
                    }
                    let mut emails = fake.emails.lock().unwrap();
                    let id = emails.entry(key).or_insert_with(|| Uuid::new_v4().to_string()).clone();
                    (StatusCode::OK, Json(json!({"id": id})))
                }
            })
        };
        let domains = {
            let fake = fake.clone();
            get(move || {
                let fake = fake.clone();
                async move {
                    let auth = fake.domains_auth.load(Ordering::SeqCst);
                    if auth != 0 {
                        return (StatusCode::from_u16(auth).unwrap(), Json(json!({"name": "restricted_api_key"})));
                    }
                    let status = fake.domain_status.lock().unwrap().clone();
                    (StatusCode::OK, Json(json!({"object": "list", "has_more": false, "data": [{"id": "d1", "name": "mail.example.test", "status": status}]})))
                }
            })
        };
        let router = Router::new().route("/emails", emails).route("/domains", domains);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (fake, format!("http://{address}"))
    }

    #[tokio::test]
    async fn resend_sends_with_the_job_id_as_idempotency_key_and_dedupes_a_restart() {
        let (fake, url) = fake_resend().await;
        let transport = Arc::new(ResendTransport::new(&url, "re_test_key", None, Some(b"webhook test secret".to_vec())).unwrap());
        let fixture = fixture(transport.clone()).await;
        assert_eq!(fixture.queue().preflight().state, "ready");
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        assert_eq!(fixture.queue().run_once().await.unwrap(), 1);
        let (id, state, _, provider_id, _, sealed) = fixture.jobs()[0].clone();
        assert_eq!((state.as_str(), sealed), ("accepted", false));
        let requests = fake.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, id);
        assert_eq!(requests[0].1["to"], json!(["person@example.test"]));
        assert_eq!(requests[0].1["from"], "Littlebox <no-reply@mail.example.test>");
        assert!(requests[0].1["text"].as_str().unwrap().contains("http://tasks.localhost/_littlebox/account#verification="));
        assert_eq!(provider_id.as_deref(), fake.emails.lock().unwrap().get(&id).map(String::as_str));

        // The Kernel stopped after Resend accepted the email and before the
        // queue recorded it. The restarted worker repeats the request with
        // the same key, and Resend answers with the same email.
        let (nonce, sealed) = fixture.queue().key.seal(&id, "http://tasks.localhost/_littlebox/account#verification=x").unwrap();
        fixture
            .db()
            .execute(
                "UPDATE jobs SET state = 'pending', provider_id = NULL, link_nonce = ?1, link_sealed = ?2",
                params![nonce, sealed],
            )
            .unwrap();
        let restarted = queue_in(&fixture.root, transport, Limits::default());
        restarted.run_preflight().await;
        assert_eq!(restarted.run_once().await.unwrap(), 1);
        let requests = fake.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].0, id, "same idempotency key");
        assert_eq!(fake.emails.lock().unwrap().len(), 1, "Resend holds one email");
        assert_eq!(fixture.jobs()[0].3, provider_id);
    }

    #[tokio::test]
    async fn resend_preflight_needs_a_verified_domain_and_a_key_that_can_read_it() {
        let (fake, url) = fake_resend().await;
        let transport = ResendTransport::new(&url, "re_test_key", None, None).unwrap();
        assert_eq!(transport.preflight("Littlebox <no-reply@mail.example.test>").await, PreflightResult::Ready);
        *fake.domain_status.lock().unwrap() = "pending".into();
        let pending = transport.preflight("no-reply@mail.example.test").await;
        assert!(matches!(&pending, PreflightResult::Refused(reason) if reason.contains("is pending")), "{pending:?}");
        assert!(matches!(transport.preflight("no-reply@other.example.test").await, PreflightResult::Refused(reason) if reason.contains("not a domain")));
        *fake.domain_status.lock().unwrap() = "verified".into();
        fake.domains_auth.store(401, Ordering::SeqCst);
        assert!(matches!(transport.preflight("no-reply@mail.example.test").await, PreflightResult::Refused(reason) if reason.contains("LITTLEBOX_RESEND_DOMAINS_KEY")));
        let unreachable = ResendTransport::new("http://127.0.0.1:9", "re_test_key", None, None).unwrap();
        assert!(matches!(unreachable.preflight("no-reply@mail.example.test").await, PreflightResult::Refused(_)));
    }

    #[tokio::test]
    async fn resend_outages_retry_and_an_unverified_sender_refuses() {
        let (fake, url) = fake_resend().await;
        let transport = ResendTransport::new(&url, "re_test_key", None, None).unwrap();
        let email = OutgoingEmail {
            id: "job-1".into(),
            from: "no-reply@mail.example.test".into(),
            subject: "s".into(),
            text: "t".into(),
            html: "h".into(),
            account: AccountMessage { to: "p@example.test".into(), purpose: Purpose::Recovery, scope: "creator".into(), link: "l".into(), expires_at: 1 },
        };
        fake.fail.store(503, Ordering::SeqCst);
        assert!(matches!(transport.send(&email).await, SendOutcome::Retry(_)));
        fake.fail.store(429, Ordering::SeqCst);
        assert!(matches!(transport.send(&email).await, SendOutcome::Retry(_)));
        fake.fail.store(403, Ordering::SeqCst);
        assert!(matches!(transport.send(&email).await, SendOutcome::SenderRefused(reason) if reason.contains("not verified")));
        fake.fail.store(422, Ordering::SeqCst);
        assert!(matches!(transport.send(&email).await, SendOutcome::Rejected(_)));
        fake.fail.store(0, Ordering::SeqCst);
        assert!(matches!(transport.send(&email).await, SendOutcome::Accepted { provider_id: Some(_), delivered: false }));
    }

    // -- SMTP -------------------------------------------------------------------

    /// A minimal SMTP server on loopback that accepts one session per
    /// connection and keeps each DATA payload.
    async fn fake_smtp() -> (Arc<SyncMutex<Vec<String>>>, u16) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let received = Arc::new(SyncMutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let store = received.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                let store = store.clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                    let mut data: Option<String> = None;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(body) = data.as_mut() {
                            if line == "." {
                                store.lock().unwrap().push(data.take().unwrap());
                                write.write_all(b"250 2.0.0 queued as FAKE1\r\n").await.unwrap();
                            } else {
                                body.push_str(&line);
                                body.push('\n');
                            }
                            continue;
                        }
                        let upper = line.to_ascii_uppercase();
                        let reply: &[u8] = if upper.starts_with("EHLO") {
                            b"250-fake\r\n250 8BITMIME\r\n"
                        } else if upper.starts_with("DATA") {
                            data = Some(String::new());
                            b"354 go ahead\r\n"
                        } else if upper.starts_with("QUIT") {
                            write.write_all(b"221 bye\r\n").await.unwrap();
                            return;
                        } else {
                            b"250 ok\r\n"
                        };
                        write.write_all(reply).await.unwrap();
                    }
                });
            }
        });
        (received, port)
    }

    #[tokio::test]
    async fn smtp_sends_with_a_stable_message_id_and_checks_the_connection() {
        let (received, port) = fake_smtp().await;
        let transport = Arc::new(SmtpTransport::new("127.0.0.1", port, SmtpTls::None, None).unwrap());
        assert_eq!(transport.preflight("no-reply@mail.example.test").await, PreflightResult::Ready);
        let fixture = fixture(transport).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        assert_eq!(fixture.queue().run_once().await.unwrap(), 1);
        let job = fixture.jobs()[0].clone();
        assert_eq!(job.1, "accepted");
        let messages = received.lock().unwrap().clone();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains(&format!("Message-ID: <{}@mail.example.test>", job.0)), "{}", messages[0]);
        assert!(messages[0].contains("Subject: Confirm your email for"));
        // Plain SMTP is refused for anything but loopback.
        assert!(SmtpTransport::new("smtp.example.test", 25, SmtpTls::None, None).is_err());
        let closed = SmtpTransport::new("127.0.0.1", 9, SmtpTls::None, None).unwrap();
        assert!(matches!(closed.preflight("no-reply@mail.example.test").await, PreflightResult::Refused(_)));
    }

    // -- Templates, secrets, restore ---------------------------------------------

    #[test]
    fn templates_escape_the_app_name_and_strip_line_breaks() {
        let (subject, text, html) = render(Purpose::Invitation, "Evil\r\nBcc: x@example.test <img src=x>", "http://app.localhost/_littlebox/account#invitation=abc");
        assert_eq!(subject, "You are invited to Evil Bcc: x@example.test <img src=x>");
        assert!(!subject.contains(['\r', '\n']));
        assert!(text.contains("http://app.localhost/_littlebox/account#invitation=abc"));
        assert!(html.contains("&lt;img src=x&gt;") && !html.contains("<img"));
        let (subject, _, _) = render(Purpose::Recovery, "  ", "l");
        assert_eq!(subject, "Reset your password for Littlebox");
    }

    #[test]
    fn a_sealed_link_opens_only_with_its_job_and_its_secret() {
        let key = LinkKey::derive(SECRET);
        let (nonce, sealed) = key.seal("job-1", "http://app/#recovery=token").unwrap();
        assert_eq!(key.open("job-1", &nonce, &sealed).as_deref(), Some("http://app/#recovery=token"));
        assert_eq!(key.open("job-2", &nonce, &sealed), None);
        assert_eq!(LinkKey::derive("another secret that is long enough!").open("job-1", &nonce, &sealed), None);
    }

    #[test]
    fn the_mail_secret_falls_back_to_the_admin_key_but_never_the_public_default() {
        assert!(std::env::var("LITTLEBOX_MAIL_SECRET").is_err());
        assert_eq!(mail_secret("an operator admin key").unwrap(), "an operator admin key");
        assert!(mail_secret(DEFAULT_ADMIN_KEY).is_err());
    }

    #[test]
    fn mail_refusals_answer_429_and_503() {
        assert_eq!(auth_error(AuthError::MailLimit("limit".into())).status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(auth_error(AuthError::MailBlocked("blocked".into())).status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(auth_error(AuthError::MailUnavailable).status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn the_signup_route_answers_429_past_a_limit() {
        let fixture = fixture_with(Scripted::new(), Limits { recipient_per_minute: 1, ..Limits::default() }).await;
        let request = |email: &str| {
            Request::builder()
                .method("POST")
                .uri("/_littlebox/auth/signup")
                .header(header::HOST, "tasks.localhost")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"email": email, "password": "correct horse battery"}).to_string()))
                .unwrap()
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("tasks.localhost"));
        let first = signup(State(fixture.state.clone()), headers.clone(), request("person@example.test")).await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let second = signup(State(fixture.state.clone()), headers, request("person@example.test")).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(fixture.jobs().len(), 1);
    }

    #[tokio::test]
    async fn a_restore_ends_pending_jobs_and_drops_their_links() {
        let fixture = fixture(Scripted::new()).await;
        fixture.signup(&fixture.app, "person@example.test").await.unwrap();
        assert_eq!(end_pending_after_restore(&fixture.root.join(MAIL_DATABASE)).unwrap(), 1);
        let job = fixture.jobs()[0].clone();
        assert_eq!((job.1.as_str(), job.4.as_deref(), job.5), ("failed", Some("restored"), false));
    }
}
