//! Where the Kernel keeps the bytes of app files, and how many it accepts.
//!
//! A Deployment's file index, `deployments/<id>/files/index.json`, always
//! stays in the Deployment's folder, so listing, visibility and the quota never
//! ask the store. Only the bytes go through [`FileStore`]. [`LocalFolder`]
//! keeps them next to the index as `<file id>.blob`, which is what every
//! Kernel did before stores existed.
//!
//! The operator sets the limits with `LITTLEBOX_FILES_QUOTA_BYTES` (all files
//! of one Deployment together, 20 MiB by default) and `LITTLEBOX_FILE_MAX_BYTES`
//! (one file, 2 MiB by default). Every refusal names the configured number.

use std::{
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

const MIB: u64 = 1024 * 1024;
pub(crate) const DEFAULT_QUOTA_BYTES: u64 = 20 * MIB;
pub(crate) const DEFAULT_FILE_MAX_BYTES: u64 = 2 * MIB;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The bytes of one stored file, or of one range of them.
pub(crate) struct Blob {
    /// The bytes `reader` yields.
    pub(crate) length: u64,
    pub(crate) reader: Pin<Box<dyn AsyncRead + Send>>,
}

/// The bytes of a Deployment's files. Every method names the Deployment, so a
/// store never derives one Deployment's keys from another's.
pub(crate) trait FileStore: Send + Sync {
    /// `local` or `s3`, as the health check and `files-migrate` name it.
    fn kind(&self) -> &'static str;
    /// Stores the bytes of `source`, a local file, as `file`. A reader sees
    /// either the whole file or nothing.
    fn put<'a>(&'a self, deployment: &'a str, file: &'a str, source: &'a Path) -> BoxFuture<'a, Result<()>>;
    /// The file's bytes, or the `range` of them. `None` when the store holds
    /// no such file.
    fn get<'a>(&'a self, deployment: &'a str, file: &'a str, range: Option<Range<u64>>) -> BoxFuture<'a, Result<Option<Blob>>>;
    /// Removes the file. Removing a file that is not there succeeds.
    fn delete<'a>(&'a self, deployment: &'a str, file: &'a str) -> BoxFuture<'a, Result<()>>;
    /// The file IDs the store holds for the Deployment, whether or not its
    /// index names them.
    fn list<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<String>>>;
}

/// Today's layout: the bytes sit beside the index in the Deployment's folder.
pub(crate) struct LocalFolder {
    deployments: PathBuf,
}

impl LocalFolder {
    pub(crate) fn new(data_root: &Path) -> Self {
        Self { deployments: data_root.join("deployments") }
    }

    fn path(&self, deployment: &str, file: &str) -> PathBuf {
        self.deployments.join(deployment).join("files").join(format!("{file}.blob"))
    }
}

impl FileStore for LocalFolder {
    fn kind(&self) -> &'static str {
        "local"
    }

    fn put<'a>(&'a self, deployment: &'a str, file: &'a str, source: &'a Path) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let target = self.path(deployment, file);
            let directory = target.parent().context("A file has no folder.")?;
            tokio::fs::create_dir_all(directory).await?;
            let temp = directory.join(format!(".{file}.blob.tmp-{}", uuid::Uuid::new_v4()));
            let copied = async {
                tokio::fs::copy(source, &temp).await?;
                tokio::fs::File::open(&temp).await?.sync_all().await?;
                tokio::fs::rename(&temp, &target).await
            }
            .await;
            if copied.is_err() {
                let _ = tokio::fs::remove_file(&temp).await;
            }
            Ok(copied?)
        })
    }

    fn get<'a>(&'a self, deployment: &'a str, file: &'a str, range: Option<Range<u64>>) -> BoxFuture<'a, Result<Option<Blob>>> {
        Box::pin(async move {
            let mut handle = match tokio::fs::File::open(self.path(deployment, file)).await {
                Ok(handle) => handle,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let size = handle.metadata().await?.len();
            let range = range.unwrap_or(0..size);
            ensure!(range.start <= range.end && range.end <= size, "The range lies outside the file.");
            handle.seek(std::io::SeekFrom::Start(range.start)).await?;
            let length = range.end - range.start;
            Ok(Some(Blob { length, reader: Box::pin(handle.take(length)) }))
        })
    }

    fn delete<'a>(&'a self, deployment: &'a str, file: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            match tokio::fs::remove_file(self.path(deployment, file)).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            }
        })
    }

    fn list<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            let mut ids = Vec::new();
            let mut entries = match tokio::fs::read_dir(self.deployments.join(deployment).join("files")).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                if let Some(id) = entry.file_name().to_str().and_then(|name| name.strip_suffix(".blob")) {
                    if crate::valid_file_id(id) {
                        ids.push(id.to_owned());
                    }
                }
            }
            ids.sort();
            Ok(ids)
        })
    }
}

