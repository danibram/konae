use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::{AppState, Deployment};

const LEASE_MS: i64 = 60_000;

pub(crate) fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        for deployment in state.deployments.read().await.values() {
            let root = state.data_root.clone();
            let id = deployment.id.clone();
            let _ = tokio::task::spawn_blocking(move || -> Result<()> {
                let path = crate::layout::database(&root, &id);
                if path.is_file() {
                    let connection = Connection::open(path)?;
                    if connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_intake_jobs')", [], |row| row.get::<_, bool>(0))? {
                        connection.execute("UPDATE __konae_intake_jobs SET state='pending',leased_until=NULL,next_attempt_at=?1 WHERE state IN ('leased','pending')", [crate::now_ms() as i64])?;
                    }
                }
                Ok(())
            }).await;
            state
                .dispatcher
                .wake(
                    crate::execution::WorkResource::FileIntake,
                    deployment.id.clone(),
                )
                .await;
        }
        loop {
            let turn = state
                .dispatcher
                .next_turn(
                    crate::execution::WorkResource::FileIntake,
                    crate::execution::WorkLimits::SERIAL,
                )
                .await;
            let app = turn.app().to_owned();
            let deployment = state
                .deployments
                .read()
                .await
                .values()
                .find(|deployment| deployment.id == app)
                .cloned();
            let Some(deployment) = deployment else {
                continue;
            };
            match run_one(&state, &deployment).await {
                Ok(true) => {
                    state
                        .dispatcher
                        .wake(crate::execution::WorkResource::FileIntake, app)
                        .await
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("File intake consumer pass failed for {app}: {error:#}");
                    defer_retry(&state, &app).await;
                }
            }
        }
    });
}

pub(super) async fn run_one(state: &AppState, deployment: &Deployment) -> Result<bool> {
    let _operation = state.copies.operations.read().await;
    let _write = crate::database_lock(state, &deployment.id).await;
    if crate::maintenance::pause(&state.data_root, &deployment.id).is_some() {
        return Ok(false);
    }
    let state = state.clone();
    let deployment = deployment.clone();
    tokio::task::spawn_blocking(move || run_one_blocking(&state, &deployment)).await?
}

