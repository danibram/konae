use crate::{CopyKey, ENCRYPTION, MANIFEST, Manifest, ManifestCache, ManifestFile, PathScope, Source,
    check_sealed, encode_manifest, expired_copies, normalize_prefix, parse_copy_id, safe_relative_path, sealed_manifest};
use crate::reader::{manifest_cache_key, read_manifest_cached};
use crate::writer::check_legacy_blobs;
use anyhow::{Context, Result, ensure};
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use ring::aead::NONCE_LEN;
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, path::Path};

pub struct S3CopyTarget<'a> {
    pub client: &'a S3Client,
    pub bucket: &'a str,
    pub prefix: &'a str,
    pub key: Option<&'a CopyKey>,
}
pub async fn send_to_s3(
    target: S3CopyTarget<'_>,
    staging: &Path,
    manifest: &Manifest,
    id: &str,
    cache: &mut ManifestCache,
    legacy: &HashMap<String, String>,
    scope: &dyn PathScope,
) -> Result<()> {
    let S3CopyTarget { client: s3, bucket, prefix, key } = target;
    let source = Source::S3 { client: s3.clone(), bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) };
    let complete = source.complete_copies().await?;
    ensure!(!complete.iter().any(|copy| copy == id), "S3 already holds complete copy {id}.");
    let previous_id = complete.into_iter().filter(|copy| copy.as_str() < id).max();
    let previous = match previous_id.as_deref() {
        Some(copy) => read_manifest_cached(&source, copy, cache).await?.ok(),
        None => None,
    };
    let previous = previous.filter(|manifest| match key {
        Some(key) => manifest.encryption.as_ref().is_some_and(|encryption| {
            encryption.algorithm == ENCRYPTION && encryption.key_check == key.fingerprint()
        }) && check_sealed(&manifest.copy_id, manifest, Some(key)).is_ok(),
        None => manifest.encryption.is_none(),
    });
    if let Some(previous) = &previous { check_legacy_blobs(previous, legacy)?; }
    let previous_files: HashMap<&str, &ManifestFile> = previous.as_ref().map(|manifest| {
        manifest.files.iter().map(|file| (file.path.as_str(), file)).collect()
    }).unwrap_or_default();
    let mut next = manifest.clone();
    next.version = 2;
    for file in &mut next.files {
        let home = previous_files.get(file.path.as_str())
            .filter(|old| old.size == file.size && old.sha256 == file.sha256)
            .and_then(|old| old.home_copy.as_deref().or(previous_id.as_deref()))
            .unwrap_or(id);
        file.home_copy = Some(home.to_owned());
    }
    let stored = match key {
        Some(key) => sealed_manifest(&next, key),
        None => next,
    };
    let manifest_bytes = encode_manifest(scope, id, &stored)?;
    let mut uploads = tokio::task::JoinSet::new();
    for file in stored.files.iter().filter(|file| file.home_copy.as_deref() == Some(id)) {
        if uploads.len() >= 8 && let Some(result) = uploads.join_next().await {
            result??;
        }
        let mut bytes = tokio::fs::read(staging.join(&file.path)).await?;
        if let Some(key) = key {
            bytes = key.seal(id, &file.path, bytes)?;
        }
        let request = s3
            .put_object()
            .bucket(bucket)
            .key(format!("{prefix}{id}/{}", file.path))
            .body(ByteStream::from(bytes));
        uploads.spawn(async move { request.send().await.map(|_| ()).map_err(anyhow::Error::from) });
    }
    while let Some(result) = uploads.join_next().await {
        result??;
    }
    for file in &stored.files {
        let home = file.home_copy.as_deref().unwrap_or(id);
        verify_s3_file(&target, home, file).await
            .with_context(|| format!("Could not verify {} in S3 copy {home}", file.path))?;
    }
    s3.put_object()
        .bucket(bucket)
        .key(format!("{prefix}{id}/{MANIFEST}"))
        .body(ByteStream::from(manifest_bytes))
        .send()
        .await?;
    cache.insert(manifest_cache_key(&source, id), Ok(stored));
    Ok(())
}

