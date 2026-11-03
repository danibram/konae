//! A custom domain: one hostname alias per Deployment (ADR-0015).
//!
//! The owner names a hostname with `domain_set`. The Kernel checks the name
//! by the rules in `domain-cases.json`, which the CLI runs too, records the
//! claim in `deployments/<id>/domain.json` with a token, and answers with the
//! TXT record `_konae.<hostname>` that must hold the token. Each later
//! `domain_set` looks the record up through the Kernel's own DNS query; when
//! it holds the token, the alias serves. From then on a request whose `Host`
//! is the alias resolves to the Deployment exactly as its generated hostname
//! does, so the Origin check, the host-only cookies, the policy, the routes,
//! the file path and the maintenance page all treat it as the app's origin.
//!
//! The Kernel issues no certificate. The operator's proxy terminates TLS for
//! the alias and forwards `Host` unchanged.
//!
//! A token is only ever issued by the Kernel that checks it. A package
//! carries the hostname without its token, because a package manifest is
//! written by whoever uploads it, and the destination issues a new token.

use super::refusal::{refuse, Refusal};
use super::*;
use std::net::{IpAddr, SocketAddr};
use anyhow::{bail, ensure, Context};
use std::sync::OnceLock;

pub(crate) const RECORD_FILE: &str = "domain.json";
/// The label under the alias that holds the token.
pub(super) const TXT_LABEL: &str = "_konae";
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const LOOKUP_ATTEMPTS: usize = 2;

/// The claim a Deployment holds on a hostname.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Record {
    pub(super) hostname: String,
    pub(super) token: String,
    pub(super) verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) verified_at: Option<u64>,
    pub(super) created_at: u64,
}

// ---------------------------------------------------------------------------
// Hostname rules

/// The installation's own names, which no alias may take.
pub(super) struct Rules {
    pub(super) suffix: String,
    pub(super) installation_hosts: Vec<String>,
}

impl Rules {
    pub(super) fn of(state: &AppState) -> Self {
        let host = |value: &str| url::Url::parse(value).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase));
        let mut installation_hosts: Vec<String> = host(&state.public_origin).into_iter().collect();
        installation_hosts.extend(state.own_origins.iter().filter_map(|origin| host(origin)));
        Self { suffix: state.host_suffix.trim_start_matches('.').to_ascii_lowercase(), installation_hosts }
    }
}

