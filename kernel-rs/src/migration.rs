//! Migration of one Deployment to another installation, and the maintenance
//! state that keeps the app from having two writers while it moves.
//! docs/plans/platform-launch.md, "Production migration", holds the
//! behaviour and ADR-0014 the decisions.
//!
//! The state lives in `deployments/<id>/migration.json`, inside the
//! Deployment's folder, so a restart, a scheduled copy and a restore all keep
//! it. While it pauses an app:
//!
//! - the app host serves a maintenance page for the published shell and
//!   refuses every other request with one message that names the operation;
//! - administrative tools that would change the app are refused;
//! - the mail worker holds the app's queued email.
//!
//! On the origin an operation goes `pausing`, `captured`, then `confirmed`
//! or `cancelled`. On the destination an imported migration package starts
//! `imported`, which is paused, and becomes `active` with `migration_activate`.
//! Activation needs the confirmation that only `migration_confirm` reveals on
//! the origin, and the origin refuses to cancel after confirming, so the
//! supported procedure never has two installations taking writes.

use std::{
    sync::{LazyLock, RwLock as SyncRwLock},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use axum::{http::Method, middleware::Next};
use rusqlite::params;

use super::mcp::{Owner, digest, owned_deployment, owned_project, store};
use super::*;

pub(crate) const MARKER_FILE: &str = "migration.json";
/// How long the origin keeps its state and shows the new address after a
/// confirmed migration.
const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const DEFAULT_WAIT_SECONDS: u64 = 30;
const MAX_WAIT_SECONDS: u64 = 120;
/// How long the mail worker puts back a held job before looking again.
pub(crate) const MAIL_HOLD_MS: i64 = 60_000;
/// The kept release payload, next to the kept package components.
const RELEASE: &str = "release";
const MAX_RELEASE_BYTES: u64 = 26_000_000;

const ORIGIN: &str = "origin";
const DESTINATION: &str = "destination";
const PAUSING: &str = "pausing";
const CAPTURED: &str = "captured";
const CONFIRMED: &str = "confirmed";
const CANCELLED: &str = "cancelled";
const IMPORTED: &str = "imported";
const ACTIVE: &str = "active";
const UNREADABLE: &str = "unreadable";

/// The migration state of one Deployment.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Marker {
    role: String,
    phase: String,
    operation_id: String,
    #[serde(default)]
    deployment_id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settle: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    captured_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manifest: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release: Option<Value>,
    /// Origin only. Revealed by `migration_confirm`; its SHA-256 travels in
    /// the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confirmation: Option<String>,
    /// Destination only: the SHA-256 the package carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confirmation_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confirmed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retain_until: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelled_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imported_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<Value>,
    /// Earlier operations on this Deployment, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<Value>,
}

impl Marker {
    /// A marker that exists but cannot be read pauses the app: the Kernel
    /// cannot tell whether the app moved.
    fn unreadable() -> Self {
        Self { role: ORIGIN.into(), phase: UNREADABLE.into(), operation_id: "unknown".into(), ..Self::default() }
    }

    /// An origin whose migration was confirmed: it stays paused for good.
    pub(crate) fn retired(&self) -> bool {
        self.role == ORIGIN && self.phase == CONFIRMED
    }

    pub(crate) fn paused(&self) -> bool {
        match self.role.as_str() {
            ORIGIN => [PAUSING, CAPTURED, CONFIRMED, UNREADABLE].contains(&self.phase.as_str()),
            DESTINATION => self.phase == IMPORTED,
            _ => true,
        }
    }

    /// The one answer to every refused request while the app is paused.
    pub(crate) fn message(&self) -> String {
        let op = &self.operation_id;
        match (self.role.as_str(), self.phase.as_str()) {
            (ORIGIN, CONFIRMED) => format!(
                "This app moved to another server with migration {op}. This copy accepts no changes; use the app at its new address."
            ),
            (ORIGIN, UNREADABLE) => format!(
                "This app is in maintenance: its migration state ({MARKER_FILE}) cannot be read, so the Kernel cannot tell whether it moved. The operator must repair or remove that file."
            ),
            (ORIGIN, _) => format!(
                "This app is paused for migration {op}. It accepts no writes, signups, recovery, administrative changes or email until the migration is confirmed or cancelled."
            ),
            _ => format!(
                "This app was imported by migration {op} and stays paused until its operator activates it with migration_activate."
            ),
        }
    }

    /// The validated new address, shown for seven days after confirmation.
    fn shown_url(&self, now: u64) -> Option<&str> {
        (self.phase == CONFIRMED && self.retain_until.is_some_and(|until| now < until)).then_some(self.new_url.as_deref()).flatten()
    }
}

// ---------------------------------------------------------------------------
// Storage

/// Markers by file path. Only this process writes them while it runs, and a
/// restore runs before it starts, so the cache stays true.
static MARKERS: LazyLock<SyncRwLock<HashMap<PathBuf, Option<Arc<Marker>>>>> = LazyLock::new(Default::default);

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\', '.'])
}

fn marker_path(data_root: &Path, id: &str) -> PathBuf {
    data_root.join("deployments").join(id).join(MARKER_FILE)
}

pub(crate) fn marker(data_root: &Path, id: &str) -> Option<Arc<Marker>> {
    if !valid_id(id) {
        return None;
    }
    let path = marker_path(data_root, id);
    if let Some(found) = MARKERS.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&path) {
        return found.clone();
    }
    let loaded = match std::fs::read(&path) {
        Ok(bytes) => Some(Arc::new(serde_json::from_slice(&bytes).unwrap_or_else(|_| Marker::unreadable()))),
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(Arc::new(Marker::unreadable())),
    };
    MARKERS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path, loaded.clone());
    loaded
}

fn save(data_root: &Path, id: &str, marker: &Marker) -> Result<()> {
    let path = marker_path(data_root, id);
    let temporary = path.with_file_name(format!(".{MARKER_FILE}.tmp-{}", Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(marker)?)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    MARKERS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path, Some(Arc::new(marker.clone())));
    Ok(())
}

/// Pauses or resumes a Deployment as an origin migration would, for tests
/// of the modules the pause reaches.
#[cfg(test)]
pub(crate) fn retire_for_test(data_root: &Path, id: &str) {
    save(data_root, id, &Marker { role: ORIGIN.into(), phase: CONFIRMED.into(), operation_id: "op-test".into(), ..Marker::default() }).unwrap();
}

