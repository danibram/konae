//! Encrypted v3 points name immutable objects directly within one repository.
mod keys;
mod prune;
mod reservation;
use reservation::{Reader, Reservation};
mod store;

use crate::{
    copy_id, safe_relative_path, CopyKey, Manifest, ManifestFile, PathScope, Source,
    MAX_MANIFEST_BYTES,
};
use anyhow::{ensure, Context, Result};
use keys::Keys;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, Write},
    path::Path,
};

pub const DEFAULT_PAGES: u32 = 16;
const ASSET_BLOCK: usize = 1024 * 1024;

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Point {
    version: u32,
    repository: String,
    copy_id: String,
    created_at: u64,
    pages_per_object: u32,
    files: Vec<Entry>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Entry {
    path: String,
    size: u64,
    sha256: String,
    page_size: Option<u32>,
    objects: Vec<String>,
}

fn hex_lower(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn valid_repository(id: &str) -> bool {
    hex_lower(id, 32)
}

fn page_size(header: &[u8]) -> Result<u32> {
    ensure!(
        header.len() >= 100 && header.starts_with(b"SQLite format 3\0"),
        "Invalid SQLite header."
    );
    let encoded = u16::from_be_bytes([header[16], header[17]]);
    let size = if encoded == 1 {
        65536
    } else {
        u32::from(encoded)
    };
    ensure!(
        size.is_power_of_two() && (512..=65536).contains(&size),
        "Invalid SQLite page size."
    );
    Ok(size)
}

fn block_size(page: Option<u32>, pages: u32) -> usize {
    page.map_or(ASSET_BLOCK, |size| (size * pages) as usize)
}

pub struct VerifiedPoint<'a> {
    source: &'a Source,
    reader: Reader,
    point: Point,
    keys: Keys,
}

impl VerifiedPoint<'_> {
    /// The collector must retain each address while this point is reachable.
    pub fn objects(&self) -> impl Iterator<Item = &str> {
        self.point
            .files
            .iter()
            .flat_map(|f| f.objects.iter().map(String::as_str))
    }

    fn object_sizes(&self) -> impl Iterator<Item = (&str, u64)> {
        let sizes = self.point.files.iter().flat_map(|file| {
            let stride = block_size(file.page_size, self.point.pages_per_object) as u64;
            (0..file.objects.len() as u64).map(move |i| (file.size - i * stride).min(stride) + 16)
        });
        self.objects().zip(sizes)
    }

    /// Wait for the backend to release this reader before closing its runtime.
    pub async fn close(mut self) -> Result<()> {
        if let Some(guard) = self.reader.0.take() {
            guard.release().await?;
        }
        Ok(())
    }

    pub fn inventory(&self) -> Manifest {
        Manifest {
            version: 3,
            copy_id: self.point.copy_id.clone(),
            created_at: self.point.created_at,
            encryption: None,
            mac: None,
            files: self
                .point
                .files
                .iter()
                .map(|entry| ManifestFile {
                    path: entry.path.clone(),
                    size: entry.size,
                    sha256: entry.sha256.clone(),
                    home_copy: None,
                })
                .collect(),
        }
    }
}