/// The hostname in its stored form, or the refusal. The CLI checks the same
/// cases with `src/lib/domain-name.ts`, with the same words.
pub(super) fn check_hostname(raw: &str, rules: &Rules) -> std::result::Result<String, Refusal> {
    let trimmed = raw.trim();
    let hostname = trimmed.strip_suffix('.').unwrap_or(trimmed).to_lowercase();
    if hostname.is_empty() {
        return Err(refuse("domain.missing", "Name a hostname, such as notes.example.org."));
    }
    if hostname.len() > 253 {
        return Err(refuse("domain.too-long", "A hostname has at most 253 characters."));
    }
    let labels: Vec<&str> = hostname.split('.').collect();
    let valid_label = |label: &&str| {
        (1..=63).contains(&label.len())
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if !labels.iter().all(valid_label) {
        return Err(refuse("domain.name-invalid", format!(
            "{hostname} is not a valid DNS name. Use labels of letters, digits and hyphens, 1 to 63 characters each, that do not start or end with a hyphen."
        )));
    }
    if labels.len() < 2 {
        return Err(refuse("domain.one-label", format!("{hostname} needs at least two labels, such as notes.example.org.")));
    }
    if labels.last().is_some_and(|label| label.bytes().all(|b| b.is_ascii_digit())) {
        return Err(refuse("domain.ip", format!("{hostname} is an IP address. Name a hostname under a domain you control.")));
    }
    if !rules.suffix.is_empty() && (hostname == rules.suffix || hostname.ends_with(&format!(".{}", rules.suffix))) {
        return Err(refuse("domain.app-suffix", format!(
            "{hostname} is under this installation's app suffix {}, where Konae names apps itself. Name a hostname outside it.",
            rules.suffix
        )));
    }
    if hostname == "localhost" || hostname.ends_with(".localhost") {
        return Err(refuse("domain.local", format!("{hostname} is a local name. Name a hostname under a domain you control.")));
    }
    if rules.installation_hosts.iter().any(|host| host == &hostname) {
        return Err(refuse("domain.installation-host", format!("{hostname} is this installation's own host. Name a hostname no Konae host uses.")));
    }
    Ok(hostname)
}

// ---------------------------------------------------------------------------
// Records and the alias index

/// Alias to its Deployment, per data directory, so tests with their own data
/// directories in one process do not see each other's aliases.
type Index = HashMap<String, (String, bool)>;
static INDEXES: OnceLock<std::sync::Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();

fn with_index<T>(state: &AppState, f: impl FnOnce(&mut Index) -> T) -> T {
    let mut indexes = INDEXES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = indexes.entry(state.data_root.clone()).or_insert_with(|| load_index(&state.data_root));
    f(index)
}

/// Reads every `domain.json` on the volume, as `load_deployments` reads the
/// records. A copy restored into this directory brings its files along.
fn load_index(root: &Path) -> Index {
    let mut index = Index::new();
    let Ok(entries) = std::fs::read_dir(crate::layout::apps(&root)) else { return index };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if let Some(record) = std::fs::read(crate::layout::private(root, &id).join(RECORD_FILE)).ok().and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok()) {
            // Two folders cannot both hold a verified claim on one name, but a
            // hand-edited volume might; the first one read keeps it.
            let keep = index.get(&record.hostname).is_none_or(|(_, verified)| !verified && record.verified);
            if keep {
                index.insert(record.hostname, (id, record.verified));
            }
        }
    }
    index
}

/// Drops a deleted Deployment's custom domain, so the name answers nothing.
pub(super) async fn forget(state: &AppState, id: &str) -> Result<()> {
    let _domains = database_lock(state, "domains").await;
    write_record(state, id, None).await
}

