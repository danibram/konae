//! Moving a Deployment to another generated hostname.
//!
//! The first publish names a Deployment `<slug>.<suffix>`, and every later
//! release keeps that name. `hostname_set` moves it to another free label
//! under the same suffix. The names the Deployment held before stay with it in
//! `apps/<id>/.konae/hostname.json`: a request on one of them gets a permanent
//! redirect to the same path on the current name, so links in email already
//! sent and bookmarks keep working, and no other app can take the name and
//! receive them.
//!
//! The new name is a new origin. Sessions are host-only, so app users sign in
//! again there, and an app with Google sign-in needs the new callback in its
//! Google client. A custom domain and a path address are untouched.

use super::refusal::{refuse, Refusal};
use super::*;
use axum::middleware::Next;
use std::sync::OnceLock;

pub const RECORD_FILE: &str = "hostname.json";

/// The generated names a Deployment held before its current one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub previous: Vec<String>,
}

/// The hostname `raw` names under `suffix`, in its stored form, or the
/// refusal. `raw` is one label, such as `notes`, or the whole name.
pub fn check_name(raw: &str, suffix: &str) -> std::result::Result<String, Refusal> {
    let suffix = suffix.trim_start_matches('.').to_ascii_lowercase();
    let trimmed = raw.trim();
    let name = trimmed.strip_suffix('.').unwrap_or(trimmed).to_lowercase();
    let label = name.strip_suffix(&format!(".{suffix}")).unwrap_or(&name);
    if label.is_empty() {
        return Err(refuse("hostname.missing", format!("Name the new hostname, such as notes or notes.{suffix}.")));
    }
    if label.contains('.') {
        return Err(refuse("hostname.outside-suffix", format!(
            "{name} is not one name under this installation's app suffix {suffix}. Name one label, such as notes, or add a hostname outside the suffix with domain_set."
        )));
    }
    let valid = (1..=63).contains(&label.len())
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-');
    if !valid {
        return Err(refuse("hostname.name-invalid", format!(
            "{label} is not a valid name. Use letters, digits and hyphens, 1 to 63 characters, that do not start or end with a hyphen."
        )));
    }
    Ok(format!("{label}.{suffix}"))
}