/// How many bytes one Deployment's files may take together, and one file alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileLimits {
    pub(crate) quota_bytes: u64,
    pub(crate) max_file_bytes: u64,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self { quota_bytes: DEFAULT_QUOTA_BYTES, max_file_bytes: DEFAULT_FILE_MAX_BYTES }
    }
}

impl FileLimits {
    /// Reads `LITTLEBOX_FILES_QUOTA_BYTES` and `LITTLEBOX_FILE_MAX_BYTES`.
    pub(crate) fn from_env() -> Result<Self> {
        Self::parse(
            std::env::var("LITTLEBOX_FILES_QUOTA_BYTES").ok().as_deref(),
            std::env::var("LITTLEBOX_FILE_MAX_BYTES").ok().as_deref(),
        )
    }

    pub(crate) fn parse(quota: Option<&str>, max_file: Option<&str>) -> Result<Self> {
        let number = |name: &str, value: Option<&str>, default: u64| -> Result<u64> {
            match value.map(str::trim).filter(|value| !value.is_empty()) {
                None => Ok(default),
                Some(value) => match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => Ok(bytes),
                    _ => bail!("{name} must be a whole number of bytes above 0, not {value:?}."),
                },
            }
        };
        let limits = Self {
            quota_bytes: number("LITTLEBOX_FILES_QUOTA_BYTES", quota, DEFAULT_QUOTA_BYTES)?,
            max_file_bytes: number("LITTLEBOX_FILE_MAX_BYTES", max_file, DEFAULT_FILE_MAX_BYTES)?,
        };
        ensure!(
            limits.max_file_bytes <= limits.quota_bytes,
            "LITTLEBOX_FILE_MAX_BYTES ({}) is larger than LITTLEBOX_FILES_QUOTA_BYTES ({}), so no file that size could be stored.",
            human_bytes(limits.max_file_bytes),
            human_bytes(limits.quota_bytes)
        );
        Ok(limits)
    }

    pub(crate) fn quota(&self) -> String {
        human_bytes(self.quota_bytes)
    }

    pub(crate) fn max_file(&self) -> String {
        human_bytes(self.max_file_bytes)
    }

    /// The refusal for an upload past the quota.
    pub(crate) fn quota_reached(&self) -> String {
        format!("This deployment has reached its {} file quota.", self.quota())
    }

    /// The refusal for an empty file or one past the per-file cap.
    pub(crate) fn file_size_refused(&self) -> String {
        format!("Files must be between 1 byte and {}.", self.max_file())
    }
}

/// `20 MiB`, `64 KiB` or `1500 bytes`: the unit that writes the number exactly.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB && bytes % GIB == 0 {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= MIB && bytes % MIB == 0 {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= 1024 && bytes % 1024 == 0 {
        format!("{} KiB", bytes / 1024)
    } else if bytes == 1 {
        "1 byte".into()
    } else {
        format!("{bytes} bytes")
    }
}

/// The installation's file store and limits, shared by every request.
#[derive(Clone)]
pub(crate) struct Files {
    pub(crate) store: Arc<dyn FileStore>,
    pub(crate) limits: FileLimits,
}

impl Files {
    /// The local folder with the default limits, as tests and a Kernel without
    /// file settings use.
    pub(crate) fn local(data_root: impl AsRef<Path>) -> Self {
        Self::new(Arc::new(LocalFolder::new(data_root.as_ref())), FileLimits::default())
    }

    pub(crate) fn new(store: Arc<dyn FileStore>, limits: FileLimits) -> Self {
        Self { store, limits }
    }