/// Pauses or resumes a Deployment as an origin migration would.
#[cfg(test)]
pub(crate) fn pause_for_test(data_root: &Path, id: &str, paused: bool) {
    let phase = if paused { PAUSING } else { CANCELLED };
    save(data_root, id, &Marker { role: ORIGIN.into(), phase: phase.into(), operation_id: "op-test".into(), ..Marker::default() }).unwrap();
}

/// Forgets cached markers under `data_root`, for tests that edit files.
#[cfg(test)]
pub(crate) fn forget(data_root: &Path) {
    MARKERS.write().unwrap().retain(|path, _| !path.starts_with(data_root));
}

/// The pause that applies to a Deployment: its own, or the installation's
/// when a restore has not reconciled its revocation record.
pub(crate) fn pause(data_root: &Path, id: &str) -> Option<(String, String, Option<String>)> {
    if let Some(message) = super::recovery::hold(data_root) {
        return Some(("restore".into(), message, None));
    }
    let marker = marker(data_root, id).filter(|marker| marker.paused())?;
    Some((marker.operation_id.clone(), marker.message(), marker.shown_url(now_ms()).map(str::to_owned)))
}

/// Whether the mail worker must hold a job of this scope.
pub(crate) fn holds_mail(data_root: &Path, scope: &str) -> bool {
    super::recovery::hold(data_root).is_some()
        || scope.strip_prefix("deployment:").and_then(|id| marker(data_root, id)).is_some_and(|marker| marker.paused())
}

/// Finds the origin Deployment of an operation. A rehearsal may import the
/// package into the same installation, so the copy shares the operation ID.
fn find_operation(data_root: &Path, operation: &str) -> Option<(String, Arc<Marker>)> {
    let entries = std::fs::read_dir(data_root.join("deployments")).ok()?;
    entries.flatten().find_map(|entry| {
        let id = entry.file_name().to_str()?.to_owned();
        let marker = marker(data_root, &id)?;
        (marker.operation_id == operation && marker.role == ORIGIN).then_some((id, marker))
    })
}

/// The Deployment an operation names, for its audit row.
pub(super) fn operation_deployment(data_root: &Path, operation: &str) -> Option<String> {
    find_operation(data_root, operation).map(|(id, _)| id)
}

// ---------------------------------------------------------------------------
// Refusals

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn maintenance_page(name: &str, operation: &str, message: &str, new_url: Option<&str>) -> Response {
    let link = new_url
        .map(|url| format!("<p><a href=\"{0}\">Open {1} at its new address</a></p><p class=\"url\">{0}</p>", escape(url), escape(name)))
        .unwrap_or_default();
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} is in maintenance</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:34rem;margin:15vh auto;padding:0 1.5rem;color:#1f2328}}\
h1{{font-size:1.4rem}}.url,.op{{color:#59636e;font-size:.85rem;word-break:break-all}}a{{color:#0a58ca}}\
@media (prefers-color-scheme: dark){{body{{background:#0d1117;color:#e6edf3}}.url,.op{{color:#9198a1}}a{{color:#4493f8}}}}</style></head>\
<body><main><h1>{title} is in maintenance</h1><p>{message}</p>{link}<p class=\"op\">Operation {operation}</p></main></body></html>",
        title = escape(name),
        message = escape(message),
        operation = escape(operation),
    );
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, html).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("300"));
    response
}

fn refusal(operation: &str, message: &str) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": message, "operationId": operation, "maintenance": true})),
    )
        .into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("300"));
    response
}

/// Middleware on every route. A request to a paused app's host gets the
/// maintenance page when it asks for a page, and a 503 that names the
/// operation for anything else: actions, uploads, signups, sign-ins,
/// recovery, invitations and HTTP routes. Nothing is redirected.
pub(super) async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path == "/_littlebox/health" {
        return next.run(request).await;
    }
    let Some(deployment) = deployment_for(&state, request.headers()).await else {
        return next.run(request).await;
    };
    let Some((operation, message, new_url)) = pause(&state.data_root, &deployment.id) else {
        return next.run(request).await;
    };
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    let page = matches!(*request.method(), Method::GET | Method::HEAD) && (!path.starts_with("/_littlebox/") || wants_page);
    // A custom domain still pointed at this origin would link to itself.
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).and_then(|value| value.split(':').next()).map(str::to_ascii_lowercase);
    let new_url = new_url.filter(|url| url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)) != host);
    if page {
        maintenance_page(&deployment.project_name, &operation, &message, new_url.as_deref())
    } else {
        refusal(&operation, &message)
    }
}

/// Tools that change nothing a paused app serves. Every other tool that
/// names a paused Deployment, its project or one of its plans is refused.
const OPEN_TOOLS: [&str; 24] = [
    "platform_guide",
    "platform_describe",
    "mail_status",
    "session_inspect",
    "projects_list",
    "project_create",
    "project_inspect",
    "project_sources_read",
    "project_sources_save",
    "tokens_list",
    "token_create",
    "token_revoke",
    "secrets_list",
    "api_keys_list",
    "usage_read",
    "signals_read",
    "signup_policy_read",
    "storage_test",
    "package_export",
    "database_export",
    "project_export",
    "schema_plan",
    "release_plan",
    "eject_plan",
];
const READ_TOOLS_SUFFIX: [&str; 2] = ["_list", "_read"];

