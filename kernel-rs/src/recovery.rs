//! Recovery after a restore: the revocation record kept off the volume.
//!
//! A copy is taken every hour, so a restore can bring back an account that
//! was suspended, or an API key that was revoked, after the copy. Each such
//! change is written to every copy target as its own small file under
//! `revocations/` before the tool reports it done. `restore` reads that
//! record from the same place it reads the copy and applies it before the
//! Kernel serves. When the record cannot be read, the restored installation
//! starts in maintenance: every app serves its maintenance page and
//! administrative changes are refused, until `konae-kernel reconcile`
//! applies the record or the operator reopens without it.
//!
//! The first copy writes `revocations/started.json`, so a target without it
//! counts as unreadable, not as empty.

use std::sync::{LazyLock, RwLock as SyncRwLock};

use anyhow::{Context, bail};
use aws_sdk_s3::primitives::ByteStream;
use rusqlite::params;

use super::copies::Target;
use super::*;

pub(crate) const LEDGER_DIR: &str = "revocations";
pub(crate) const LEDGER_START: &str = "started.json";
/// In the data directory: the restore could not apply the record.
pub(crate) const HOLD_FILE: &str = "recovery-hold.json";

const RECONCILE_USAGE: &str = "Usage: konae-kernel reconcile --from <copy directory | s3://bucket/prefix> | --without-record";

/// One suspension or revocation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) at: u64,
    /// `app-user`, `creator` or `api-key`.
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployment_id: Option<String>,
    pub(crate) subject: String,
    /// `suspended` or `revoked`.
    pub(crate) state: String,
}

impl Entry {
    pub(crate) fn new(kind: &str, deployment_id: Option<&str>, subject: &str, state: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            at: now_ms(),
            kind: kind.into(),
            deployment_id: deployment_id.map(str::to_owned),
            subject: subject.into(),
            state: state.into(),
        }
    }

    fn file_name(&self) -> String {
        format!("{:013}-{}.json", self.at, self.id)
    }
}

// ---------------------------------------------------------------------------
// The installation hold

static HOLDS: LazyLock<SyncRwLock<HashMap<PathBuf, Option<String>>>> = LazyLock::new(Default::default);

/// The message every paused request gets while a restore is unreconciled.
pub(crate) fn hold(data_root: &Path) -> Option<String> {
    let path = data_root.join(HOLD_FILE);
    if let Some(found) = HOLDS.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&path) {
        return found.clone();
    }
    let held = match std::fs::read(&path) {
        Ok(bytes) => {
            let reason = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["reason"].as_str().map(str::to_owned)).unwrap_or_default();
            Some(format!(
                "This installation was restored from a copy and stays in maintenance until its record of suspensions and revocations is applied. {reason} The operator runs konae-kernel reconcile."
            ))
        }
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some("This installation is in maintenance: its recovery state cannot be read.".into()),
    };
    HOLDS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path, held.clone());
    held
}

pub(crate) fn set_hold(data_root: &Path, reason: &str) -> Result<()> {
    std::fs::write(data_root.join(HOLD_FILE), serde_json::to_vec_pretty(&json!({"reason": reason, "at": now_ms()}))?)?;
    HOLDS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&data_root.join(HOLD_FILE));
    Ok(())
}

fn clear_hold(data_root: &Path) -> Result<()> {
    match std::fs::remove_file(data_root.join(HOLD_FILE)) {
        Ok(()) => {}
        Err(error_value) if error_value.kind() == std::io::ErrorKind::NotFound => {}
        Err(error_value) => return Err(error_value.into()),
    }
    HOLDS.write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&data_root.join(HOLD_FILE));
    Ok(())
}

// ---------------------------------------------------------------------------
// Writing the record

/// Writes `entry` to every copy target. With no target configured there is
/// no copy to restore and nothing to record.
pub(crate) async fn write(targets: &[Target], s3: Option<&S3Client>, bucket: &str, entry: &Entry) -> Result<()> {
    let bytes = serde_json::to_vec(entry)?;
    for target in targets {
        match target {
            Target::Directory(directory) => {
                let (directory, name, bytes) = (directory.join(LEDGER_DIR), entry.file_name(), bytes.clone());
                tokio::task::spawn_blocking(move || write_file(&directory, &name, &bytes)).await??;
            }
            Target::S3 { prefix } => {
                let s3 = s3.context("S3 is not configured.")?;
                s3.put_object()
                    .bucket(bucket)
                    .key(format!("{prefix}{LEDGER_DIR}/{}", entry.file_name()))
                    .body(ByteStream::from(bytes.clone()))
                    .send()
                    .await?;
            }
        }
    }
    Ok(())
}