impl Point {
    fn validate(&self, repository: &str, id: &str, scope: &dyn PathScope) -> Result<()> {
        ensure!(
            self.version == 3
                && self.repository == repository
                && self.copy_id == id
                && copy_id(self.created_at) == id,
            "Manifest names another format, repository or point."
        );
        check_pages(self.pages_per_object)?;
        let mut paths = HashSet::new();
        for file in &self.files {
            ensure!(
                safe_relative_path(&file.path)
                    && scope.allows(&file.path)
                    && !file.path.contains('\0')
                    && paths.insert(file.path.as_str()),
                "Invalid or duplicate backup path."
            );
            ensure!(
                hex_lower(&file.sha256, 64) && file.objects.iter().all(|s| hex_lower(s, 64)),
                "Invalid digest or object address."
            );
            ensure!(
                scope.sqlite_image(&file.path) == file.page_size.is_some(),
                "Invalid SQLite declaration."
            );
            if let Some(size) = file.page_size {
                ensure!(
                    size.is_power_of_two()
                        && (512..=65536).contains(&size)
                        && file.size > 0
                        && file.size % u64::from(size) == 0,
                    "Invalid SQLite image length or page size."
                );
            }
            let size = block_size(file.page_size, self.pages_per_object);
            ensure!(
                file.size.div_ceil(size as u64) == file.objects.len() as u64,
                "Wrong object count for {}.",
                file.path
            );
        }
        for file in &self.files {
            for (slash, _) in file.path.match_indices('/') {
                ensure!(
                    !paths.contains(&file.path[..slash]),
                    "A backup file is also a directory."
                );
            }
        }
        Ok(())
    }
}

fn check_pages(pages: u32) -> Result<()> {
    ensure!(
        (1..=64).contains(&pages),
        "Use between 1 and 64 SQLite pages per object."
    );
    Ok(())
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Publication {
    pub staged_bytes_read: u64,
    pub directory_bytes_read: u64,
    pub directory_bytes_written: u64,
    pub new_objects: usize,
    pub reused_objects: usize,
}

#[derive(Debug)]
struct UnconfirmedWrite;
impl std::fmt::Display for UnconfirmedWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The destination did not confirm the v3 point write.")
    }
}

pub struct Repository<'a> {
    source: &'a Source,
    identity: &'a str,
    keys: Keys,
    previous: Vec<Keys>,
}

impl<'a> Repository<'a> {
    pub fn new(source: &'a Source, identity: &'a str, key: &CopyKey) -> Result<Self> {
        ensure!(
            valid_repository(identity),
            "Repository ID must be 32 lowercase hexadecimal digits."
        );
        Ok(Self {
            source,
            identity,
            keys: Keys::new(key, identity),
            previous: Vec::new(),
        })
    }

    pub fn with_previous(mut self, keys: &[CopyKey]) -> Self {
        self.previous = keys
            .iter()
            .map(|key| Keys::new(key, self.identity))
            .collect();
        self
    }

    pub async fn points(&self) -> Result<Vec<String>> {
        Ok(store::roots(self.source).await?.points)
    }

