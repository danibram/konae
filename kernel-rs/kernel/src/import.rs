
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::package::InitialPackage;
use super::schema_change::{self, Change, Mode};
use super::*;

/// What a backup holds besides the database file of a Data panel backup.
pub const HOLDS_DATA: &str = "data";
pub const HOLDS_SOURCES: &str = "sources";

const SPACE_MARGIN: u64 = 64 * 1024 * 1024;
const SOURCES_FILE: &str = "sources.json";

/// The schema the incoming database takes before it serves: `schema.sql`
/// and the actions that must prepare against it.
pub struct Target {
    pub sql: String,
    pub program: ActionProgram,
    pub schema_transforms: Option<String>,
}

impl Target {
    /// The release that comes with the data.
    pub fn release(input: &DeployInput) -> Result<Self> {
        Ok(Self {
            sql: input.project.files.get("schema.sql").cloned().unwrap_or_default(),
            program: serde_json::from_str(input.project.files.get("actions.json").context("actions.json is required.")?)?,
            schema_transforms: input.project.files.get("schema-transforms.json").cloned(),
        })
    }

    /// The `schema.sql` and `actions.json` in the app's `app/`, which the
    /// Kernel opens at its next start.
    pub fn sources(state: &AppState, id: &str) -> Result<Self> {
        let sources = layout::sources(&state.data_root, id);
        Ok(Self {
            sql: std::fs::read_to_string(sources.join("schema.sql")).unwrap_or_default(),
            program: serde_json::from_str(&std::fs::read_to_string(sources.join("actions.json")).context("app/actions.json is missing.")?)?,
            schema_transforms: read_optional_transform_document(&sources.join("schema-transforms.json"))?,
        })
    }

    /// The tables and indexes the live database has now, and the actions the app runs.
    pub fn live(state: &AppState, deployment: &Deployment) -> Result<Self> {
        let settings = Settings::read(&state.data_root, &deployment.id)?;
        Ok(Self {
            sql: live_schema(&layout::database(&state.data_root, &deployment.id))?,
            program: serde_json::from_str(&deployment.actions).context("The app's actions cannot be read.")?,
            schema_transforms: settings.released.and_then(|released| released.schema_transforms),
        })
    }
}