pub(super) fn read_record(state: &AppState, id: &str) -> Option<Record> {
    let bytes = std::fs::read(crate::layout::private(&state.data_root, id).join(RECORD_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_record(state: &AppState, id: &str, record: Option<&Record>) -> Result<()> {
    let path = crate::layout::private(&state.data_root, id).join(RECORD_FILE);
    let previous = read_record(state, id);
    match record {
        Some(record) => write_atomic(&path, &serde_json::to_vec_pretty(record)?).await?,
        None => match tokio::fs::remove_file(&path).await {
            Err(error_value) if error_value.kind() != std::io::ErrorKind::NotFound => return Err(error_value.into()),
            _ => {}
        },
    }
    with_index(state, |index| {
        if let Some(previous) = previous {
            if index.get(&previous.hostname).is_some_and(|(holder, _)| holder == id) {
                index.remove(&previous.hostname);
            }
        }
        if let Some(record) = record {
            index.insert(record.hostname.clone(), (id.to_owned(), record.verified));
        }
    });
    Ok(())
}

fn request_host(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host.split(':').next()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The Deployment ID a verified alias serves.
pub(super) fn serving(state: &AppState, host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    with_index(state, |index| index.get(&host).filter(|(_, verified)| *verified).map(|(id, _)| id.clone()))
}

/// Whether any Deployment claims this name, verified or not. An `Origin` on
/// such a name counts as an app origin, which only refuses more.
pub(super) fn claimed(state: &AppState, host: &str) -> bool {
    with_index(state, |index| index.contains_key(host))
}

/// Whether the request arrived on a verified alias rather than on a host the
/// installation generated.
pub(super) fn is_alias_request(state: &AppState, headers: &HeaderMap) -> bool {
    request_host(headers).is_some_and(|host| serving(state, &host).is_some())
}

/// The address the app's email links and its migration notice use: the
/// verified alias when there is one, the generated URL otherwise.
pub(super) fn app_url(state: &AppState, deployment: &Deployment) -> String {
    match read_record(state, &deployment.id).filter(|record| record.verified) {
        Some(record) => deployment_url(&state.public_origin, &record.hostname),
        None => deployment.url.clone(),
    }
}

/// Refuses a name another existing Deployment serves.
async fn ensure_free(state: &AppState, id: &str, hostname: &str) -> Result<()> {
    let holder = with_index(state, |index| index.get(hostname).filter(|(holder, verified)| *verified && holder != id).map(|(holder, _)| holder.clone()));
    if let Some(holder) = holder {
        if state.deployments.read().await.values().any(|d| d.id == holder) {
            bail!("{hostname} already serves another Deployment on this installation. Clear it there with domain_clear first.");
        }
    }
    Ok(())
}

fn new_token() -> String {
    format!("konae-verify-{}", Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// DNS

/// The resolver the Kernel asks: `KONAE_DNS_RESOLVER` as `ip` or
/// `ip:port`, else the first `nameserver` in `/etc/resolv.conf`.
pub(super) fn resolver() -> Result<SocketAddr> {
    if let Ok(value) = std::env::var("KONAE_DNS_RESOLVER") {
        let value = value.trim();
        return value
            .parse::<SocketAddr>()
            .or_else(|_| value.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 53)))
            .map_err(|_| anyhow::anyhow!("KONAE_DNS_RESOLVER must be an IP address with an optional port, such as 1.1.1.1 or 127.0.0.1:5353."));
    }
    let conf = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    conf.lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|rest| rest.trim().split('%').next()?.parse::<IpAddr>().ok())
        .map(|ip| SocketAddr::new(ip, 53))
        .next()
        .context("No DNS resolver: set KONAE_DNS_RESOLVER or list a nameserver in /etc/resolv.conf.")
}

fn query(id: u16, name: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(name.len() + 18);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 16, 0, 1]);
    packet
}

/// The offset after a name, following no pointer.
fn skip_name(packet: &[u8], mut at: usize) -> Result<usize> {
    loop {
        let length = *packet.get(at).context("The DNS answer ends inside a name.")? as usize;
        if length == 0 {
            return Ok(at + 1);
        }
        if length & 0xc0 == 0xc0 {
            return Ok(at + 2);
        }
        ensure!(length & 0xc0 == 0, "The DNS answer has a malformed name.");
        at += length + 1;
    }
}

/// The TXT strings in an answer, each record's character strings joined.
/// `None` means the name does not exist or has no TXT record.
fn parse_answer(packet: &[u8], id: u16) -> Result<Option<Vec<String>>> {
    ensure!(packet.len() >= 12, "The DNS answer is too short.");
    ensure!(u16::from_be_bytes([packet[0], packet[1]]) == id, "The DNS answer has another ID.");
    ensure!(packet[2] & 0x80 != 0, "The DNS server sent a query, not an answer.");
    ensure!(packet[2] & 0x02 == 0, "The DNS answer was truncated.");
    match packet[3] & 0x0f {
        0 => {}
        3 => return Ok(None),
        code => bail!("The DNS server answered with code {code}."),
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(packet, at)? + 4;
    }
    let mut texts = Vec::new();
    for _ in 0..answers {
        at = skip_name(packet, at)?;
        let header = packet.get(at..at + 10).context("The DNS answer ends inside a record.")?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = u16::from_be_bytes([header[8], header[9]]) as usize;
        at += 10;
        let data = packet.get(at..at + length).context("The DNS answer ends inside a record.")?;
        at += length;
        if kind != 16 {
            continue;
        }
        let mut text = Vec::new();
        let mut cursor = 0;
        while cursor < data.len() {
            let size = data[cursor] as usize;
            text.extend_from_slice(data.get(cursor + 1..cursor + 1 + size).context("The TXT record is malformed.")?);
            cursor += 1 + size;
        }
        texts.push(String::from_utf8_lossy(&text).into_owned());
    }
    Ok((!texts.is_empty()).then_some(texts))
}

/// Asks `server` for the TXT records of `name` over UDP.
pub(super) async fn lookup_txt(server: SocketAddr, name: &str) -> Result<Option<Vec<String>>> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let socket = tokio::net::UdpSocket::bind(bind).await?;
    socket.connect(server).await?;
    let mut buffer = vec![0u8; 4096];
    for _ in 0..LOOKUP_ATTEMPTS {
        let id: u16 = rand_u16();
        socket.send(&query(id, name)).await?;
        let deadline = tokio::time::Instant::now() + LOOKUP_TIMEOUT;
        loop {
            match tokio::time::timeout_at(deadline, socket.recv(&mut buffer)).await {
                Err(_) => break,
                Ok(Err(error_value)) => return Err(error_value.into()),
                // A stray datagram with another ID is not our answer.
                Ok(Ok(size)) if size >= 2 && u16::from_be_bytes([buffer[0], buffer[1]]) != id => continue,
                Ok(Ok(size)) => return parse_answer(&buffer[..size], id),
            }
        }
    }
    bail!("{server} did not answer within {} seconds.", LOOKUP_TIMEOUT.as_secs() * LOOKUP_ATTEMPTS as u64)
}