/// Runs before every administrative tool.
pub(super) async fn admin_guard(state: &AppState, name: &str, a: &Value) -> Result<()> {
    if name.starts_with("migration_")
        || name.starts_with("transfer_")
        || OPEN_TOOLS.contains(&name)
        || READ_TOOLS_SUFFIX.iter().any(|suffix| name.ends_with(suffix))
        || (name == "database_execute" && ["inspect", "browse", "list-backups"].contains(&a["operation"].as_str().unwrap_or("")))
    {
        return Ok(());
    }
    if let Some(message) = super::recovery::hold(&state.data_root) {
        bail!(message);
    }
    let deployment = if let Some(id) = a["deploymentId"].as_str() {
        Some(id.to_owned())
    } else {
        let project = match (a["projectId"].as_str(), a["planId"].as_str()) {
            (Some(project), _) => Some(project.to_owned()),
            (None, Some(plan)) if name == "release_apply" => {
                let row: Option<(String, Option<String>)> = store(state)?
                    .query_row("SELECT project,receipt FROM plans WHERE id=?1", [plan], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                match row {
                    // An applied plan only returns its receipt.
                    Some((_, Some(_))) => return Ok(()),
                    Some((project, None)) => Some(project),
                    None => None,
                }
            }
            (None, Some(plan)) => super::eject::plan_project(&store(state)?, plan),
            (None, None) => None,
        };
        match project {
            Some(project) => find_project_deployment(&*state.deployments.read().await, &project).map(|d| d.id),
            None => None,
        }
    };
    if let Some((_, message, _)) = deployment.and_then(|id| pause(&state.data_root, &id)) {
        bail!(message);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Package stamp and import

/// Checks the `migration` stamp of a version 2 manifest.
pub(super) fn check_stamp(stamp: &Value) -> Result<()> {
    let operation = stamp["operationId"].as_str().unwrap_or("");
    ensure!(valid_id(operation) && operation.len() <= 64, "The package's migration has no valid operation ID.");
    let hash = stamp["confirmationHash"].as_str().unwrap_or("");
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "The package's migration has no valid confirmation hash."
    );
    Ok(())
}

/// Runs inside the release that imports a package, after the components are
/// in the Deployment's folder and before the Deployment serves. A migration
/// package leaves the app paused until `migration_activate`.
pub(super) fn imported(data_root: &Path, deployment_id: &str, project_id: &str, manifest: &package::Manifest) -> Result<()> {
    let Some(stamp) = manifest.migration.as_ref() else { return Ok(()) };
    check_stamp(stamp)?;
    let marker = Marker {
        role: DESTINATION.into(),
        phase: IMPORTED.into(),
        operation_id: stamp["operationId"].as_str().unwrap_or_default().to_owned(),
        deployment_id: deployment_id.to_owned(),
        project_id: project_id.to_owned(),
        started_at: now_ms(),
        package_id: Some(manifest.package_id.clone()),
        confirmation_hash: stamp["confirmationHash"].as_str().map(str::to_owned),
        origin: stamp.get("origin").cloned(),
        imported_at: Some(now_ms()),
        ..Marker::default()
    };
    save(data_root, deployment_id, &marker)
}

pub(super) fn install(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS package_imports(package_id TEXT NOT NULL, project TEXT NOT NULL, plan TEXT NOT NULL, PRIMARY KEY(package_id, project));",
    )?;
    Ok(())
}

/// A second `release_plan` with a package this project already imported
/// gets the first plan and its receipt instead of a refusal.
pub(super) fn replay_import(state: &AppState, owner: &Owner, project: &str, package_id: &str) -> Result<Option<Value>> {
    let db = store(state)?;
    let replay: Option<(String, String)> = db
        .query_row(
            "SELECT i.plan, p.receipt FROM package_imports i JOIN plans p ON p.id=i.plan
             WHERE i.package_id=?1 AND i.project=?2 AND p.owner=?3 AND p.receipt IS NOT NULL",
            params![package_id, project, owner.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match replay {
        Some((plan, receipt)) => Some(json!({
            "planId": plan,
            "projectId": project,
            "applied": true,
            "packageId": package_id,
            "receipt": serde_json::from_str::<Value>(&receipt)?,
        })),
        None => None,
    })
}

pub(super) fn remember_import(state: &AppState, package_id: &str, project: &str, plan: &str) -> Result<()> {
    store(state)?.execute(
        "INSERT INTO package_imports(package_id,project,plan) VALUES(?1,?2,?3)
         ON CONFLICT(package_id,project) DO UPDATE SET plan=excluded.plan",
        params![package_id, project, plan],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tools

/// `migration_status` for a credential that may read; the rest for one
/// with `manage_lifecycle`.
pub(super) fn tools_for(scopes: &[String]) -> Vec<Value> {
    let has = |scope: &str| scopes.iter().any(|s| s == scope);
    tools()
        .into_iter()
        .filter(|tool| if tool["name"] == "migration_status" { has("read") || has("manage_lifecycle") } else { has("manage_lifecycle") })
        .collect()
}

fn tools() -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    vec![
        super::mcp::tool(
            "migration_begin",
            "Begin migrating a Deployment to another installation. The app pauses at once: its host serves a maintenance page and refuses writes, signups, recovery, administrative changes and email. The Kernel waits up to waitSeconds for operations and email in progress, then captures the final state as a migration package and returns the operation with its downloads. Call it again to continue a pause that is still settling or to download the same package again. Requires manage_lifecycle and export_data.",
            json!({"deploymentId":text,"waitSeconds":{"type":"integer","minimum":1,"maximum":MAX_WAIT_SECONDS}}),
            &["deploymentId"],
            false,
        ),
        super::mcp::tool(
            "migration_status",
            "Read the migration state of a Deployment: its operation, phase, whether it is paused, the package, and the retention date once confirmed. Requires read.",
            json!({"deploymentId":text}),
            &["deploymentId"],
            true,
        ),
        super::mcp::tool(
            "migration_confirm",
            "Confirm a captured migration at the origin. The origin stays paused for good, keeps its state for seven days and shows newUrl on its maintenance page until then. Returns a receipt with the confirmation the destination needs for migration_activate. After this the migration cannot be cancelled. Repeating returns the receipt. Requires manage_lifecycle.",
            json!({"operationId":text,"newUrl":{"type":"string","minLength":1,"maxLength":2048}}),
            &["operationId"],
            false,
        ),
        super::mcp::tool(
            "migration_cancel",
            "Cancel a migration that was not confirmed: the origin accepts writes again. Email ended by the pause is not sent again; resend invitations and ask people to request new links. Repeating returns the receipt. Requires manage_lifecycle.",
            json!({"operationId":text}),
            &["operationId"],
            false,
        ),
        super::mcp::tool(
            "migration_activate",
            "Activate a Deployment imported from a migration package, with the confirmation from migration_confirm on the origin. Ends any session, link or API key that arrived with the package and starts serving. With resendInvitations, emails every pending invitation a new link on this host. Repeating returns the receipt. Requires manage_lifecycle; resending also needs manage_users.",
            json!({"deploymentId":text,"confirmation":text,"resendInvitations":{"type":"boolean"}}),
            &["deploymentId", "confirmation"],
            false,
        ),
    ]
}

pub(super) async fn admin_call(state: &AppState, owner: &Owner, principal: &str, name: &str, a: &Value) -> Result<Value> {
    match name {
        "migration_begin" => begin(state, owner, principal, a).await,
        "migration_status" => {
            let id = a["deploymentId"].as_str().unwrap_or_default();
            let deployment = owned_deployment(state, owner, id, "read").await?;
            Ok(match marker(&state.data_root, &deployment.id) {
                Some(marker) => view(&marker),
                None => json!({"deploymentId": deployment.id, "phase": "none", "paused": super::recovery::hold(&state.data_root).is_some()}),
            })
        }
        "migration_confirm" => confirm(state, owner, a).await,
        "migration_cancel" => cancel(state, owner, a).await,
        "migration_activate" => activate(state, owner, a).await,
        _ => bail!("Unknown tool."),
    }
}

/// What a caller may see of a marker: everything but the confirmation.
fn view(marker: &Marker) -> Value {
    let mut value = serde_json::to_value(marker).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.remove("confirmation");
        object.remove("history");
        // The confirm receipt holds the confirmation; only migration_confirm returns it.
        if let Some(receipt) = object.get_mut("receipt").and_then(Value::as_object_mut) {
            receipt.remove("confirmation");
        }
        object.insert("paused".into(), json!(marker.paused()));
        if marker.paused() {
            object.insert("message".into(), json!(marker.message()));
        }
    }
    value
}

fn random_secret() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| anyhow::anyhow!("Could not generate a confirmation."))?;
    Ok(hex::encode(bytes))
}

