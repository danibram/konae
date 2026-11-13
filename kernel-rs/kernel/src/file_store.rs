//! Where the Kernel keeps the bytes of app files, and how many it accepts.
//!
//! A Deployment's file index, `apps/<id>/data/files/index.json`, always
//! stays in the app folder, so listing, visibility and the quota never ask
//! the store. Only the bytes go through [`FileStore`]. [`LocalFolder`] keeps
//! them next to the index as `<file id>.blob`.
//!
//! The operator sets the limits with `KONAE_FILES_QUOTA_BYTES` (all files
//! of one Deployment together, 20 MiB by default) and `KONAE_FILE_MAX_BYTES`
//! (one file, 2 MiB by default). Every refusal names the configured number.

use std::{
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use aws_sdk_s3::Client as S3Client;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

const MIB: u64 = 1024 * 1024;
pub const DEFAULT_QUOTA_BYTES: u64 = 20 * MIB;
pub const DEFAULT_FILE_MAX_BYTES: u64 = 2 * MIB;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The bytes of one stored file, or of one range of them.
pub struct Blob {
    /// The bytes `reader` yields.
    pub length: u64,
    pub reader: Pin<Box<dyn AsyncRead + Send>>,
}

/// The bytes of a Deployment's files. Every method names the Deployment, so a
/// store never derives one Deployment's keys from another's.
pub trait FileStore: Send + Sync {
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
    /// index names them, sorted.
    fn list<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move { Ok(self.objects(deployment).await?.into_iter().map(|object| object.id).collect()) })
    }
    /// The same files with the time each was written.
    fn objects<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<StoredObject>>>;
    /// Every Deployment ID the store holds any file for. A bucket may also
    /// name one whose files are all gone, as SeaweedFS does.
    fn deployments(&self) -> BoxFuture<'_, Result<Vec<String>>>;
}

/// One file a store holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredObject {
    pub id: String,
    /// Milliseconds since the epoch when the bytes were written, when the store says.
    pub modified_ms: Option<u64>,
}

/// The bytes sit beside the index in the app's `data/files/`.
pub struct LocalFolder {
    root: PathBuf,
}

impl LocalFolder {
    pub fn new(data_root: &Path) -> Self {
        Self { root: data_root.to_owned() }
    }

    fn path(&self, deployment: &str, file: &str) -> PathBuf {
        crate::layout::files(&self.root, deployment).join(format!("{file}.blob"))
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

    fn objects<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<StoredObject>>> {
        Box::pin(async move {
            let mut objects = Vec::new();
            let mut entries = match tokio::fs::read_dir(crate::layout::files(&self.root, deployment)).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(objects),
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                if let Some(id) = entry.file_name().to_str().and_then(|name| name.strip_suffix(".blob")) {
                    if crate::valid_file_id(id) {
                        let modified_ms = entry
                            .metadata()
                            .await?
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|elapsed| elapsed.as_millis() as u64);
                        objects.push(StoredObject { id: id.to_owned(), modified_ms });
                    }
                }
            }
            objects.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(objects)
        })
    }

    fn deployments(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move {
            let mut ids = Vec::new();
            let mut entries = match tokio::fs::read_dir(crate::layout::apps(&self.root)).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                if entry.file_type().await?.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        ids.push(name.to_owned());
                    }
                }
            }
            ids.sort();
            Ok(ids)
        })
    }
}

/// An S3-compatible bucket. Each file is the object
/// `<prefix><deployment id>/<file id>`. It sends only `PutObject`,
/// `GetObject` with a `Range`, `DeleteObject` and `ListObjectsV2`, the
/// operations R2, Scaleway and Hetzner all document (ADR-0009).
pub struct S3Store {
    client: S3Client,
    pub settings: S3Settings,
}

/// A bucket, prefix and the keys that reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Settings {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// Empty, or ending in `/`.
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}

/// Looks up one setting by name, from the environment or a test's table.
pub type Vars<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Reads one setting from the environment, trimmed. An empty value counts as unset.
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

fn path_style(get: Vars<'_>, name: &str) -> Result<bool> {
    match get(name).as_deref() {
        None | Some("1") | Some("true") => Ok(true),
        Some("0") | Some("false") => Ok(false),
        Some(_) => bail!("{name} must be 1 or 0."),
    }
}

impl S3Settings {
    /// The store for file bytes is `files/` in `KONAE_S3_BUCKET`. `None`
    /// keeps them in the data directory.
    pub fn from_env() -> Result<Option<Self>> {
        Self::files(&env_var)
    }

    pub fn files(get: Vars<'_>) -> Result<Option<Self>> {
        let Some(bucket) = get("KONAE_S3_BUCKET") else { return Ok(None) };
        let required = |name: &str| get(name).with_context(|| format!("KONAE_S3_BUCKET is set, so {name} is required."));
        Ok(Some(Self {
            endpoint: required("KONAE_S3_ENDPOINT")?,
            region: get("KONAE_S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            bucket,
            prefix: FILES_PREFIX.into(),
            access_key: required("KONAE_S3_ACCESS_KEY")?,
            secret_key: required("KONAE_S3_SECRET_KEY")?,
            path_style: path_style(get, "KONAE_S3_PATH_STYLE")?,
        }))
    }

    /// Copies and the revocation record use `KONAE_S3_BUCKET` with an empty
    /// prefix. `None` without it.
    pub fn copies(get: Vars<'_>) -> Result<Option<Self>> {
        Ok(Self::files(get)?.map(|settings| Self { prefix: String::new(), ..settings }))
    }
}

/// The prefix of file bytes in the installation's one bucket.
pub const FILES_PREFIX: &str = "files/";

impl S3Store {
    pub fn new(settings: S3Settings) -> Self {
        Self { client: s3_client(&settings), settings }
    }

