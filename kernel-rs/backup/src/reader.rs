use crate::{CopyKey, ENCRYPTION, MANIFEST, MAX_MANIFEST_BYTES, MAX_SKIPPED_COPIES, Manifest, ManifestFile,
    PathScope, check_manifest_paths, parse_copy_id, safe_relative_path};
use anyhow::{Context, Result};
use std::path::Path;
#[cfg(feature = "s3")]
use crate::{ManifestCache, list_s3_keys, read_s3_bytes};
#[cfg(feature = "s3")]
use aws_sdk_s3::Client as S3Client;
use anyhow::{bail, ensure};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub enum Source {
    Directory(PathBuf),
    #[cfg(feature = "s3")]
    S3 { client: S3Client, bucket: String, prefix: String },
}

// Directory copies store regular files and hard links. Resolve each component
// inside the selected repository before opening manifest or payload bytes.
pub(crate) fn directory_file(root: &Path, copy: &str, relative: &str) -> Result<PathBuf> {
    ensure!(safe_relative_path(relative) && safe_relative_path(copy) && !copy.contains('/'), "Invalid copy path.");
    let mut path = root.join(copy);
    for name in relative.split('/') {
        ensure!(std::fs::symlink_metadata(&path)?.is_dir(), "Expected a directory in copy {copy}.");
        path.push(name);
    }
    ensure!(std::fs::symlink_metadata(&path)?.is_file(), "Expected a regular file in copy {copy}.");
    Ok(path)
}

impl Source {
    pub async fn complete_copies(&self) -> Result<Vec<String>> {
        let mut ids = match self {
            Source::Directory(root) => {
                let mut ids = Vec::new();
                for entry in std::fs::read_dir(root).with_context(|| format!("Could not read {}", root.display()))? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if parse_copy_id(&name).is_some() && entry.path().join(MANIFEST).is_file() {
                        ids.push(name);
                    }
                }
                ids
            }
            #[cfg(feature = "s3")]
            Source::S3 { client, bucket, prefix } => list_s3_keys(client, bucket, prefix)
                .await?
                .iter()
                .filter_map(|key| key[prefix.len()..].strip_suffix(&format!("/{MANIFEST}")).map(str::to_owned))
                .filter(|id| parse_copy_id(id).is_some())
                .collect(),
        };
        ids.sort();
        Ok(ids)
    }

    pub async fn read(&self, copy: &str, path: &str) -> Result<Vec<u8>> {
        match self {
            Source::Directory(root) => Ok(tokio::fs::read(directory_file(root, copy, path)?).await?),
            #[cfg(feature = "s3")]
            Source::S3 { client, bucket, prefix } => read_s3_bytes(client, bucket, &format!("{prefix}{copy}/{path}")).await,
        }
    }

    async fn read_manifest_bytes(&self, copy: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Source::Directory(root) => {
                use tokio::io::AsyncReadExt;
                let file = tokio::fs::File::open(directory_file(root, copy, MANIFEST)?).await?;
                if file.metadata().await?.len() > MAX_MANIFEST_BYTES { return Ok(None); }
                let mut bytes = Vec::new();
                file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes).await?;
                Ok((bytes.len() as u64 <= MAX_MANIFEST_BYTES).then_some(bytes))
            }
            #[cfg(feature = "s3")]
            Source::S3 { client, bucket, prefix } => {
                let object = client.get_object().bucket(bucket).key(format!("{prefix}{copy}/{MANIFEST}")).send().await?;
                if object.content_length().is_some_and(|length| length > MAX_MANIFEST_BYTES as i64) {
                    return Ok(None);
                }
                let mut body = object.body;
                let mut bytes = Vec::new();
                while let Some(chunk) = body.try_next().await? {
                    if bytes.len().saturating_add(chunk.len()) as u64 > MAX_MANIFEST_BYTES { return Ok(None); }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(Some(bytes))
            }
        }
    }
}