fn keep_dir(data_root: &Path, id: &str, operation: &str) -> PathBuf {
    data_root.join("deployments").join(id).join("migration").join(operation)
}

async fn begin(state: &AppState, owner: &Owner, principal: &str, a: &Value) -> Result<Value> {
    let id = a["deploymentId"].as_str().unwrap_or_default();
    let wait = Duration::from_secs(a["waitSeconds"].as_u64().unwrap_or(DEFAULT_WAIT_SECONDS).clamp(1, MAX_WAIT_SECONDS));
    let deployment = owned_deployment(state, owner, id, "manage_lifecycle").await?;
    owned_project(state, owner, &deployment.project_id, "export_data")?;
    if let Some(message) = super::recovery::hold(&state.data_root) {
        bail!(message);
    }
    let mut marker = {
        // Releases and policy changes hold this lock, so none lands after the pause.
        let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
        let current = marker(&state.data_root, id);
        match current.as_deref() {
            Some(m) if m.role == DESTINATION && m.phase == IMPORTED => bail!(
                "This app was imported by migration {} and is not active yet. Activate it with migration_activate before migrating it again.",
                m.operation_id
            ),
            Some(m) if m.role == ORIGIN && m.phase == CONFIRMED => bail!(
                "Migration {} was confirmed: this app now takes its writes at the destination. Migrate it from there.",
                m.operation_id
            ),
            Some(m) if m.phase == UNREADABLE => bail!(m.message()),
            Some(m) if m.role == ORIGIN && (m.phase == PAUSING || m.phase == CAPTURED) => m.clone(),
            previous => {
                let mut history = previous.map(|m| m.history.clone()).unwrap_or_default();
                if let Some(m) = previous {
                    history.push(json!({"operationId": m.operation_id, "role": m.role, "phase": m.phase}));
                }
                let marker = Marker {
                    role: ORIGIN.into(),
                    phase: PAUSING.into(),
                    operation_id: Uuid::new_v4().to_string(),
                    deployment_id: id.to_owned(),
                    project_id: deployment.project_id.clone(),
                    started_at: now_ms(),
                    started_by: Some(owner.id.clone()),
                    confirmation: Some(random_secret()?),
                    history,
                    ..Marker::default()
                };
                save(&state.data_root, id, &marker)?;
                marker
            }
        }
    };
    // A copy does not carry the kept package, so an origin restored while
    // captured captures again. It was paused since, so the state is the same.
    if marker.phase == CAPTURED && !keep_dir(&state.data_root, id, &marker.operation_id).join(RELEASE).is_file() {
        marker.phase = PAUSING.into();
    }
    if marker.phase == PAUSING {
        let settled = settle(state, &deployment, wait).await.map_err(still_paused)?;
        let done = settled["waitingFor"].is_null();
        marker.settle = Some(settled);
        if !done {
            save(&state.data_root, id, &marker)?;
            let mut answer = view(&marker);
            answer["next"] = json!("The app is paused but work in progress has not settled. Call migration_begin again to keep waiting, or migration_cancel to resume the app.");
            return Ok(answer);
        }
        let downloads = capture(state, owner, principal, &deployment, &mut marker).await.map_err(still_paused)?;
        save(&state.data_root, id, &marker)?;
        let mut answer = view(&marker);
        answer["downloads"] = Value::Object(downloads);
        answer["next"] = next_steps();
        return Ok(answer);
    }
    // Captured already: offer the same package again.
    let keep = keep_dir(&state.data_root, id, &marker.operation_id);
    let manifest: package::Manifest = serde_json::from_value(marker.manifest.clone().context("The captured manifest is missing.")?)?;
    let mut downloads = package::offer(state, principal, id, &deployment.project_id, &keep, &manifest.contents).await.map_err(still_paused)?;
    downloads.extend(offer_release(state, principal, &deployment, &marker, &keep).await.map_err(still_paused)?);
    let mut answer = view(&marker);
    answer["downloads"] = Value::Object(downloads);
    answer["next"] = next_steps();
    Ok(answer)
}

/// A failure after the pause names the cause and the two ways on.
fn still_paused(error_value: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("{error_value:#}. The app stays paused: call migration_begin again to retry, or migration_cancel to resume it.")
}

fn next_steps() -> Value {
    json!([
        "Download every component and the release with transfer_read.",
        "On the destination, create the project, upload each component with transfer_begin and its component name, and release the downloaded release payload with initialPackage naming the manifest and the uploads.",
        "Check the imported app with read tools; it stays paused.",
        "Run migration_confirm here, then migration_activate on the destination with the confirmation it returns.",
    ])
}

