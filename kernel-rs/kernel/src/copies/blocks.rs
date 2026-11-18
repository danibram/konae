use super::{AppState, CopyKey, Installation, Manifest, Source, StagedCopy, Target};
use anyhow::{ensure, Context, Result};
use konae_backup::blocks::{valid_repository, Repository, DEFAULT_PAGES};
use std::path::Path;

pub(crate) fn identity(path: &Path) -> Option<&str> {
    if path.parent()?.file_name()? != "copies-v3" {
        return None;
    }
    path.file_name()?.to_str().filter(|id| valid_repository(id))
}

pub(crate) fn source_identity(source: &Source) -> Option<&str> {
    match source {
        Source::Directory(path) => identity(path),
        Source::S3 { prefix, .. } => identity(Path::new(prefix)),
    }
}

/// Resolve existing ancestors before checking where a new repository will live.
pub(super) fn effective(path: &Path) -> Result<std::path::PathBuf> {
    let path = std::path::absolute(path)?;
    let mut resolved = std::path::PathBuf::new();
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            resolved.pop();
            continue;
        }
        resolved.push(component);
        match resolved.symlink_metadata() {
            Ok(_) => resolved = resolved.canonicalize()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(resolved)
}

pub(super) fn targets(data: &Path, s3: bool, keyed: bool) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    let directory = crate::file_store::env_var("KONAE_COPY_V3_DIR");
    let remote = crate::file_store::env_var("KONAE_COPY_V3_S3");
    if let Some(directory) = directory {
        let path = std::path::absolute(directory)?;
        let id = identity(&path)
            .context("KONAE_COPY_V3_DIR must end in copies-v3/<32 lowercase hex digits>.")?;
        ensure!(
            Some(id) != remote.as_deref(),
            "Give each v3 repository its own ID."
        );
        let data = effective(data)?;
        let actual = effective(&path)?;
        for component in actual.components() {
            if let Some(name) = component.as_os_str().to_str() {
                ensure!(
                    !name.starts_with(".partial-") && super::parse_copy_id(name).is_none(),
                    "A legacy cleaner can remove the ancestor {name} of this v3 directory."
                );
            }
        }
        ensure!(
            identity(&actual) == Some(id),
            "The resolved v3 directory must retain its repository ID."
        );
        ensure!(
            !actual.starts_with(&data)
                && !data.starts_with(&actual)
                && !path
                    .components()
                    .any(|part| part == std::path::Component::ParentDir),
            "Choose a v3 repository outside the data directory."
        );
        targets.push(Target::BlocksDirectory(actual));
    }
    if let Some(id) = remote {
        ensure!(s3 && valid_repository(&id), "KONAE_COPY_V3_S3 requires a configured bucket and a 32-digit lowercase hex repository ID.");
        targets.push(Target::BlocksS3 {
            prefix: format!("copies-v3/{id}/"),
        });
    }
    ensure!(
        targets.is_empty() || keyed,
        "Set KONAE_COPY_SECRET for v3 copies."
    );
    Ok(targets)
}

pub(crate) fn source(
    target: &Target,
    client: Option<&aws_sdk_s3::Client>,
    bucket: &str,
) -> Result<Source> {
    Ok(match target {
        Target::BlocksDirectory(path) => Source::Directory(path.clone()),
        Target::BlocksS3 { prefix } => Source::S3 {
            client: client.cloned().context("S3 is not configured.")?,
            bucket: bucket.into(),
            prefix: prefix.clone(),
        },
        _ => anyhow::bail!("Expected a v3 target."),
    })
}

fn repository<'a>(state: &AppState, source: &'a Source) -> Result<Repository<'a>> {
    let key = state
        .copies
        .key()
        .context("Set KONAE_COPY_SECRET for v3 copies.")?;
    let (active, previous) = key.block_keys();
    Ok(Repository::new(
        source,
        source_identity(source).context("Invalid v3 repository path.")?,
        active,
    )?
    .with_previous(previous))
}

pub(super) async fn prune(state: &AppState, target: &Target, current: &str) -> Result<usize> {
    let source = source(target, state.s3.as_ref(), &state.data_bucket)?;
    repository(state, &source)?
        .prune(state.installation.as_ref(), crate::now_ms(), current)
        .await
}

pub(super) async fn publish(
    state: &AppState,
    target: &Target,
    staging: &Path,
    cut: &StagedCopy,
) -> Result<()> {
    let source = source(target, state.s3.as_ref(), &state.data_bucket)?;
    let repo = repository(state, &source)?;
    repo.publish(
        staging,
        &cut.manifest,
        state.installation.as_ref(),
        DEFAULT_PAGES,
        &cut.legacy_blobs,
    )
    .await?;
    Ok(())
}

pub(super) async fn restore(
    source: &Source,
    identity: &str,
    key: Option<&CopyKey>,
    selected: Option<&str>,
    installation: &Installation,
    staging: &Path,
) -> Result<Manifest> {
    let (active, previous) = key
        .context("Set KONAE_COPY_SECRET to open a v3 point.")?
        .block_keys();
    let repo = Repository::new(source, identity, active)?.with_previous(previous);
    let point = repo.open_selected(selected, installation).await?;
    let result = repo.restore(&point, staging).await;
    let inventory = point.inventory();
    point.close().await?;
    result.map(|()| inventory)
}