pub async fn read_manifest(source: &Source, copy: &str) -> Result<std::result::Result<Manifest, String>> {
    let Some(bytes) = source.read_manifest_bytes(copy).await? else {
        return Ok(Err(format!("The manifest of copy {copy} is larger than 4 MiB.")));
    };
    let manifest: Manifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(Err(format!("The manifest of copy {copy} is invalid."))),
    };
    if manifest.version != 1 && manifest.version != 2 {
        return Ok(Err(format!("The manifest version of copy {copy} is not supported.")));
    }
    Ok(Ok(manifest))
}

#[cfg(feature = "s3")]
pub fn manifest_cache_key(source: &Source, copy: &str) -> String {
    match source {
        Source::Directory(path) => format!("{}:{copy}", path.display()),
        Source::S3 { bucket, prefix, .. } => format!("{bucket}/{prefix}{copy}"),
    }
}

#[cfg(feature = "s3")]
pub(crate) async fn read_manifest_cached(source: &Source, copy: &str, cache: &mut ManifestCache) -> Result<std::result::Result<Manifest, String>> {
    let cache_key = manifest_cache_key(source, copy);
    if let Some(manifest) = cache.get(&cache_key) {
        return Ok(manifest.clone());
    }
    let manifest = read_manifest(source, copy).await?;
    cache.insert(cache_key, manifest.clone());
    Ok(manifest)
}

fn not_encrypted(copy: &str) -> String {
    format!("Copy {copy} is not encrypted, but KONAE_COPY_SECRET is set, and a copy this installation sends to S3 with the secret is always encrypted.")
}

pub fn check_sealed(copy: &str, manifest: &Manifest, key: Option<&CopyKey>) -> Result<()> {
    let Some(encryption) = &manifest.encryption else {
        return Ok(());
    };
    ensure!(encryption.algorithm == ENCRYPTION, "Copy {copy} uses {:?}, which this Kernel cannot open.", encryption.algorithm);
    let key = key.with_context(|| {
        format!("Copy {copy} is encrypted. Set KONAE_COPY_SECRET to the secret the copies were made with. Nothing was installed.")
    })?;
    ensure!(
        key.fingerprint() == encryption.key_check,
        "KONAE_COPY_SECRET does not open copy {copy}: it is not the secret the copy was encrypted with. Nothing was installed."
    );
    match &manifest.mac {
        Some(mac) => ensure!(
            key.verify_manifest(copy, manifest, mac),
            "Copy {copy} has a manifest that does not match its MAC: the file list was altered after the Kernel wrote it. Nothing was installed."
        ),
        None => bail!("Copy {copy} has no manifest MAC. The Kernel signs every manifest it encrypts, so this one was altered. Nothing was installed."),
    }
    Ok(())
}

pub async fn newest_authentic_copy(source: &Source, key: &CopyKey, report: &mut dyn FnMut(String)) -> Result<String> {
    let copies = source.complete_copies().await?;
    ensure!(!copies.is_empty(), "No complete copy found. A complete copy has a manifest.json.");
    let mut skipped = 0;
    let mut newest_reason = String::new();
    for copy in copies.iter().rev() {
        ensure!(
            skipped < MAX_SKIPPED_COPIES,
            "Stopped after {MAX_SKIPPED_COPIES} skipped copies without finding one that could be authenticated. The newest: {newest_reason} Nothing was installed."
        );
        let manifest = read_manifest(source, copy)
            .await
            .map_err(|error| anyhow::anyhow!("Could not read the manifest of copy {copy}: {error:#}. Nothing was installed."))?;
        let verdict = manifest.and_then(|manifest| {
            if manifest.encryption.is_none() {
                return Err(not_encrypted(copy));
            }
            check_sealed(copy, &manifest, Some(key)).map_err(|error| format!("{error:#}"))
        });
        match verdict {
            Ok(()) => return Ok(copy.clone()),
            Err(reason) => {
                let reason = reason.trim_end_matches(" Nothing was installed.").to_owned();
                report(format!("Skipped copy {copy}: {reason}"));
                if skipped == 0 {
                    newest_reason = reason;
                }
                skipped += 1;
            }
        }
    }
    bail!(
        "No copy in the source could be authenticated, {skipped} skipped. The newest: {newest_reason} While KONAE_COPY_SECRET is set, only a copy encrypted and signed with it is chosen automatically, and a copy that is not encrypted never is. Nothing was installed."
    )
}