/// Waits, up to `wait`, for operations and email in progress. Writes hold
/// the project, data or identity lock, so taking each once after the pause
/// means every write that started before it has finished. A job the mail
/// worker claimed may be with the provider; it is waited for. Jobs still
/// queued are ended, because the pause holds them and their links would not
/// work at the destination.
async fn settle(state: &AppState, deployment: &Deployment, wait: Duration) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + wait;
    for name in [format!("project:{}", deployment.project_id), deployment.id.clone(), format!("identity:{}", deployment.id)] {
        if tokio::time::timeout_at(deadline, database_lock(state, &name)).await.is_err() {
            return Ok(json!({"waitingFor": "operations", "checkedAt": now_ms()}));
        }
    }
    let mail = state.data_root.join(mail::MAIL_DATABASE);
    let scope = format!("deployment:{}", deployment.id);
    if !mail.is_file() {
        return Ok(json!({"waitingFor": null, "settledAt": now_ms(), "mailEnded": 0}));
    }
    loop {
        let sending: i64 = {
            let db = Connection::open(&mail)?;
            db.busy_timeout(Duration::from_secs(5))?;
            db.query_row(
                "SELECT COUNT(*) FROM jobs WHERE scope=?1 AND state='pending' AND leased_until IS NOT NULL AND leased_until>=?2",
                params![scope, now_ms() as i64],
                |r| r.get(0),
            )?
        };
        if sending == 0 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({"waitingFor": "mail", "sending": sending, "checkedAt": now_ms()}));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let db = Connection::open(&mail)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let ended = db.execute(
        "UPDATE jobs SET state='failed', error_kind='migration', link_nonce=NULL, link_sealed=NULL, leased_until=NULL, updated_at=?2
         WHERE scope=?1 AND state='pending'",
        params![scope, now_ms() as i64],
    )?;
    Ok(json!({"waitingFor": null, "settledAt": now_ms(), "mailEnded": ended}))
}

/// The files of a live artifact folder as a builder sends them: UTF-8 text as
/// is, and a binary file, such as an image, as base64 named in `encodings`.
fn live_artifact_files(root: &Path) -> Result<(serde_json::Map<String, Value>, serde_json::Map<String, Value>)> {
    fn walk(root: &Path, directory: &Path, files: &mut serde_json::Map<String, Value>, encodings: &mut serde_json::Map<String, Value>) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, files, encodings)?;
            } else if entry.file_type()?.is_file() {
                let relative = path.strip_prefix(root)?.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                match String::from_utf8(std::fs::read(&path)?) {
                    Ok(text) => {
                        files.insert(relative, Value::String(text));
                    }
                    Err(binary) => {
                        files.insert(relative.clone(), Value::String(encode_base64(binary.as_bytes())));
                        encodings.insert(relative, Value::String("base64".into()));
                    }
                }
            }
        }
        Ok(())
    }
    let mut files = serde_json::Map::new();
    let mut encodings = serde_json::Map::new();
    walk(root, root, &mut files, &mut encodings)?;
    Ok((files, encodings))
}

/// The release payload that republishes the app as it is served: its live
/// artifact, its actions and its visibility. It travels next to the package
/// so the destination needs nothing else.
async fn release_payload(state: &AppState, owner: &Owner, deployment: &Deployment) -> Result<Vec<u8>> {
    let version = deployment.artifact_version.clone().context("This Deployment has no versioned artifact to migrate. Publish it once, then migrate.")?;
    let root = artifact_root(state, &deployment.id).join(&version);
    let (project, _) = owned_project(state, owner, &deployment.project_id, "export_data")?;
    let (files, encodings) = tokio::task::spawn_blocking(move || live_artifact_files(&root)).await??;
    ensure!(files.contains_key("index.html"), "The live artifact has no index.html.");
    let visibility = if is_private(deployment) { "private" } else { "public" };
    let slug = project["slug"].as_str().map(str::to_owned).unwrap_or_else(|| safe_slug(&deployment.project_name));
    let bytes = serde_json::to_vec(&json!({
        "project": {
            "id": deployment.project_id,
            "name": deployment.project_name,
            "slug": slug,
            "manifest": deployment.manifest,
            "files": {"actions.json": deployment.actions},
        },
        "visibility": visibility,
        "artifact": {"version": 1, "entrypoint": "index.html", "files": files, "encodings": encodings},
    }))?;
    ensure!((bytes.len() as u64) <= MAX_RELEASE_BYTES, "The live artifact exceeds the 26 MB release limit.");
    Ok(bytes)
}

/// Captures the paused app into its kept folder and returns the downloads.
async fn capture(
    state: &AppState,
    owner: &Owner,
    principal: &str,
    deployment: &Deployment,
    marker: &mut Marker,
) -> Result<serde_json::Map<String, Value>> {
    let keep = keep_dir(&state.data_root, &deployment.id, &marker.operation_id);
    let _ = tokio::fs::remove_dir_all(&keep).await;
    let release = release_payload(state, owner, deployment).await?;
    let release_content = json!({"size": release.len(), "sha256": digest(&release)});
    let confirmation = marker.confirmation.clone().context("The operation has no confirmation.")?;
    let stamp = json!({
        "operationId": marker.operation_id,
        "confirmationHash": digest(confirmation.as_bytes()),
        "origin": {"deploymentId": deployment.id, "projectId": deployment.project_id, "url": deployment.url},
        "release": release_content,
    });
    let exported = package::export_with(state, owner, principal, &deployment.id, Some(package::Stamp { migration: stamp, keep: keep.clone() })).await?;
    tokio::fs::write(keep.join(RELEASE), &release).await?;
    marker.phase = CAPTURED.into();
    marker.captured_at = Some(now_ms());
    marker.package_id = exported["manifest"]["packageId"].as_str().map(str::to_owned);
    marker.manifest = Some(exported["manifest"].clone());
    marker.release = Some(release_content);
    let mut downloads = exported["downloads"].as_object().cloned().unwrap_or_default();
    downloads.extend(offer_release(state, principal, deployment, marker, &keep).await?);
    Ok(downloads)
}