fn write_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{name}.tmp"));
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, directory.join(name))?;
    Ok(())
}

/// Records a change made by a running Kernel. The change already holds on
/// this volume; an error means it is not yet safe from a restore, and the
/// caller reports that instead of success.
pub(crate) async fn record(state: &AppState, entry: Entry) -> Result<()> {
    let targets = state.copies.ledger_targets();
    if targets.is_empty() {
        return Ok(());
    }
    write(targets, state.s3.as_ref(), &state.data_bucket, &entry).await.map_err(|error_value| {
        anyhow::anyhow!(
            "The change holds on this server, but it could not be recorded at the copy target, so a restore could undo it: {error_value:#}. Repeat the same request to record it."
        )
    })
}

/// Records a change made by an operator command, which runs without a
/// Kernel. Reads the copy settings from the environment.
pub(crate) fn record_offline(data_root: &Path, entry: Entry) -> Result<()> {
    let read = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
    if read("KONAE_COPY_DIR").is_none() && read("KONAE_COPY_S3_PREFIX").is_none() {
        return Ok(());
    }
    let run = async {
        let s3 = s3_client_from_env().await;
        let Some(config) = copies::CopyConfig::from_env(data_root, s3.is_some())? else { return Ok(()) };
        let bucket = env("KONAE_DATA_BUCKET", "konae-data");
        write(&config.targets, s3.as_ref(), &bucket, &entry).await
    };
    let result = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(run)),
        Err(_) => tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(run),
    };
    result.context("The change holds on this server, but it could not be recorded at the copy target, so a restore could undo it. Run the command again")
}

