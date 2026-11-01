//! A path address: an app answers on `<domain>/p/<id>`, and on
//! `<domain>/p/<name>` when it is named (docs/adr/0024-an-installation-names-its-hosts-from-one-domain-and-an-app-may-answer-on-a-path.md).
//!
//! The owner turns the path on with `path_set`. The state lives in
//! `deployments/<id>/path.json`, next to `domain.json`. The Kernel keeps the
//! enabled ids and names in memory, so `deployment_for_segment` answers the
//! router without a file read. Every path and the home share the origin
//! `<domain>`, so an app with accounts, or a private app, needs the owner's
//! `acceptSharedOrigin` before its path turns on. `has_accounts` decides that.
//!
//! A package carries the path's state and the name, never the id. The
//! destination keeps the name when it is free there.

use super::path_name::check_name;
use super::refusal::{refuse, Refusal};
use super::*;
use anyhow::{bail, Context};
use axum::http::Uri;
use axum::middleware::Next;
use std::sync::{Mutex, OnceLock};

/// Set on a request that answers a path address or the home, to the id of the
/// Deployment it answers. `route` removes any copy a client sent before it
/// resolves, so only this layer can set it, and `deployment_for` reads it.
pub(crate) const RESOLVED_HEADER: &str = "x-konae-path-deployment";
/// Set beside `RESOLVED_HEADER` on a request that came in under `/p/<segment>`,
/// to that prefix, so a handler can tell a path address from the home after
/// the router stripped it. `route` removes any copy a client sent.
pub(crate) const PREFIX_HEADER: &str = "x-konae-path-prefix";

/// The `/p/<segment>` prefix the router stripped from this request, if any.
pub(crate) fn prefix(headers: &HeaderMap) -> Option<&str> {
    headers.get(PREFIX_HEADER)?.to_str().ok()
}

pub(crate) const RECORD_FILE: &str = "path.json";

/// The path a Deployment holds. A name stays on the record while the path
/// is off, and only an enabled name holds it on the installation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Record {
    #[serde(default)]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) accept_shared_origin: bool,
}

/// The path a package carries. It holds the owner's decision about this app,
/// never the Deployment id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Carried {
    pub(super) enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) accept_shared_origin: bool,
}

// ---------------------------------------------------------------------------
// The index of enabled ids and names

/// Enabled ids and names to their Deployment, per data directory, so tests
/// with their own data directories in one process do not see each other's.
type Index = HashMap<String, String>;
static INDEXES: OnceLock<Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();

fn with_index<T>(state: &AppState, f: impl FnOnce(&mut Index) -> T) -> T {
    let mut indexes = INDEXES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = indexes.entry(state.data_root.clone()).or_insert_with(|| load_index(&state.data_root));
    f(index)
}

/// Reads every `path.json` on the volume, as the Kernel reads the records at
/// startup. The Kernel calls `load` once so the index is ready before a request.
pub(crate) fn load(state: &AppState) {
    with_index(state, |_| ());
}

fn load_index(root: &Path) -> Index {
    let mut index = Index::new();
    let Ok(entries) = std::fs::read_dir(crate::layout::apps(&root)) else { return index };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Some(record) = std::fs::read(crate::layout::private(root, &id).join(RECORD_FILE)).ok().and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok()) else {
            continue;
        };
        if record.enabled {
            // Two folders cannot both hold a name on a healthy volume; a
            // hand-edited one keeps the first name read.
            index.entry(id.clone()).or_insert_with(|| id.clone());
            if let Some(name) = record.name {
                index.entry(name).or_insert_with(|| id.clone());
            }
        }
    }
    index
}

/// The Deployment a path segment names: an enabled id or an enabled name.
/// The router passes the text after `/p/`, lowercased.
pub(crate) fn deployment_for_segment(state: &AppState, segment: &str) -> Option<String> {
    with_index(state, |index| index.get(segment).cloned())
}

/// The path name whose Deployment answers `<domain>/`. The operator claims it
/// with `path_set` like any other name, before anyone else does.
pub(crate) const HOME_NAME: &str = "konae-home";

// ---------------------------------------------------------------------------
// Resolution and the router