    pub(crate) fn with_limits(mut self, limits: FileLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// Reads a whole blob into memory, for the small export format, whose bundle
/// is capped at 16 MiB. The file path and the MCP read stream instead.
pub(crate) async fn read_all(store: &dyn FileStore, deployment: &str, file: &str) -> Result<Option<Vec<u8>>> {
    let Some(mut blob) = store.get(deployment, file, None).await? else { return Ok(None) };
    let mut bytes = Vec::with_capacity(blob.length as usize);
    blob.reader.read_to_end(&mut bytes).await?;
    Ok(Some(bytes))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn limits_default_to_todays_numbers_and_refuse_nonsense() {
        assert_eq!(FileLimits::parse(None, None).unwrap(), FileLimits { quota_bytes: 20 * MIB, max_file_bytes: 2 * MIB });
        assert_eq!(FileLimits::parse(Some(""), Some(" ")).unwrap(), FileLimits::default());
        let raised = FileLimits::parse(Some("1073741824"), Some("26214400")).unwrap();
        assert_eq!((raised.quota(), raised.max_file()), ("1 GiB".into(), "25 MiB".into()));
        assert_eq!(raised.quota_reached(), "This deployment has reached its 1 GiB file quota.");
        assert_eq!(raised.file_size_refused(), "Files must be between 1 byte and 25 MiB.");
        for bad in ["0", "-1", "20MiB", "1.5"] {
            let refused = FileLimits::parse(Some(bad), None).unwrap_err().to_string();
            assert!(refused.contains("LITTLEBOX_FILES_QUOTA_BYTES must be a whole number"), "{refused}");
        }
        let inverted = FileLimits::parse(Some("1048576"), Some("2097152")).unwrap_err().to_string();
        assert!(inverted.contains("(2 MiB) is larger than LITTLEBOX_FILES_QUOTA_BYTES (1 MiB)"), "{inverted}");
        assert_eq!(human_bytes(1500), "1500 bytes");
        assert_eq!(human_bytes(65536), "64 KiB");
    }

    /// The contract every store keeps. `files.rs` runs the upload, read and
    /// delete handlers over each store as well.
    pub(crate) async fn store_contract(store: &dyn FileStore, scratch: &Path) {
        let deployment = uuid::Uuid::new_v4().to_string();
        let (a, b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(scratch).unwrap();
        let source = scratch.join("source");
        std::fs::write(&source, b"0123456789").unwrap();
        assert!(store.get(&deployment, &a, None).await.unwrap().is_none());
        assert!(store.list(&deployment).await.unwrap().is_empty());
        store.put(&deployment, &a, &source).await.unwrap();
        std::fs::write(&source, vec![9u8; 200_000]).unwrap();
        store.put(&deployment, &b, &source).await.unwrap();
        assert_eq!(read_all(store, &deployment, &a).await.unwrap().unwrap(), b"0123456789");
        assert_eq!(read_all(store, &deployment, &b).await.unwrap().unwrap(), vec![9u8; 200_000]);
        let mut part = store.get(&deployment, &a, Some(2..5)).await.unwrap().unwrap();
        assert_eq!(part.length, 3);
        let mut bytes = Vec::new();
        part.reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"234");
        let mut listed = vec![a.clone(), b.clone()];
        listed.sort();
        assert_eq!(store.list(&deployment).await.unwrap(), listed);
        // Another Deployment sees none of them.
        assert!(store.list(&uuid::Uuid::new_v4().to_string()).await.unwrap().is_empty());
        store.delete(&deployment, &a).await.unwrap();
        store.delete(&deployment, &a).await.unwrap();
        assert!(store.get(&deployment, &a, None).await.unwrap().is_none());
        assert_eq!(store.list(&deployment).await.unwrap(), vec![b.clone()]);
        store.delete(&deployment, &b).await.unwrap();
    }

    #[tokio::test]
    async fn the_local_folder_keeps_the_store_contract() {
        let root = std::env::temp_dir().join(format!("littlebox-local-store-{}", uuid::Uuid::new_v4()));
        store_contract(&LocalFolder::new(&root), &root.join("scratch")).await;
        let _ = std::fs::remove_dir_all(root);
    }
}