fn run_one_blocking(state: &AppState, deployment: &Deployment) -> Result<bool> {
    let path = crate::layout::database(&state.data_root, &deployment.id);
    if !path.is_file() {
        return Ok(false);
    }
    let connection = crate::app_db::open(&path)?;
    let internal = connection.kernel_effect_token();
    let transaction = connection.unchecked_transaction()?;
    crate::app_data::with_kernel_effect_access(&internal, || {
        super::accept::ensure_schema(&transaction)
    })?;
    let now = crate::now_ms() as i64;
    let job: Option<(String, String, String, String, String, String)> =
        crate::app_data::with_kernel_effect_access(
            &internal,
            || -> rusqlite::Result<Option<(String, String, String, String, String, String)>> {
                transaction.query_row(
            "SELECT j.id,j.version_id,j.consumer_id,j.action,j.input,j.action_json FROM __konae_intake_jobs j WHERE j.state IN ('pending','leased') AND j.next_attempt_at<=?1 AND (j.leased_until IS NULL OR j.leased_until<?1) ORDER BY j.created_at,j.id LIMIT 1",
            [now], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        ).optional()
            },
        )?;
    let Some((job_id, version_id, consumer_id, action_name, saved_input, saved_action)) = job
    else {
        return Ok(false);
    };
    crate::app_data::with_kernel_effect_access(&internal, || {
        transaction.execute("UPDATE __konae_intake_jobs SET state='leased',leased_until=?1,attempts=attempts+1 WHERE id=?2", params![now + LEASE_MS,job_id])
    })?;
    transaction.commit()?;

    let transaction = connection.unchecked_transaction()?;
    let details: (String, String, i64, String) =
        crate::app_data::with_kernel_effect_access(&internal, || {
            transaction.query_row(
                "SELECT path,sha256,size,source_id FROM __konae_intake_versions WHERE id=?1",
                [&version_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })?;
    let mut input: Value = serde_json::from_str(&saved_input)?;
    let object = input
        .as_object_mut()
        .context("File intake consumer input must be an object.")?;
    object.insert("versionId".into(), json!(version_id));
    object.insert("sourceId".into(), json!(details.3));
    object.insert("path".into(), json!(details.0));
    object.insert("sha256".into(), json!(details.1));
    object.insert("size".into(), json!(details.2));
    let action: crate::actions::ActionDefinition = serde_json::from_str(&saved_action)
        .context("The accepted file intake action cannot be read.")?;
    super::accept::validate_consumer_action(&consumer_id, &action_name, &action)?;
    let accepted = action
        .bindings
        .iter()
        .filter(|binding| binding.source == "input")
        .filter_map(|binding| binding.key.as_deref())
        .collect::<std::collections::HashSet<_>>();
    input
        .as_object_mut()
        .expect("validated object")
        .retain(|key, _| accepted.contains(key.as_str()));
    let bindings = crate::resolve_bindings(&action, &input, None).map_err(anyhow::Error::msg)?;
    crate::run_mutation_on_connection(&transaction, &action, bindings)?;
    crate::app_data::with_kernel_effect_access(&internal, || -> rusqlite::Result<()> {
        transaction.execute(
            "INSERT INTO __konae_intake_effects(version_id,consumer_id,job_id,committed_at) VALUES(?1,?2,?3,?4) ON CONFLICT(version_id,consumer_id) DO NOTHING",
            params![version_id,consumer_id,job_id,now],
        )?;
        transaction.execute(
            "UPDATE __konae_intake_jobs SET state='done',leased_until=NULL WHERE id=?1",
            [&job_id],
        )?;
        Ok(())
    })?;
    transaction.commit()?;
    Ok(true)
}

async fn defer_retry(state: &AppState, app: &str) {
    let _write = crate::database_lock(state, app).await;
    let path = crate::layout::database(&state.data_root, app);
    let now = crate::now_ms() as i64;
    if let Ok(connection) = Connection::open(path) {
        let _ = connection.execute("UPDATE __konae_intake_jobs SET state='pending',leased_until=NULL,next_attempt_at=?1 WHERE state='leased'", [now + 5_000]);
    }
    let dispatcher = state.dispatcher.clone();
    let app = app.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        dispatcher
            .wake(crate::execution::WorkResource::FileIntake, app)
            .await;
    });
}

pub(crate) fn check_pending(connection: &Connection) -> Result<()> {
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_intake_jobs')", [], |row| row.get(0))?;
    if !exists {
        return Ok(());
    }
    let mut statement = connection.prepare("SELECT consumer_id,action_json FROM __konae_intake_jobs WHERE state IN ('pending','leased')")?;
    let jobs = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for job in jobs {
        let (consumer, raw) = job?;
        let action: crate::actions::ActionDefinition = serde_json::from_str(&raw)?;
        connection.prepare(&action.statement).with_context(|| {
            format!(
                "Pending file intake consumer {consumer} no longer prepares against schema.sql."
            )
        })?;
    }
    Ok(())
}

pub(crate) fn prepare_transfer(state: &AppState, app: &str) -> Result<()> {
    let path = crate::layout::database(&state.data_root, app);
    if !path.is_file() {
        return Ok(());
    }
    let connection = Connection::open(path)?;
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_intake_jobs')", [], |row| row.get(0))?;
    if exists {
        let active: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM __konae_intake_jobs WHERE state='leased' AND leased_until>?1)", [crate::now_ms() as i64], |row| row.get(0))?;
        anyhow::ensure!(
            !active,
            "File intake has an active consumer lease; try the transfer again after it finishes."
        );
    }
    Ok(())
}

pub(crate) fn pause_export(path: &std::path::Path) -> Result<()> {
    let connection = Connection::open(path)?;
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_intake_jobs')", [], |row| row.get(0))?;
    if exists {
        connection.execute(
            "UPDATE __konae_intake_jobs SET state='pending',leased_until=NULL WHERE state='leased'",
            [],
        )?;
    }
    Ok(())
}

pub(crate) fn resume_import(path: &std::path::Path) -> Result<()> {
    pause_export(path)
}
