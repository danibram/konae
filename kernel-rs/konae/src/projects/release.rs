use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::mcp::{self, Owner};
use crate::projects::sources;
use crate::deployments::validate_release;
use crate::runtime::response_value;
use crate::state::Konae;
use crate::transfers;
use crate::*;

/// Remembers which export each project imported, so a retried import
/// returns its first receipt.
pub(crate) fn install_imports(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS package_imports(package_id TEXT NOT NULL, project TEXT NOT NULL, plan TEXT NOT NULL, PRIMARY KEY(package_id, project));",
    )?;
    Ok(())
}

/// A second `release_plan` with an export this project already imported
/// gets the first plan and its receipt instead of a refusal.
fn replay_import(
    state: &Konae,
    owner: &Owner,
    project: &str,
    package_id: &str,
) -> Result<Option<Value>> {
    let db = mcp::store(state)?;
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

fn remember_import(state: &Konae, package_id: &str, project: &str, plan: &str) -> Result<()> {
    mcp::store(state)?.execute(
        "INSERT INTO package_imports(package_id,project,plan) VALUES(?1,?2,?3)
         ON CONFLICT(package_id,project) DO UPDATE SET plan=excluded.plan",
        params![package_id, project, plan],
    )?;
    Ok(())
}

pub(crate) fn source_revision_refusal(observed: u64) -> anyhow::Error {
    crate::refusal::refuse("sources.revision-conflict", format!("Sources changed since revision {observed}. Run konae pull --dir <folder>, then publish again.")).into()
}

async fn release_bytes(state: &Konae, upload: &Value, transfer_id: &str) -> Result<Vec<u8>> {
    sources::ensure_no_component(upload)?;
    let size = upload["size"].as_u64().unwrap_or(0) as usize;
    anyhow::ensure!(
        size <= crate::size::RELEASE_BYTES,
        "{}",
        crate::size::release_refused(size)
    );
    Ok(tokio::fs::read(transfers::transfer_path(state, transfer_id)?).await?)
}

async fn release_upload_ids(
    state: &Konae,
    principal: &str,
    release_id: &str,
    sources_id: Option<&str>,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    if let Ok(upload) = transfers::transfer(state, principal, &json!({"transferId": release_id})) {
        let payload: Value =
            serde_json::from_slice(&release_bytes(state, &upload, release_id).await?)?;
        if let Some(components) = payload["initialPackage"]["components"].as_object() {
            ids.extend(
                components
                    .values()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    if let Some(sources) = sources_id {
        ids.push(sources.to_owned());
    }
    ids.push(release_id.to_owned());
    Ok(ids)
}

pub(crate) async fn cleanup_release_uploads(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    plan_id: &str,
) -> Result<()> {
    let (release_id, sources_id, recorded_ids): (String, Option<String>, String) = {
        let db = mcp::store(state)?;
        db.query_row(
            "SELECT transfer,sources,cleanup_uploads FROM plans WHERE id=?1 AND owner=?2",
            params![plan_id, owner.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    };

    let mut ids: Vec<String> = serde_json::from_str(&recorded_ids)?;
    if ids.is_empty() {
        ids = release_upload_ids(state, principal, &release_id, sources_id.as_deref()).await?;
    }
    for transfer_id in ids {
        let still_available = transfers::completed_upload(&state.kernel, principal, &transfer_id)?;
        if still_available {
            transfers::transfer_call(
                state,
                principal,
                "transfer_cancel",
                &json!({"transferId":transfer_id}),
                false,
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) fn transfer_project_for_owner(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    transfer_id: &str,
) -> Result<(Option<String>, bool)> {
    if let Some(upload) =
        transfers::owned_transfer(&state.kernel, principal, &json!({"transferId":transfer_id}))?
    {
        return Ok((upload["projectId"].as_str().map(str::to_owned), true));
    }
    let db = mcp::store(state)?;
    let mut query =
        db.prepare("SELECT project,transfer,sources,cleanup_uploads FROM plans WHERE owner=?1")?;
    let plans = query
        .query_map([&owner.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (project, release_id, sources_id, cleanup_json) in plans {
        let cleanup_ids: Vec<String> = serde_json::from_str(&cleanup_json)?;
        if release_id == transfer_id
            || sources_id.as_deref() == Some(transfer_id)
            || cleanup_ids.iter().any(|id| id == transfer_id)
        {
            return Ok((Some(project), false));
        }
    }
    Ok((None, false))
}

#[derive(serde::Deserialize)]
struct ReleaseSources {
    files: HashMap<String, String>,
    #[serde(default)]
    encodings: HashMap<String, String>,
    #[serde(default)]
    directories: Vec<String>,
}

async fn release_input(
    state: &Konae,
    principal: &str,
    transfer_id: &str,
    sources_id: Option<&str>,
    artifact_id: Option<&str>,
) -> Result<DeployInput> {
    let upload = transfers::transfer(state, principal, &json!({"transferId": transfer_id}))?;
    anyhow::ensure!(
        upload["complete"] == true && upload["direction"] == "upload",
        "Upload is not complete."
    );
    let mut payload: Value =
        serde_json::from_slice(&release_bytes(state, &upload, transfer_id).await?)?;
    if let Some(artifact_id) = artifact_id {
        anyhow::ensure!(
            payload.get("artifact").is_none(),
            "Send artifact in the release JSON or artifactTransferId, not both."
        );
        let project_id = payload["project"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Release has no project ID."))?;
        let artifact_upload =
            transfers::transfer(state, principal, &json!({"transferId": artifact_id}))?;
        anyhow::ensure!(
            artifact_upload["complete"] == true
                && artifact_upload["direction"] == "upload"
                && artifact_upload["projectId"] == project_id,
            "Completed artifact upload for this project required."
        );
        sources::ensure_no_component(&artifact_upload)?;
        payload["artifact"] = serde_json::from_slice(
            &tokio::fs::read(transfers::transfer_path(state, artifact_id)?).await?,
        )?;
    }
    let mut input: DeployInput = kernel::functions::parse_deploy_input(payload)?;
    anyhow::ensure!(
        upload["projectId"] == input.project.id,
        "Artifact belongs to another project."
    );
    if let Some(sources_id) = sources_id {
        anyhow::ensure!(
            input.project.files.is_empty() && input.project.encodings.is_empty(),
            "Send the files of app/ in sourcesTransferId or in project.files, not in both."
        );
        let upload = transfers::transfer(state, principal, &json!({"transferId": sources_id}))?;
        anyhow::ensure!(
            upload["complete"] == true
                && upload["direction"] == "upload"
                && upload["projectId"] == input.project.id,
            "Completed upload of the sources for this project required."
        );
        sources::ensure_no_component(&upload)?;
        let bytes = tokio::fs::read(transfers::transfer_path(state, sources_id)?).await?;
        let sources: ReleaseSources = serde_json::from_slice(&bytes).map_err(|error_value| {
            anyhow::anyhow!(
                "The sources upload must be the JSON {{ files, encodings }}: {error_value}"
            )
        })?;
        input.project.files = sources.files;
        input.project.encodings = sources.encodings;
        input.project.directories = sources.directories;
    }
    Ok(input)
}

pub(crate) async fn export(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Value> {
    let deployment = mcp::owned_deployment(
        state,
        owner,
        a["projectId"].as_str().unwrap(),
        "export_data",
    )
    .await?;
    crate::package::export(state, deployment, principal, |deployment| {
        let (_, published) =
            mcp::owned_project(state, owner, &deployment.project_id, "export_data")?;
        let sources: Option<u64> = mcp::store(state)?
            .query_row(
                "SELECT sources FROM projects WHERE id=?1",
                [&deployment.project_id],
                |r| r.get(0),
            )
            .optional()?
            .filter(|saves: &u64| *saves > 0);
        Ok(crate::package::Revisions { published, sources })
    })
    .await
}

pub(crate) async fn plan(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Value> {
    let transfer_id = a["transferId"].as_str().unwrap();
    let sources_id = a["sourcesTransferId"].as_str();
    let input = release_input(state, principal, transfer_id, sources_id, None).await?;
    let artifact_sha256 =
        transfers::transfer(state, principal, &json!({"transferId": transfer_id}))?["sha256"]
            .clone();
    let _guard = database_lock(state, &format!("project:{}", input.project.id)).await;
    let (_, revision) = mcp::owned_project(state, owner, &input.project.id, "publish")?;
    let source_revision: u64 = mcp::store(state)?.query_row(
        "SELECT sources FROM projects WHERE id=?1",
        [&input.project.id],
        |r| r.get(0),
    )?;
    let has_deployment =
        find_project_deployment(&*state.deployments.read().await, &input.project.id).is_some();
    let observed = a["observedSourcesRevision"].as_u64();
    if (has_deployment || source_revision > 0) && observed != Some(source_revision) {
        return Err(source_revision_refusal(observed.unwrap_or(0)));
    }
    if let Some(package) = input.initial_package.as_ref() {
        if let Some(replay) = replay_import(
            state,
            owner,
            &input.project.id,
            &package.manifest.package_id,
        )? {
            return Ok(replay);
        }
        crate::package::verify(state, principal, &input.project.id, package).await?;
    }
    let schema = validate_release(state, &input, None).await?;
    kernel::functions::prepare_artifact(input.server_artifact.as_ref()).await?;

    let existing = find_project_deployment(&*state.deployments.read().await, &input.project.id);
    let (hostname, first_release) = match existing {
        Some(deployment) => (Some(deployment.hostname), false),
        None => (
            crate::hostname::planned(state, &input.project.slug, &input.project.id).await,
            true,
        ),
    };
    let url = hostname
        .as_deref()
        .map(|hostname| deployment_url(&state.public_origin, hostname));
    let id = Uuid::new_v4().to_string();
    let expires = now_ms() + 900_000;

    let cleanup_uploads = release_upload_ids(state, principal, transfer_id, sources_id).await?;
    mcp::store(state)?.execute("INSERT INTO plans(id,owner,project,revision,transfer,expires,sources,source_revision,cleanup_uploads) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,owner.id,input.project.id,revision,transfer_id,expires,sources_id,source_revision,serde_json::to_string(&cleanup_uploads)?])?;
    if let Some(package) = input.initial_package.as_ref() {
        remember_import(state, &package.manifest.package_id, &input.project.id, &id)?;
    }
    Ok(
        json!({"planId":id,"projectId":input.project.id,"expectedRevision":revision,"expectedSourcesRevision":source_revision,"artifactSha256":artifact_sha256,"visibility":input.visibility,"expiresAt":expires,"schema":schema.to_json(),"hostname":hostname,"url":url,"firstRelease":first_release}),
    )
}

pub(crate) async fn apply(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Value> {
    crate::copies::Pending::through_catalogue(async {
            let id = a["planId"].as_str().unwrap();
            let row: (String,u64,String,u64,Option<String>,Option<String>,u64)=mcp::store(state)?.query_row("SELECT project,revision,transfer,expires,receipt,sources,source_revision FROM plans WHERE id=?1 AND owner=?2",params![id,owner.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
            let _guard = database_lock(state, &format!("project:{}", row.0)).await;
            let (mut metadata, revision) = mcp::owned_project(state, owner, &row.0, "publish")?;
            let receipt: Option<String> =
                mcp::store(state)?
                    .query_row("SELECT receipt FROM plans WHERE id=?1", [id], |r| r.get(0))?;
            if let Some(receipt) = receipt {
                return Ok(serde_json::from_str(&receipt)?);
            }
            anyhow::ensure!(row.3 > now_ms(), "Release plan expired.");
            anyhow::ensure!(
                revision == row.1,
                "Project revision changed. Create a new release plan."
            );
            let source_revision: u64 = mcp::store(state)?.query_row("SELECT sources FROM projects WHERE id=?1", [&row.0], |r| r.get(0))?;
            if source_revision != row.6 {
                return Err(source_revision_refusal(row.6));
            }
            let mut input = release_input(state, principal, &row.2, row.5.as_deref(), None).await?;
            anyhow::ensure!(input.project.id == row.0, "Artifact belongs to another project.");

            input.contact_email = contact_email(state, owner, &row.0, input.contact_email.as_deref()).await?;


            input.schema_confirmation = a["confirmSchema"].as_str().map(str::to_owned);
            metadata["name"] = json!(input.project.name);
            metadata["slug"] = json!(input.project.slug);
            metadata["manifest"] = input.project.manifest.clone();
            let receipt = response_value(deploy_input(state, input).await).await?;
            let mut db = mcp::store(state)?;
            let tx = db.transaction()?;
            tx.execute("UPDATE projects SET revision=revision+1,sources=sources+1,body=?4 WHERE id=?1 AND owner=?2 AND revision=?3 AND sources=?5",params![row.0,owner.id,revision,metadata.to_string(),source_revision])?;
            tx.execute(
                "UPDATE plans SET receipt=?1 WHERE id=?2",
                params![receipt.to_string(), id],
            )?;
            tx.commit()?;
            drop(db);
            Ok(receipt)
        }).await
}

const IMPORT_REPLACE: &str = "replace must be sources, data or both.";

// A refused release can leave a complete app after its import is reversed.
// The outer result tells the copy journal whether the catalogue is consistent.
pub(crate) async fn import(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    a: &Value,
) -> Result<Result<Value>> {
    use crate::import::{Outcome, Target};
    let deployment = mcp::owned_deployment(
        state,
        owner,
        a["projectId"].as_str().unwrap_or_default(),
        "publish",
    )
    .await?;
    let replace = a["replace"].as_str().unwrap_or_default();
    let (data, sources) = match replace {
        "sources" => (false, true),
        "data" => (true, false),
        "both" => (true, true),
        _ => anyhow::bail!(IMPORT_REPLACE),
    };
    anyhow::ensure!(
        data == a.get("package").is_some(),
        "Pass package exactly when replace is data or both."
    );
    anyhow::ensure!(
        sources == a.get("transferId").is_some(),
        "Pass transferId exactly when replace is sources or both."
    );
    anyhow::ensure!(
        !a.get("sourcesTransferId").is_some() || sources,
        "Pass sourcesTransferId only when replace is sources or both."
    );
    anyhow::ensure!(
        !a.get("artifactTransferId").is_some() || sources,
        "Pass artifactTransferId only when replace is sources or both."
    );
    let package: Option<crate::package::InitialPackage> = a
        .get("package")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?;
    let confirmation = a["confirmSchema"].as_str();
    let project = deployment.project_id.clone();
    let _guard = database_lock(state, &format!("project:{project}")).await;
    let _release_guard =
        kernel::named_lock(state, &format!("publish:{}", deployment.id), true).await;
    let (mut metadata, revision) = mcp::owned_project(state, owner, &project, "publish")?;
    if let Some(package) = &package {
        crate::package::verify(state, principal, &project, package).await?;
    }
    let release = match a["transferId"].as_str() {
        Some(transfer_id) => {
            let upload =
                transfers::transfer(state, principal, &json!({"transferId": transfer_id}))?;
            anyhow::ensure!(
                upload["complete"] == true
                    && upload["direction"] == "upload"
                    && upload["projectId"] == project.as_str(),
                "Completed upload for this project required."
            );
            let mut payload: Value =
                serde_json::from_slice(&release_bytes(state, &upload, transfer_id).await?)?;
            let input = release_input(
                state,
                principal,
                transfer_id,
                a["sourcesTransferId"].as_str(),
                a["artifactTransferId"].as_str(),
            )
            .await?;
            payload["project"]["files"] = json!(&input.project.files);
            payload["project"]["encodings"] = json!(&input.project.encodings);
            payload["artifact"] = json!({
                "version": input.artifact.version,
                "entrypoint": input.artifact.entrypoint,
                "files": input.artifact.files,
                "encodings": input.artifact.encodings,
            });
            anyhow::ensure!(
                input.project.id == project,
                "The release belongs to another project."
            );
            let against = match &package {
                Some(package) => crate::package::component_paths(state, &project, package)?
                    .into_iter()
                    .find(|(name, _)| name == "database")
                    .map(|(_, path)| path),
                None => None,
            };
            let change =
                kernel::deployments::validate_import_release(state, &input, against.as_deref()).await?;
            anyhow::ensure!(
                input.initial_package.is_none()
                    && input.initial_signup_policy.is_none()
                    && input.initial_sign_in_methods.is_none()
                    && input.initial_modules.is_none(),
                "An import into an existing app keeps its Signup Policy and sign-in methods, and takes its data as package."
            );
            if change
                .confirmation()
                .is_some_and(|expected| confirmation != Some(expected.as_str()))
            {
                return Ok(Ok(
                    json!({"imported": false, "replace": replace, "schema": change.to_json()}),
                ));
            }
            Some((payload, change))
        }
        None => None,
    };

    let resolved_contact_email = match &release {
        Some((payload, _)) => {
            Some(contact_email(state, owner, &project, payload["contactEmail"].as_str()).await?)
        }
        None => None,
    };
    let (backup, change) = match &package {
        Some(package) => {
            let target = match &release {
                Some((payload, _)) => {
                    Target::release(&serde_json::from_value::<DeployInput>(payload.clone())?)?
                }
                None => Target::live(state, &deployment)?,
            };
            match crate::import::import_data(
                state,
                &deployment,
                package,
                &target,
                confirmation,
                sources,
            )
            .await?
            {
                Outcome::NeedsConfirmation(change) => {
                    return Ok(Ok(
                        json!({"imported": false, "replace": replace, "schema": change.to_json()}),
                    ));
                }
                Outcome::Imported { backup, change } => (backup, change),
            }
        }
        None => (
            crate::import::backup_sources(state, &deployment).await?,
            release
                .as_ref()
                .map(|(_, change)| change.clone())
                .unwrap_or_default(),
        ),
    };
    let mut answer =
        json!({"imported": true, "replace": replace, "backup": backup, "schema": change.to_json()});
    if let Some((mut payload, _)) = release {
        payload["contactEmail"] = resolved_contact_email
            .flatten()
            .map_or(Value::Null, Value::String);
        payload["schemaConfirmation"] =
            confirmation.map_or(Value::Null, |value| Value::String(value.to_owned()));
        for key in ["name", "slug", "manifest"] {
            if let Some(value) = payload["project"].get(key) {
                metadata[key] = value.clone();
            }
        }
        let receipt = match response_value(
            kernel::deploy_import_held(
                state,
                kernel::functions::parse_deploy_input(payload.clone())?,
            )
            .await,
        )
        .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                return restore_import_after_release_failure(
                    state,
                    &deployment,
                    package.is_some(),
                    &backup,
                    error,
                )
                .await;
            }
        };
        mcp::store(state)?.execute(
            "UPDATE projects SET revision=revision+1, sources=sources+1, body=?3 WHERE id=?1 AND revision=?2",
            params![project, revision, metadata.to_string()],
        )?;
        answer["deployment"] = receipt;
    }
    Ok(Ok(answer))
}

pub(crate) async fn restore_import_after_release_failure(
    state: &Konae,
    deployment: &Deployment,
    has_package: bool,
    backup: &Value,
    error: anyhow::Error,
) -> Result<Result<Value>> {
    if !has_package {
        return Err(error);
    }
    let id = backup["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("The import backup has no id."))?;
    let path = crate::layout::backups(&state.data_root, &deployment.id).join(format!("{id}.json"));
    let record: crate::DatabaseBackup = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    let _data = database_lock(state, &deployment.id).await;
    crate::import::restore_held(&state.kernel, deployment, &record)
        .await
        .map_err(|restore| {
            anyhow::anyhow!("Import failed: {error:#}. Restoring its backup failed: {restore:#}")
        })?;
    Ok(Err(error))
}

async fn contact_email(
    state: &Konae,
    owner: &Owner,
    project: &str,
    given: Option<&str>,
) -> Result<Option<String>> {
    let given = given
        .map(|given| {
            crate::auth::normalize_email(given)
                .map_err(|_| anyhow::anyhow!(crate::notify::CONTACT_EMAIL_INVALID))
        })
        .transpose()?;
    let deployment = find_project_deployment(&*state.deployments.read().await, project);
    if deployment.is_none() && given.is_some() {
        return Ok(given);
    }
    let kept = deployment.and_then(|deployment| deployment.contact_email);
    Ok(kept.or(given).or_else(|| owner_email(state, &owner.id)))
}

fn owner_email(state: &Konae, owner: &str) -> Option<String> {
    let db = Connection::open_with_flags(
        state.data_root.join("owners.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    db.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    db.query_row(
        "SELECT email FROM app_users WHERE id = ?1 AND state = 'active'",
        [owner],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}