    fn key(&self, deployment: &str, file: &str) -> String {
        format!("{}{deployment}/{file}", self.settings.prefix)
    }

    /// Every key under `prefix`, with its modification time, page by page.
    async fn keys(&self, prefix: &str, delimiter: bool) -> Result<(Vec<(String, Option<u64>)>, Vec<String>)> {
        let mut keys = Vec::new();
        let mut folders = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut request = self.client.list_objects_v2().bucket(&self.settings.bucket).prefix(prefix);
            if delimiter { request = request.delimiter("/"); }
            let page = request.set_continuation_token(continuation.take()).send().await
                .with_context(|| format!("Listing s3://{}/{prefix} failed", self.settings.bucket))?;
            for object in page.contents() {
                if let Some(key) = object.key() {
                    let modified = object.last_modified().and_then(|time| u64::try_from(time.to_millis().ok()?).ok());
                    keys.push((key.to_owned(), modified));
                }
            }
            folders.extend(page.common_prefixes().iter().filter_map(|folder| folder.prefix().map(str::to_owned)));
            match page.next_continuation_token() {
                Some(token) if page.is_truncated() == Some(true) => continuation = Some(token.to_owned()),
                _ => break,
            }
        }
        Ok((keys, folders))
    }
}

/// The one client builder for files, copies and the revocation record.
pub fn s3_client(settings: &S3Settings) -> S3Client {
    let credentials = aws_credential_types::Credentials::new(&settings.access_key, &settings.secret_key, None, None, "konae-storage");
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(settings.region.clone()))
        .credentials_provider(credentials)
        .endpoint_url(&settings.endpoint)
        .force_path_style(settings.path_style)
        // A checksum header on every request is an SDK default that not
        // every S3-compatible store accepts; the Kernel checks SHA-256 itself.
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired)
        .build();
    S3Client::from_conf(config)
}

fn not_found<E>(error: &aws_sdk_s3::error::SdkError<E>) -> bool {
    error.raw_response().is_some_and(|response| response.status().as_u16() == 404)
}

impl FileStore for S3Store {
    fn kind(&self) -> &'static str {
        "s3"
    }

    fn put<'a>(&'a self, deployment: &'a str, file: &'a str, source: &'a Path) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let length = tokio::fs::metadata(source).await?.len();
            let body = aws_sdk_s3::primitives::ByteStream::from_path(source).await?;
            self.client
                .put_object()
                .bucket(&self.settings.bucket)
                .key(self.key(deployment, file))
                .content_length(length as i64)
                .body(body)
                .send()
                .await
                .with_context(|| format!("Storing file {file} in s3://{} failed", self.settings.bucket))?;
            Ok(())
        })
    }

    fn get<'a>(&'a self, deployment: &'a str, file: &'a str, range: Option<Range<u64>>) -> BoxFuture<'a, Result<Option<Blob>>> {
        Box::pin(async move {
            if let Some(range) = &range {
                ensure!(range.start <= range.end, "The range lies outside the file.");
                if range.start == range.end {
                    return Ok(Some(Blob { length: 0, reader: Box::pin(tokio::io::empty()) }));
                }
            }
            let mut request = self.client.get_object().bucket(&self.settings.bucket).key(self.key(deployment, file));
            if let Some(range) = &range {
                request = request.range(format!("bytes={}-{}", range.start, range.end - 1));
            }
            let object = match request.send().await {
                Ok(object) => object,
                Err(error) if not_found(&error) => return Ok(None),
                Err(error) => return Err(anyhow::Error::new(error).context(format!("Reading file {file} from s3://{} failed", self.settings.bucket))),
            };
            let length = object.content_length().and_then(|length| u64::try_from(length).ok()).context("The store did not say how long the file is.")?;
            if let Some(range) = &range {
                ensure!(length == range.end - range.start, "The store answered a different range than the one asked for.");
            }
            Ok(Some(Blob { length, reader: Box::pin(object.body.into_async_read()) }))
        })
    }

    fn delete<'a>(&'a self, deployment: &'a str, file: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            match self.client.delete_object().bucket(&self.settings.bucket).key(self.key(deployment, file)).send().await {
                Ok(_) => Ok(()),
                Err(error) if not_found(&error) => Ok(()),
                Err(error) => Err(anyhow::Error::new(error).context(format!("Removing file {file} from s3://{} failed", self.settings.bucket))),
            }
        })
    }

    fn objects<'a>(&'a self, deployment: &'a str) -> BoxFuture<'a, Result<Vec<StoredObject>>> {
        Box::pin(async move {
            let folder = format!("{}{deployment}/", self.settings.prefix);
            let (keys, _) = self.keys(&folder, false).await?;
            let mut objects: Vec<StoredObject> = keys
                .into_iter()
                .filter_map(|(key, modified_ms)| {
                    let id = key.strip_prefix(&folder)?;
                    crate::valid_file_id(id).then(|| StoredObject { id: id.to_owned(), modified_ms })
                })
                .collect();
            objects.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(objects)
        })
    }

    fn deployments(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move {
            let (_, folders) = self.keys(&self.settings.prefix, true).await?;
            let mut ids: Vec<String> = folders
                .iter()
                .filter_map(|folder| folder.strip_prefix(&self.settings.prefix)?.strip_suffix('/'))
                .filter(|id| !id.is_empty() && !id.contains('/'))
                .map(str::to_owned)
                .collect();
            ids.sort();
            Ok(ids)
        })
    }
}