/// Whether `hostname` is free for this Deployment: not the installation's
/// own host, and not a name another app holds now or held before.
pub fn check_free(hostname: &str, installation_hosts: &[String], taken: bool) -> std::result::Result<(), Refusal> {
    if installation_hosts.iter().any(|host| host == hostname) {
        return Err(refuse("hostname.installation-host", format!("{hostname} is this installation's own host. Choose another name.")));
    }
    if taken {
        return Err(refuse("hostname.taken", format!("{hostname} is another app's address on this installation. Choose another name.")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Records and the index of previous names

/// Previous name to the Deployment that held it, per data directory, as
/// `domain.rs` keeps its aliases.
type Index = HashMap<String, String>;
static INDEXES: OnceLock<std::sync::Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();

fn with_index<T>(state: &AppState, f: impl FnOnce(&mut Index) -> T) -> T {
    let mut indexes = INDEXES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = indexes.entry(state.data_root.clone()).or_insert_with(|| load_index(&state.data_root));
    f(index)
}

/// Reads every `hostname.json` on the volume. A copy restored into this
/// directory brings its files along.
fn load_index(root: &Path) -> Index {
    let mut index = Index::new();
    let Ok(entries) = std::fs::read_dir(crate::layout::apps(root)) else { return index };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
        for name in read_at(root, &id).previous {
            index.entry(name).or_insert_with(|| id.clone());
        }
    }
    index
}

fn read_at(root: &Path, id: &str) -> Record {
    std::fs::read(crate::layout::private(root, id).join(RECORD_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn read_record(state: &AppState, id: &str) -> Record {
    read_at(&state.data_root, id)
}

async fn write_record(state: &AppState, id: &str, record: &Record) -> Result<()> {
    let path = crate::layout::private(&state.data_root, id).join(RECORD_FILE);
    let before = read_record(state, id);
    if record.previous.is_empty() {
        match tokio::fs::remove_file(&path).await {
            Err(error_value) if error_value.kind() != std::io::ErrorKind::NotFound => return Err(error_value.into()),
            _ => {}
        }
    } else {
        tokio::fs::create_dir_all(crate::layout::private(&state.data_root, id)).await?;
        write_atomic(&path, &serde_json::to_vec_pretty(record)?).await?;
    }
    with_index(state, |index| {
        for name in &before.previous {
            if index.get(name).is_some_and(|holder| holder == id) {
                index.remove(name);
            }
        }
        for name in &record.previous {
            index.insert(name.clone(), id.to_owned());
        }
    });
    Ok(())
}

/// The Deployment that held `host` before its current name.
pub fn held_by(state: &AppState, host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    with_index(state, |index| index.get(&host).cloned())
}

/// Frees a deleted Deployment's previous names.
pub async fn forget(state: &AppState, id: &str) -> Result<()> {
    let _names = database_lock(state, "hostnames").await;
    write_record(state, id, &Record::default()).await
}

// ---------------------------------------------------------------------------
// The move

pub async fn admin_call(state: &AppState, deployment: &Deployment, name: &str, a: &Value) -> Result<Value> {
    match name {
        "hostname_set" => set(state, &deployment.id, a["name"].as_str().unwrap_or_default()).await,
        _ => anyhow::bail!("Unknown tool."),
    }
}

fn view(state: &AppState, deployment: &Deployment, changed: bool) -> Value {
    json!({
        "deploymentId": deployment.id,
        "hostname": deployment.hostname,
        "url": deployment.url,
        "changed": changed,
        "previousHostnames": read_record(state, &deployment.id).previous,
        "googleCallback": format!("{}{}", deployment.url, google::CALLBACK_PATH),
    })
}

/// Moves the Deployment `id` to the generated hostname `raw` names.
pub async fn set(state: &AppState, id: &str, raw: &str) -> Result<Value> {
    let hostname = check_name(raw, &state.host_suffix)?;
    let found = deployment_by_id(state, id).await.ok_or_else(|| anyhow::anyhow!("The app {id} is not published."))?;
    // Releases and every settings change hold the project lock, so none of
    // them can write the old name back over this one.
    let _project = database_lock(state, &format!("project:{}", found.project_id)).await;
    let _names = database_lock(state, "hostnames").await;
    let current = deployment_by_id(state, id).await.ok_or_else(|| anyhow::anyhow!("The app {id} is not published."))?;
    if current.hostname == hostname {
        return Ok(view(state, &current, false));
    }
    let taken_now = state.deployments.read().await.get(&hostname).is_some_and(|holder| holder.id != current.id);
    let held_before = held_by(state, &hostname).is_some_and(|holder| holder != current.id);
    check_free(&hostname, &domain::Rules::of(state).installation_hosts, taken_now || held_before || domain::claimed(state, &hostname))?;

    let mut record = read_record(state, &current.id);
    record.previous.retain(|name| name != &hostname && name != &current.hostname);
    record.previous.push(current.hostname.clone());
    write_record(state, &current.id, &record).await?;

    let mut updated = current.clone();
    updated.url = deployment_url(&state.public_origin, &hostname);
    updated.hostname = hostname;
    persist_deployment(state, &updated).await?;
    {
        let mut deployments = state.deployments.write().await;
        deployments.remove(&current.hostname);
        deployments.insert(updated.hostname.clone(), updated.clone());
    }
    Ok(view(state, &updated, true))
}

// ---------------------------------------------------------------------------
// The redirect

fn request_host(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host.split(':').next()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Answers a request on a name a Deployment held before with a permanent
/// redirect to the same path and query on its current name. A 308 keeps the
/// method and body, and the browser keeps the fragment, so an invitation
/// link sent before the move still opens.
pub async fn redirect(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(host) = request_host(request.headers()) else { return next.run(request).await };
    if state.deployments.read().await.contains_key(&host) {
        return next.run(request).await;
    }
    let Some(id) = held_by(&state, &host) else { return next.run(request).await };
    let Some(deployment) = deployment_by_id(&state, &id).await else { return next.run(request).await };
    let target = request.uri().path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let location = format!("{}{target}", deployment.url);
    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(root: &Path) -> AppState {
        AppState {
            public_origin: "https://konae.example.com".into(),
            host_suffix: "apps.example.com".into(),
            ..AppState::for_test(root)
        }
    }

    async fn publish(state: &AppState, project: &str) -> Deployment {
        let body = json!({
            "project": {"id": project, "name": project, "slug": project, "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>"}}
        });
        assert_eq!(deploy(state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        state.deployments.read().await.values().find(|d| d.project_id == project).cloned().unwrap()
    }

    fn host(name: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(name).unwrap());
        headers
    }

    #[test]
    fn names_are_checked_by_the_cases() {
        let file: Value = serde_json::from_str(include_str!("hostname-cases.json")).unwrap();
        let suffix = file["context"]["suffix"].as_str().unwrap();
        let hosts: Vec<String> = serde_json::from_value(file["context"]["installationHosts"].clone()).unwrap();
        let cases = file["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let outcome = check_name(case["name"].as_str().unwrap(), suffix)
                .and_then(|hostname| check_free(&hostname, &hosts, case["taken"].as_bool().unwrap_or(false)).map(|()| hostname));
            if case["error"].is_null() {
                assert_eq!(outcome, Ok(case["hostnameOut"].as_str().unwrap().to_owned()), "{}", case["case"]);
            } else {
                crate::refusal::assert_case(case, outcome);
            }
        }
    }

    /// The app answers on the new name, its old name redirects there and
    /// stays its own, a republish keeps the new name, and the record
    /// survives a restart.
    #[tokio::test]
    async fn a_moved_app_answers_on_its_new_name_and_its_old_name_redirects() {
        let root = std::env::temp_dir().join(format!("konae-hostname-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(crate::layout::apps(&root)).await.unwrap();
        let state = test_state(&root);
        let notes = publish(&state, "notes").await;
        let other = publish(&state, "other").await;
        assert_eq!(notes.hostname, "notes.apps.example.com");

        // Another app's current name, and the installation's own host, are refused.
        let taken = set(&state, &notes.id, "other").await.unwrap_err();
        assert_eq!(crate::refusal::of(&taken).unwrap().code, "hostname.taken");
        assert_eq!(taken.to_string(), "other.apps.example.com is another app's address on this installation. Choose another name.");

        let moved = set(&state, &notes.id, "journal").await.unwrap();
        assert_eq!(moved["hostname"], "journal.apps.example.com");
        assert_eq!(moved["url"], "https://journal.apps.example.com");
        assert_eq!(moved["changed"], true);
        assert_eq!(moved["previousHostnames"], json!(["notes.apps.example.com"]));
        assert_eq!(moved["googleCallback"], "https://journal.apps.example.com/_konae/auth/google/callback");
        assert_eq!(deployment_for(&state, &host("journal.apps.example.com")).await.map(|d| d.id), Some(notes.id.clone()));
        assert!(deployment_for(&state, &host("notes.apps.example.com")).await.is_none());
        assert_eq!(domain::app_url(&state, &deployment_by_id(&state, &notes.id).await.unwrap()), "https://journal.apps.example.com");

        // Same name again changes nothing.
        assert_eq!(set(&state, &notes.id, "journal.apps.example.com").await.unwrap()["changed"], false);

        // The old name stays with this app: another app cannot take it, nor can a first publish.
        let held = set(&state, &other.id, "notes").await.unwrap_err();
        assert_eq!(crate::refusal::of(&held).unwrap().code, "hostname.taken");
        let body = json!({
            "project": {"id": "again", "name": "again", "slug": "notes", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>shell</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        let again = deployment_by_id(&state, "again").await.unwrap();
        assert!(again.hostname.starts_with("notes-"), "{}", again.hostname);

        // The old name redirects with path, query and method kept.
        let app = stack(state.clone(), Router::new());
        let request = Request::builder()
            .method("POST")
            .uri("/_konae/invitation?x=1")
            .header(header::HOST, "notes.apps.example.com")
            .body(Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(response.headers()[header::LOCATION], "https://journal.apps.example.com/_konae/invitation?x=1");

        // A republish keeps the new name.
        let body = json!({
            "project": {"id": "notes", "name": "notes", "slug": "notes", "manifest": {},
                "files": {"actions.json": "{\"version\":1,\"actions\":{}}", "schema.sql": ""}},
            "visibility": "public",
            "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>two</p>"}}
        });
        assert_eq!(deploy(&state, body.to_string().as_bytes()).await.status(), StatusCode::OK);
        assert_eq!(deployment_by_id(&state, &notes.id).await.unwrap().hostname, "journal.apps.example.com");

        // The record and the name come back after a restart.
        let reloaded = load_deployments(&root, &state.public_origin).await.unwrap();
        assert_eq!(reloaded.get("journal.apps.example.com").map(|d| d.id.clone()), Some(notes.id.clone()));
        assert_eq!(load_index(&root).get("notes.apps.example.com"), Some(&notes.id));

        // Moving back to the old name takes it off the redirect list.
        let back = set(&state, &notes.id, "notes").await.unwrap();
        assert_eq!(back["previousHostnames"], json!(["journal.apps.example.com"]));
        assert_eq!(deployment_for(&state, &host("notes.apps.example.com")).await.map(|d| d.id), Some(notes.id.clone()));

        // Deleting the app frees its previous names.
        forget(&state, &notes.id).await.unwrap();
        assert_eq!(held_by(&state, "journal.apps.example.com"), None);
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