async fn offer_release(state: &AppState, principal: &str, deployment: &Deployment, marker: &Marker, keep: &Path) -> Result<serde_json::Map<String, Value>> {
    let release = marker.release.as_ref().context("The captured release is missing.")?;
    let content = package::Content {
        name: RELEASE.into(),
        size: release["size"].as_u64().unwrap_or_default(),
        sha256: release["sha256"].as_str().unwrap_or_default().to_owned(),
    };
    package::offer(state, principal, &deployment.id, &deployment.project_id, keep, &[content]).await
}

/// An absolute `https` URL, or `http` on a loopback host, with no
/// credentials, query or fragment, on another host than this app's.
fn validate_new_url(value: &str, own_host: &str) -> Result<String> {
    let parsed = url::Url::parse(value).context("newUrl is not an absolute URL.")?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let loopback = host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "[::1]";
    ensure!(parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback), "newUrl must use https.");
    ensure!(!host.is_empty(), "newUrl has no host.");
    ensure!(parsed.username().is_empty() && parsed.password().is_none(), "newUrl must not carry credentials.");
    ensure!(parsed.query().is_none() && parsed.fragment().is_none(), "newUrl must not carry a query or a fragment.");
    ensure!(host != own_host.to_ascii_lowercase(), "newUrl must point at the destination, not at this app.");
    Ok(parsed.to_string())
}

/// The operation, its Deployment, and the caller's right to change it.
async fn owned_operation(state: &AppState, owner: &Owner, a: &Value) -> Result<(Deployment, Marker)> {
    let operation = a["operationId"].as_str().unwrap_or_default();
    let (id, _) = find_operation(&state.data_root, operation).context("Migration operation not found.")?;
    let deployment = owned_deployment(state, owner, &id, "manage_lifecycle").await?;
    let marker = marker(&state.data_root, &id).context("Migration operation not found.")?;
    ensure!(marker.operation_id == operation && marker.role == ORIGIN, "Migration operation not found on this origin.");
    Ok((deployment, (*marker).clone()))
}

async fn confirm(state: &AppState, owner: &Owner, a: &Value) -> Result<Value> {
    let (deployment, _) = owned_operation(state, owner, a).await?;
    let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
    let (_, mut marker) = owned_operation(state, owner, a).await?;
    let requested = a["newUrl"].as_str().map(|url| validate_new_url(url, &deployment.hostname)).transpose()?;
    match marker.phase.as_str() {
        CONFIRMED => {
            ensure!(
                requested.is_none() || requested == marker.new_url,
                "Migration {} was confirmed with another newUrl.",
                marker.operation_id
            );
            return marker.receipt.clone().context("The confirmation receipt is missing.");
        }
        CANCELLED => bail!("Migration {} was cancelled. Begin a new migration.", marker.operation_id),
        PAUSING => bail!("Migration {} has not captured the app yet. Call migration_begin until it returns the package.", marker.operation_id),
        _ => {}
    }
    let at = now_ms();
    let retain_until = at + RETENTION_MS;
    marker.phase = CONFIRMED.into();
    marker.confirmed_at = Some(at);
    marker.retain_until = Some(retain_until);
    // Without a newUrl, a verified custom domain is the new address: it
    // travels in the package, and the owner points it at the destination.
    marker.new_url = requested.or_else(|| {
        let url = super::domain::app_url(state, &deployment);
        (url != deployment.url).then(|| format!("{url}/"))
    });
    let requested = marker.new_url.clone();
    let receipt = json!({
        "operationId": marker.operation_id,
        "deploymentId": deployment.id,
        "packageId": marker.package_id,
        "confirmedAt": at,
        "confirmedBy": owner.id,
        "retainUntil": retain_until,
        "newUrl": requested,
        "confirmation": marker.confirmation,
        "effects": {
            "origin": "Stays in maintenance and accepts no writes. Its state stays on this server until retainUntil; its maintenance page shows newUrl until then.",
            "destination": "Activate the imported app with migration_activate and this confirmation.",
            "cancel": "No longer possible. To come back, pause the destination and migrate its state here.",
        },
    });
    marker.receipt = Some(receipt.clone());
    save(&state.data_root, &deployment.id, &marker)?;
    Ok(receipt)
}

async fn cancel(state: &AppState, owner: &Owner, a: &Value) -> Result<Value> {
    let (deployment, _) = owned_operation(state, owner, a).await?;
    let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
    let (_, mut marker) = owned_operation(state, owner, a).await?;
    match marker.phase.as_str() {
        CANCELLED => return marker.receipt.clone().context("The cancellation receipt is missing."),
        CONFIRMED => bail!(
            "Migration {} was confirmed, so its destination may already accept writes. Cancelling would leave two installations writing. To come back, pause the destination and migrate its state here.",
            marker.operation_id
        ),
        _ => {}
    }
    let at = now_ms();
    let keep = keep_dir(&state.data_root, &deployment.id, &marker.operation_id);
    let _ = tokio::fs::remove_dir_all(&keep).await;
    marker.phase = CANCELLED.into();
    marker.cancelled_at = Some(at);
    marker.confirmation = None;
    let receipt = json!({
        "operationId": marker.operation_id,
        "deploymentId": deployment.id,
        "cancelledAt": at,
        "cancelledBy": owner.id,
        "mailEnded": marker.settle.as_ref().and_then(|settle| settle["mailEnded"].as_u64()).unwrap_or(0),
        "effects": {
            "origin": "Accepts writes, signups, recovery and email again.",
            "email": "Email ended by the pause is not sent. Resend pending invitations; people ask again for verification or recovery links.",
            "destination": "A package already imported elsewhere can never be activated: its confirmation was not issued and is now discarded.",
        },
    });
    marker.receipt = Some(receipt.clone());
    save(&state.data_root, &deployment.id, &marker)?;
    Ok(receipt)
}