/// How many bytes one Deployment's files may take together, and one file alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileLimits {
    pub quota_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self { quota_bytes: DEFAULT_QUOTA_BYTES, max_file_bytes: DEFAULT_FILE_MAX_BYTES }
    }
}

impl FileLimits {
    /// Reads `KONAE_FILES_QUOTA_BYTES` and `KONAE_FILE_MAX_BYTES`.
    pub fn from_env() -> Result<Self> {
        Self::parse(
            std::env::var("KONAE_FILES_QUOTA_BYTES").ok().as_deref(),
            std::env::var("KONAE_FILE_MAX_BYTES").ok().as_deref(),
        )
    }

    pub fn parse(quota: Option<&str>, max_file: Option<&str>) -> Result<Self> {
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
            quota_bytes: number("KONAE_FILES_QUOTA_BYTES", quota, DEFAULT_QUOTA_BYTES)?,
            max_file_bytes: number("KONAE_FILE_MAX_BYTES", max_file, DEFAULT_FILE_MAX_BYTES)?,
        };
        ensure!(
            limits.max_file_bytes <= limits.quota_bytes,
            "KONAE_FILE_MAX_BYTES ({}) is larger than KONAE_FILES_QUOTA_BYTES ({}), so no file that size could be stored.",
            human_bytes(limits.max_file_bytes),
            human_bytes(limits.quota_bytes)
        );
        Ok(limits)
    }

    pub fn quota(&self) -> String {
        human_bytes(self.quota_bytes)
    }

    pub fn max_file(&self) -> String {
        human_bytes(self.max_file_bytes)
    }

    /// The refusal for an upload past the quota.
    pub fn quota_reached(&self) -> String {
        format!("This deployment has reached its {} file quota.", self.quota())
    }

    /// The refusal for an empty file or one past the per-file cap.
    pub fn file_size_refused(&self) -> String {
        format!("Files must be between 1 byte and {}.", self.max_file())
    }
}