async fn verify_s3_file(target: &S3CopyTarget<'_>, home: &str, file: &ManifestFile) -> Result<()> {
    let object = format!("{}{home}/{}", target.prefix, file.path);
    let response = target.client.get_object().bucket(target.bucket).key(object).send().await?;
    let overhead = if target.key.is_some() { (NONCE_LEN + 16) as u64 } else { 0 };
    let expected = file.size.checked_add(overhead).context("Object size overflow")?;
    if let Some(length) = response.content_length() {
        ensure!(u64::try_from(length)? == expected, "S3 reports {length} bytes. The manifest requires {expected}.");
    }
    let mut body = response.body;
    let mut length = 0u64;
    let mut sealed = Vec::new();
    let mut hash = Sha256::new();
    while let Some(chunk) = body.try_next().await? {
        length += chunk.len() as u64;
        ensure!(length <= expected, "S3 returned more than {expected} bytes.");
        if target.key.is_some() { sealed.extend_from_slice(&chunk); } else { hash.update(&chunk); }
    }
    ensure!(length == expected, "S3 returned {length} bytes. The manifest requires {expected}.");
    if let Some(key) = target.key {
        let plain = key.open(home, &file.path, &sealed).context("The stored bytes do not decrypt.")?;
        hash.update(plain);
    }
    ensure!(hex::encode(hash.finalize()) == file.sha256, "The stored bytes do not match the manifest SHA-256.");
    Ok(())
}
pub async fn prune_s3_cached(s3: &S3Client, bucket: &str, prefix: &str, current: &str, now: u64, cache: &mut ManifestCache) -> Result<usize> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for key in list_s3_keys(s3, bucket, prefix).await? {
        let Some((id, _)) = key[prefix.len()..].split_once('/') else { continue };
        if parse_copy_id(id).is_some() {
            groups.entry(id.to_owned()).or_default().push(key);
        }
    }
    let manifest_suffix = format!("/{MANIFEST}");
    let complete = groups
        .iter()
        .filter(|(_, keys)| keys.iter().any(|key| key.ends_with(&manifest_suffix)))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let expired = expired_copies(&complete, now);
    let mut keep = complete.iter().filter(|id| !expired.contains(id)).cloned().collect::<HashSet<_>>();
    keep.insert(current.to_owned());
    let mut marked = HashSet::new();
    for id in &keep {
        let source = Source::S3 { client: s3.clone(), bucket: bucket.to_owned(), prefix: normalize_prefix(prefix) };
        let manifest = read_manifest_cached(&source, id, cache).await?
            .map_err(|reason| anyhow::anyhow!("Cannot prune copy {id} because its manifest is invalid: {reason}"))?;
        ensure!(manifest.copy_id == *id, "Cannot prune copy {id} because its manifest names another copy.");
        for file in manifest.files {
            let home = file.home_copy.as_deref().unwrap_or(id);
            ensure!(safe_relative_path(&file.path) && parse_copy_id(home).is_some(), "Cannot prune copy {id} because it has an invalid object reference.");
            marked.insert(format!("{prefix}{home}/{}", file.path));
        }
    }
    for id in &expired {
        let key = format!("{prefix}{id}/{MANIFEST}");
        s3.delete_object().bucket(bucket).key(key).send().await?;
    }
    for (id, keys) in &groups {
        for key in keys {
            if key.ends_with(&manifest_suffix) {
                if !keep.contains(id) && !expired.contains(id) {
                    s3.delete_object().bucket(bucket).key(key).send().await?;
                }
            } else if !marked.contains(key) {
                s3.delete_object().bucket(bucket).key(key).send().await?;
            }
        }
    }
    let cache_prefix = format!("{bucket}/{}", normalize_prefix(prefix));
    cache.retain(|key, _| {
        let Some(id) = key.strip_prefix(&cache_prefix) else { return true };
        parse_copy_id(id).is_none() || keep.contains(id)
    });
    Ok(expired.len())
}

pub async fn list_s3_keys(s3: &S3Client, bucket: &str, prefix: &str) -> Result<Vec<String>> {
    let objects = list_s3_objects(s3, bucket, prefix).await?;
    Ok(objects.into_iter().map(|o| o.key.unwrap()).collect())
}

pub(crate) async fn list_s3_objects(s3: &S3Client, bucket: &str, prefix: &str) -> Result<Vec<aws_sdk_s3::types::Object>> {
    let mut objects = Vec::new();
    let mut tokens = HashSet::new();
    let mut names = HashSet::new();
    let mut continuation = None;
    loop {
        let page = s3.list_objects_v2().bucket(bucket).prefix(prefix)
            .set_continuation_token(continuation.take()).send().await
            .with_context(|| format!("Listing s3://{bucket}/{prefix} failed"))?;
        ensure!(page.key_count() == Some(i32::try_from(page.contents().len())?),
            "Incomplete S3 list page for {prefix}.");
        for object in page.contents() {
            let name = object.key().context("S3 listed an object without a key.")?;
            ensure!(name.starts_with(prefix) && names.insert(name.to_owned()),
                "S3 listed a duplicate key or a key outside {prefix}.");
        }
        match (page.is_truncated(), page.next_continuation_token()) {
            (Some(false), None) => continuation = None,
            (Some(true), Some(token)) if !token.is_empty() && tokens.insert(token.to_owned()) => {
                continuation = Some(token.to_owned())
            }
            _ => anyhow::bail!("Incomplete S3 pagination for {prefix}."),
        }
        objects.extend(page.contents.unwrap_or_default());
        if continuation.is_none() { return Ok(objects); }
    }
}

pub async fn read_s3_bytes(s3: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let object = s3.get_object().bucket(bucket).key(key).send().await?;
    Ok(object.body.collect().await?.into_bytes().to_vec())
}
