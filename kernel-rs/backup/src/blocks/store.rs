use crate::Source;
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::Path,
};

#[cfg(feature = "s3")]
use {
    aws_sdk_s3::types::ChecksumMode,
    base64::{engine::general_purpose::STANDARD, Engine},
    sha2::{Digest, Sha256},
};

pub(super) async fn read(source: &Source, group: &str, id: &str, limit: usize) -> Result<Vec<u8>> {
    match source {
        Source::Directory(root) => {
            let path = crate::reader::directory_file(root, group, id)?;
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= limit, "Backup object exceeds {limit} bytes.");
            Ok(bytes)
        }
        #[cfg(feature = "s3")]
        Source::S3 {
            client,
            bucket,
            prefix,
        } => {
            let object = client
                .get_object()
                .bucket(bucket)
                .key(format!("{prefix}{group}/{id}"))
                .send()
                .await?;
            let mut body = object.body;
            let mut bytes = Vec::new();
            while let Some(chunk) = body.try_next().await? {
                ensure!(
                    chunk.len() <= limit - bytes.len(),
                    "Backup object exceeds {limit} bytes."
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        }
    }
}

struct Temporary(std::path::PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn directory_put(root: &Path, group: &str, id: &str, bytes: &[u8]) -> Result<bool> {
    fs::create_dir_all(root)?;
    let dir = root.join(group);
    match fs::create_dir(&dir) {
        Ok(()) => fs::File::open(root)?.sync_all()?,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    ensure!(
        dir.symlink_metadata()?.is_dir(),
        "Expected repository directory {group}."
    );
    let final_path = dir.join(id);
    if final_path.try_exists()? {
        return Ok(false);
    }
    let temporary = Temporary(dir.join(format!(".write-{}", uuid::Uuid::new_v4())));
    let mut file = fs::File::create_new(&temporary.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    match fs::hard_link(&temporary.0, final_path) {
        Ok(()) => {
            fs::File::open(dir)?.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Returns false when another publication already owns the name.
pub(super) async fn put(source: &Source, group: &str, id: &str, bytes: &[u8]) -> Result<bool> {
    match source {
        Source::Directory(root) => directory_put(root, group, id, bytes),
        #[cfg(feature = "s3")]
        Source::S3 {
            client,
            bucket,
            prefix,
        } => {
            let result = client
                .put_object()
                .bucket(bucket)
                .key(format!("{prefix}{group}/{id}"))
                .if_none_match("*")
                .checksum_sha256(STANDARD.encode(Sha256::digest(bytes)))
                .body(bytes.to_vec().into())
                .send()
                .await;
            match result {
                Ok(_) => Ok(true),
                Err(e) if e.raw_response().is_some_and(|r| r.status().as_u16() == 412) => Ok(false),
                Err(e) => Err(e.into()),
            }
        }
    }
}

pub(super) async fn matches(source: &Source, id: &str, bytes: &[u8]) -> Result<bool> {
    match source {
        Source::Directory(root) => {
            if !root.join("objects").join(id).try_exists()? {
                return Ok(false);
            }
        }
        #[cfg(feature = "s3")]
        Source::S3 {
            client,
            bucket,
            prefix,
        } => {
            let response = client
                .head_object()
                .bucket(bucket)
                .key(format!("{prefix}objects/{id}"))
                .checksum_mode(ChecksumMode::Enabled)
                .send()
                .await;
            match response {
                Ok(head) => {
                    ensure!(
                        head.content_length() == Some(bytes.len() as i64),
                        "Stored block length differs."
                    );
                    if let Some(checksum) = head.checksum_sha256() {
                        ensure!(
                            checksum == STANDARD.encode(Sha256::digest(bytes)),
                            "Stored block checksum differs."
                        );
                        return Ok(true);
                    }
                }
                Err(e) if e.raw_response().is_some_and(|r| r.status().as_u16() == 404) => {
                    return Ok(false)
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    ensure!(
        read(source, "objects", id, bytes.len()).await? == bytes,
        "Stored block differs from its address."
    );
    Ok(true)
}

pub(super) struct Stored {
    pub name: String,
    pub size: u64,
    pub modified_ms: u64,
}

pub(super) async fn list(source: &Source, group: &str) -> Result<Vec<Stored>> {
    let mut stored = Vec::new();
    match source {
        Source::Directory(root) => {
            let dir = root.join(group);
            match dir.symlink_metadata() {
                Ok(meta) => ensure!(meta.is_dir(), "Expected repository directory {group}."),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(stored),
                Err(e) => return Err(e.into()),
            }
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .ok()
                    .context("Invalid repository filename.")?;
                let meta = match entry.path().symlink_metadata() {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == ErrorKind::NotFound && temporary(source, &name) => {
                        continue
                    }
                    Err(e) => return Err(e.into()),
                };
                ensure!(meta.is_file(), "Expected a regular file in {group}.");
                stored.push(Stored {
                    name,
                    size: meta.len(),
                    modified_ms: meta
                        .modified()?
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_millis()
                        .try_into()?,
                });
            }
        }
        #[cfg(feature = "s3")]
        Source::S3 {
            client,
            bucket,
            prefix,
        } => {
            let base = format!("{prefix}{group}/");
            for object in crate::s3::list_s3_objects(client, bucket, &base).await? {
                stored.push(Stored {
                    name: object.key().context("Missing listed object key.")?[base.len()..]
                        .to_owned(),
                    size: object
                        .size()
                        .context("Missing listed object size.")?
                        .try_into()?,
                    modified_ms: object
                        .last_modified()
                        .context("Missing listed object timestamp.")?
                        .to_millis()?
                        .try_into()?,
                });
            }
        }
    }
    Ok(stored)
}

pub(super) fn temporary(source: &Source, name: &str) -> bool {
    matches!(source, Source::Directory(_))
        && name
            .strip_prefix(".write-")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

pub(super) struct Roots {
    pub points: Vec<String>,
    pub abandoned: Vec<String>,
}

pub(super) async fn roots(source: &Source) -> Result<Roots> {
    let mut points = Vec::new();
    let mut abandoned = Vec::new();
    for object in list(source, "points").await? {
        if temporary(source, &object.name) {
            abandoned.push(object.name);
        } else {
            ensure!(
                crate::parse_copy_id(&object.name).is_some(),
                "Unknown v3 point name."
            );
            points.push(object.name);
        }
    }
    points.sort();
    Ok(Roots { points, abandoned })
}

pub(super) async fn remove(source: &Source, group: &str, name: &str) -> Result<()> {
    match source {
        Source::Directory(root) => {
            fs::remove_file(crate::reader::directory_file(root, group, name)?)?;
        }
        #[cfg(feature = "s3")]
        Source::S3 {
            client,
            bucket,
            prefix,
        } => {
            client
                .delete_object()
                .bucket(bucket)
                .key(format!("{prefix}{group}/{name}"))
                .send()
                .await?;
        }
    }
    Ok(())
}

pub(super) fn sync_group(source: &Source, group: &str) -> Result<()> {
    match source {
        Source::Directory(root) => fs::File::open(root.join(group))?.sync_all()?,
        #[cfg(feature = "s3")]
        Source::S3 { .. } => {}
    }
    Ok(())
}