fn rand_u16() -> u16 {
    let bytes = Uuid::new_v4().into_bytes();
    u16::from_be_bytes([bytes[0], bytes[1]])
}

// ---------------------------------------------------------------------------
// Tools

fn view(state: &AppState, deployment: &Deployment, record: Option<&Record>, dns: Option<String>) -> Value {
    let Some(record) = record else {
        return json!({"deploymentId": deployment.id, "hostname": null, "status": "none", "generatedUrl": deployment.url});
    };
    let name = format!("{TXT_LABEL}.{}", record.hostname);
    let mut value = json!({
        "deploymentId": deployment.id,
        "hostname": record.hostname,
        "status": if record.verified { "active" } else { "pending" },
        "url": deployment_url(&state.public_origin, &record.hostname),
        "generatedUrl": deployment.url,
        "record": {"type": "TXT", "name": name, "value": record.token},
        "verifiedAt": record.verified_at,
    });
    if record.verified {
        value["next"] = json!("The alias serves. Your proxy must terminate TLS for it and forward Host unchanged.");
    } else {
        value["next"] = json!(format!("Add a TXT record {name} holding {}, then call domain_set again with {}.", record.token, record.hostname));
    }
    if let Some(dns) = dns {
        value["dns"] = json!(dns);
    }
    value
}

/// Reads, sets or clears a Deployment's custom domain. The caller has
/// checked who may.
pub(crate) async fn admin_call(state: &AppState, deployment: &Deployment, name: &str, a: &Value) -> Result<Value> {
    match name {
        "domain_read" => {
            let mut value = view(state, &deployment, read_record(state, &deployment.id).as_ref(), None);
            // The CLI checks a name by these before it calls domain_set.
            let rules = Rules::of(state);
            value["rules"] = json!({"suffix": rules.suffix, "installationHosts": rules.installation_hosts});
            Ok(value)
        }
        "domain_clear" => {
            let _domains = database_lock(state, "domains").await;
            let previous = read_record(state, &deployment.id);
            write_record(state, &deployment.id, None).await?;
            Ok(json!({"deploymentId": deployment.id, "cleared": previous.map(|record| record.hostname), "status": "none", "generatedUrl": deployment.url}))
        }
        "domain_set" => set(state, deployment, a["hostname"].as_str().unwrap_or_default(), resolver()).await,
        _ => bail!("Unknown tool."),
    }
}

