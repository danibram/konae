use super::*;

fn pilot_scope(profile: &str) -> Installation {
    let mut scope = Installation::kernel();
    if profile == "konae" {
        for file in ["owners.sqlite", "mcp.sqlite"] {
            scope.databases.push(crate::installation::RootDatabase { file, sign_out: &[] });
        }
    }
    scope
}

/// The pilot checks these files against a stopped fixture before restore signs anyone out.
#[tokio::test]
#[ignore = "run through ops/tests/writer-pilot.py with disposable repositories"]
async fn read_pilot_cut() -> Result<()> {
    let from = std::env::var("PILOT_SOURCE")?;
    if from.starts_with("s3://") {
        let endpoint = url::Url::parse(&std::env::var("KONAE_S3_ENDPOINT")?)?;
        ensure!(endpoint.scheme() == "http" && endpoint.host_str() == Some("127.0.0.1")
            && from.starts_with("s3://konae-test/"), "Use the loopback pilot bucket.");
    }
    let source = source_for(&from).await?;
    let id = std::env::var("PILOT_COPY")?;
    let output = PathBuf::from(std::env::var("PILOT_IMAGE")?);
    ensure!(!output.exists(), "The pilot image must be new.");
    let scope = pilot_scope(&std::env::var("PILOT_BINARY")?);
    let key = CopyKey::from_env()?;
    if let Some(repository) = blocks::source_identity(&source) {
        blocks::restore(&source, repository, key.as_ref(), Some(&id), &scope, &output).await?;
    } else {
        CopyReader::open(&source, &id, key.as_ref().map(CopyKey::files), &scope)
            .await?.restore(&output).await?;
    }
    Ok(())
}

/// Replay the local cut on an empty MinIO so a failed live S3 cut cannot mask a format defect.
#[tokio::test]
#[ignore = "run through ops/tests/writer-pilot.py with disposable repositories"]
async fn replay_pilot_cut() -> Result<()> {
    let endpoint = url::Url::parse(&std::env::var("KONAE_S3_ENDPOINT")?)?;
    ensure!(endpoint.scheme() == "http" && endpoint.host_str() == Some("127.0.0.1"));
    let (client, bucket) = crate::copy_bucket_from_env()?.context("Missing MinIO")?;
    ensure!(bucket == "konae-test");
    let scope = pilot_scope(&std::env::var("PILOT_BINARY")?);
    let id = std::env::var("PILOT_COPY")?;
    let dir = PathBuf::from(std::env::var("PILOT_DIRECTORY")?);
    let local = Source::Directory(dir.clone());
    let reader = CopyReader::open(&local, &id, None, &scope).await?;
    let manifest = reader.manifest();
    let key = CopyKey::from_env()?.context("Missing pilot key")?;
    let name = std::env::var("PILOT_TARGET")?;
    let target = if name == "s3" {
        Target::S3 { prefix: S3_PREFIX.into() }
    } else {
        ensure!(konae_backup::blocks::valid_repository(&name));
        Target::BlocksS3 { prefix: format!("copies-v3/{name}/") }
    };
    recovery::start_with_key(&target, Some(&client), &bucket, Some(&key)).await?;
    let staging = dir.join(&id);
    match &target {
        Target::S3 { prefix } => {
            let target = S3CopyTarget { client: &client, bucket: &bucket, prefix, key: Some(key.files()) };
            send_to_s3(target, &staging, manifest, &id, &mut ManifestCache::new(), &HashMap::new(), &scope).await?;
        }
        Target::BlocksS3 { .. } => {
            let source = blocks::source(&target, Some(&client), &bucket)?;
            let repo = konae_backup::blocks::Repository::new(&source, &name, key.block_keys().0)?;
            repo.publish(&staging, manifest, &scope, konae_backup::blocks::DEFAULT_PAGES, &HashMap::new()).await?;
        }
        _ => unreachable!(),
    }
    Ok(())
}