async fn activate(state: &AppState, owner: &Owner, a: &Value) -> Result<Value> {
    let id = a["deploymentId"].as_str().unwrap_or_default();
    let deployment = owned_deployment(state, owner, id, "manage_lifecycle").await?;
    let resend = a["resendInvitations"].as_bool().unwrap_or(false);
    if resend {
        owned_project(state, owner, &deployment.project_id, "manage_users")?;
    }
    let mut marker = {
        let _project = database_lock(state, &format!("project:{}", deployment.project_id)).await;
        let mut marker = match marker(&state.data_root, id) {
            Some(marker) => (*marker).clone(),
            None => bail!("This app was not imported by a migration, so it has nothing to activate."),
        };
        ensure!(marker.role == DESTINATION, "Only an app imported by a migration can be activated. This one is the origin of migration {}.", marker.operation_id);
        if marker.phase == ACTIVE {
            return marker.receipt.clone().context("The activation receipt is missing.");
        }
        let confirmation = a["confirmation"].as_str().unwrap_or_default();
        ensure!(
            marker.confirmation_hash.as_deref() == Some(digest(confirmation.as_bytes()).as_str()),
            "This confirmation does not belong to migration {}. Use the confirmation migration_confirm returned on the origin; a cancelled migration has none.",
            marker.operation_id
        );
        // Nothing can sign in while the app is paused; this ends anything
        // the package or a rehearsal left behind.
        let revoked = {
            let _data = database_lock(state, id).await;
            let _identity = database_lock(state, &format!("identity:{id}")).await;
            let identity = deployment_dir(state, id).join("identity.sqlite");
            if identity.is_file() { package::revoke_access(&identity)? } else { 0 }
        };
        let pending = pending_invitations(&deployment_dir(state, id).join("identity.sqlite"))?;
        let at = now_ms();
        marker.phase = ACTIVE.into();
        marker.activated_at = Some(at);
        marker.receipt = Some(json!({
            "operationId": marker.operation_id,
            "deploymentId": id,
            "url": deployment.url,
            "packageId": marker.package_id,
            "activatedAt": at,
            "activatedBy": owner.id,
            "revoked": revoked,
            "signing": "No session, email link, API key or route signing secret came from the origin. Users sign in again with their passwords; set route secrets with secret_set and create API keys again.",
            "invitations": {"pending": pending, "resent": []},
        }));
        save(&state.data_root, id, &marker)?;
        marker
    };
    if resend {
        let mut results = Vec::new();
        for invitation in marker.receipt.as_ref().and_then(|r| r["invitations"]["pending"].as_array().cloned()).unwrap_or_default() {
            let invitation_id = invitation["invitationId"].as_str().unwrap_or_default();
            let arguments = json!({"deploymentId": id, "invitationId": invitation_id, "idempotencyKey": format!("migration-{}-{invitation_id}", marker.operation_id)});
            let outcome = super::invitations::admin_call(state, owner, "app_invitation_resend", &arguments).await;
            results.push(match outcome {
                Ok(value) => json!({"invitationId": invitation_id, "ok": true, "result": value}),
                Err(error_value) => json!({"invitationId": invitation_id, "ok": false, "error": error_value.to_string()}),
            });
        }
        if let Some(receipt) = marker.receipt.as_mut() {
            receipt["invitations"]["resent"] = json!(results);
        }
        save(&state.data_root, id, &marker)?;
    }
    marker.receipt.clone().context("The activation receipt is missing.")
}