/// `20 MiB`, `64 KiB` or `1500 bytes`: the unit that writes the number exactly.
pub fn human_bytes(bytes: u64) -> String {
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
pub struct Files {
    pub store: Arc<dyn FileStore>,
    pub limits: FileLimits,
}

impl Files {
    /// The local folder with the default limits, for tests.
    #[cfg(any(test, feature = "testing"))]
    pub fn local(data_root: impl AsRef<Path>) -> Self {
        Self::new(Arc::new(LocalFolder::new(data_root.as_ref())), FileLimits::default())
    }

    pub fn new(store: Arc<dyn FileStore>, limits: FileLimits) -> Self {
        Self { store, limits }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn with_limits(mut self, limits: FileLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// One Deployment's files for code that runs on a blocking thread, such as
/// the package capture and import, which read and write archives with
/// `std::io`. Calls from inside `spawn_blocking` only.
pub struct BlockingFiles {
    store: Arc<dyn FileStore>,
    deployment: String,
    handle: tokio::runtime::Handle,
}

impl BlockingFiles {
    /// Takes the current runtime, so call it before `spawn_blocking`.
    pub fn new(store: Arc<dyn FileStore>, deployment: &str) -> Self {
        Self { store, deployment: deployment.to_owned(), handle: tokio::runtime::Handle::current() }
    }

    /// The file's length and a reader over its bytes, or `None`.
    pub fn open(&self, file: &str) -> Result<Option<(u64, impl std::io::Read + use<>)>> {
        let blob = self.handle.block_on(self.store.get(&self.deployment, file, None))?;
        Ok(blob.map(|blob| (blob.length, tokio_util::io::SyncIoBridge::new_with_handle(blob.reader, self.handle.clone()))))
    }

    pub fn put(&self, file: &str, source: &Path) -> Result<()> {
        self.handle.block_on(self.store.put(&self.deployment, file, source))
    }
}

// ---------------------------------------------------------------------------
// Which store holds the bytes

/// The file in the data directory that says which store holds the bytes.
pub const MARKER_FILE: &str = "files-store.json";
const ORPHAN_GRACE_MS: u64 = 60 * 60 * 1000;

/// What `files-store.json` records: `{"store":"local"}`, or the bucket and
/// prefix of an S3 store. Keys and region stay in the environment.
pub(crate) fn describe(settings: Option<&S3Settings>) -> serde_json::Value {
    match settings {
        None => serde_json::json!({"store": "local"}),
        Some(s3) => serde_json::json!({"store": "s3", "endpoint": s3.endpoint, "bucket": s3.bucket, "prefix": s3.prefix}),
    }
}

pub(crate) fn spoken(recorded: &serde_json::Value) -> String {
    match recorded["store"].as_str() {
        Some("s3") => format!(
            "s3://{}/{} at {}",
            recorded["bucket"].as_str().unwrap_or("?"),
            recorded["prefix"].as_str().unwrap_or(""),
            recorded["endpoint"].as_str().unwrap_or("?")
        ),
        _ => "the data directory".into(),
    }
}

pub(crate) async fn read_marker(data_root: &Path) -> Result<Option<serde_json::Value>> {
    match tokio::fs::read(data_root.join(MARKER_FILE)).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| format!("{MARKER_FILE} is not valid JSON."))?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn write_marker(data_root: &Path, value: &serde_json::Value) -> Result<()> {
    crate::write_atomic(&data_root.join(MARKER_FILE), &serde_json::to_vec_pretty(value)?).await?;
    Ok(())
}

/// Whether any app folder still holds a `.blob`.
async fn local_bytes_remain(data_root: &Path) -> Result<bool> {
    let local = LocalFolder::new(data_root);
    for deployment in local.deployments().await? {
        if !local.list(&deployment).await?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Files {
    /// The store and limits the environment names, checked against the store
    /// `files-store.json` records. A Kernel never starts on a store that does
    /// not hold its files.
    pub async fn from_env(data_root: &Path) -> Result<Self> {
        Self::open(data_root, FileLimits::from_env()?, S3Settings::from_env()?).await
    }

    pub async fn open(data_root: &Path, limits: FileLimits, settings: Option<S3Settings>) -> Result<Self> {
        let wanted = describe(settings.as_ref());
        match read_marker(data_root).await? {
            Some(recorded) if recorded == wanted => {}
            Some(recorded) => {
                let command = match (recorded["store"].as_str(), settings.is_some()) {
                    (Some("s3"), false) => format!("Set the S3 settings it was started with again, or run {} files-migrate --to local with them set.", crate::program()),
                    (Some("s3"), true) => format!(
                        "Set the S3 settings it was started with again. To move the files to this bucket, stop the Kernel, run {program} files-migrate --to local with the settings it was started with, then {program} files-migrate --to s3 with these.",
                        program = crate::program()
                    ),
                    _ => format!("Stop the Kernel and run {} files-migrate --to s3 with these settings first.", crate::program()),
                };
                bail!("The files of this installation are in {}, but the settings name {}. {command}", spoken(&recorded), spoken(&wanted));
            }
            None => {
                if settings.is_some() && local_bytes_remain(data_root).await? {
                    bail!("The settings name {}, but the files of this installation are in the data directory. Stop the Kernel and run {} files-migrate --to s3 first.", spoken(&wanted), crate::program());
                }
                write_marker(data_root, &wanted).await?;
            }
        }
        let store: Arc<dyn FileStore> = match settings {
            Some(settings) => Arc::new(S3Store::new(settings)),
            None => Arc::new(LocalFolder::new(data_root)),
        };
        Ok(Self::new(store, limits))
    }
}

/// The IDs and sizes a Deployment's index names, or none without an index.
pub async fn indexed_files(data_root: &Path, deployment: &str) -> Result<Option<Vec<(String, u64)>>> {
    let path = crate::layout::files(&data_root, &deployment).join("index.json");
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let index: Vec<serde_json::Value> = serde_json::from_slice(&bytes).with_context(|| format!("{} is not valid JSON.", path.display()))?;
    index
        .iter()
        .map(|file| {
            let id = file["id"].as_str().filter(|id| crate::valid_file_id(id)).with_context(|| format!("{} names an invalid file id.", path.display()))?;
            Ok((id.to_owned(), file["size"].as_u64().unwrap_or(0)))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// Removes every file the store holds for a Deployment whose folder is gone.
pub async fn forget_deployment(store: &dyn FileStore, deployment: &str) {
    match store.list(deployment).await {
        Ok(ids) => {
            for id in ids {
                if let Err(error) = store.delete(deployment, &id).await {
                    eprintln!("konae: could not remove file {id} of Deployment {deployment}: {error:#}");
                }
            }
        }
        Err(error) => eprintln!("konae: could not list the files of Deployment {deployment}: {error:#}"),
    }
}

/// Copies `blob` into `target`, returning its length and SHA-256.
pub async fn save_blob(mut blob: Blob, target: &Path) -> Result<(u64, String)> {
    use tokio::io::AsyncWriteExt;
    let mut output = tokio::fs::File::create(target).await?;
    let mut hasher = sha2::Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let read = blob.reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
        output.write_all(&buffer[..read]).await?;
        length += read as u64;
    }
    output.sync_all().await?;
    Ok((length, hex::encode(sha2::Digest::finalize(hasher))))
}

async fn blob_digest(mut blob: Blob) -> Result<(u64, String)> {
    let mut hasher = sha2::Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let read = blob.reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
        length += read as u64;
    }
    Ok((length, hex::encode(sha2::Digest::finalize(hasher))))
}

/// What `files-migrate` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migrated {
    pub deployments: usize,
    pub files: usize,
    pub bytes: u64,
}

/// Copies every indexed file from `source` to `target`, reads each back from
/// `target` and compares its SHA-256 with the source's, records `target` in
/// `files-store.json`, and only then removes the files from `source`. Any
/// mismatch or missing file stops it before the record changes.
pub async fn migrate(data_root: &Path, source: &dyn FileStore, target: &dyn FileStore, target_marker: &serde_json::Value) -> Result<Migrated> {
    let scratch = data_root.join("transfers");
    tokio::fs::create_dir_all(&scratch).await?;
    let mut summary = Migrated::default();
    let mut moved: Vec<(String, String)> = Vec::new();
    for deployment in LocalFolder::new(data_root).deployments().await? {
        let Some(files) = indexed_files(data_root, &deployment).await? else { continue };
        summary.deployments += 1;
        for (id, size) in files {
            let blob = source
                .get(&deployment, &id, None)
                .await?
                .with_context(|| format!("File {id} of Deployment {deployment} is in the index but not in {}. Nothing was switched.", source.kind()))?;
            let temp = scratch.join(format!(".files-migrate-{}", uuid::Uuid::new_v4()));
            let copied = async {
                let (length, sha256) = save_blob(blob, &temp).await?;
                ensure!(length == size, "File {id} of Deployment {deployment} holds {length} bytes, but its index says {size}. Nothing was switched.");
                target.put(&deployment, &id, &temp).await?;
                let stored = target.get(&deployment, &id, None).await?.with_context(|| format!("File {id} did not arrive in {}.", target.kind()))?;
                let (stored_length, stored_sha256) = blob_digest(stored).await?;
                ensure!(
                    stored_length == length && stored_sha256 == sha256,
                    "File {id} of Deployment {deployment} reads back from {} with another SHA-256. Nothing was switched.",
                    target.kind()
                );
                Ok::<_, anyhow::Error>(length)
            }
            .await;
            let _ = tokio::fs::remove_file(&temp).await;
            summary.bytes += copied?;
            summary.files += 1;
            moved.push((deployment.clone(), id));
        }
    }
    write_marker(data_root, target_marker).await?;
    for (deployment, id) in moved {
        if let Err(error) = source.delete(&deployment, &id).await {
            eprintln!("konae: file {id} of Deployment {deployment} is in {} now; removing the old copy failed: {error:#}", target.kind());
        }
    }
    Ok(summary)
}

/// What `files-reconcile` found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub removed: usize,
    pub kept_recent: usize,
}

/// Removes the files a store holds that no index names, once they are older
/// than an hour; a younger one may be an upload whose index change has not
/// committed yet. A Deployment without a folder has no index, so all its
/// files go.
pub async fn reconcile(data_root: &Path, store: &dyn FileStore, now_ms: u64, dry_run: bool) -> Result<Reconciled> {
    let mut summary = Reconciled::default();
    for deployment in store.deployments().await? {
        let indexed: std::collections::HashSet<String> = indexed_files(data_root, &deployment)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        for object in store.objects(&deployment).await? {
            if indexed.contains(&object.id) {
                continue;
            }
            if object.modified_ms.is_none_or(|at| now_ms.saturating_sub(at) < ORPHAN_GRACE_MS) {
                summary.kept_recent += 1;
                continue;
            }
            if !dry_run {
                store.delete(&deployment, &object.id).await?;
            }
            summary.removed += 1;
        }
    }
    Ok(summary)
}

/// A running Kernel holds an exclusive lock on this file in its data
/// directory for as long as it serves, so an operator command that must not
/// race uploads can tell it is running.
pub const KERNEL_LOCK_FILE: &str = "kernel.lock";

fn lock_file(data_root: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(data_root)?;
    let path = data_root.join(KERNEL_LOCK_FILE);
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("Could not open {}.", path.display()))
}

/// The lock a serving Kernel holds. A second Kernel on the same data
/// directory, or one started while `files-migrate` runs, refuses to start.
pub fn hold_kernel_lock(data_root: &Path) -> Result<std::fs::File> {
    let file = lock_file(data_root)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => bail!(
            "Another Kernel or a files command is using {}. Stop it before starting this Kernel.",
            data_root.display()
        ),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Takes the Kernel's lock for an operator command, so the command refuses
/// while a Kernel serves and a Kernel cannot start while the command runs.
pub fn hold_while_stopped(data_root: &Path, command: &str) -> Result<std::fs::File> {
    let file = lock_file(data_root)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => bail!(
            "A Kernel is running on {}. Stop it before {command}, so no upload lands while the command reads the files.",
            data_root.display()
        ),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn migrate_usage() -> String {
    format!("Usage: {} files-migrate --to s3|local", crate::program())
}

fn reconcile_usage() -> String {
    format!("Usage: {} files-reconcile [--dry-run]", crate::program())
}

/// `files-migrate` and `files-reconcile`. Both read `KONAE_DATA_DIR` and
/// the `KONAE_S3_*` settings.
pub async fn operator_command() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_root = PathBuf::from(std::env::var("KONAE_DATA_DIR").unwrap_or_else(|_| "/data".into()));
    match args.first().map(String::as_str) {
        Some("files-migrate") => {
            let to = match args.get(1..).unwrap_or_default() {
                [flag, to] if flag == "--to" && (to == "s3" || to == "local") => to.as_str(),
                _ => bail!("{}", migrate_usage()),
            };
            let _stopped = hold_while_stopped(&data_root, "files-migrate")?;
            let settings = S3Settings::from_env()?.context("files-migrate needs the S3 settings of the bucket, in both directions.")?;
            let recorded = read_marker(&data_root).await?.unwrap_or_else(|| describe(None));
            let bucket = describe(Some(&settings));
            let local = LocalFolder::new(&data_root);
            let s3 = S3Store::new(settings);
            let (source, target, marker): (&dyn FileStore, &dyn FileStore, serde_json::Value) = match to {
                "s3" if recorded == bucket => {
                    println!("The files are already in {}.", spoken(&bucket));
                    return Ok(true);
                }
                "s3" if recorded["store"] == "local" => (&local, &s3, bucket),
                "local" if recorded["store"] == "local" => {
                    println!("The files are already in the data directory.");
                    return Ok(true);
                }
                "local" if recorded == bucket => (&s3, &local, describe(None)),
                _ => bail!("The files are in {}, which is not {}. files-migrate moves files between the data directory and the bucket the settings name.", spoken(&recorded), spoken(&bucket)),
            };
            let summary = migrate(&data_root, source, target, &marker).await?;
            println!(
                "Moved {} files ({} bytes) of {} Deployments to {}, each read back with the same SHA-256. Start the Kernel with {}.",
                summary.files,
                summary.bytes,
                summary.deployments,
                spoken(&marker),
                if to == "s3" { "the same S3 settings" } else { "no KONAE_S3_BUCKET" }
            );
            Ok(true)
        }
        Some("files-reconcile") => {
            let dry_run = match args.get(1..).unwrap_or_default() {
                [] => false,
                [flag] if flag == "--dry-run" => true,
                _ => bail!("{}", reconcile_usage()),
            };
            let _stopped = hold_while_stopped(&data_root, "files-reconcile")?;
            let files = Files::from_env(&data_root).await?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
            let summary = reconcile(&data_root, files.store.as_ref(), now, dry_run).await?;
            println!(
                "{} {} files no index names from {}; kept {} younger than an hour.",
                if dry_run { "Would remove" } else { "Removed" },
                summary.removed,
                files.store.kind(),
                summary.kept_recent
            );
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Reads a whole blob into memory, for tests that compare stored bytes. The
/// file path and the MCP read stream instead.
#[cfg(any(test, feature = "testing"))]
pub async fn read_all(store: &dyn FileStore, deployment: &str, file: &str) -> Result<Option<Vec<u8>>> {
    let Some(mut blob) = store.get(deployment, file, None).await? else { return Ok(None) };
    let mut bytes = Vec::with_capacity(blob.length as usize);
    blob.reader.read_to_end(&mut bytes).await?;
    Ok(Some(bytes))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn table<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    }

    /// One bucket holds file bytes under `files/` and copies at its root.
    #[test]
    fn the_s3_settings_name_one_bucket() {
        let full = [
            ("KONAE_S3_BUCKET", "konae"), ("KONAE_S3_ENDPOINT", "https://s3.example"),
            ("KONAE_S3_ACCESS_KEY", "a"), ("KONAE_S3_SECRET_KEY", "s"),
        ];
        let files = S3Settings::files(&table(&full)).unwrap().unwrap();
        assert_eq!((files.bucket.as_str(), files.prefix.as_str(), files.region.as_str(), files.path_style), ("konae", "files/", "us-east-1", true));
        let copies = S3Settings::copies(&table(&full)).unwrap().unwrap();
        assert_eq!((copies.bucket.as_str(), copies.prefix.as_str()), ("konae", ""));
        // An endpoint and keys without a bucket keep files in the data directory and copies off S3.
        let no_bucket = &full[1..];
        assert!(S3Settings::files(&table(no_bucket)).unwrap().is_none());
        assert!(S3Settings::copies(&table(no_bucket)).unwrap().is_none());
        let missing = S3Settings::files(&table(&[("KONAE_S3_BUCKET", "konae")])).unwrap_err().to_string();
        assert_eq!(missing, "KONAE_S3_BUCKET is set, so KONAE_S3_ENDPOINT is required.");
        let keyless = S3Settings::files(&table(&full[..2])).unwrap_err().to_string();
        assert_eq!(keyless, "KONAE_S3_BUCKET is set, so KONAE_S3_ACCESS_KEY is required.");
    }

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
            assert!(refused.contains("KONAE_FILES_QUOTA_BYTES must be a whole number"), "{refused}");
        }
        let inverted = FileLimits::parse(Some("1048576"), Some("2097152")).unwrap_err().to_string();
        assert!(inverted.contains("(2 MiB) is larger than KONAE_FILES_QUOTA_BYTES (1 MiB)"), "{inverted}");
        assert_eq!(human_bytes(1500), "1500 bytes");
        assert_eq!(human_bytes(65536), "64 KiB");
    }

    /// The contract every store keeps. `files.rs` runs the upload, read and
    /// delete handlers over each store as well.
    pub async fn store_contract(store: &dyn FileStore, scratch: &Path) {
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

    /// An in-process S3 server for tests: path-style PutObject, GetObject with
    /// one range, DeleteObject and ListObjectsV2 with prefix, delimiter and
    /// two keys per page, so paging runs. It checks no signature. Set
    /// `KONAE_TEST_S3_ENDPOINT` (and `KONAE_TEST_S3_BUCKET`) to run the
    /// same tests against a real S3-compatible server instead.
    pub async fn fake_s3() -> String {
        use axum::{
            Router,
            body::{Body, Bytes},
            extract::{Path as UrlPath, Query, State},
            http::{HeaderMap, StatusCode, header},
            response::{IntoResponse, Response},
            routing::get,
        };
        use std::collections::{BTreeMap, HashMap};
        type Objects = Arc<std::sync::Mutex<BTreeMap<String, (Vec<u8>, u64)>>>;
        fn xml(text: &str) -> String {
            text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        }
        fn missing() -> Response {
            (StatusCode::NOT_FOUND, "<Error><Code>NoSuchKey</Code><Message>missing</Message></Error>").into_response()
        }
        async fn put(State(objects): State<Objects>, UrlPath((bucket, key)): UrlPath<(String, String)>, body: Bytes) -> Response {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            objects.lock().unwrap().insert(format!("{bucket}/{key}"), (body.to_vec(), now));
            (StatusCode::OK, [(header::ETAG, "\"fake\"")]).into_response()
        }
        async fn read(State(objects): State<Objects>, UrlPath((bucket, key)): UrlPath<(String, String)>, headers: HeaderMap) -> Response {
            let Some((bytes, _)) = objects.lock().unwrap().get(&format!("{bucket}/{key}")).cloned() else { return missing() };
            let range = headers.get(header::RANGE).and_then(|value| value.to_str().ok()).and_then(|value| {
                let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
                Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
            });
            match range {
                Some((start, end)) => {
                    let end = end.min(bytes.len() - 1);
                    let part = bytes[start..=end].to_vec();
                    let content_range = format!("bytes {start}-{end}/{}", bytes.len());
                    (StatusCode::PARTIAL_CONTENT, [(header::CONTENT_RANGE, content_range)], part).into_response()
                }
                None => (StatusCode::OK, bytes).into_response(),
            }
        }
        async fn remove(State(objects): State<Objects>, UrlPath((bucket, key)): UrlPath<(String, String)>) -> Response {
            objects.lock().unwrap().remove(&format!("{bucket}/{key}"));
            StatusCode::NO_CONTENT.into_response()
        }
        async fn list(State(objects): State<Objects>, UrlPath(bucket): UrlPath<String>, Query(query): Query<HashMap<String, String>>) -> Response {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let delimiter = query.get("delimiter").cloned();
            let after = query.get("continuation-token").cloned().unwrap_or_default();
            let mut entries: Vec<(String, Option<u64>)> = Vec::new();
            for (stored, (_, modified)) in objects.lock().unwrap().iter() {
                let Some(key) = stored.strip_prefix(&format!("{bucket}/")) else { continue };
                let Some(rest) = key.strip_prefix(&prefix) else { continue };
                match delimiter.as_deref().and_then(|delimiter| rest.find(delimiter)) {
                    Some(end) => {
                        let folder = format!("{prefix}{}/", &rest[..end]);
                        if !entries.iter().any(|(name, kind)| name == &folder && kind.is_none()) {
                            entries.push((folder, None));
                        }
                    }
                    None => entries.push((key.to_owned(), Some(*modified))),
                }
            }
            let page: Vec<_> = entries.into_iter().filter(|(name, _)| name.as_str() > after.as_str()).collect();
            let truncated = page.len() > 2;
            let page = &page[..page.len().min(2)];
            let mut body = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{bucket}</Name><Prefix>{}</Prefix><KeyCount>{}</KeyCount><IsTruncated>{truncated}</IsTruncated>", xml(&prefix), page.len());
            if truncated {
                body.push_str(&format!("<NextContinuationToken>{}</NextContinuationToken>", xml(&page[page.len() - 1].0)));
            }
            for (name, modified) in page {
                match modified {
                    Some(modified) => {
                        let seconds = modified / 1000;
                        let (year, month, day) = crate::copies::civil_from_days((seconds / 86_400) as i64);
                        let time = seconds % 86_400;
                        body.push_str(&format!(
                            "<Contents><Key>{}</Key><LastModified>{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z</LastModified><Size>0</Size></Contents>",
                            xml(name), time / 3600, time / 60 % 60, time % 60
                        ));
                    }
                    None => body.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", xml(name))),
                }
            }
            body.push_str("</ListBucketResult>");
            ([(header::CONTENT_TYPE, "application/xml")], Body::from(body)).into_response()
        }
        let objects: Objects = Arc::default();
        let app = Router::new()
            .route("/{bucket}", get(list))
            .route("/{bucket}/", get(list))
            .route("/{bucket}/{*key}", get(read).put(put).delete(remove))
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(objects);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    /// An `S3Store` on `KONAE_TEST_S3_ENDPOINT` when it is set, on the
    /// in-process fake otherwise, under a fresh prefix.
    pub async fn test_s3_store() -> S3Store {
        let endpoint = match std::env::var("KONAE_TEST_S3_ENDPOINT") {
            Ok(endpoint) if !endpoint.is_empty() => endpoint,
            _ => fake_s3().await,
        };
        S3Store::new(S3Settings {
            endpoint,
            region: "us-east-1".into(),
            bucket: std::env::var("KONAE_TEST_S3_BUCKET").unwrap_or_else(|_| "konae-files".into()),
            prefix: format!("test-{}/", uuid::Uuid::new_v4().simple()),
            access_key: "konae".into(),
            secret_key: "konae-secret".into(),
            path_style: true,
        })
    }

    #[tokio::test]
    async fn the_s3_store_keeps_the_store_contract() {
        let root = std::env::temp_dir().join(format!("konae-s3-store-{}", uuid::Uuid::new_v4()));
        let store = test_s3_store().await;
        store_contract(&store, &root).await;
        // Five Deployments list across pages of two.
        let source = root.join("one");
        std::fs::write(&source, b"x").unwrap();
        let mut expected = Vec::new();
        for _ in 0..5 {
            let deployment = uuid::Uuid::new_v4().to_string();
            store.put(&deployment, &uuid::Uuid::new_v4().to_string(), &source).await.unwrap();
            expected.push(deployment);
        }
        let listed = store.deployments().await.unwrap();
        assert!(expected.iter().all(|deployment| listed.contains(deployment)), "{listed:?}");
        expected.sort();
        let objects = store.objects(&expected[0]).await.unwrap();
        assert_eq!(objects.len(), 1);
        assert!(objects[0].modified_ms.is_some_and(|at| at > 1_700_000_000_000));
        let _ = std::fs::remove_dir_all(root);
    }

    fn index(root: &Path, deployment: &str, files: &[(&str, u64)]) {
        let directory = crate::layout::files(&root, &deployment);
        std::fs::create_dir_all(&directory).unwrap();
        let entries: Vec<_> = files.iter().map(|(id, size)| serde_json::json!({"id": id, "size": size, "visibility": "private"})).collect();
        std::fs::write(directory.join("index.json"), serde_json::to_vec(&entries).unwrap()).unwrap();
    }

    async fn put_bytes(store: &dyn FileStore, root: &Path, deployment: &str, id: &str, bytes: &[u8]) {
        std::fs::create_dir_all(root).unwrap();
        let temp = root.join(format!("put-{}", uuid::Uuid::new_v4()));
        std::fs::write(&temp, bytes).unwrap();
        store.put(deployment, id, &temp).await.unwrap();
        std::fs::remove_file(temp).unwrap();
    }

    /// `files-migrate` moves every indexed file to the bucket and back with the
    /// same bytes, records the store, and leaves no copy in the old one.
    #[test]
    fn files_commands_refuse_while_a_kernel_holds_the_data_directory() {
        let root = std::env::temp_dir().join(format!("konae-kernel-lock-{}", uuid::Uuid::new_v4()));
        let kernel = hold_kernel_lock(&root).unwrap();
        for command in ["files-migrate", "files-reconcile"] {
            let refused = hold_while_stopped(&root, command).unwrap_err().to_string();
            assert!(refused.starts_with("A Kernel is running on ") && refused.contains(&format!("Stop it before {command}")), "{refused}");
        }
        assert!(hold_kernel_lock(&root).unwrap_err().to_string().contains("Another Kernel or a files command"));
        drop(kernel);
        // Once the Kernel stops, the command runs, and a Kernel waits for it.
        let migrating = hold_while_stopped(&root, "files-migrate").unwrap();
        assert!(hold_kernel_lock(&root).is_err());
        drop(migrating);
        assert!(hold_kernel_lock(&root).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn files_migrate_moves_every_indexed_file_and_switches_the_record() {
        let root = std::env::temp_dir().join(format!("konae-files-migrate-{}", uuid::Uuid::new_v4()));
        let local = LocalFolder::new(&root);
        let s3 = test_s3_store().await;
        let bucket = describe(Some(&s3.settings));
        let (deployment, a, b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        put_bytes(&local, &root, &deployment, &a, b"alpha").await;
        put_bytes(&local, &root, &deployment, &b, &vec![3u8; 150_000]).await;
        index(&root, &deployment, &[(&a, 5), (&b, 150_000)]);

        // A Kernel started with the bucket while the bytes are in the folder refuses.
        let refused = Files::open(&root, FileLimits::default(), Some(s3.settings.clone())).await.err().unwrap().to_string();
        assert!(refused.contains(&format!("run {} files-migrate --to s3 first", crate::program())), "{refused}");

        let moved = migrate(&root, &local, &s3, &bucket).await.unwrap();
        assert_eq!(moved, Migrated { deployments: 1, files: 2, bytes: 150_005 });
        assert_eq!(read_all(&s3, &deployment, &a).await.unwrap().unwrap(), b"alpha");
        assert!(local.list(&deployment).await.unwrap().is_empty());
        assert_eq!(read_marker(&root).await.unwrap().unwrap(), bucket);
        assert_eq!(Files::open(&root, FileLimits::default(), Some(s3.settings.clone())).await.unwrap().store.kind(), "s3");
        let local_refused = Files::open(&root, FileLimits::default(), None).await.err().unwrap().to_string();
        assert!(local_refused.contains("Set the S3 settings it was started with again"), "{local_refused}");

        migrate(&root, &s3, &local, &describe(None)).await.unwrap();
        assert_eq!(read_all(&local, &deployment, &b).await.unwrap().unwrap(), vec![3u8; 150_000]);
        assert!(s3.list(&deployment).await.unwrap().is_empty());
        assert_eq!(Files::open(&root, FileLimits::default(), None).await.unwrap().store.kind(), "local");

        // A file the index names but the source lacks stops the move before the record changes.
        index(&root, &deployment, &[(&a, 5), (&b, 150_000), (&uuid::Uuid::new_v4().to_string(), 1)]);
        let missing = migrate(&root, &local, &s3, &bucket).await.unwrap_err().to_string();
        assert!(missing.contains("is in the index but not in local. Nothing was switched."), "{missing}");
        assert_eq!(read_marker(&root).await.unwrap().unwrap(), describe(None));
        assert_eq!(local.list(&deployment).await.unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    /// `files-reconcile` removes what no index names once it is an hour old,
    /// and everything of a Deployment whose folder is gone.
    #[tokio::test]
    async fn files_reconcile_removes_old_objects_no_index_names() {
        let root = std::env::temp_dir().join(format!("konae-files-reconcile-{}", uuid::Uuid::new_v4()));
        let s3 = test_s3_store().await;
        let (live, gone) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        let (kept, orphan, stray) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        put_bytes(&s3, &root, &live, &kept, b"kept").await;
        put_bytes(&s3, &root, &live, &orphan, b"orphan").await;
        put_bytes(&s3, &root, &gone, &stray, b"stray").await;
        index(&root, &live, &[(&kept, 4)]);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        assert_eq!(reconcile(&root, &s3, now, false).await.unwrap(), Reconciled { removed: 0, kept_recent: 2 });
        let later = now + ORPHAN_GRACE_MS + 60_000;
        assert_eq!(reconcile(&root, &s3, later, true).await.unwrap(), Reconciled { removed: 2, kept_recent: 0 });
        assert_eq!(s3.list(&live).await.unwrap().len(), 2, "a dry run removes nothing");
        assert_eq!(reconcile(&root, &s3, later, false).await.unwrap(), Reconciled { removed: 2, kept_recent: 0 });
        assert_eq!(s3.list(&live).await.unwrap(), vec![kept]);
        assert!(s3.list(&gone).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn the_local_folder_keeps_the_store_contract() {
        let root = std::env::temp_dir().join(format!("konae-local-store-{}", uuid::Uuid::new_v4()));
        store_contract(&LocalFolder::new(&root), &root.join("scratch")).await;
        let _ = std::fs::remove_dir_all(root);
    }
}
