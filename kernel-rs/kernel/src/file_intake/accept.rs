use std::path::Path;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AppState, Deployment, app_data::database_lock};

pub const DECLARATION_FILE: &str = "file-intake.json";
pub const MAX_DECLARATION_BYTES: usize = 64 * 1024;
pub const MAX_SOURCES: usize = 32;
pub const MAX_CONSUMERS: usize = 32;
pub const MAX_PATH_BYTES: usize = 1024;
pub const MAX_PENDING_JOBS_PER_APP: usize = 1000;

struct StagedInput(std::path::PathBuf);
impl Drop for StagedInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub version: u64,
    pub sources: Vec<Source>,
    pub consumers: Vec<Consumer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub version: u64,
    #[serde(default)]
    pub replay: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    pub id: String,
    pub source: String,
    pub action: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptOutcome {
    pub version_id: String,
    pub created: bool,
}

/// Validates the app-owned source and consumer declarations.
pub fn validate_declaration(raw: &str) -> Result<Declaration> {
    ensure!(
        raw.len() <= MAX_DECLARATION_BYTES,
        "file-intake.json exceeds 65536 bytes."
    );
    let declaration: Declaration =
        serde_json::from_str(raw).context("file-intake.json is not valid JSON.")?;
    ensure!(
        declaration.version == 1,
        "file-intake.json version must be 1."
    );
    ensure!(
        !declaration.sources.is_empty() && declaration.sources.len() <= MAX_SOURCES,
        "file-intake.json must declare between 1 and 32 sources."
    );
    ensure!(
        declaration.consumers.len() <= MAX_CONSUMERS,
        "file-intake.json may declare at most 32 consumers."
    );
    let mut sources = std::collections::HashSet::new();
    for source in &declaration.sources {
        valid_id(&source.id, "source")?;
        ensure!(
            source.version == 1,
            "File intake source {} version must be 1.",
            source.id
        );
        ensure!(
            source.replay.is_empty() || source.replay == "delivery-key-or-current-path-sha256",
            "File intake source {} has an unsupported replay rule.",
            source.id
        );
        ensure!(
            sources.insert(source.id.as_str()),
            "file-intake.json declares source {} twice.",
            source.id
        );
    }
    let mut consumers = std::collections::HashSet::new();
    for consumer in &declaration.consumers {
        valid_id(&consumer.id, "consumer")?;
        valid_id(&consumer.source, "source")?;
        ensure!(
            sources.contains(consumer.source.as_str()),
            "File intake consumer {} names undeclared source {}.",
            consumer.id,
            consumer.source
        );
        ensure!(
            consumer.action.len() <= 128
                && consumer.action.split('.').count() == 2
                && consumer.action.split('.').all(valid_component),
            "File intake consumer {} must name a declared mutation action.",
            consumer.id
        );
        ensure!(
            consumers.insert(consumer.id.as_str()),
            "file-intake.json declares consumer {} twice.",
            consumer.id
        );
    }
    Ok(declaration)
}