/// Invitations that still wait for their recipient. Their links did not
/// travel; each needs a resend from this host.
fn pending_invitations(identity: &Path) -> Result<Vec<Value>> {
    if !identity.is_file() {
        return Ok(Vec::new());
    }
    let db = Connection::open(identity)?;
    if !table_exists(&db, "app_invitations")? {
        return Ok(Vec::new());
    }
    let mut query = db.prepare("SELECT id, email FROM app_invitations WHERE state='pending' ORDER BY created_at, id")?;
    let rows = query
        .query_map([], |r| Ok(json!({"invitationId": r.get::<_, String>(0)?, "email": r.get::<_, String>(1)?})))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!("littlebox-migration-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("deployments/app-1")).unwrap();
        root
    }

    fn origin(phase: &str) -> Marker {
        Marker { role: ORIGIN.into(), phase: phase.into(), operation_id: "op-1".into(), ..Marker::default() }
    }

    #[test]
    fn every_phase_but_cancelled_and_active_pauses_and_names_the_operation() {
        for phase in [PAUSING, CAPTURED, CONFIRMED] {
            let marker = origin(phase);
            assert!(marker.paused(), "{phase}");
            assert!(marker.message().contains("op-1"), "{phase}");
        }
        assert!(!origin(CANCELLED).paused());
        let imported = Marker { role: DESTINATION.into(), phase: IMPORTED.into(), operation_id: "op-2".into(), ..Marker::default() };
        assert!(imported.paused());
        assert!(imported.message().contains("migration_activate"));
        let active = Marker { phase: ACTIVE.into(), ..imported };
        assert!(!active.paused());
    }

    #[test]
    fn the_marker_survives_a_restart_and_an_unreadable_one_pauses() {
        let root = scratch();
        save(&root, "app-1", &origin(CAPTURED)).unwrap();
        forget(&root);
        let loaded = marker(&root, "app-1").unwrap();
        assert_eq!(loaded.phase, CAPTURED);
        assert!(pause(&root, "app-1").unwrap().1.contains("op-1"));
        std::fs::write(marker_path(&root, "app-1"), "{not json").unwrap();
        forget(&root);
        let broken = marker(&root, "app-1").unwrap();
        assert!(broken.paused());
        assert!(broken.message().contains("cannot be read"));
        assert!(marker(&root, "../escape").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_mail_worker_holds_a_paused_apps_email_only() {
        let root = scratch();
        std::fs::create_dir_all(root.join("deployments/app-2")).unwrap();
        save(&root, "app-1", &origin(PAUSING)).unwrap();
        assert!(holds_mail(&root, "deployment:app-1"));
        assert!(!holds_mail(&root, "deployment:app-2"));
        assert!(!holds_mail(&root, "creator"));
        save(&root, "app-1", &origin(CANCELLED)).unwrap();
        assert!(!holds_mail(&root, "deployment:app-1"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_new_url_shows_for_seven_days_after_confirmation_only() {
        let now = now_ms();
        let mut marker = origin(CONFIRMED);
        marker.new_url = Some("https://new.example/".into());
        marker.retain_until = Some(now + RETENTION_MS);
        assert_eq!(marker.shown_url(now), Some("https://new.example/"));
        assert_eq!(marker.shown_url(now + RETENTION_MS), None);
        marker.phase = CAPTURED.into();
        assert_eq!(marker.shown_url(now), None);
    }

    #[test]
    fn new_urls_must_be_https_elsewhere_and_plain() {
        assert_eq!(validate_new_url("https://notes.example.org", "notes-1.example.com").unwrap(), "https://notes.example.org/");
        assert!(validate_new_url("http://127.0.0.1:9000/", "app.localhost").is_ok());
        for refused in [
            "http://notes.example.org",
            "javascript:alert(1)",
            "https://user:pass@notes.example.org",
            "https://notes.example.org/?next=x",
            "https://notes.example.org/#x",
            "https://app.example.com",
            "notes.example.org",
        ] {
            assert!(validate_new_url(refused, "app.example.com").is_err(), "{refused}");
        }
    }

    #[test]
    fn the_maintenance_page_escapes_what_it_shows() {
        let response = maintenance_page("<b>Notes</b>", "op-1", "Paused & waiting", Some("https://new.example/\"x"));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = futures_body(response);
        assert!(body.contains("&lt;b&gt;Notes&lt;/b&gt;"));
        assert!(body.contains("Paused &amp; waiting"));
        assert!(body.contains("https://new.example/&quot;x"));
        assert!(!body.contains("<script"));
    }

    fn futures_body(response: Response) -> String {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let bytes = runtime.block_on(axum::body::to_bytes(response.into_body(), 1 << 20)).unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn state(root: &Path) -> AppState {
        AppState {
            deployments: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            copies: Arc::new(copies::CopyStatus::default()),
            accounts: accounts::AccountSettings::local(),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            mcp_origin: None,
            path_origin: None,
            home_deployment: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        }
    }

    fn app(id: &str) -> Deployment {
        Deployment {
            id: id.into(),
            project_id: format!("project-{id}"),
            project_name: "Mail".into(),
            url: format!("http://{id}.localhost"),
            created_at: 0,
            manifest: json!({"visibility": "public"}),
            hostname: format!("{id}.localhost"),
            actions: "{}".into(),
            artifact_version: None,
            database_model: None,
            signup_policy: SignupPolicy::default(),
        }
    }

    /// A job the worker has claimed may be with the provider, so settling
    /// waits for it, within its bound. Queued jobs of the paused app end;
    /// another app's jobs are untouched.
    #[tokio::test]
    async fn settling_waits_for_mail_in_flight_and_ends_the_paused_apps_queued_mail() {
        let root = scratch();
        let mail = Connection::open(root.join(mail::MAIL_DATABASE)).unwrap();
        mail::install(&mail).unwrap();
        let job = |id: &str, scope: &str, leased: Option<i64>| {
            mail.execute(
                "INSERT INTO jobs(id,purpose,recipient,scope,app_name,token_hash,link_expires_at,state,next_attempt_at,leased_until,created_at,updated_at)
                 VALUES(?1,'invitation','a@example.test',?2,'Mail','hash',9999999999,'pending',0,?3,0,0)",
                params![id, scope, leased],
            )
            .unwrap();
        };
        let later = now_ms() as i64 + 600_000;
        job("queued", "deployment:app-1", None);
        job("sending", "deployment:app-1", Some(later));
        job("other", "deployment:app-2", None);
        let state = state(&root);
        let waiting = settle(&state, &app("app-1"), Duration::from_millis(600)).await.unwrap();
        assert_eq!(waiting["waitingFor"], "mail");
        assert_eq!(waiting["sending"], 1);
        // The provider answered; the lease is gone.
        mail.execute("UPDATE jobs SET leased_until=NULL WHERE id='sending'", []).unwrap();
        let settled = settle(&state, &app("app-1"), Duration::from_secs(2)).await.unwrap();
        assert!(settled["waitingFor"].is_null());
        assert_eq!(settled["mailEnded"], 2);
        let states: Vec<(String, String)> = {
            let mut query = mail.prepare("SELECT id, state FROM jobs ORDER BY id").unwrap();
            query.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(states, vec![("other".into(), "pending".into()), ("queued".into(), "failed".into()), ("sending".into(), "failed".into())]);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// An operation still holding a lock keeps the pause settling.
    #[tokio::test]
    async fn settling_waits_for_an_operation_in_progress() {
        let root = scratch();
        let state = state(&root);
        let held = database_lock(&state, "app-1").await;
        let waiting = settle(&state, &app("app-1"), Duration::from_millis(300)).await.unwrap();
        assert_eq!(waiting["waitingFor"], "operations");
        drop(held);
        assert!(settle(&state, &app("app-1"), Duration::from_secs(1)).await.unwrap()["waitingFor"].is_null());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_status_view_never_shows_the_confirmation() {
        let mut marker = origin(CONFIRMED);
        marker.confirmation = Some("secret-confirmation".into());
        marker.receipt = Some(json!({"operationId": "op-1", "confirmation": "secret-confirmation"}));
        let shown = view(&marker).to_string();
        assert!(!shown.contains("secret-confirmation"), "{shown}");
        assert!(shown.contains("op-1"));
    }

    #[test]
    fn a_live_artifact_travels_with_its_binary_files_as_base64() {
        let root = std::env::temp_dir().join(format!("littlebox-live-artifact-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("index.html"), "<p>home</p>").unwrap();
        std::fs::write(root.join("img/logo.png"), [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
        let (files, encodings) = live_artifact_files(&root).unwrap();
        assert_eq!(files["index.html"], json!("<p>home</p>"));
        assert_eq!(encodings, json!({"img/logo.png": "base64"}).as_object().unwrap().clone());
        // The payload decodes back to the same bytes the destination will serve.
        let artifact: BuildArtifact = serde_json::from_value(json!({"version": 1, "entrypoint": "index.html", "files": files, "encodings": encodings})).unwrap();
        assert_eq!(super::super::artifact::decode(&artifact).unwrap()["img/logo.png"], [0x89, b'P', b'N', b'G', 0xff, 0xfe]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stamps_need_an_operation_and_a_confirmation_hash() {
        assert!(check_stamp(&json!({"operationId": "op-1", "confirmationHash": "a".repeat(64)})).is_ok());
        assert!(check_stamp(&json!({"operationId": "../x", "confirmationHash": "a".repeat(64)})).is_err());
        assert!(check_stamp(&json!({"operationId": "op-1"})).is_err());
        assert!(check_stamp(&json!({"operationId": "op-1", "confirmationHash": "A".repeat(64)})).is_err());
    }
}