/// The host name of `<domain>`, lowercased, or `None` without `KONAE_DOMAIN`.
pub(crate) fn path_host(state: &AppState) -> Option<String> {
    url::Url::parse(state.path_origin.as_deref()?).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// The host name a request names, with its port removed.
fn request_host(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    url::Url::parse(&format!("http://{host}")).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Whether a request names the path host. Its port is not compared, so
/// `localhost:8080` in a Host header matches a path origin on `localhost`.
pub(crate) fn on_path_host(state: &AppState, headers: &HeaderMap) -> bool {
    match (request_host(headers), path_host(state)) {
        (Some(requested), Some(path)) => requested == path,
        _ => false,
    }
}

/// What the path host answers for one request.
pub(crate) struct Resolution {
    /// The Deployment that answers, or `None` when no Deployment does.
    pub(crate) deployment_id: Option<String>,
    /// `/p/<segment>` when the path names an address, as the client sent it.
    pub(crate) prefix: Option<String>,
}

/// Where a request on the path host goes. `None` when the request names
/// another host. A path under `/p/` names an address; any other path belongs
/// to the home. The segment is lowercased here, and the home is the
/// Deployment whose enabled path holds the name `konae-home`.
pub(crate) async fn resolve(state: &AppState, headers: &HeaderMap, path: &str) -> Option<Resolution> {
    if !on_path_host(state, headers) {
        return None;
    }
    if let Some(rest) = path.strip_prefix("/p/") {
        let segment = rest.split('/').next().unwrap_or_default();
        return Some(Resolution {
            deployment_id: deployment_for_segment(state, &segment.to_ascii_lowercase()),
            prefix: Some(format!("/p/{segment}")),
        });
    }
    Some(Resolution { deployment_id: deployment_for_segment(state, HOME_NAME), prefix: None })
}

/// Runs before routing, on every request, and answers a path address as the
/// same routes under its prefix. `/p/<segment>/rest?query` routes as `/rest?query`,
/// so `/_konae/mcp`, the files, the runtime and the HTTP routes answer
/// unchanged. `/p/<segment>` with no slash redirects to `/p/<segment>/`, so the
/// relative URLs of the page resolve under the prefix. A `Location` the app
/// sends from the root is prefixed too. Other paths on the path host belong to
/// the home, and with no home they answer the normal not-found.
pub(crate) async fn route(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    request.headers_mut().remove(RESOLVED_HEADER);
    request.headers_mut().remove(PREFIX_HEADER);
    let Some(resolution) = resolve(&state, request.headers(), request.uri().path()).await else {
        return next.run(request).await;
    };
    let Some(prefix) = resolution.prefix else {
        if let Some(id) = resolution.deployment_id {
            request.headers_mut().insert(RESOLVED_HEADER, HeaderValue::from_str(&id).unwrap());
        }
        return next.run(request).await;
    };
    // An unknown segment passes through, so the routes answer the normal not-found.
    let Some(id) = resolution.deployment_id else {
        return next.run(request).await;
    };
    if request.uri().path() == prefix {
        let query = request.uri().query().map(|query| format!("?{query}")).unwrap_or_default();
        return (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, format!("{prefix}/{query}"))]).into_response();
    }
    let rest = request.uri().path()[prefix.len()..].to_owned();
    let path_and_query = match request.uri().query() {
        Some(query) => format!("{rest}?{query}"),
        None => rest,
    };
    match path_and_query.parse::<Uri>() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(_) => return error(StatusCode::BAD_REQUEST, "The request path is not valid."),
    }
    request.headers_mut().insert(RESOLVED_HEADER, HeaderValue::from_str(&id).unwrap());
    if let Ok(value) = HeaderValue::from_str(&prefix) {
        request.headers_mut().insert(PREFIX_HEADER, value);
    }
    let mut response = next.run(request).await;
    if let Some(location) = response.headers().get(header::LOCATION).and_then(|value| value.to_str().ok()) {
        if location.starts_with('/') && !location.starts_with("//") {
            let located = format!("{prefix}{location}");
            response.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&located).unwrap());
        }
    }
    response
}

/// Refuses a request on the path host whose Referer is a path address of
/// another Deployment, since that page belongs to another app asking this
/// one. Every path shares one origin, so this is defense in depth and not
/// isolation. A Referer that is absent, unparseable or on another host does not count.
pub(crate) fn foreign_referer(state: &AppState, headers: &HeaderMap, deployment_id: &str) -> bool {
    if !on_path_host(state, headers) {
        return false;
    }
    let Some(referer) = headers.get(header::REFERER).and_then(|value| value.to_str().ok()).and_then(|value| url::Url::parse(value).ok()) else {
        return false;
    };
    if referer.host_str().map(str::to_ascii_lowercase) != path_host(state) {
        return false;
    }
    let Some(rest) = referer.path().strip_prefix("/p/") else {
        return false;
    };
    let segment = rest.split('/').next().unwrap_or_default().to_ascii_lowercase();
    deployment_for_segment(state, &segment).is_some_and(|holder| holder != deployment_id)
}

// ---------------------------------------------------------------------------
// Records

pub(super) fn read_record(state: &AppState, id: &str) -> Option<Record> {
    let bytes = std::fs::read(crate::layout::private(&state.data_root, id).join(RECORD_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes or removes the record and moves the index to match. Callers hold
/// the `paths` lock.
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
        let previous_name = previous.as_ref().and_then(|record| record.name.clone());
        for key in std::iter::once(id.to_owned()).chain(previous_name) {
            if index.get(&key).is_some_and(|holder| holder == id) {
                index.remove(&key);
            }
        }
        if let Some(record) = record.filter(|record| record.enabled) {
            index.insert(id.to_owned(), id.to_owned());
            if let Some(name) = &record.name {
                index.insert(name.clone(), id.to_owned());
            }
        }
    });
    Ok(())
}

/// Turns a Deployment's path on with the owner's acceptance, for other modules' tests.
#[cfg(test)]
pub(crate) async fn turn_on_for_test(state: &AppState, id: &str, name: Option<&str>) {
    let record = Record { enabled: true, name: name.map(str::to_owned), accept_shared_origin: true };
    write_record(state, id, Some(&record)).await.unwrap();
}

/// Drops a deleted Deployment's path, so its id and name answer nothing.
pub(super) async fn forget(state: &AppState, id: &str) -> Result<()> {
    let _paths = database_lock(state, "paths").await;
    write_record(state, id, None).await
}