    pub async fn open(&self, id: &str, scope: &dyn PathScope) -> Result<VerifiedPoint<'a>> {
        self.open_selected(Some(id), scope).await
    }

    pub async fn open_selected(
        &self,
        id: Option<&str>,
        scope: &dyn PathScope,
    ) -> Result<VerifiedPoint<'a>> {
        let guard = Reservation::acquire(self.source, false).await?;
        let read = async {
            match id {
                Some(id) => self.read_point(id, scope).await,
                None => {
                    let id = self
                        .points()
                        .await?
                        .pop()
                        .context("No complete v3 point.")?;
                    self.read_point(&id, scope).await
                }
            }
        }
        .await;
        match read {
            Ok(mut point) => {
                point.reader = Reader(Some(guard));
                Ok(point)
            }
            Err(e) => {
                guard.release().await?;
                Err(e)
            }
        }
    }

    /// The caller supplies an immutable snapshot or holds a repository reservation.
    async fn read_point(&self, id: &str, scope: &dyn PathScope) -> Result<VerifiedPoint<'a>> {
        ensure!(crate::parse_copy_id(id).is_some(), "Invalid point ID.");
        let bytes = store::read(self.source, "points", id, MAX_MANIFEST_BYTES as usize).await?;
        self.decode(id, &bytes, scope)
    }

    fn decode(&self, id: &str, bytes: &[u8], scope: &dyn PathScope) -> Result<VerifiedPoint<'a>> {
        ensure!(
            bytes.len() as u64 <= MAX_MANIFEST_BYTES,
            "Sealed v3 manifest exceeds 4 MiB."
        );
        let (keys, plain) = std::iter::once(&self.keys)
            .chain(&self.previous)
            .find_map(|keys| {
                keys.open_point(id, bytes)
                    .ok()
                    .map(|plain| (keys.clone(), plain))
            })
            .context("No supplied v3 key authenticates this manifest.")?;
        let point: Point = serde_json::from_slice(&plain)?;
        point.validate(self.identity, id, scope)?;
        Ok(VerifiedPoint {
            source: self.source,
            point,
            keys,
            reader: Reader(None),
        })
    }

    pub async fn publish(
        &self,
        staging: &Path,
        manifest: &Manifest,
        scope: &dyn PathScope,
        pages: u32,
        legacy_blobs: &HashMap<String, String>,
    ) -> Result<Publication> {
        match self.source {
            Source::Directory(root) => fs::create_dir_all(root)?,
            #[cfg(feature = "s3")]
            Source::S3 { .. } => {}
        }
        let guard = Reservation::acquire(self.source, false).await?;
        let result = self
            .write_point(staging, manifest, scope, pages, legacy_blobs)
            .await;
        // A timed-out manifest PUT can still commit after the client returns.
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.is::<UnconfirmedWrite>())
        {
            return result;
        }
        guard.release().await?;
        result
    }

    async fn write_point(
        &self,
        staging: &Path,
        manifest: &Manifest,
        scope: &dyn PathScope,
        pages: u32,
        legacy_blobs: &HashMap<String, String>,
    ) -> Result<Publication> {
        check_pages(pages)?;
        crate::check_manifest_paths(scope, &manifest.copy_id, manifest)?;
        ensure!(
            crate::parse_copy_id(&manifest.copy_id) == Some(manifest.created_at),
            "Invalid cut timestamp."
        );
        let id = &manifest.copy_id;
        if !legacy_blobs.is_empty()
            && let Some(previous) = self.points().await?.last()
        {
            let point = self.read_point(previous, scope).await?;
            crate::writer::check_legacy_blobs(&point.inventory(), legacy_blobs)?;
        }
        let mut point = Point {
            version: 3,
            repository: self.identity.into(),
            copy_id: id.clone(),
            created_at: manifest.created_at,
            pages_per_object: pages,
            files: Vec::new(),
        };
        let mut stats = Publication::default();
        let mut seen = HashSet::new();
        let directory = matches!(self.source, Source::Directory(_));
        let mut references = 0u64;
        for file in &manifest.files {
            let mut input = fs::File::open(staging.join(&file.path))?;
            let page = if scope.sqlite_image(&file.path) {
                let mut header = [0; 100];
                input.read_exact(&mut header)?;
                stats.staged_bytes_read += 100;
                input.rewind()?;
                Some(page_size(&header)?)
            } else {
                None
            };
            let stride = block_size(page, pages);
            references = references
                .checked_add(file.size.div_ceil(stride as u64))
                .context("Object count overflow")?;
            ensure!(
                references <= MAX_MANIFEST_BYTES / 66,
                "Object references exceed the manifest limit."
            );
            let mut entry = Entry {
                path: file.path.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
                page_size: page,
                objects: Vec::new(),
            };
            let mut digest = Sha256::new();
            let mut remaining = file.size;
            while remaining > 0 {
                let n = remaining.min(stride as u64) as usize;
                let mut bytes = vec![0; n];
                input.read_exact(&mut bytes)?;
                stats.staged_bytes_read += n as u64;
                remaining -= n as u64;
                digest.update(&bytes);
                let address = self.keys.address(&bytes);
                if seen.insert(address.clone()) {
                    let sealed = self.keys.seal_block(&address, bytes)?;
                    let length = sealed.len() as u64;
                    if store::matches(self.source, &address, &sealed).await? {
                        stats.reused_objects += 1;
                        if directory {
                            stats.directory_bytes_read += length;
                        }
                    } else if store::put(self.source, "objects", &address, &sealed).await? {
                        stats.new_objects += 1;
                        if directory {
                            stats.directory_bytes_written += length;
                            ensure!(
                                store::matches(self.source, &address, &sealed).await?,
                                "Published block disappeared."
                            );
                            stats.directory_bytes_read += length;
                        }
                    } else {
                        ensure!(
                            store::matches(self.source, &address, &sealed).await?,
                            "Concurrent block publication disappeared."
                        );
                        stats.reused_objects += 1;
                        if directory {
                            stats.directory_bytes_read += length;
                        }
                    }
                }
                entry.objects.push(address);
            }
            ensure!(
                input.read(&mut [0])? == 0 && hex::encode(digest.finalize()) == file.sha256,
                "Staged file {} changed.",
                file.path
            );
            point.files.push(entry);
        }
        #[cfg(test)]
        tests::checkpoint("publish");
        let bytes = self.keys.seal_point(id, serde_json::to_vec(&point)?)?;
        self.decode(id, &bytes, scope)?;
        if store::put(self.source, "points", id, &bytes)
            .await
            .context(UnconfirmedWrite)?
        {
            if directory {
                stats.directory_bytes_written += bytes.len() as u64;
            }
        } else {
            ensure!(
                self.read_point(id, scope).await?.point == point,
                "A different complete point already owns {id}."
            );
            if directory {
                stats.directory_bytes_read += bytes.len() as u64;
            }
        }
        Ok(stats)
    }

    /// Creates a private staging tree. The caller controls activation.
    pub async fn restore(&self, point: &VerifiedPoint<'_>, staging: &Path) -> Result<()> {
        ensure!(
            point.point.repository == self.identity && std::ptr::eq(point.source, self.source),
            "Point belongs to another repository."
        );
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(staging)?;
        #[cfg(test)]
        tests::checkpoint("restore");
        let result = self.write_files(&point.point, &point.keys, staging).await;
        if result.is_err() {
            fs::remove_dir_all(staging)?;
        }
        result
    }

    async fn write_files(&self, point: &Point, keys: &Keys, staging: &Path) -> Result<()> {
        for file in &point.files {
            let path = staging.join(&file.path);
            fs::create_dir_all(path.parent().context("Missing parent directory")?)?;
            let mut out = fs::File::create_new(path)?;
            let mut digest = Sha256::new();
            let mut remaining = file.size;
            for id in &file.objects {
                let n = remaining.min(block_size(file.page_size, point.pages_per_object) as u64)
                    as usize;
                let sealed = store::read(self.source, "objects", id, n + 16).await?;
                ensure!(sealed.len() == n + 16, "Invalid block length.");
                let plain = keys.open_block(id, sealed)?;
                if remaining == file.size
                    && let Some(size) = file.page_size
                {
                    ensure!(
                        page_size(&plain)? == size,
                        "SQLite page size differs from its manifest."
                    );
                }
                out.write_all(&plain)?;
                remaining -= n as u64;
                digest.update(plain);
            }
            ensure!(
                remaining == 0 && hex::encode(digest.finalize()) == file.sha256,
                "Reconstructed file {} differs from the manifest.",
                file.path
            );
        }
        Ok(())
    }
}

/// Appends a named record next to the points. A retry must supply the same bytes.
pub async fn append_record(
    source: &Source,
    collection: &str,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    for part in [collection, name] {
        ensure!(
            safe_relative_path(part) && !part.contains('/'),
            "Invalid record name."
        );
    }
    ensure!(
        !matches!(collection, "objects" | "points" | "collectors" | "users"),
        "Reserved v3 collection name."
    );
    if !store::put(source, collection, name, bytes).await? {
        ensure!(
            store::read(source, collection, name, bytes.len()).await? == bytes,
            "A different record already owns {name}."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