pub async fn verified_bytes(source: &Source, copy: &str, manifest: &Manifest, file: &ManifestFile, key: Option<&CopyKey>) -> Result<Vec<u8>> {
    let home = file.home_copy.as_deref().unwrap_or(copy);
    let mut bytes = source.read(home, &file.path).await.with_context(|| format!("Could not read {} from copy {home}", file.path))?;
    if let (Some(_), Some(key)) = (&manifest.encryption, key) {
        bytes = key
            .open(home, &file.path, &bytes)
            .with_context(|| format!("Copy {copy} is damaged: {} does not decrypt. Nothing was installed.", file.path))?;
    }
    ensure!(
        bytes.len() as u64 == file.size && hex::encode(Sha256::digest(&bytes)) == file.sha256,
        "Copy {copy} is damaged: {} does not match its checksum. Nothing was installed.",
        file.path
    );
    Ok(bytes)
}

#[derive(Debug)]
pub struct UnencryptedS3Copy(String);

impl std::fmt::Display for UnencryptedS3Copy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&not_encrypted(&self.0))
    }
}

impl std::error::Error for UnencryptedS3Copy {}

/// Checks the manifest at open and verifies each file before returning its bytes.
pub struct CopyReader<'a> {
    source: &'a Source,
    id: String,
    manifest: Manifest,
    key: Option<&'a CopyKey>,
}

impl<'a> CopyReader<'a> {
    pub async fn open(
        source: &'a Source,
        id: &str,
        key: Option<&'a CopyKey>,
        scope: &dyn PathScope,
    ) -> Result<Self> {
        ensure!(
            !id.is_empty() && !id.contains('/') && id != "." && id != "..",
            "The copy ID is invalid."
        );
        let manifest = read_manifest(source, id)
            .await
            .with_context(|| format!("Copy {id} has no manifest.json, so it is incomplete or missing."))?
            .map_err(|reason| anyhow::anyhow!("{reason} Nothing was installed."))?;
        #[cfg(feature = "s3")]
        if matches!(source, Source::S3 { .. }) && key.is_some() && manifest.encryption.is_none() {
            return Err(UnencryptedS3Copy(id.to_owned()).into());
        }
        check_sealed(id, &manifest, key)?;
        check_manifest_paths(scope, id, &manifest)?;

        Ok(Self { source, id: id.to_owned(), manifest, key })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub async fn verify(&self) -> Result<()> {
        for file in &self.manifest.files {
            verified_bytes(self.source, &self.id, &self.manifest, file, self.key).await?;
        }
        Ok(())
    }

    /// Create a fresh directory and remove it if a file fails verification.
    pub async fn restore(&self, staging: &Path) -> Result<()> {
        tokio::fs::create_dir(staging).await?;
        let result = self.write_files(staging).await;
        if result.is_err() {
            tokio::fs::remove_dir_all(staging).await?;
        }
        result
    }

    async fn write_files(&self, staging: &Path) -> Result<()> {
        for file in &self.manifest.files {
            let bytes = verified_bytes(self.source, &self.id, &self.manifest, file, self.key).await?;
            let target = staging.join(&file.path);
            tokio::fs::create_dir_all(target.parent().expect("file has a parent")).await?;
            tokio::fs::write(target, bytes).await?;
        }
        Ok(())
    }
}