async fn set(state: &AppState, deployment: &Deployment, raw: &str, server: Result<SocketAddr>) -> Result<Value> {
    let hostname = check_hostname(raw, &Rules::of(state))?;
    let (record, fresh) = {
        let _domains = database_lock(state, "domains").await;
        ensure_free(state, &deployment.id, &hostname).await?;
        let existing = read_record(state, &deployment.id);
        if let Some(record) = existing.as_ref().filter(|record| record.hostname == hostname && record.verified) {
            return Ok(view(state, &deployment, Some(record), None));
        }
        // One token per Deployment, kept when the hostname changes, so the
        // owner can add the record for a new name before switching to it.
        let token = existing.as_ref().map(|record| record.token.clone()).unwrap_or_else(new_token);
        let (record, fresh) = match existing {
            Some(record) if record.hostname == hostname => (record, false),
            _ => (Record { hostname: hostname.clone(), token, verified: false, verified_at: None, created_at: now_ms() }, true),
        };
        write_record(state, &deployment.id, Some(&record)).await?;
        (record, fresh)
    };
    let name = format!("{TXT_LABEL}.{hostname}");
    let found = match server {
        Ok(server) => lookup_txt(server, &name).await,
        Err(error_value) => Err(error_value),
    };
    let values = match found {
        Ok(Some(values)) => values,
        Ok(None) => return Ok(view(state, &deployment, Some(&record), Some(format!("No TXT record at {name} yet.")))),
        Err(error_value) => return Ok(view(state, &deployment, Some(&record), Some(format!("The DNS lookup of {name} failed: {error_value:#}")))),
    };
    if !values.iter().any(|value| value.trim() == record.token) {
        let shown = values.iter().map(|value| format!("\"{}\"", value.chars().take(80).collect::<String>())).collect::<Vec<_>>().join(", ");
        // The owner could not have known a token this call issued.
        if fresh {
            return Ok(view(state, &deployment, Some(&record), Some(format!("The TXT record at {name} holds {shown}, not this token yet."))));
        }
        bail!("The TXT record at {name} holds {shown}, not this Deployment's token. Set it to {} and call domain_set again.", record.token);
    }
    let _domains = database_lock(state, "domains").await;
    ensure_free(state, &deployment.id, &hostname).await?;
    // Another call may have cleared or replaced the claim during the lookup.
    let Some(mut current) = read_record(state, &deployment.id).filter(|current| current.hostname == hostname && current.token == record.token) else {
        bail!("The custom domain of this Deployment changed during the check. Call domain_set again.");
    };
    if !current.verified {
        current.verified = true;
        current.verified_at = Some(now_ms());
        write_record(state, &deployment.id, Some(&current)).await?;
    }
    Ok(view(state, &deployment, Some(&current), Some(format!("{name} holds the token."))))
}

// ---------------------------------------------------------------------------
// Packages

/// The hostname a package carries, without its token.
pub(super) fn for_package(state: &AppState, id: &str) -> Option<String> {
    read_record(state, id).map(|record| record.hostname)
}