pub fn validate_consumers(
    declaration: &Declaration,
    program: &crate::actions::ActionProgram,
) -> Result<()> {
    for consumer in &declaration.consumers {
        let action = program.actions.get(&consumer.action).with_context(|| {
            format!(
                "File intake consumer {} names missing action {}.",
                consumer.id, consumer.action
            )
        })?;
        ensure!(
            action.kind == "mutation",
            "File intake consumer {} action {} must be a mutation.",
            consumer.id,
            consumer.action
        );
        ensure!(
            !action
                .bindings
                .iter()
                .any(|binding| binding.source == "actor"),
            "File intake consumer {} action cannot bind a human actor.",
            consumer.id
        );
        let allowed = action
            .bindings
            .iter()
            .filter(|binding| binding.source == "input")
            .filter_map(|binding| binding.key.as_deref())
            .collect::<std::collections::HashSet<_>>();
        let input = consumer
            .input
            .as_object()
            .context("File intake consumer input must be an object.")?;
        ensure!(
            !input.keys().any(|key| matches!(
                key.as_str(),
                "versionId" | "sourceId" | "path" | "sha256" | "size"
            )),
            "File intake consumer {} input uses a reserved version field.",
            consumer.id
        );
        ensure!(
            input.keys().all(|key| allowed.contains(key.as_str())),
            "File intake consumer {} input has an undeclared action field.",
            consumer.id
        );
        for binding in action
            .bindings
            .iter()
            .filter(|binding| binding.source == "input")
        {
            if let Some(key) = binding.key.as_deref() {
                let supplied_by_version =
                    matches!(key, "versionId" | "sourceId" | "path" | "sha256" | "size");
                ensure!(
                    binding.nullable || input.contains_key(key) || supplied_by_version,
                    "File intake consumer {} does not supply action input {key}.",
                    consumer.id
                );
            }
        }
    }
    Ok(())
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_id(value: &str, kind: &str) -> Result<()> {
    ensure!(
        valid_component(value),
        "File intake {kind} ID must use 1 to 64 ASCII letters, digits, '_' or '-'."
    );
    Ok(())
}

/// Accepts a completed staged file for a trusted caller that already bound its
/// authenticated credential to this Deployment and source ID.
pub async fn accept_staged(
    state: &AppState,
    deployment: &Deployment,
    source_id: &str,
    staged_bytes: &Path,
    relative_path: &str,
    delivery_key: Option<&str>,
) -> Result<AcceptOutcome> {
    let path = normalize_relative_path(relative_path)?;
    let metadata = tokio::fs::metadata(staged_bytes)
        .await
        .context("The staged intake file cannot be read.")?;
    ensure!(
        metadata.is_file()
            && metadata.len() > 0
            && metadata.len() <= state.files.limits.max_file_bytes,
        "File intake bytes must be between 1 byte and {} bytes.",
        state.files.limits.max_file_bytes
    );
    let staging = state.data_root.join("transfers");
    tokio::fs::create_dir_all(&staging).await?;
    let frozen_path = staging.join(format!(".intake-{}", uuid::Uuid::new_v4()));
    tokio::fs::copy(staged_bytes, &frozen_path)
        .await
        .context("The intake bytes could not be staged locally.")?;
    let _frozen = StagedInput(frozen_path.clone());
    let _write = database_lock(state, &deployment.id).await;
    let active = state
        .deployments
        .read()
        .await
        .values()
        .find(|current| {
            current.id == deployment.id
                && current.project_id == deployment.project_id
                && current.artifact_version == deployment.artifact_version
        })
        .cloned()
        .context("File intake caller did not resolve the current app Deployment.")?;
    ensure!(
        crate::modules::is_on(&active.modules, "file-intake"),
        "Turn on the file-intake module before accepting files."
    );
    let declaration = declaration(&active)?;
    let source = declaration
        .sources
        .iter()
        .find(|source| source.id == source_id)
        .with_context(|| {
            format!(
                "App {} has no declared file intake source {source_id}.",
                deployment.id
            )
        })?;
    if let Some(key) = delivery_key {
        ensure!(
            !key.is_empty() && key.len() <= 512 && !key.bytes().any(|byte| byte.is_ascii_control()),
            "File intake delivery key must contain 1 to 512 non-control bytes."
        );
    }
    let checksum_path = frozen_path.clone();
    let (size, digest) =
        tokio::task::spawn_blocking(move || crate::copies::checksum_file(&checksum_path)).await??;
    ensure!(
        size == metadata.len(),
        "The staged intake file changed while it was being read."
    );
    let database = crate::layout::database(&state.data_root, &deployment.id);
    let connection = Connection::open(&database)
        .with_context(|| format!("Cannot open app database for {}.", deployment.id))?;
    ensure_schema(&connection)?;

    if let Some(key) = delivery_key {
        let previous: Option<(String, String, String)> = connection.query_row(
            "SELECT version_id, sha256, path FROM __konae_intake_deliveries WHERE source_id=?1 AND delivery_key=?2",
            params![source_id, key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if let Some((version_id, old_digest, old_path)) = previous {
            ensure!(
                old_digest == digest && old_path == path,
                "File intake delivery key was reused for different bytes or a different path."
            );
            return Ok(AcceptOutcome {
                version_id,
                created: false,
            });
        }
    } else {
        let previous: Option<(String, String)> = connection.query_row(
            "SELECT version_id, sha256 FROM __konae_intake_current WHERE source_id=?1 AND path=?2",
            params![source_id, path], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((version_id, old_digest)) = previous {
            if old_digest == digest {
                return Ok(AcceptOutcome {
                    version_id,
                    created: false,
                });
            }
        }
    }

    let refs = crate::modules::blob_references(&state.data_root, &deployment.id)?;
    let used = refs.iter().fold(0u64, |total, reference| {
        total.saturating_add(reference.size)
    });
    ensure!(
        used.saturating_add(size) <= state.files.limits.quota_bytes,
        "File intake exceeds the app's {} byte file quota.",
        state.files.limits.quota_bytes
    );
    let version_id = uuid::Uuid::new_v4().to_string();
    state
        .files
        .store
        .put(&deployment.id, &version_id, &frozen_path)
        .await
        .context("The intake blob could not be staged.")?;
    let consumers = declaration
        .consumers
        .iter()
        .filter(|consumer| consumer.source == source_id)
        .collect::<Vec<_>>();
    let pending: usize = connection.query_row(
        "SELECT count(*) FROM __konae_intake_jobs WHERE state IN ('pending','leased')",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        pending.saturating_add(consumers.len()) <= MAX_PENDING_JOBS_PER_APP,
        "File intake already has 1000 pending consumer jobs for this app."
    );
    let transaction = connection.unchecked_transaction()?;
    let now = crate::now_ms() as i64;
    transaction.execute(
        "INSERT INTO __konae_intake_versions(id,source_id,source_version,path,sha256,size,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![version_id, source_id, source.version as i64, path, digest, size as i64, now],
    )?;
    if let Some(key) = delivery_key {
        transaction.execute(
            "INSERT INTO __konae_intake_deliveries(source_id,delivery_key,version_id,sha256,path) VALUES(?1,?2,?3,?4,?5)",
            params![source_id, key, version_id, digest, path],
        )?;
    }
    transaction.execute(
        "INSERT INTO __konae_intake_current(source_id,path,version_id,sha256) VALUES(?1,?2,?3,?4) ON CONFLICT(source_id,path) DO UPDATE SET version_id=excluded.version_id,sha256=excluded.sha256",
        params![source_id, path, version_id, digest],
    )?;
    for consumer in consumers {
        let input = serde_json::to_string(&consumer.input)?;
        let program = serde_json::from_str::<Value>(&deployment.actions)?;
        let action = program
            .get("actions")
            .and_then(Value::as_object)
            .and_then(|actions| actions.get(&consumer.action))
            .with_context(|| {
                format!(
                    "File intake consumer {} names missing action {}.",
                    consumer.id, consumer.action
                )
            })?;
        ensure!(
            action.get("kind").and_then(Value::as_str) == Some("mutation"),
            "File intake consumer {} must name a mutation action.",
            consumer.id
        );
        transaction.execute(
            "INSERT INTO __konae_intake_jobs(id,version_id,consumer_id,action,input,action_json,state,next_attempt_at,created_at) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,?7)",
            params![uuid::Uuid::new_v4().to_string(), version_id, consumer.id, consumer.action, input, action.to_string(), now],
        )?;
    }
    transaction.commit()?;
    state
        .dispatcher
        .wake(
            crate::execution::WorkResource::FileIntake,
            deployment.id.clone(),
        )
        .await;
    Ok(AcceptOutcome {
        version_id,
        created: true,
    })
}

fn declaration(deployment: &Deployment) -> Result<Declaration> {
    let value = deployment
        .manifest
        .get("fileIntake")
        .context("The active Deployment has no pinned file-intake.json declaration.")?;
    validate_declaration(&serde_json::to_string(value)?)
}

pub fn normalize_relative_path(path: &str) -> Result<String> {
    ensure!(
        !path.is_empty()
            && path.len() <= MAX_PATH_BYTES
            && !path.starts_with('/')
            && !path.contains('\\')
            && !path.bytes().any(|byte| byte.is_ascii_control()),
        "File intake path must be a normalized relative path of at most 1024 bytes."
    );
    let parts = path.split('/').collect::<Vec<_>>();
    ensure!(
        parts
            .iter()
            .all(|part| !part.is_empty() && *part != "." && *part != ".."),
        "File intake path must not contain empty, '.' or '..' components."
    );
    Ok(path.to_owned())
}

pub(super) fn ensure_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(include_str!("schema.sql"))?;
    Ok(())
}
