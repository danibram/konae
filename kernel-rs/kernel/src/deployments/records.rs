use crate::*;
use super::settings::released_settings;

pub async fn persist_deployment(state: &AppState, deployment: &Deployment) -> Result<()> {
    persist_applied(state, deployment, None, None).await
}

/// Records the artifact currently in a folder without replacing its saved draft.
pub async fn persist_folder_snapshot(state: &AppState, deployment: &Deployment, artifact_digest: &str) -> Result<()> {
    persist_applied(state, deployment, Some(artifact_digest), None).await
}

pub async fn persist_folder_snapshot_with_transforms(state: &AppState, deployment: &Deployment, artifact_digest: &str, schema_transforms: Option<&str>) -> Result<()> {
    persist_applied(state, deployment, Some(artifact_digest), Some(schema_transforms)).await
}

async fn persist_applied(state: &AppState, deployment: &Deployment, artifact_digest: Option<&str>, schema_transforms: Option<Option<&str>>) -> Result<()> {
    let mut settings = match Settings::read(&state.data_root, &deployment.id) {
        Ok(settings) => settings,
        Err(_) if !layout::settings(&state.data_root, &deployment.id).exists() => Settings::of(deployment),
        Err(error) => return Err(error),
    };
    let record_path = layout::record(&state.data_root, &deployment.id);
    let installed = match tokio::fs::read(&record_path).await {
        Ok(bytes) => serde_json::from_slice::<Installed>(&bytes).context("The installed Deployment record is malformed.")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Installed {
            hostname: deployment.hostname.clone(), created_at: deployment.created_at,
            released_at: Some(deployment.released_at), source_digest: deployment.source_digest.clone(),
            actions: deployment.actions.clone(), artifact_version: deployment.artifact_version.clone(), released: None,
        },
        Err(error) => return Err(error.into()),
    };
    let recovered = crate::sources::capture_released_state(&state.data_root, &deployment.id, &settings, &installed)?;
    settings.signup_policy = deployment.signup_policy.clone();
    settings.sign_in_methods = deployment.sign_in_methods.clone();
    settings.contact_email = deployment.contact_email.clone();
    settings.manifest = crate::sources::manifest_without_runtime(&settings.manifest);
    let mut released = released_settings(deployment);
    if released.schema_sql.trim().is_empty() { released.schema_sql = recovered.schema_sql; }
    if let Some(schema_transforms) = schema_transforms {
        released.schema_transforms = schema_transforms.map(str::to_owned);
    } else if released.schema_transforms.is_none() {
        released.schema_transforms = recovered.schema_transforms;
    }
    if released.source_digest.is_none() { released.source_digest = recovered.source_digest; }
    released.artifact_digest = artifact_digest.map(str::to_owned).or(recovered.artifact_digest);
    settings.released = Some(released.clone());
    persist_settings_and_record(state, deployment, settings, released).await
}

/// Persists the new source and artifact state after a successful release.
pub async fn persist_release(state: &AppState, deployment: &Deployment, artifact_digest: &str, directories: &[String], schema_transforms: Option<&str>, initial_modules: Option<&std::collections::BTreeMap<String, Value>>) -> Result<()> {
    let mut settings = Settings::of(deployment);
    modules::preserve_release_state(&mut settings, Settings::read(&state.data_root, &deployment.id).ok(), initial_modules);
    settings.directories = directories.to_vec();
    let mut released = settings.released.clone().expect("Settings::of records released settings");
    released.artifact_digest = Some(artifact_digest.to_owned());
    released.schema_transforms = schema_transforms.map(str::to_owned);
    settings.released = Some(released.clone());
    persist_settings_and_record(state, deployment, settings, released).await
}