/// Records a package's hostname on the Deployment it is installed as, with a
/// token this Kernel issues. It serves only after `domain_set` finds the new
/// token in DNS.
pub(super) async fn arrive(state: &AppState, id: &str, hostname: Option<&str>) -> Result<()> {
    let Some(raw) = hostname else { return Ok(()) };
    let hostname = check_hostname(raw, &Rules::of(state)).map_err(|message| anyhow::anyhow!("The package's custom domain: {message}"))?;
    let _domains = database_lock(state, "domains").await;
    ensure_free(state, &id, &hostname).await?;
    let record = Record { hostname, token: new_token(), verified: false, verified_at: None, created_at: now_ms() };
    write_record(state, &id, Some(&record)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI runs the same file in src/lib/domain-name.test.ts.
    #[test]
    fn hostnames_are_refused_with_the_words_the_cli_uses() {
        let file: Value = serde_json::from_str(include_str!("domain-cases.json")).unwrap();
        let rules = Rules {
            suffix: file["context"]["suffix"].as_str().unwrap().into(),
            installation_hosts: serde_json::from_value(file["context"]["installationHosts"].clone()).unwrap(),
        };
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for case in cases {
            let outcome = check_hostname(case["hostname"].as_str().unwrap(), &rules);
            if case["error"].is_null() {
                assert_eq!(outcome, Ok(case["hostnameOut"].as_str().unwrap().to_owned()), "{}", case["case"]);
            } else {
                crate::refusal::assert_case(case, outcome);
            }
        }
    }

    type Zone = Arc<std::sync::Mutex<HashMap<String, Vec<String>>>>;

    /// A resolver on a local port that answers TXT queries from a table, the
    /// way the probes' fake resolver does.
    async fn fake_resolver(zone: Zone) -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 512];
            loop {
                let Ok((size, peer)) = socket.recv_from(&mut buffer).await else { return };
                let end = skip_name(&buffer[..size], 12).unwrap();
                let mut labels = Vec::new();
                let mut at = 12;
                while buffer[at] != 0 {
                    let length = buffer[at] as usize;
                    labels.push(String::from_utf8_lossy(&buffer[at + 1..at + 1 + length]).into_owned());
                    at += length + 1;
                }
                let answers = zone.lock().unwrap().get(&labels.join(".")).cloned();
                let mut reply = buffer[..2].to_vec();
                reply.extend_from_slice(&[0x81, if answers.is_some() { 0x80 } else { 0x83 }, 0, 1]);
                let answers = answers.unwrap_or_default();
                reply.extend_from_slice(&(answers.len() as u16).to_be_bytes());
                reply.extend_from_slice(&[0, 0, 0, 0]);
                reply.extend_from_slice(&buffer[12..end + 4]);
                for value in answers {
                    // A record split into two character strings, as long TXT values are.
                    let (first, second) = value.split_at(value.len() / 2);
                    reply.extend_from_slice(&[0xc0, 0x0c, 0, 16, 0, 1, 0, 0, 0, 60]);
                    reply.extend_from_slice(&((first.len() + second.len() + 2) as u16).to_be_bytes());
                    reply.push(first.len() as u8);
                    reply.extend_from_slice(first.as_bytes());
                    reply.push(second.len() as u8);
                    reply.extend_from_slice(second.as_bytes());
                }
                let _ = socket.send_to(&reply, peer).await;
            }
        });
        address
    }

    fn zone(records: &[(&str, &[&str])]) -> Zone {
        Arc::new(std::sync::Mutex::new(
            records.iter().map(|(name, values)| (name.to_string(), values.iter().map(|v| v.to_string()).collect())).collect(),
        ))
    }

    #[tokio::test]
    async fn the_lookup_reads_txt_records_and_tells_a_missing_name_apart() {
        let server = fake_resolver(zone(&[("_konae.notes.example.org", &["konae-verify-abc", "v=spf1 -all"])])).await;
        assert_eq!(
            lookup_txt(server, "_konae.notes.example.org").await.unwrap(),
            Some(vec!["konae-verify-abc".to_owned(), "v=spf1 -all".to_owned()])
        );
        assert_eq!(lookup_txt(server, "_konae.other.example.org").await.unwrap(), None);
    }

    fn test_state(root: &Path) -> AppState {
        AppState {
            public_origin: "https://konae.example.com".into(),
            host_suffix: "apps.example.com".into(),
            ..AppState::for_test(&root)
        }
    }

    async fn publish(state: &AppState, project: &str) -> Deployment {
        let actions = json!({"version": 1, "actions": {"items.count": {"kind": "query", "access": "anonymous",
            "statement": "SELECT 1 AS n LIMIT 1", "bindings": [], "http": {"method": "GET", "path": "/api/count"}}}});
        let body = json!({
            "project": {"id": project, "name": project, "slug": project, "manifest": {},
                "files": {"actions.json": actions.to_string(), "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        state.deployments.read().await.values().find(|d| d.project_id == project).cloned().unwrap()
    }

    fn host(name: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(name).unwrap());
        headers
    }

    /// The alias serves only once the Kernel's own lookup finds the token,
    /// then answers as the app's origin next to the generated hostname, and
    /// one name serves one Deployment.
    #[tokio::test]
    async fn an_alias_serves_after_its_txt_record_and_only_for_one_deployment() {
        let root = std::env::temp_dir().join(format!("konae-domain-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let notes = publish(&state, "notes").await;
        let other = publish(&state, "other").await;
        let records = zone(&[]);
        let server = fake_resolver(records.clone()).await;
        let set = |id: &str, hostname: &str| {
            let (state, id, hostname) = (state.clone(), id.to_owned(), hostname.to_owned());
            async move {
                let deployment = deployment_by_id(&state, &id).await.unwrap();
                set(&state, &deployment, &hostname, Ok(server)).await
            }
        };

        // A name under the installation's suffix is refused before anything is stored.
        assert_eq!(
            set(&notes.id, "notes.apps.example.com").await.unwrap_err().to_string(),
            "notes.apps.example.com is under this installation's app suffix apps.example.com, where Konae names apps itself. Name a hostname outside it."
        );
        assert!(read_record(&state, &notes.id).is_none());

        // The first call issues the token; the name does not serve yet.
        let pending = set(&notes.id, "Notes.Example.org.").await.unwrap();
        assert_eq!(pending["status"], "pending");
        assert_eq!(pending["hostname"], "notes.example.org");
        assert_eq!(pending["record"]["name"], "_konae.notes.example.org");
        assert_eq!(pending["dns"], "No TXT record at _konae.notes.example.org yet.");
        let token = pending["record"]["value"].as_str().unwrap().to_owned();
        assert!(token.starts_with("konae-verify-"));
        assert!(deployment_for(&state, &host("notes.example.org")).await.is_none());

        // A record that holds something else is refused and serves nothing.
        records.lock().unwrap().insert("_konae.notes.example.org".into(), vec!["konae-verify-someone-else".into()]);
        let wrong = set(&notes.id, "notes.example.org").await.unwrap_err().to_string();
        assert_eq!(wrong, format!("The TXT record at _konae.notes.example.org holds \"konae-verify-someone-else\", not this Deployment's token. Set it to {token} and call domain_set again."));
        assert!(deployment_for(&state, &host("notes.example.org")).await.is_none());

        // The right token makes it serve, with any port and case.
        records.lock().unwrap().insert("_konae.notes.example.org".into(), vec!["v=spf1 -all".into(), token.clone()]);
        let active = set(&notes.id, "notes.example.org").await.unwrap();
        assert_eq!(active["status"], "active");
        assert_eq!(active["url"], "https://notes.example.org");
        for name in ["notes.example.org", "Notes.Example.Org:443", notes.hostname.as_str()] {
            assert_eq!(deployment_for(&state, &host(name)).await.map(|d| d.id), Some(notes.id.clone()), "{name}");
        }
        assert_eq!(app_url(&state, &notes), "https://notes.example.org");
        assert_eq!(app_url(&state, &other), other.url);

        // The alias is the app's origin: same-origin from itself, an app
        // origin to administration, HSTS without includeSubDomains.
        let mut from_alias = host("notes.example.org");
        from_alias.insert(header::ORIGIN, HeaderValue::from_static("https://notes.example.org"));
        assert!(same_origin(&from_alias));
        assert!(security::from_app_origin(&state, &from_alias).await);
        let mut cross = host("notes.example.org");
        cross.insert(header::ORIGIN, HeaderValue::from_str(&other.url).unwrap());
        assert!(!same_origin(&cross));
        assert_eq!(security::hsts(&state, &host("notes.example.org")), Some("max-age=31536000"));
        assert_eq!(security::hsts(&state, &host(&notes.hostname)), Some(security::HSTS));
        // A route answers on the alias as on the generated hostname.
        let request = Request::builder().uri("/api/count").header(header::HOST, "notes.example.org").body(Body::empty()).unwrap();
        let response = http::get_route(State(state.clone()), axum::extract::Path("api/count".into()), request).await;
        assert_eq!(response.status(), StatusCode::OK);

        // A refused hostname keeps its code through the tool, for mcp.rs to answer.
        let local = set(&other.id, "notes.localhost").await.unwrap_err();
        let refused = crate::refusal::of(&local).expect("domain_set refuses with a code");
        assert_eq!((refused.code, local.to_string().as_str()), ("domain.local", "notes.localhost is a local name. Name a hostname under a domain you control."));

        // Another Deployment cannot take the name, even with its own record.
        let taken = set(&other.id, "notes.example.org").await.unwrap_err().to_string();
        assert_eq!(taken, "notes.example.org already serves another Deployment on this installation. Clear it there with domain_clear first.");
        // Nor can a package that names it.
        let arriving = crate::layout::data(&state.data_root, "arriving");
        tokio::fs::create_dir_all(&arriving).await.unwrap();
        assert_eq!(arrive(&state, "arriving", Some("notes.example.org")).await.unwrap_err().to_string(), taken);

        // Clearing stops the alias; the generated hostname keeps answering.
        let cleared = admin_call(&state, &notes, "domain_clear", &json!({})).await.unwrap();
        assert_eq!(cleared["cleared"], "notes.example.org");
        assert!(deployment_for(&state, &host("notes.example.org")).await.is_none());
        assert!(!security::from_app_origin(&state, &from_alias).await);
        assert_eq!(deployment_for(&state, &host(&notes.hostname)).await.map(|d| d.id), Some(notes.id.clone()));

        // A package's name arrives pending with a token this Kernel issues,
        // so a token copied from DNS into a manifest proves nothing.
        assert_eq!(for_package(&state, &notes.id), None);
        arrive(&state, "arriving", Some("notes.example.org")).await.unwrap();
        let arrived = read_record(&state, "arriving").unwrap();
        assert!(!arrived.verified);
        assert_ne!(arrived.token, token);
        assert_eq!(serving(&state, "notes.example.org"), None);

        // The index is rebuilt from the folders, as after a restart or a restore.
        let shop = set(&notes.id, "shop.example.org").await.unwrap();
        let second = shop["record"]["value"].as_str().unwrap().to_owned();
        assert_ne!(second, token, "a cleared claim's token is gone");
        records.lock().unwrap().insert("_konae.shop.example.org".into(), vec![second.clone()]);
        assert_eq!(set(&notes.id, "shop.example.org").await.unwrap()["status"], "active");
        // Switching names keeps the token, so a record added first switches at once.
        records.lock().unwrap().insert("_konae.www.example.org".into(), vec![second.clone()]);
        assert_eq!(set(&notes.id, "www.example.org").await.unwrap()["status"], "active");
        assert_eq!(serving(&state, "shop.example.org"), None);
        let restored = load_index(&root);
        assert_eq!(restored.get("www.example.org"), Some(&(notes.id.clone(), true)));
        assert_eq!(restored.get("shop.example.org"), None);
        assert_eq!(restored.get("notes.example.org"), Some(&("arriving".to_owned(), false)));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn answers_with_another_id_or_a_failure_code_are_refused() {
        let mut packet = query(7, "_konae.example.org");
        packet[2] |= 0x80;
        assert_eq!(parse_answer(&packet, 7).unwrap(), None);
        assert!(parse_answer(&packet, 8).is_err());
        packet[3] = 0x82;
        assert!(parse_answer(&packet, 7).unwrap_err().to_string().contains("code 2"));
        assert!(parse_answer(&packet[..5], 7).is_err());
    }
}