fn read_optional_transform_document(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(document) => Ok(Some(document)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// What an import did, or the plan it needs confirmed before it does anything.
pub enum Outcome {
    NeedsConfirmation(Change),
    Imported { backup: Value, change: Change },
}

struct Incoming {
    database: PathBuf,
    identity: Option<PathBuf>,
    files: Option<PathBuf>,
    usage: Option<PathBuf>,
}

impl Incoming {
    fn size(&self) -> u64 {
        [Some(&self.database), self.identity.as_ref(), self.files.as_ref(), self.usage.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum()
    }
}

/// The tables and indexes of the database at `path`, as SQL that creates
/// them, so the Kernel's schema change can bring another database to it.
pub fn live_schema(path: &Path) -> Result<String> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare(
        "SELECT sql FROM sqlite_master WHERE type IN ('table','index') AND sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND lower(name) NOT IN ('__konae_schema_state','__konae_schema_history') ORDER BY rowid",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut schema = String::new();
    for sql in rows {
        let sql = sql?;
        let created = ["CREATE TABLE", "CREATE UNIQUE INDEX", "CREATE INDEX"].iter().find_map(|keyword| {
            let head = sql.get(..keyword.len()).filter(|head| head.eq_ignore_ascii_case(keyword))?;
            let rest = &sql[head.len()..];
            Some(if rest.trim_start().get(..13).is_some_and(|word| word.eq_ignore_ascii_case("IF NOT EXISTS")) {
                sql.clone()
            } else {
                format!("{keyword} IF NOT EXISTS{rest}")
            })
        });
        schema.push_str(&created.with_context(|| format!("The live database holds a statement the Kernel cannot read back: {sql}"))?);
        schema.push_str(";\n");
    }
    Ok(schema)
}

fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn plan(state: &AppState, database: &Path, target: &Target) -> Result<Change> {
    let scratch = state.data_root.join("transfers").join(format!(".import-plan-{}.sqlite", Uuid::new_v4()));
    std::fs::create_dir_all(state.data_root.join("transfers"))?;
    let result = std::fs::copy(database, &scratch)
        .map_err(anyhow::Error::from)
        .and_then(|_| schema_change::apply_with_transforms(&scratch, &target.sql, &target.program, target.schema_transforms.as_deref(), Mode::Plan));
    remove_database(&scratch);
    result
}

/// Plans the import of a package into an existing app and, when its schema
/// change loses nothing or `confirmation` matches the plan, backs up what it
/// replaces and puts the package's data in place. `sources` says the caller
/// replaces the sources next, so the backup keeps them too. The caller has
/// checked the package (`package::verify`) and holds the project lock.
pub async fn import_data(
    state: &AppState,
    deployment: &Deployment,
    package: &InitialPackage,
    target: &Target,
    confirmation: Option<&str>,
    sources: bool,
) -> Result<Outcome> {
    let uploads = package::component_paths(state, &deployment.project_id, package)?;
    let find = |name: &str| uploads.iter().find(|(component, _)| component == name).map(|(_, path)| path.clone());
    let incoming = Incoming {
        database: find("database").context("The package has no database.")?,
        identity: find("identity"),
        files: find("files"),
        usage: find("usage"),
    };
    import_incoming(state, deployment, incoming, target, confirmation, sources).await
}

/// Imports the `data/` of the app folder `folder`, an export, into an
/// existing app, as `import_data` imports a package. The sources stay; the
/// database takes the schema of `target`. It checks each file as
/// `package::verify` checks an upload, and the caller holds the project lock.
pub async fn import_folder(state: &AppState, deployment: &Deployment, folder: &Path, target: &Target, confirmation: Option<&str>) -> Result<Outcome> {
    let data = folder.join("data");
    let database = data.join("database.sqlite");
    ensure!(database.is_file(), "{} holds no database.sqlite to import.", data.display());
    let stage = state.data_root.join("transfers").join(format!(".import-folder-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&stage).await?;
    let result = async {
        let present = |name: &str| Some(data.join(name)).filter(|path| path.is_file());
        let files = if data.join("files").is_dir() {
            let archive = stage.join("files.tar");
            package::folder_files_tar(&data.join("files"), &archive)?;
            Some(archive)
        } else {
            None
        };
        let incoming = Incoming { database, identity: present("identity.sqlite"), files, usage: present(usage::USAGE_DATABASE) };
        package::ensure_space(package::free_bytes(&state.data_root)?, incoming.size() * 2 + SPACE_MARGIN, "import this folder")?;
        let components = [("database", Some(&incoming.database)), ("identity", incoming.identity.as_ref()), ("files", incoming.files.as_ref()), ("usage", incoming.usage.as_ref())]
            .into_iter()
            .filter_map(|(name, path)| path.map(|path| (name.to_owned(), path.clone())))
            .collect::<Vec<_>>();
        let limits = state.files.limits;
        tokio::task::spawn_blocking(move || package::check_components(&components, &limits)).await??;
        import_incoming(state, deployment, incoming, target, confirmation, false).await
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    let change = result?;
    if let Some(modules) = crate::folder::decide_app_modules(&state.data_root, &deployment.id).await? {
        let mut deployments = state.deployments.write().await;
        if let Some(current) = deployments.values_mut().find(|current| current.id == deployment.id) {
            current.modules = modules;
        }
    }
    Ok(change)
}

async fn import_incoming(
    state: &AppState,
    deployment: &Deployment,
    incoming: Incoming,
    target: &Target,
    confirmation: Option<&str>,
    sources: bool,
) -> Result<Outcome> {
    let change = plan(state, &incoming.database, target)?;
    if change.confirmation().is_some_and(|expected| confirmation != Some(expected.as_str())) {
        return Ok(Outcome::NeedsConfirmation(change));
    }
    let _data = database_lock(state, &deployment.id).await;
    let mut holds = vec![HOLDS_DATA];
    if sources {
        holds.push(HOLDS_SOURCES);
    }
    let backup = backup_held(state, deployment, "Before import", &holds).await?;
    let applied = replace_held(state, deployment, incoming, Some((target, confirmation))).await;
    match applied {
        Ok(change) => Ok(Outcome::Imported { backup, change }),
        Err(error_value) => Err(error_value.context(format!("The import stopped. Backup {} holds the app as it was before.", backup["id"].as_str().unwrap_or_default()))),
    }
}

/// Backs up the app's saved sources alone, before a release from an import
/// replaces them. The caller holds the project lock.
pub async fn backup_sources(state: &AppState, deployment: &Deployment) -> Result<Value> {
    let _data = database_lock(state, &deployment.id).await;
    backup_held(state, deployment, "Before import", &[HOLDS_SOURCES]).await
}

async fn backup_held(state: &AppState, deployment: &Deployment, label: &str, holds: &[&str]) -> Result<Value> {
    let id = Uuid::new_v4().to_string();
    let backups = layout::backups(&state.data_root, &deployment.id);
    let folder = backups.join(&id);
    tokio::fs::create_dir_all(&folder).await?;
    let written = async {
        if holds.contains(&HOLDS_DATA) {
            let data = layout::data(&state.data_root, &deployment.id);
            let needed = package::folder_size(&data)? + SPACE_MARGIN;
            package::ensure_space(package::free_bytes(&state.data_root)?, needed, "back up this app")?;
            let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
            let blobs = file_store::BlockingFiles::new(state.files.store.clone(), &deployment.id);
            let target = folder.clone();
            let modules = deployment.modules.clone();
            tokio::task::spawn_blocking(move || package::snapshot(&data, &target, &blobs, &modules)).await??;
        }
        if holds.contains(&HOLDS_SOURCES) {
            let saved = sources::read(&state.data_root, &deployment.id).await?;
            tokio::fs::write(folder.join(SOURCES_FILE), serde_json::to_vec(&saved.unwrap_or(Value::Null))?).await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error_value) = written {
        let _ = tokio::fs::remove_dir_all(&folder).await;
        return Err(error_value.context("The backup before the import failed, so nothing changed"));
    }
    let size = folder_bytes(&folder)?;
    let record = DatabaseBackup {
        id: id.clone(),
        label: label.to_owned(),
        created_at: now_ms(),
        size_bytes: size,
        key: id.clone(),
        holds: holds.iter().map(|part| (*part).to_owned()).collect(),
    };
    write_atomic(&backups.join(format!("{id}.json")), &serde_json::to_vec(&record)?).await?;
    Ok(record.view())
}

fn folder_bytes(folder: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(folder)? {
        total += entry?.metadata()?.len();
    }
    Ok(total)
}

/// Brings back a backup this module wrote: its data and, when it holds
/// them, the saved sources. It first backs up the same parts, so a restore
/// can be undone too. Sessions and API keys stay as they are now. The
/// database keeps the schema it had; the next release brings it to the
/// sources. The caller holds the project and data locks.
pub async fn restore_held(state: &AppState, deployment: &Deployment, record: &DatabaseBackup) -> Result<Value> {
    let folder = layout::backups(&state.data_root, &deployment.id).join(&record.key);
    ensure!(tokio::fs::try_exists(&folder).await?, "Backup {} has no files.", record.id);
    let holds: Vec<&str> = record.holds.iter().map(String::as_str).collect();
    let saved = if holds.contains(&HOLDS_SOURCES) {
        Some(serde_json::from_slice::<Value>(&tokio::fs::read(folder.join(SOURCES_FILE)).await?)?)
    } else {
        None
    };
    let incoming = if holds.contains(&HOLDS_DATA) {
        let path = |name: &str| Some(folder.join(name)).filter(|path| path.exists());
        Some(Incoming {
            database: path("database.sqlite").context("The backup has no database.")?,
            identity: path("identity.sqlite"),
            files: path("files.tar"),
            usage: path(usage::USAGE_DATABASE),
        })
    } else {
        None
    };
    if let Some(incoming) = &incoming {
        package::ensure_space(package::free_bytes(&state.data_root)?, incoming.size() * 2 + SPACE_MARGIN, "restore this backup")?;
    }
    let before = backup_held(state, deployment, "Before restore", &holds).await?;
    if let Some(incoming) = incoming {
        replace_held(state, deployment, incoming, None).await?;
    }
    if let Some(saved) = saved.filter(|saved| !saved.is_null()) {
        sources::write(&state.data_root, &deployment.id, &saved).await?;
    }
    Ok(json!({"id": record.id, "holds": record.holds, "before": before}))
}

async fn replace_held(state: &AppState, deployment: &Deployment, incoming: Incoming, schema: Option<(&Target, Option<&str>)>) -> Result<Change> {
    let stage = state.data_root.join("transfers").join(format!(".import-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&stage).await?;
    let result = async {
        let live = layout::data(&state.data_root, &deployment.id);
        let database = stage.join("database.sqlite");
        std::fs::copy(&incoming.database, &database)?;
        let change = match schema {
            Some((target, confirmation)) => schema_change::apply_with_transforms(&database, &target.sql, &target.program, target.schema_transforms.as_deref(), Mode::Apply(confirmation))?,
            None => Change::default(),
        };
        let identity = match &incoming.identity {
            Some(source) => {
                let staged = stage.join("identity.sqlite");
                std::fs::copy(source, &staged)?;
                AuthService::open(&staged).map_err(|error_value| anyhow::anyhow!(error_value.to_string()))?;
                Some(staged)
            }
            None => None,
        };
        let usage = match &incoming.usage {
            Some(source) => {
                let staged = stage.join(usage::USAGE_DATABASE);
                std::fs::copy(source, &staged)?;
                Some(staged)
            }
            None => None,
        };
        usage::flush();
        let _identity = database_lock(state, &format!("identity:{}", deployment.id)).await;
        if let Some(staged) = &identity {
            carry_access(staged, &live.join("identity.sqlite"))?;
        }
        let index = match &incoming.files {
            Some(archive) => {
                let accounts = Connection::open(identity.as_ref().map_or_else(|| live.join("identity.sqlite"), Clone::clone))?;
                let staged = stage.join("files");
                let blobs = file_store::BlockingFiles::new(state.files.store.clone(), &deployment.id);
                let (archive, limits, staged_files) = (archive.clone(), state.files.limits, staged.clone());
                tokio::task::spawn_blocking(move || package::unpack_into(&archive, &accounts, &staged_files, &blobs, &limits)).await??;
                Some(staged.join("index.json"))
            }
            None => None,
        };
        replace_database_file(&database, &live.join("database.sqlite"))?;
        if let Some(staged) = identity {
            replace_database_file(&staged, &live.join("identity.sqlite"))?;
        }
        let live_usage = live.join(usage::USAGE_DATABASE);
        match usage {
            Some(staged) => replace_database_file(&staged, &live_usage)?,
            None => remove_database(&live_usage),
        }
        if let Some(index) = index {
            let files = layout::files(&state.data_root, &deployment.id);
            tokio::fs::create_dir_all(&files).await?;
            tokio::fs::rename(&index, files.join("index.json")).await?;
            forget_unlisted(state, deployment).await?;
        }
        Ok::<_, anyhow::Error>(change)
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    result
}

fn carry_access(incoming: &Path, live: &Path) -> Result<()> {
    let connection = Connection::open(incoming)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON;")?;
    for table in ["app_sessions", "account_challenges", "entry_links"] {
        connection.execute(&format!("DELETE FROM \"{table}\""), [])?;
    }
    super::invitations::forget_tokens(&connection)?;
    connection.execute_batch("DROP TABLE IF EXISTS main.api_keys")?;
    super::http::keys_table(&connection)?;
    if !live.exists() {
        return Ok(());
    }
    let live_has_keys = super::table_exists(&Connection::open(live)?, "api_keys")?;
    connection.execute("ATTACH DATABASE ?1 AS live", [live.to_string_lossy().as_ref()])?;
    connection.execute(
        "INSERT INTO main.app_sessions (token_hash, user_id, expires_at, entered_by)
         SELECT token_hash, user_id, expires_at, entered_by FROM live.app_sessions
         WHERE user_id IN (SELECT id FROM main.app_users WHERE state = 'active')",
        [],
    )?;
    if live_has_keys {
        let live_keys = {
            let mut statement = connection.prepare("SELECT name FROM pragma_table_info('api_keys', 'live')")?;
            let names = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            names
        };
        let shared = super::http::API_KEY_COLUMNS
            .iter()
            .filter(|name| live_keys.iter().any(|live| live.eq_ignore_ascii_case(name)))
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        connection.execute(&format!("INSERT INTO main.api_keys ({shared}) SELECT {shared} FROM live.api_keys"), [])?;
    }
    connection.execute("DETACH DATABASE live", [])?;
    Ok(())
}

async fn forget_unlisted(state: &AppState, deployment: &Deployment) -> Result<()> {
    let listed: std::collections::HashSet<String> = file_store::indexed_files(&state.data_root, &deployment.id)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    for id in state.files.store.list(&deployment.id).await? {
        if !listed.contains(&id) {
            state.files.store.delete(&deployment.id, &id).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn importing_a_database_keeps_its_transformation_journal_and_rows() {
        let root = std::env::temp_dir().join(format!("konae-import-transform-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::for_test(&root);
        let incoming = root.join("incoming.sqlite");
        initialize_database(&incoming).unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);";
        let document = serde_json::json!({
            "version": 1,
            "transitions": [{
                "id": "baseline",
                "predecessor": null,
                "before": format!("sha256:{}", "0".repeat(64)),
                "after": crate::schema_transform::fingerprint_schema(schema).unwrap(),
                "maxRows": 10,
                "statements": ["CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)"]
            }]
        }).to_string();
        let definition = crate::schema_transform::parse(&document).unwrap();
        let db = Connection::open(&incoming).unwrap();
        let tx = db.unchecked_transaction().unwrap();
        crate::schema_transform::initialize_fresh_baseline(&tx, &definition, schema).unwrap();
        tx.commit().unwrap();
        db.execute("INSERT INTO notes(body) VALUES('kept')", []).unwrap();
        drop(db);
        let target = Target {
            sql: schema.into(),
            program: serde_json::from_str(r#"{"version":1,"actions":{}}"#).unwrap(),
            schema_transforms: Some(document),
        };
        let deployment: Deployment = serde_json::from_value(serde_json::json!({
            "id": "notes_app", "projectId": "notes_app", "projectName": "Notes", "slug": "notes",
            "url": "http://notes.localhost", "createdAt": 1, "releasedAt": 1, "manifest": {},
            "hostname": "notes.localhost", "actions": "{}", "artifactVersion": "v1",
            "signupPolicy": {"mode":"open", "allowlist":[]}, "signInMethods": ["password"]
        })).unwrap();
        let files = layout::data(&root, &deployment.id);
        std::fs::create_dir_all(&files).unwrap();
        let incoming_files = Incoming { database: incoming, identity: None, files: None, usage: None };
        replace_held(&state, &deployment, incoming_files, Some((&target, None))).await.unwrap();
        let live = Connection::open(layout::database(&root, &deployment.id)).unwrap();
        let body: String = live.query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(body, "kept");
        let history = crate::schema_transform::load_history(&live).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].id, "baseline");
        assert!(history.entries[0].baseline_skipped);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn importing_folder_files_turns_on_the_files_module_and_returns_that_state() {
        let root = std::env::temp_dir().join(format!("konae-import-files-module-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut state = AppState::for_test(&root);
        let id = "import_files";
        let mut deployment: Deployment = serde_json::from_value(serde_json::json!({
            "id":id,"projectId":id,"projectName":"Import files","slug":"import-files","url":"http://import-files.localhost",
            "createdAt":1,"releasedAt":1,"manifest":{"visibility":"public"},"hostname":"import-files.localhost","actions":"{}","artifactVersion":"v1",
            "signupPolicy":{"mode":"open","allowlist":[]},"signInMethods":["password"]
        })).unwrap();
        deployment.modules = crate::modules::defaults();
        deployment.modules.get_mut("files").unwrap()["on"] = serde_json::json!(false);
        state.deployments = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([(deployment.hostname.clone(), deployment.clone())])));
        crate::Settings::of(&deployment).write(&root).await.unwrap();
        let live = layout::database(&root, id);
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        initialize_database(&live).unwrap();

        let folder = root.join("incoming");
        let data = folder.join("data");
        let files = data.join("files");
        std::fs::create_dir_all(&files).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/module-files-0.9.0");
        std::fs::copy(fixture.join("files-index.json"), files.join("index.json")).unwrap();
        std::fs::copy(fixture.join("files-018f2b2c-4a10-7a01-8000-000000000001.blob"), files.join("018f2b2c-4a10-7a01-8000-000000000001.blob")).unwrap();
        let incoming_db = data.join("database.sqlite");
        std::fs::create_dir_all(&data).unwrap();
        initialize_database(&incoming_db).unwrap();
        let target = Target { sql:String::new(), program:serde_json::from_str(r#"{"version":1,"actions":{}}"#).unwrap(), schema_transforms:None };
        import_folder(&state, &deployment, &folder, &target, None).await.unwrap();
        let current = state.deployments.read().await.values().next().unwrap().clone();
        assert_eq!(current.modules["files"]["on"], true);
        assert_eq!(crate::Settings::read(&root, id).unwrap().modules["files"]["on"], true);
        assert!(layout::files(&root, id).join("018f2b2c-4a10-7a01-8000-000000000001.blob").exists());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn the_live_schema_runs_twice_and_creates_the_same_tables() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("live.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, done INTEGER DEFAULT 0);
             CREATE TABLE IF NOT EXISTS tags(name TEXT);
             CREATE INDEX notes_body ON notes(body);
             create unique index tags_name on tags(name);
             CREATE TABLE __konae_schema_state(format_version INTEGER NOT NULL);
             CREATE TABLE __konae_schema_history(id TEXT);",
        )
        .unwrap();
        let schema = live_schema(&path).unwrap();
        assert!(schema.starts_with("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY"), "{schema}");
        assert!(schema.contains("CREATE UNIQUE INDEX IF NOT EXISTS tags_name on tags(name);"), "{schema}");
        assert!(!schema.contains("__konae_schema_"), "internal journal tables belong in the database file, not schema.sql: {schema}");
        let copy = Connection::open_in_memory().unwrap();
        copy.execute_batch(&schema).unwrap();
        copy.execute_batch(&schema).unwrap();
        let tables: i64 = copy.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0)).unwrap();
        assert_eq!(tables, 4);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_incoming_identity_keeps_the_live_sessions_and_keys() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let live = AuthService::open(root.join("live.sqlite")).unwrap();
        let ana = live.signup("ana@example.test", "ana's password").unwrap();
        let bo = live.signup("bo@example.test", "bo's password").unwrap();
        {
            let db = Connection::open(root.join("live.sqlite")).unwrap();
            super::super::http::keys_table(&db).unwrap();
            db.execute_batch("INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES('k1', 'h1', 'live key', '[]', '[]', 1);").unwrap();
        }
        let incoming_path = root.join("incoming.sqlite");
        let incoming = AuthService::open(&incoming_path).unwrap();
        incoming
            .transaction(|tx| {
                tx.execute("INSERT INTO app_users (id, email, password_hash, created_at) VALUES (?1, 'ana@example.test', 'x', 0)", [&ana.user.user_id])?;
                tx.execute_batch("CREATE TABLE api_keys(id TEXT PRIMARY KEY, name TEXT, revoked_at INTEGER); INSERT INTO api_keys VALUES('k1','live key',1);")?;
                Ok(())
            })
            .unwrap();
        carry_access(&incoming_path, &root.join("live.sqlite")).unwrap();
        let after = AuthService::open(&incoming_path).unwrap();
        assert_eq!(after.authenticate(Some(&ana.token)).unwrap().unwrap().user_id, ana.user.user_id);
        assert!(after.authenticate(Some(&bo.token)).unwrap().is_none(), "Bo is not in the new data");
        let revoked: Option<i64> = after.read(|db| db.query_row("SELECT revoked_at FROM api_keys WHERE id='k1'", [], |row| row.get(0)).map_err(AuthError::Storage)).unwrap();
        assert_eq!(revoked, None, "the live key keeps working");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_incoming_identity_without_keys_takes_the_live_ones() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        AuthService::open(root.join("live.sqlite")).unwrap();
        let live = Connection::open(root.join("live.sqlite")).unwrap();
        super::super::http::keys_table(&live).unwrap();
        live.execute_batch("INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES('k1', 'h1', 'live key', '[]', '[]', 1);").unwrap();
        drop(live);
        let incoming_path = root.join("incoming.sqlite");
        AuthService::open(&incoming_path).unwrap();
        carry_access(&incoming_path, &root.join("live.sqlite")).unwrap();
        let name: String = Connection::open(&incoming_path).unwrap().query_row("SELECT name FROM api_keys WHERE id='k1'", [], |row| row.get(0)).unwrap();
        assert_eq!(name, "live key");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn carry_access_never_runs_sql_text_read_from_the_live_file() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let victim = root.join("victim.sqlite");
        Connection::open(&victim).unwrap().execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES('kept');").unwrap();
        let live_path = root.join("live.sqlite");
        AuthService::open(&live_path).unwrap();
        {
            let db = Connection::open(&live_path).unwrap();
            super::super::http::keys_table(&db).unwrap();
            db.execute_batch("INSERT INTO api_keys(id, token_hash, name, actions, files, created_at) VALUES('k1', 'h1', 'live key', '[]', '[]', 1);").unwrap();
            let evil = format!("; ATTACH DATABASE '{}' AS o; UPDATE o.t SET v = 'changed'", victim.display());
            db.execute_batch("PRAGMA writable_schema=ON;").unwrap();
            db.execute("UPDATE sqlite_master SET sql = sql || ?1 WHERE type='table' AND name='api_keys'", [evil]).unwrap();
            db.execute_batch("PRAGMA writable_schema=OFF;").unwrap();
        }
        let incoming_path = root.join("incoming.sqlite");
        AuthService::open(&incoming_path).unwrap();
        carry_access(&incoming_path, &live_path).unwrap();
        let value: String = Connection::open(&victim).unwrap().query_row("SELECT v FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(value, "kept", "text from the live file ran as SQL");
        let incoming = Connection::open(&incoming_path).unwrap();
        let columns: Vec<String> = incoming
            .prepare("SELECT name FROM pragma_table_info('api_keys') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(columns, ["id", "token_hash", "name", "actions", "files", "created_at", "revoked_at", "revoked_reason"]);
        let name: String = incoming.query_row("SELECT name FROM api_keys WHERE id='k1'", [], |row| row.get(0)).unwrap();
        assert_eq!(name, "live key");
        let _ = std::fs::remove_dir_all(root);
    }

    const KEY_ROW: &str = "(id, token_hash, name, actions, files, created_at, revoked_at) VALUES";

    fn keys_of(path: &Path) -> Vec<(String, Option<i64>)> {
        Connection::open(path)
            .unwrap()
            .prepare("SELECT id, revoked_at FROM api_keys ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_key_table_in_the_incoming_file_never_survives_when_the_live_file_has_none() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        AuthService::open(root.join("live.sqlite")).unwrap();
        let incoming_path = root.join("incoming.sqlite");
        AuthService::open(&incoming_path).unwrap();
        let incoming = Connection::open(&incoming_path).unwrap();
        super::super::http::keys_table(&incoming).unwrap();
        incoming.execute_batch(&format!("INSERT INTO api_keys {KEY_ROW} ('uploader', 'uploader-hash', 'Uploader', '[]', '[]', 1, NULL);")).unwrap();
        drop(incoming);
        carry_access(&incoming_path, &root.join("live.sqlite")).unwrap();
        assert_eq!(keys_of(&incoming_path), Vec::<(String, Option<i64>)>::new(), "the uploader's key does not reach the importer's app");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_revoked_at_column_spelled_in_capitals_does_not_bring_revoked_keys_back() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let live_path = root.join("live.sqlite");
        AuthService::open(&live_path).unwrap();
        let live = Connection::open(&live_path).unwrap();
        super::super::http::keys_table(&live).unwrap();
        live.execute_batch(&format!(
            "INSERT INTO api_keys {KEY_ROW} ('k-live', 'h1', 'Live', '[]', '[]', 1, NULL), ('k-revoked', 'h2', 'Revoked', '[]', '[]', 1, 5);"
        ))
        .unwrap();
        drop(live);
        let incoming_path = root.join("incoming.sqlite");
        AuthService::open(&incoming_path).unwrap();
        Connection::open(&incoming_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE api_keys(id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                   actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, REVOKED_AT INTEGER, revoked_reason TEXT)",
            )
            .unwrap();
        carry_access(&incoming_path, &live_path).unwrap();
        assert_eq!(keys_of(&incoming_path), [("k-live".to_owned(), None), ("k-revoked".to_owned(), Some(5))]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_live_key_table_spelled_in_capitals_is_carried() {
        let root = std::env::temp_dir().join(format!("konae-import-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let live_path = root.join("live.sqlite");
        AuthService::open(&live_path).unwrap();
        Connection::open(&live_path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE API_KEYS(id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                   actions TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER, revoked_reason TEXT);
                 INSERT INTO api_keys {KEY_ROW} ('k1', 'h1', 'Live', '[]', '[]', 1, NULL);"
            ))
            .unwrap();
        let incoming_path = root.join("incoming.sqlite");
        AuthService::open(&incoming_path).unwrap();
        carry_access(&incoming_path, &live_path).unwrap();
        assert_eq!(keys_of(&incoming_path), [("k1".to_owned(), None)]);
        let _ = std::fs::remove_dir_all(root);
    }
}