pub(crate) async fn persist_settings_and_record(state: &AppState, deployment: &Deployment, settings: Settings, released: ReleasedSettings) -> Result<()> {
    let settings_path = layout::settings(&state.data_root, &deployment.id);
    let record_path = layout::record(&state.data_root, &deployment.id);
    let settings_before = match tokio::fs::read(&settings_path).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let record_before = match tokio::fs::read(&record_path).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let settings_bytes = settings.folder_bytes()?;
    let installed = Installed {
        hostname: deployment.hostname.clone(), created_at: deployment.created_at,
        released_at: Some(deployment.released_at), source_digest: deployment.source_digest.clone(),
        actions: deployment.actions.clone(), artifact_version: deployment.artifact_version.clone(), released: Some(released),
    };
    let record_bytes = serde_json::to_vec_pretty(&installed)?;
    tokio::fs::create_dir_all(layout::private(&state.data_root, &deployment.id)).await?;
    write_atomic(&settings_path, &settings_bytes).await?;
    if let Err(error) = write_atomic(&record_path, &record_bytes).await {
        let settings_restore = restore_optional(&settings_path, settings_before.as_deref()).await;
        let record_restore = restore_optional(&record_path, record_before.as_deref()).await;
        if let Err(restore) = settings_restore { anyhow::bail!("Writing the Deployment record failed: {error}. Restoring konae.json failed: {restore:#}"); }
        if let Err(restore) = record_restore { anyhow::bail!("Writing the Deployment record failed: {error}. Restoring the previous record failed: {restore:#}"); }
        return Err(error.into());
    }
    Ok(())
}

async fn restore_optional(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => write_atomic(path, bytes).await?,
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

/// Writes the Deployment's two files without a runtime, for tests that seed a volume.
#[cfg(any(test, feature = "testing"))]
pub fn write_record_for_test(root: &Path, deployment: &Deployment) {
    let installed = Installed {
        hostname: deployment.hostname.clone(),
        created_at: deployment.created_at,
        released_at: Some(deployment.released_at),
        source_digest: deployment.source_digest.clone(),
        actions: deployment.actions.clone(),
        artifact_version: deployment.artifact_version.clone(),
        released: Some(released_settings(deployment)),
    };
    std::fs::create_dir_all(layout::private(root, &deployment.id)).unwrap();
    std::fs::write(layout::settings(root, &deployment.id), Settings::of(deployment).folder_bytes().unwrap()).unwrap();
    std::fs::write(layout::record(root, &deployment.id), serde_json::to_vec(&installed).unwrap()).unwrap();
}

/// Reads every published app on the volume. A folder without a Deployment
/// record is an app that was never published. A Kernel on a fresh volume
/// starts empty; the `restore` command installs copied folders first.
pub async fn load_deployments(root: &Path, public_origin: &str) -> Result<HashMap<String, Deployment>> {
    let mut result = HashMap::new();
    let mut entries = tokio::fs::read_dir(layout::apps(root)).await?;
    while let Some(entry) = entries.next_entry().await? {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(bytes) = tokio::fs::read(layout::record(root, &id)).await else { continue };
        let installed: Installed = serde_json::from_slice(&bytes).with_context(|| format!("{} is not a Deployment record.", layout::record(root, &id).display()))?;
        if let Err(error) = crate::folder::decide_app_modules(root, &id, crate::folder::ModuleSource::Installed).await {
            eprintln!("konae: skipping apps/{id}: {error:#}");
            continue;
        }
        let settings = match Settings::read(root, &id) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("konae: skipping apps/{id}: {} is not readable: {error:#}", layout::settings(root, &id).display());
                continue;
            }
        };
        let deployment = settings.deployment(installed, public_origin);
        result.insert(deployment.hostname.clone(), deployment);
    }
    Ok(result)
}

pub(crate) struct Recorded(Vec<(PathBuf, Option<Vec<u8>>)>);

impl Recorded {
    pub(crate) async fn read(root: &Path, id: &str) -> Self {
        let mut files = Vec::new();
        for path in [layout::settings(root, id), layout::record(root, id)] {
            let bytes = tokio::fs::read(&path).await.ok();
            files.push((path, bytes));
        }
        Self(files)
    }

    pub(crate) async fn put_back(&self) {
        for (path, bytes) in &self.0 {
            let restored = match bytes {
                Some(bytes) => write_atomic(path, bytes).await,
                None => tokio::fs::remove_file(path).await,
            };
            if let Err(error_value) = restored {
                eprintln!("Could not put back {} after a failed release: {error_value}", path.display());
            }
        }
    }
}