/// Whether the app has accounts: it is private, its signup is not Closed, or
/// one of its actions is not anonymous. An actions file the Kernel cannot read
/// counts as having accounts.
pub(super) fn has_accounts(deployment: &Deployment) -> bool {
    is_private(deployment) || deployment.signup_policy.mode != SignupMode::Closed || actions_need_accounts(&deployment.actions)
}

fn actions_need_accounts(actions: &str) -> bool {
    match serde_json::from_str::<ActionProgram>(actions) {
        Ok(program) => program.actions.values().any(|action| action.access != "anonymous"),
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// Refusals

/// Another enabled Deployment holds the name. The message never says who.
fn taken(name: &str) -> Refusal {
    refuse("path.taken", format!("{name} is taken on this installation. Choose another name, such as spring-menu."))
}

/// The risk of a shared origin, in the words the owner reads before accepting.
fn shared_origin(domain: &str) -> Refusal {
    refuse(
        "path.shared-origin",
        format!(
            "This app has accounts or is private, so a path address on {domain} shares its origin with the home and every other app there. Any other app on {domain}, including other owners' apps, can use the sessions of this app's users. The app's own hostname keeps its sessions on its own origin. Set acceptSharedOrigin to true to accept this, or close signup and keep every action anonymous so the app has no sessions to share."
        ),
    )
}

/// The domain a path answers on, from `<scheme>://<domain>[:port]`.
fn domain_of(origin: &str) -> String {
    url::Url::parse(origin).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| origin.to_owned())
}

/// Refuses a name another enabled Deployment holds. A name nobody has enabled
/// is free, and a Deployment that holds it stays the holder.
async fn ensure_free(state: &AppState, id: &str, name: &str) -> std::result::Result<(), Refusal> {
    let holder = with_index(state, |index| index.get(name).filter(|holder| holder.as_str() != id).cloned());
    match holder {
        Some(holder) if state.deployments.read().await.values().any(|d| d.id == holder) => Err(taken(name)),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Tools

pub(super) fn read_tool() -> Value {
    mcp::tool(
        "path_read",
        "Read a deployment's path address: whether it is on, its name, whether the owner accepted the shared origin, and the addresses it answers on.",
        json!({"deploymentId":{"type":"string","minLength":1}}),
        &["deploymentId"],
        true,
    )
}

pub(super) fn tools() -> Vec<Value> {
    vec![
        mcp::tool(
            "path_set",
            "Answer a deployment on <domain>/p/<id>, and on <domain>/p/<name> when it has a name. The path shares its origin with the home and every other path on this installation, so an app with accounts or a private app needs acceptSharedOrigin true before its path turns on. A name is 3 to 40 letters, digits and hyphens, and no other Deployment may hold it. An omitted name or acceptSharedOrigin keeps the stored value, a null name removes the name, and path_clear turns the path off and removes both. Read back with path_read. Requires publish.",
            json!({
                "deploymentId":{"type":"string","minLength":1},
                "enabled":{"type":"boolean"},
                "name":{"type":["string","null"],"minLength":1,"maxLength":260},
                "acceptSharedOrigin":{"type":"boolean"}
            }),
            &["deploymentId", "enabled"],
            false,
        ),
        mcp::tool(
            "path_clear",
            "Turn off a deployment's path address and forget its name. The generated hostname and any custom domain keep answering. Requires publish.",
            json!({"deploymentId":{"type":"string","minLength":1}}),
            &["deploymentId"],
            false,
        ),
    ]
}

/// The state the owner reads. Addresses are listed only while the path is on
/// and the installation names a domain.
fn view(state: &AppState, id: &str, record: Option<Record>) -> Value {
    let record = record.unwrap_or_default();
    let mut addresses = Vec::new();
    if let (true, Some(origin)) = (record.enabled, state.path_origin.as_deref()) {
        addresses.push(format!("{origin}/p/{id}"));
        if let Some(name) = &record.name {
            addresses.push(format!("{origin}/p/{name}"));
        }
    }
    let mut value = json!({
        "deploymentId": id,
        "enabled": record.enabled,
        "name": record.name,
        "acceptSharedOrigin": record.accept_shared_origin,
        "addresses": addresses,
        "domainConfigured": state.path_origin.is_some(),
    });
    if state.path_origin.is_none() {
        value["next"] = json!("Path addresses need KONAE_DOMAIN on this installation. Set it, then call path_set again.");
    } else if !record.enabled {
        value["next"] = json!("The path is off. Call path_set with enabled true to get its addresses.");
    } else {
        value["next"] = json!("Read the addresses back with path_read after a change.");
    }
    value
}

pub(super) async fn admin_call(state: &AppState, owner: &mcp::Owner, name: &str, a: &Value) -> Result<Value> {
    let id = a["deploymentId"].as_str().unwrap_or_default();
    match name {
        "path_read" => {
            let deployment = mcp::owned_deployment(state, owner, id, "read").await?;
            Ok(view(state, &deployment.id, read_record(state, &deployment.id)))
        }
        "path_clear" => {
            let deployment = mcp::owned_deployment(state, owner, id, "publish").await?;
            let _paths = database_lock(state, "paths").await;
            write_record(state, &deployment.id, None).await?;
            Ok(view(state, &deployment.id, None))
        }
        "path_set" => set(state, owner, a).await,
        _ => bail!("Unknown tool."),
    }
}

async fn set(state: &AppState, owner: &mcp::Owner, a: &Value) -> Result<Value> {
    let id = a["deploymentId"].as_str().unwrap_or_default();
    let deployment = mcp::owned_deployment(state, owner, id, "publish").await?;
    let enabled = a["enabled"].as_bool().context("enabled must be true or false.")?;
    let _paths = database_lock(state, "paths").await;
    let previous = read_record(state, &deployment.id).unwrap_or_default();
    // An omitted name keeps the stored one; a null name removes it and keeps the path on or off as asked.
    let name = match a.get("name") {
        Some(Value::String(raw)) => Some(check_name(raw)?),
        Some(Value::Null) => None,
        _ => previous.name.clone(),
    };
    let accept_shared_origin = a["acceptSharedOrigin"].as_bool().unwrap_or(previous.accept_shared_origin);
    // A name only holds the installation while its path is on, so turning a path off never refuses.
    if let (true, Some(name)) = (enabled, &name) {
        ensure_free(state, &deployment.id, name).await?;
    }
    if enabled {
        let origin = state.path_origin.as_deref().context("Path addresses need KONAE_DOMAIN on this installation. Set it, then turn the path on again.")?;
        if has_accounts(&deployment) && !accept_shared_origin {
            return Err(shared_origin(&domain_of(origin)).into());
        }
    }
    let record = Record { enabled, name, accept_shared_origin };
    write_record(state, &deployment.id, Some(&record)).await?;
    Ok(view(state, &deployment.id, Some(record)))
}

// ---------------------------------------------------------------------------
// Packages

/// The path a package carries, or none when the Deployment has no path record.
pub(super) fn for_package(state: &AppState, id: &str) -> Option<Carried> {
    read_record(state, id).map(|record| Carried { enabled: record.enabled, name: record.name, accept_shared_origin: record.accept_shared_origin })
}

/// Puts a package's path on the Deployment it installs as. A name another
/// enabled Deployment holds, or one the rules refuse, is dropped. A path
/// without the owner's acceptance, on an app with accounts, stays off. The
/// returned lines are the import notice; they name what changed and why.
pub(super) async fn arrive(state: &AppState, deployment: &Deployment, carried: Option<&Carried>) -> Result<Vec<String>> {
    let Some(carried) = carried else { return Ok(Vec::new()) };
    let _paths = database_lock(state, "paths").await;
    let mut notices = Vec::new();
    let name = match carried.name.as_deref() {
        None => None,
        Some(raw) => match check_name(raw) {
            Ok(checked) => match ensure_free(state, &deployment.id, &checked).await {
                Ok(()) => Some(checked),
                Err(_) => {
                    notices.push(format!("The package's path name {checked} is taken on this installation, so the import dropped it. Set another with path_set."));
                    None
                }
            },
            Err(refusal) => {
                notices.push(format!("The package's path name {raw} is not valid here, so the import dropped it. {refusal}"));
                None
            }
        },
    };
    let mut enabled = carried.enabled;
    if enabled && has_accounts(deployment) && !carried.accept_shared_origin {
        enabled = false;
        notices.push("The path stays off: the app has accounts or is private, and the package does not accept the shared origin. Call path_set with acceptSharedOrigin true to turn it on.".to_owned());
    }
    if enabled && state.path_origin.is_none() {
        notices.push("This installation has no KONAE_DOMAIN, so the path address does not answer until it is set.".to_owned());
    }
    let record = Record { enabled, name, accept_shared_origin: carried.accept_shared_origin };
    write_record(state, &deployment.id, Some(&record)).await?;
    Ok(notices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusals a case file names, asserted with the words the Kernel gives.
    #[test]
    fn refusals_are_given_with_the_words_and_codes_the_case_file_names() {
        let file: Value = serde_json::from_str(include_str!("path-state-cases.json")).unwrap();
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 2);
        for case in cases {
            let outcome: std::result::Result<(), Refusal> = match case["code"].as_str().unwrap() {
                "path.taken" => Err(taken(case["name"].as_str().unwrap())),
                "path.shared-origin" => Err(shared_origin(case["domain"].as_str().unwrap())),
                other => panic!("{other} has no builder in path.rs"),
            };
            crate::refusal::assert_case(case, outcome);
        }
    }

    fn test_state(root: &Path, origin: Option<&str>) -> AppState {
        AppState {
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            data_root: root.into(),
            files: crate::file_store::Files::local(root),
            copies: Arc::new(copies::CopyStatus::default()),
            admin_key: "test-admin".into(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "https://app.konae.es".into(),
            mcp_origin: None,
            host_suffix: "konae.es".into(),
            path_origin: origin.map(str::to_owned),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        }
    }

    /// A folder for a Deployment that holds a path record, without a Deployment behind it.
    async fn folder(state: &AppState, id: &str) {
        tokio::fs::create_dir_all(crate::layout::data(&state.data_root, id)).await.unwrap();
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
        let request = Request::builder().header("x-konae-admin-key", "test-admin").body(Body::from(body.to_string())).unwrap();
        assert_eq!(deploy(State(state.clone()), request).await.status(), StatusCode::OK);
        mcp::store(state).unwrap().execute("INSERT INTO projects(id,owner,body) VALUES(?1,'o','{}')", [project]).unwrap();
        state.deployments.read().await.values().find(|d| d.project_id == project).cloned().unwrap()
    }

    fn owner() -> mcp::Owner {
        mcp::Owner { id: "o".into(), display: None, scopes: vec!["read".into(), "publish".into()], projects: vec![], session: false }
    }

    async fn set(state: &AppState, id: &str, args: Value) -> Result<Value> {
        let mut a = args;
        a["deploymentId"] = json!(id);
        admin_call(state, &owner(), "path_set", &a).await
    }

    /// Only enabled ids and names answer, and a disabled path frees its name.
    #[tokio::test]
    async fn a_segment_answers_only_while_its_path_is_on() {
        let root = std::env::temp_dir().join(format!("konae-path-segment-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root, Some("https://konae.es"));
        folder(&state, "d1").await;
        folder(&state, "d2").await;
        write_record(&state, "d1", Some(&Record { enabled: true, name: Some("spring-menu".into()), accept_shared_origin: true })).await.unwrap();
        write_record(&state, "d2", Some(&Record { enabled: false, name: Some("winter-menu".into()), accept_shared_origin: false })).await.unwrap();

        assert_eq!(deployment_for_segment(&state, "d1").as_deref(), Some("d1"));
        assert_eq!(deployment_for_segment(&state, "spring-menu").as_deref(), Some("d1"));
        assert_eq!(deployment_for_segment(&state, "d2"), None);
        assert_eq!(deployment_for_segment(&state, "winter-menu"), None);
        assert_eq!(deployment_for_segment(&state, "missing"), None);

        write_record(&state, "d1", Some(&Record { enabled: false, name: Some("spring-menu".into()), accept_shared_origin: true })).await.unwrap();
        assert_eq!(deployment_for_segment(&state, "d1"), None);
        assert_eq!(deployment_for_segment(&state, "spring-menu"), None);
        // The freed name can be taken by the other Deployment.
        write_record(&state, "d2", Some(&Record { enabled: true, name: Some("spring-menu".into()), accept_shared_origin: false })).await.unwrap();
        assert_eq!(deployment_for_segment(&state, "spring-menu").as_deref(), Some("d2"));

        // A rebuilt index, as after a restart, holds the same answers.
        assert_eq!(load_index(&root).get("spring-menu").map(String::as_str), Some("d2"));
        assert_eq!(load_index(&root).get("d1"), None);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// Only a private app, one with signup not Closed, or one with a non-anonymous action has accounts.
    #[tokio::test]
    async fn an_app_has_accounts_when_it_is_private_open_or_calls_a_non_anonymous_action() {
        let root = std::env::temp_dir().join(format!("konae-path-accounts-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root, Some("https://konae.es"));
        let app = publish(&state, "plain").await;
        // Public, signup Open by default: accounts.
        assert_eq!(app.signup_policy.mode, SignupMode::Open);
        assert!(has_accounts(&app));

        let mut closed = app.clone();
        closed.manifest["visibility"] = json!("public");
        closed.signup_policy.mode = SignupMode::Closed;
        assert!(!has_accounts(&closed), "public, signup Closed and only anonymous actions");

        let mut invitation = closed.clone();
        invitation.signup_policy.mode = SignupMode::Invitation;
        assert!(has_accounts(&invitation), "signup by invitation is an account");

        let mut private = closed.clone();
        private.manifest["visibility"] = json!("private");
        assert!(has_accounts(&private), "a private app has accounts");

        let mut authenticated = closed.clone();
        let mut actions: Value = serde_json::from_str(&closed.actions).unwrap();
        actions["actions"]["items.count"]["access"] = json!("authenticated");
        authenticated.actions = actions.to_string();
        assert!(has_accounts(&authenticated), "an authenticated action needs an account");

        let mut unreadable = closed.clone();
        unreadable.actions = "not json".into();
        assert!(has_accounts(&unreadable), "an actions file the Kernel cannot read counts as accounts");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The owner's words for the path, and what each tool answers on the way.
    #[tokio::test]
    async fn path_set_checks_the_name_the_domain_and_the_shared_origin_before_it_writes() {
        let root = std::env::temp_dir().join(format!("konae-path-set-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root, Some("https://konae.es"));
        let notes = publish(&state, "notes").await;
        let other = publish(&state, "other").await;

        // An app with accounts needs the owner's acceptance before the path turns on.
        let refused = set(&state, &notes.id, json!({"enabled": true})).await.unwrap_err();
        let refusal = crate::refusal::of(&refused).expect("path_set refuses with a code");
        assert_eq!(refusal.code, "path.shared-origin");
        assert!(refusal.message.contains("konae.es"));
        assert!(read_record(&state, &notes.id).is_none(), "a refused call writes nothing");
        assert_eq!(deployment_for_segment(&state, &notes.id), None);

        // Accepted, the path answers on the id and the name.
        let on = set(&state, &notes.id, json!({"enabled": true, "name": "Spring-Menu", "acceptSharedOrigin": true})).await.unwrap();
        assert_eq!(on["name"], "spring-menu");
        assert_eq!(on["addresses"], json!([format!("https://konae.es/p/{}", notes.id), "https://konae.es/p/spring-menu"]));
        assert_eq!(deployment_for_segment(&state, "spring-menu").as_deref(), Some(notes.id.as_str()));

        // Another app cannot take the name, and the answer does not say who holds it.
        let taken = set(&state, &other.id, json!({"enabled": true, "name": "spring-menu", "acceptSharedOrigin": true})).await.unwrap_err();
        let refusal = crate::refusal::of(&taken).expect("path_set refuses with a code");
        assert_eq!(refusal.code, "path.taken");
        assert_eq!(refusal.message, "spring-menu is taken on this installation. Choose another name, such as spring-menu.");
        assert!(!refusal.message.contains(&notes.id));

        // A bad name is refused with the rule's code, before anything is written.
        let bad = set(&state, &other.id, json!({"enabled": false, "name": "app"})).await.unwrap_err();
        assert_eq!(crate::refusal::of(&bad).map(|r| r.code), Some("path.reserved"));

        // Turning the path off frees the name for the other app.
        let off = set(&state, &notes.id, json!({"enabled": false})).await.unwrap();
        assert_eq!(off["name"], "spring-menu", "the name stays on the record");
        assert_eq!(off["addresses"], json!([]));
        assert_eq!(deployment_for_segment(&state, "spring-menu"), None);
        let other_on = set(&state, &other.id, json!({"enabled": true, "name": "spring-menu", "acceptSharedOrigin": true})).await.unwrap();
        assert_eq!(other_on["addresses"].as_array().unwrap().len(), 2);

        // Turning the first back on finds the name held.
        let back = set(&state, &notes.id, json!({"enabled": true})).await.unwrap_err();
        assert_eq!(crate::refusal::of(&back).map(|r| r.code), Some("path.taken"));

        // A null name drops the name and leaves the path on.
        let unnamed = set(&state, &other.id, json!({"enabled": true, "name": null})).await.unwrap();
        assert_eq!((unnamed["enabled"].clone(), unnamed["name"].clone()), (json!(true), Value::Null));
        assert_eq!(unnamed["addresses"], json!([format!("https://konae.es/p/{}", other.id)]));
        assert_eq!(unnamed["domainConfigured"], json!(true));
        assert_eq!(deployment_for_segment(&state, "spring-menu"), None);

        // path_read shows the stored state; path_clear forgets it.
        let read = admin_call(&state, &owner(), "path_read", &json!({"deploymentId": other.id})).await.unwrap();
        assert_eq!((read["enabled"].clone(), read["acceptSharedOrigin"].clone()), (json!(true), json!(true)));
        let cleared = admin_call(&state, &owner(), "path_clear", &json!({"deploymentId": other.id})).await.unwrap();
        assert_eq!((cleared["enabled"].clone(), cleared["name"].clone()), (json!(false), Value::Null));
        assert_eq!(deployment_for_segment(&state, "spring-menu"), None);
        forget(&state, &notes.id).await.unwrap();
        assert!(read_record(&state, &notes.id).is_none());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// Without KONAE_DOMAIN the path has no address, so turning it on is refused.
    #[tokio::test]
    async fn an_installation_without_a_domain_reads_the_path_but_does_not_turn_it_on() {
        let root = std::env::temp_dir().join(format!("konae-path-nodomain-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root, None);
        let app = publish(&state, "solo").await;
        let read = admin_call(&state, &owner(), "path_read", &json!({"deploymentId": app.id})).await.unwrap();
        assert_eq!(read["addresses"], json!([]));
        assert_eq!(read["next"], "Path addresses need KONAE_DOMAIN on this installation. Set it, then call path_set again.");
        let refused = set(&state, &app.id, json!({"enabled": true, "acceptSharedOrigin": true})).await.unwrap_err();
        assert_eq!(refused.to_string(), "Path addresses need KONAE_DOMAIN on this installation. Set it, then turn the path on again.");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A package carries the path without the id, and the destination keeps a free name only.
    #[tokio::test]
    async fn a_package_keeps_a_free_name_and_says_what_it_dropped() {
        let root = std::env::temp_dir().join(format!("konae-path-arrive-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root, Some("https://konae.es"));
        let notes = publish(&state, "notes").await;
        set(&state, &notes.id, json!({"enabled": true, "acceptSharedOrigin": true, "name": "spring-menu"})).await.unwrap();

        let carried = for_package(&state, &notes.id).unwrap();
        let wire = serde_json::to_value(&carried).unwrap();
        assert_eq!(wire, json!({"enabled": true, "name": "spring-menu", "acceptSharedOrigin": true}), "no id travels");

        // The name is taken on the destination: the import keeps the path on, drops the name, and says so.
        let mut arriving = notes.clone();
        arriving.id = "arriving".into();
        folder(&state, "arriving").await;
        let notices = arrive(&state, &arriving, Some(&carried)).await.unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("The package's path name spring-menu is taken on this installation"), "{notices:?}");
        assert!(!notices[0].contains(&notes.id), "the notice does not name the holder");
        assert_eq!(read_record(&state, "arriving"), Some(Record { enabled: true, name: None, accept_shared_origin: true }));
        assert_eq!(deployment_for_segment(&state, "arriving").as_deref(), Some("arriving"));

        // A name the rules refuse is dropped the same way.
        let mut invalid = carried.clone();
        invalid.name = Some("app".into());
        let notices = arrive(&state, &arriving, Some(&invalid)).await.unwrap();
        assert!(notices[0].contains("is not valid here"), "{notices:?}");
        assert!(notices[0].contains("app is reserved"), "{notices:?}");

        // A path without the acceptance stays off on an app with accounts, and keeps its free name.
        let mut unaccepted = carried.clone();
        unaccepted.name = Some("fresh-menu".into());
        unaccepted.accept_shared_origin = false;
        let notices = arrive(&state, &arriving, Some(&unaccepted)).await.unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("The path stays off"), "{notices:?}");
        assert_eq!(read_record(&state, "arriving"), Some(Record { enabled: false, name: Some("fresh-menu".into()), accept_shared_origin: false }));
        assert_eq!(deployment_for_segment(&state, "arriving"), None);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The host an installation's path addresses answer on.
    const PATH_HOST: &str = "konae.es";

    /// One request through the Kernel's stack, as `main` serves it.
    async fn call(state: &AppState, method: &str, host: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Response {
        use tower::ServiceExt;
        let mut builder = Request::builder().method(method).uri(path).header(header::HOST, host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(Body::from(body.to_owned())).unwrap();
        crate::stack(state.clone()).oneshot(request).await.unwrap()
    }

    async fn get(state: &AppState, path: &str) -> Response {
        call(state, "GET", PATH_HOST, path, &[], "").await
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// A state for a Kernel on `https://konae.es`, with two apps whose
    /// paths are on. The first answers as `spring-menu`, the second as `winter-menu`.
    async fn two_path_apps(root: &Path) -> (AppState, Deployment, Deployment) {
        let state = test_state(root, Some("https://konae.es"));
        let first = publish(&state, "spring").await;
        let second = publish(&state, "winter").await;
        set(&state, &first.id, json!({"enabled": true, "name": "spring-menu", "acceptSharedOrigin": true})).await.unwrap();
        set(&state, &second.id, json!({"enabled": true, "name": "winter-menu", "acceptSharedOrigin": true})).await.unwrap();
        (state, first, second)
    }

    async fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("konae-path-{label}-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        root
    }

    /// The path address serves the app under its id and its name, and the
    /// routes answer under the prefix with the Deployment they belong to.
    #[tokio::test]
    async fn a_path_address_serves_the_app_and_its_routes_under_the_prefix() {
        let root = scratch("serve").await;
        let (state, first, second) = two_path_apps(&root).await;

        for path in [format!("/p/{}/", first.id), "/p/spring-menu/".to_owned(), format!("/p/{}/", first.id.to_uppercase())] {
            let response = get(&state, &path).await;
            assert_eq!(response.status(), 200, "{path}");
            assert!(text(response).await.contains("shell"), "{path}");
        }
        let counted = get(&state, &format!("/p/{}/api/count", first.id)).await;
        assert_eq!(counted.status(), 200);
        let answer: Value = serde_json::from_str(&text(counted).await).unwrap();
        assert_eq!(answer["result"][0]["n"], 1, "{answer}");
        assert_eq!(get(&state, &format!("/p/{}/api/count", second.id)).await.status(), 200);

        let runtime = call(
            &state,
            "POST",
            PATH_HOST,
            &format!("/p/{}/_konae/mcp", first.id),
            &[("content-type", "application/json"), ("accept", "application/json, text/event-stream")],
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
        )
        .await;
        assert_eq!(runtime.status(), 200);
        assert!(text(runtime).await.contains("result"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// An unknown segment, and an empty one, answer the normal not-found.
    #[tokio::test]
    async fn an_unknown_segment_answers_not_found() {
        let root = scratch("unknown").await;
        let (state, _, _) = two_path_apps(&root).await;
        for path in ["/p/missing/", "/p/missing", "/p/", "/p/missing/_konae/mcp"] {
            let response = get(&state, path).await;
            assert_eq!(response.status(), 404, "{path}");
            let body: Value = serde_json::from_str(&text(response).await).unwrap();
            assert_eq!(body["error"], "Konae deployment not found.", "{path}");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// `/p/<segment>` without a slash redirects, keeping the query, so the
    /// relative URLs of the page resolve under the prefix.
    #[tokio::test]
    async fn a_bare_address_redirects_to_its_slash_form_and_keeps_the_query() {
        let root = scratch("redirect").await;
        let (state, first, _) = two_path_apps(&root).await;
        let response = get(&state, &format!("/p/{}", first.id)).await;
        assert_eq!(response.status(), 308);
        assert_eq!(response.headers()[header::LOCATION], format!("/p/{}/", first.id).as_str());
        let response = get(&state, "/p/spring-menu?a=1").await;
        assert_eq!(response.status(), 308);
        assert_eq!(response.headers()[header::LOCATION], "/p/spring-menu/?a=1");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The app whose path holds the name `konae-home` answers `/` and every
    /// other non-prefix path. Without one, `/` answers the normal not-found.
    #[tokio::test]
    async fn the_app_named_konae_home_answers_the_root_of_the_path_host() {
        let root = scratch("home").await;
        let (state, first, _) = two_path_apps(&root).await;
        assert_eq!(get(&state, "/").await.status(), 404, "no home answers nothing");
        assert_eq!(get(&state, "/api/count").await.status(), 404);

        set(&state, &first.id, json!({"enabled": true, "name": "konae-home"})).await.unwrap();
        let home = get(&state, "/").await;
        assert_eq!(home.status(), 200);
        assert!(text(home).await.contains("shell"));
        assert_eq!(get(&state, "/api/count").await.status(), 200, "a non-prefix path belongs to the home");
        assert_eq!(get(&state, "/p/konae-home/").await.status(), 200, "the home keeps its path address");

        // Turning the path off takes the home away again.
        set(&state, &first.id, json!({"enabled": false})).await.unwrap();
        assert_eq!(get(&state, "/").await.status(), 404);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A path address gets the app policy and the installation's HSTS, and
    /// the administrative routes refuse its origin.
    #[tokio::test]
    async fn a_path_address_gets_the_app_policy_and_refuses_administration() {
        let root = scratch("policy").await;
        let (state, first, _) = two_path_apps(&root).await;
        let response = get(&state, &format!("/p/{}/", first.id)).await;
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_owned();
        assert!(policy.starts_with("default-src 'self'; script-src 'self';"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'") && policy.contains("base-uri 'none'"));
        assert_eq!(response.headers()[header::STRICT_TRANSPORT_SECURITY], "max-age=31536000; includeSubDomains");
        assert_eq!(response.headers()[header::X_FRAME_OPTIONS], "DENY");

        let refused = call(&state, "POST", PATH_HOST, "/mcp", &[("origin", "https://konae.es"), ("content-type", "application/json")], &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string()).await;
        assert_eq!(refused.status(), 403);
        let owner = call(&state, "POST", PATH_HOST, "/_konae/owner/login", &[("origin", "https://konae.es"), ("content-type", "application/json")], "{}").await;
        assert_eq!(owner.status(), 403);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// Only the runtime checks a Referer, and only for another app's path.
    #[tokio::test]
    async fn the_runtime_refuses_a_referer_from_another_apps_path() {
        let root = scratch("referer").await;
        let (state, first, second) = two_path_apps(&root).await;
        let call_with = |referer: &str| {
            let path = format!("/p/{}/_konae/mcp", first.id);
            let state = state.clone();
            let referer = referer.to_owned();
            async move {
                call(
                    &state,
                    "POST",
                    PATH_HOST,
                    &path,
                    &[("content-type", "application/json"), ("accept", "application/json, text/event-stream"), ("referer", referer.as_str())],
                    &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
                )
                .await
                .status()
            }
        };
        assert_eq!(call_with(&format!("https://konae.es/p/{}/", first.id)).await, 200);
        assert_eq!(call_with("https://konae.es/p/spring-menu/").await, 200);
        assert_eq!(call_with(&format!("https://konae.es/p/{}/", second.id)).await, 403);
        assert_eq!(call_with("https://konae.es/p/winter-menu/x").await, 403);
        assert_eq!(call_with("https://example.org/p/winter-menu/").await, 200, "a Referer on another host is not this rule's");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// The session cookie on a path address names its Deployment, and the
    /// login page on a path address redirects under the prefix.
    #[tokio::test]
    async fn the_session_cookie_on_a_path_address_names_its_deployment() {
        let root = scratch("cookie").await;
        let (state, first, second) = two_path_apps(&root).await;
        let signup = call(
            &state,
            "POST",
            PATH_HOST,
            &format!("/p/{}/_konae/auth/signup", first.id),
            &[("content-type", "application/json")],
            &json!({"email": "reader@example.com", "password": "correct horse battery"}).to_string(),
        )
        .await;
        assert_eq!(signup.status(), 200);
        let cookie = signup.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
        let name = format!("__Secure-konae_app_session_{}", first.id);
        assert!(cookie.starts_with(&format!("{name}=")), "{cookie}");
        assert!(cookie.contains("; Path=/;") && cookie.contains("; Secure"), "{cookie}");

        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, PATH_HOST.parse().unwrap());
        assert_eq!(crate::app_cookie_name(&state, &first.id, &headers), name);
        assert_eq!(crate::app_cookie_name(&state, &second.id, &headers), format!("__Secure-konae_app_session_{}", second.id));
        let session = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_owned();

        let signed_in = call(&state, "GET", PATH_HOST, &format!("/p/{}/_konae/login", first.id), &[("cookie", &format!("{name}={session}"))], "").await;
        assert_eq!(signed_in.status(), 303);
        assert_eq!(signed_in.headers()[header::LOCATION], format!("/p/{}/", first.id).as_str());
        let other_app = call(&state, "GET", PATH_HOST, &format!("/p/{}/_konae/login", second.id), &[("cookie", &format!("{name}={session}"))], "").await;
        assert_eq!(other_app.status(), 200, "the first app's session does not sign in the second");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// A generated hostname answers as it did, with its own cookie and policy,
    /// and a client-sent resolution header does not move it to another app.
    #[tokio::test]
    async fn a_generated_hostname_behaves_as_before() {
        let root = scratch("generated").await;
        let (state, first, second) = two_path_apps(&root).await;
        let hostname = first.hostname.clone();
        let host = format!("{hostname}:8080");
        let response = call(&state, "GET", &host, "/", &[(RESOLVED_HEADER, second.id.as_str())], "").await;
        assert_eq!(response.status(), 200);
        assert!(text(response).await.contains("shell"));
        let response = call(&state, "GET", &host, "/", &[], "").await;
        assert!(response.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().starts_with("default-src 'self'; script-src 'self';"));

        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        assert_eq!(crate::app_cookie_name(&state, &first.id, &headers), "__Host-konae_app_session");
        // A path address is not a path on a generated host.
        assert_ne!(call(&state, "GET", &host, "/p/spring-menu/", &[], "").await.status(), 200);
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