/// Marks a target as keeping a record. Every copy writes it again.
pub(crate) async fn start(target: &Target, s3: Option<&S3Client>, bucket: &str) -> Result<()> {
    let bytes = serde_json::to_vec(&json!({"version": 1}))?;
    match target {
        Target::Directory(directory) => {
            let directory = directory.join(LEDGER_DIR);
            tokio::task::spawn_blocking(move || write_file(&directory, LEDGER_START, &bytes)).await?
        }
        Target::S3 { prefix } => {
            s3.context("S3 is not configured.")?
                .put_object()
                .bucket(bucket)
                .key(format!("{prefix}{LEDGER_DIR}/{LEDGER_START}"))
                .body(ByteStream::from(bytes))
                .send()
                .await?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Applying the record

/// Parses the record's files. `None` when a file does not parse, because a
/// record with a hole cannot prove an account was not suspended.
pub(crate) fn parse(files: &[Vec<u8>]) -> Result<Vec<Entry>> {
    files
        .iter()
        .map(|bytes| serde_json::from_slice::<Entry>(bytes).context("The revocation record holds a file that does not parse."))
        .collect()
}

/// Applies the record to a data directory that is not serving. Only the
/// newest entry for each account or key counts, and only a suspension or a
/// revocation changes anything: a restore never lifts a suspension.
/// Returns how many rows changed.
pub(crate) fn reconcile(root: &Path, entries: &[Entry]) -> Result<usize> {
    let mut latest: HashMap<(String, Option<String>, String), &Entry> = HashMap::new();
    for entry in entries {
        let key = (entry.kind.clone(), entry.deployment_id.clone(), entry.subject.clone());
        let newer = latest.get(&key).is_none_or(|seen| (entry.at, &entry.id) > (seen.at, &seen.id));
        if newer {
            latest.insert(key, entry);
        }
    }
    let mut changed = 0;
    for entry in latest.values() {
        let identity = match (entry.kind.as_str(), entry.deployment_id.as_deref()) {
            ("creator", _) => root.join("owners.sqlite"),
            (_, Some(deployment)) if !deployment.is_empty() && !deployment.contains(['/', '\\', '.']) => {
                root.join("deployments").join(deployment).join("identity.sqlite")
            }
            _ => continue,
        };
        if !identity.is_file() {
            continue;
        }
        let db = Connection::open(&identity)?;
        changed += match (entry.kind.as_str(), entry.state.as_str()) {
            ("app-user" | "creator", "suspended") => {
                let mut rows = db.execute("UPDATE app_users SET state='suspended' WHERE id=?1 AND state<>'suspended'", [&entry.subject])?;
                rows += db.execute("DELETE FROM app_sessions WHERE user_id=?1", [&entry.subject])?;
                if table_exists(&db, "account_challenges")? {
                    rows += db.execute("DELETE FROM account_challenges WHERE user_id=?1 AND consumed_at IS NULL", [&entry.subject])?;
                }
                rows
            }
            ("api-key", "revoked") if table_exists(&db, "api_keys")? => db.execute(
                "UPDATE api_keys SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
                params![now_ms() as i64, entry.subject],
            )?,
            _ => 0,
        };
    }
    Ok(changed)
}

/// Applies the record read from a copy source, or holds the installation.
/// Returns how many rows changed, or the reason it held.
pub(crate) fn apply_or_hold(root: &Path, record: Result<Option<Vec<Vec<u8>>>>) -> Result<std::result::Result<usize, String>> {
    let reason = match record {
        Ok(Some(files)) => match parse(&files).and_then(|entries| reconcile(root, &entries)) {
            Ok(changed) => return Ok(Ok(changed)),
            Err(error_value) => format!("{error_value:#}"),
        },
        Ok(None) => format!("The copy source has no {LEDGER_DIR}/{LEDGER_START}, so the record cannot be read."),
        Err(error_value) => format!("The record could not be read: {error_value:#}"),
    };
    set_hold(root, &reason)?;
    Ok(Err(reason))
}

/// `konae-kernel reconcile`. Returns false for any other command.
pub async fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("reconcile") {
        return Ok(false);
    }
    let root = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    match args.get(1).map(String::as_str) {
        Some("--without-record") if args.len() == 2 => {
            clear_hold(&root)?;
            println!(
                "Reopened {} without the revocation record. Accounts suspended and API keys revoked after the restored copy may be active again; check them in each app's Users view and revoke again. Start the Kernel again; a running Kernel keeps the maintenance it read at startup.",
                root.display()
            );
        }
        Some("--from") if args.len() == 3 => {
            let record = copies::read_record(&args[2]).await;
            match apply_or_hold(&root, record)? {
                Ok(changed) => {
                    clear_hold(&root)?;
                    println!(
                        "Applied the revocation record to {}: {changed} rows changed. Start the Kernel again; a running Kernel keeps the maintenance it read at startup.",
                        root.display()
                    );
                }
                Err(reason) => bail!("The installation stays in maintenance. {reason}"),
            }
        }
        _ => bail!("{RECONCILE_USAGE}"),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthService;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("konae-recovery-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn entries_reach_every_directory_target_and_parse_back() {
        let (one, two) = (scratch(), scratch());
        let targets = [Target::Directory(one.clone()), Target::Directory(two.clone())];
        start(&targets[0], None, "data").await.unwrap();
        let entry = Entry::new("app-user", Some("app-1"), "user-1", "suspended");
        write(&targets, None, "data", &entry).await.unwrap();
        for root in [&one, &two] {
            let files = std::fs::read_dir(root.join(LEDGER_DIR)).unwrap().count();
            assert!(files >= 1);
        }
        let record = copies::read_record(one.to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(parse(&record).unwrap(), vec![entry]);
        assert!(copies::read_record(two.to_str().unwrap()).await.unwrap().is_none(), "no started.json means no readable record");
        std::fs::remove_dir_all(one).unwrap();
        std::fs::remove_dir_all(two).unwrap();
    }

    fn state_with(root: &Path, target: &Path) -> AppState {
        let config = copies::CopyConfig { targets: vec![Target::Directory(target.to_owned())], interval: std::time::Duration::from_secs(3600) };
        AppState {
            deployments: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            artifact_bucket: "artifacts".into(),
            data_bucket: "data".into(),
            files: crate::file_store::Files::local(&root),
            data_root: root.into(),
            copies: Arc::new(copies::CopyStatus::new(Some(&config))),
            accounts: accounts::AccountSettings::local(),
            admin_key: "test-admin".into(),
            public_origin: "http://localhost".into(),
            mcp_origin: None,
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(limits::Guards::new(Vec::new())),
        }
    }

    /// A change that cannot reach the copy target is reported, not
    /// confirmed, so the caller knows a restore could undo it.
    #[tokio::test]
    async fn a_change_is_confirmed_only_once_it_is_off_the_volume() {
        let (root, target) = (scratch(), scratch());
        std::fs::create_dir_all(&root).unwrap();
        record(&state_with(&root, &target), Entry::new("api-key", Some("app-1"), "key-1", "revoked")).await.unwrap();
        assert_eq!(std::fs::read_dir(target.join(LEDGER_DIR)).unwrap().count(), 1);
        // A plain file where the record's folder should be.
        let blocked = scratch();
        std::fs::write(&blocked, "not a directory").unwrap();
        let refused = record(&state_with(&root, &blocked), Entry::new("api-key", Some("app-1"), "key-2", "revoked")).await.unwrap_err().to_string();
        assert!(refused.contains("could not be recorded at the copy target") && refused.contains("Repeat the same request"), "{refused}");
        for path in [root, target] {
            std::fs::remove_dir_all(path).unwrap();
        }
        std::fs::remove_file(blocked).unwrap();
    }

    #[test]
    fn a_restore_never_revives_a_suspended_account_or_a_revoked_key() {
        let root = scratch();
        let folder = root.join("deployments/app-1");
        std::fs::create_dir_all(&folder).unwrap();
        let app = AuthService::open(folder.join("identity.sqlite")).unwrap();
        let kept = app.signup("kept@example.test", "correct horse battery").unwrap();
        let gone = app.signup("gone@example.test", "correct horse battery").unwrap();
        Connection::open(folder.join("identity.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE api_keys(id TEXT PRIMARY KEY, revoked_at INTEGER); INSERT INTO api_keys VALUES('key-1', NULL), ('key-2', NULL);")
            .unwrap();
        let creators = AuthService::open(root.join("owners.sqlite")).unwrap();
        let creator = creators.signup("creator@example.test", "correct horse battery").unwrap();
        let entries = vec![
            Entry::new("app-user", Some("app-1"), &gone.user.user_id, "suspended"),
            Entry::new("api-key", Some("app-1"), "key-1", "revoked"),
            Entry::new("creator", None, &creator.user.user_id, "suspended"),
            // An entry for another app or an unsafe path changes nothing.
            Entry::new("app-user", Some("../app-1"), &kept.user.user_id, "suspended"),
        ];
        assert!(reconcile(&root, &entries).unwrap() >= 4);
        let state = |path: &Path, id: &str| -> String {
            Connection::open(path).unwrap().query_row("SELECT state FROM app_users WHERE id=?1", [id], |r| r.get(0)).unwrap()
        };
        assert_eq!(state(&folder.join("identity.sqlite"), &gone.user.user_id), "suspended");
        assert_ne!(state(&folder.join("identity.sqlite"), &kept.user.user_id), "suspended");
        assert_eq!(state(&root.join("owners.sqlite"), &creator.user.user_id), "suspended");
        let sessions: i64 = Connection::open(folder.join("identity.sqlite"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM app_sessions WHERE user_id=?1", [&gone.user.user_id], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
        let revoked: Vec<(String, Option<i64>)> = {
            let db = Connection::open(folder.join("identity.sqlite")).unwrap();
            let mut query = db.prepare("SELECT id, revoked_at FROM api_keys ORDER BY id").unwrap();
            query.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
        };
        assert!(revoked[0].1.is_some() && revoked[1].1.is_none());
        // Applying the same record twice changes nothing more.
        assert_eq!(reconcile(&root, &entries).unwrap(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_record_holds_the_installation_until_reconciled() {
        let root = scratch();
        std::fs::create_dir_all(&root).unwrap();
        let reason = apply_or_hold(&root, Ok(None)).unwrap().unwrap_err();
        assert!(reason.contains("started.json"));
        assert!(hold(&root).unwrap().contains("reconcile"));
        assert!(super::super::maintenance::pause(&root, "any-app").is_some());
        assert!(super::super::maintenance::holds_mail(&root, "creator"));
        let broken = apply_or_hold(&root, Ok(Some(vec![b"{not json".to_vec()]))).unwrap().unwrap_err();
        assert!(broken.contains("does not parse"));
        let failed = apply_or_hold(&root, Err(anyhow::anyhow!("bucket unreachable"))).unwrap().unwrap_err();
        assert!(failed.contains("bucket unreachable"));
        assert_eq!(apply_or_hold(&root, Ok(Some(Vec::new()))).unwrap(), Ok(0));
        clear_hold(&root).unwrap();
        assert!(hold(&root).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
