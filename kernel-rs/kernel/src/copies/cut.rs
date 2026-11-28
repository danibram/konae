use super::{history, DEPLOYMENT_DATABASES};
use crate::{file_store::Blob, layout, AppState};
use anyhow::{ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use konae_backup::SqliteSnapshot;
use std::{
    collections::HashMap,
    fs::{self, File, Metadata},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Leave time for the task that releases the permits to run before two seconds.
const PAUSE: Duration = Duration::from_millis(1850);

fn regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file(),
                "Expected a regular file at {}.",
                path.display()
            );
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

struct Budget {
    deadline: Instant,
    bytes: u64,
    readers: usize,
}

impl Budget {
    fn check(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.deadline,
            "The copy exceeded its staging pause."
        );
        Ok(())
    }

    fn reserve(&mut self, bytes: u64) -> Result<()> {
        self.check()?;
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .context("Insufficient staging space for the copy.")?;
        Ok(())
    }

    fn reader(&mut self) -> Result<()> {
        self.readers = self
            .readers
            .checked_sub(1)
            .context("The copy exceeded its reader limit.")?;
        Ok(())
    }

    fn file(&mut self, from: &Path, to: &Path) -> Result<u64> {
        let mut input = File::open(from)?;
        let mut output = File::create(to)?;
        let mut total = 0;
        let mut buf = [0; 64 * 1024];
        loop {
            self.check()?;
            let n = input.read(&mut buf)?;
            if n == 0 {
                return Ok(total);
            }
            self.reserve(n as u64)?;
            output.write_all(&buf[..n])?;
            total += n as u64;
        }
    }
}

struct Image {
    raw: PathBuf,
    target: PathBuf,
    wal: Option<SqliteSnapshot>,
}

struct Attachment {
    id: String,
    path: String,
    size: u64,
    expected: Option<String>,
    legacy: bool,
    reader: Option<Blob>,
    digest: Option<String>,
    version: Option<String>,
}

impl Attachment {
    async fn open(&mut self, state: &AppState, app: &str) -> Result<()> {
        let id = &self.id;
        let (blob, version) = state
            .files
            .store
            .get_for_copy(app, id)
            .await?
            .with_context(|| format!("Indexed file {id} is missing from the file store."))?;
        ensure!(
            blob.length == self.size,
            "File {id} has {} bytes in the store. The index requires {}.",
            blob.length,
            self.size
        );
        self.version = version;
        self.reader = Some(blob);
        Ok(())
    }

    async fn save(&mut self, staging: &Path) -> Result<()> {
        let blob = self
            .reader
            .take()
            .context("The attachment has no open reader.")?;
        let (size, digest) = crate::file_store::save_blob(blob, &staging.join(&self.path)).await?;
        ensure!(
            size == self.size,
            "Read {size} bytes for {}. The index requires {}.",
            self.path,
            self.size
        );
        ensure!(
            self.expected.as_ref().is_none_or(|hash| hash == &digest),
            "File {} does not match its recorded SHA-256.",
            self.path
        );
        self.digest = Some(digest);
        Ok(())
    }
}

struct Cut {
    budget: Budget,
    images: Vec<Image>,
    blobs: Vec<Attachment>,
}

impl Cut {
    fn database(&mut self, source: &Path, target: PathBuf) -> Result<()> {
        let pin = SqliteSnapshot::pin(source)
            .with_context(|| format!("Could not pin {}", source.display()))?;
        self.budget.reserve(pin.bytes.saturating_mul(2))?;
        let raw = target.with_extension("copy-image");
        let wal = if pin.wal {
            self.budget.reader()?;
            Some(pin)
        } else {
            pin.materialize(&raw, || self.budget.check())?;
            None
        };
        self.images.push(Image { raw, target, wal });
        Ok(())
    }

    fn deployment(&mut self, source: &Path, target: &Path) -> Result<()> {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            self.budget.check()?;
            let entry = entry?;
            let name = entry.file_name();
            let (from, to) = (entry.path(), target.join(&name));
            let kind = entry.file_type()?;
            match name.to_str() {
                Some(layout::SETTINGS_FILE) if kind.is_file() => {
                    self.budget.file(&from, &to)?;
                }
                Some("app" | "dist" | "server") if kind.is_dir() => self.tree(&from, &to)?,
                Some("data") if kind.is_dir() => {
                    fs::create_dir_all(&to)?;
                    for name in DEPLOYMENT_DATABASES {
                        let path = from.join(name);
                        if regular(&path)? {
                            self.database(&path, to.join(name))?;
                        }
                    }
                    let directory = from.join("files");
                    if directory.try_exists()? {
                        ensure!(
                            directory.symlink_metadata()?.is_dir(),
                            "Expected an app files directory."
                        );
                    }
                    let index = directory.join("index.json");
                    if regular(&index)? {
                        fs::create_dir_all(to.join("files"))?;
                        self.budget.file(&index, &to.join("files/index.json"))?;
                    }
                }
                Some(".konae") => {
                    ensure!(kind.is_dir(), "The app's .konae path must be a directory.");
                    fs::create_dir_all(&to)?;
                    for name in [
                        "deployment.json",
                        "sources.json",
                        crate::maintenance::MARKER_FILE,
                        crate::domain::RECORD_FILE,
                        crate::path::RECORD_FILE,
                        crate::hostname::RECORD_FILE,
                    ] {
                        let path = from.join(name);
                        if regular(&path)? {
                            self.budget.file(&path, &to.join(name))?;
                        }
                    }
                    let history_dir = from.join("backups");
                    if history_dir.try_exists()? {
                        ensure!(
                            history_dir.symlink_metadata()?.is_dir(),
                            "The app's backups path must be a directory."
                        );
                        history::stage_records(&history_dir, &to.join("backups"), |a, b| {
                            self.budget.file(a, b)
                        })?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn tree(&mut self, source: &Path, target: &Path) -> Result<()> {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            self.budget.check()?;
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') && name != ".konae" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.tree(&entry.path(), &target.join(name))?;
            } else if kind.is_file() {
                self.budget.file(&entry.path(), &target.join(name))?;
            }
        }
        Ok(())
    }

    async fn attachments(
        &mut self,
        state: &AppState,
        staging: &Path,
        app: &str,
        prior: &HashMap<&str, &str>,
    ) -> Result<()> {
        let index = layout::files(staging, app).join("index.json");
        let index_bytes = match fs::read(index) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"[]".to_vec(),
            Err(error) => return Err(error.into()),
        };
        let references = crate::modules::blob_references(&state.data_root, app)?;
        if !references.is_empty() {
            fs::create_dir_all(layout::files(staging, app))?;
        }
        for mut file in attachments(app, &index_bytes, prior, references)? {
            self.budget.reserve(file.size)?;
            self.budget.reader()?;
            file.open(state, app).await?;
            self.blobs.push(file);
        }
        Ok(())
    }
}

fn attachments(
    app: &str,
    index: &[u8],
    prior: &HashMap<&str, &str>,
    references: Vec<crate::modules::BlobReference>,
) -> Result<Vec<Attachment>> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(index)?;
    let indexed = entries.iter().map(|entry| {
        let id = entry["id"].as_str().context("The file index has an invalid ID.")?;
        Ok::<_, anyhow::Error>((id.to_owned(), entry))
    }).collect::<Result<HashMap<_, _>>>()?;
    references.into_iter().map(|reference| {
            let id = reference.id.as_str();
            ensure!(crate::files::valid_file_id(id), "Blob reference {id} has an invalid ID.");
            let entry = indexed.get(id).copied();
            let size = reference.size;
            let indexed_digest = entry.and_then(|entry| entry
                .get("sha256"))
                .map(|v| v.as_str().context("Invalid indexed SHA-256."))
                .transpose()?;
            let path = format!("apps/{app}/data/files/{id}.blob");
            Ok(Attachment {
                id: id.to_owned(),
                expected: indexed_digest
                    .or_else(|| prior.get(path.as_str()).copied())
                    .map(str::to_owned),
                path,
                size,
                legacy: indexed_digest.is_none(),
                reader: None,
                digest: None,
                version: None,
            })
        })
        .collect()
}

struct Index {
    // Keeping the old file open prevents the filesystem from recycling its inode.
    _file: File,
    metadata: Metadata,
    bytes: Vec<u8>,
}

struct AppInventory {
    index: Option<Index>,
    references: Vec<crate::modules::BlobReference>,
}

impl AppInventory {
    fn matches(&self, current: &Self) -> bool {
        let index_matches = match (&self.index, &current.index) {
            (Some(old), Some(new)) => old.matches(new),
            (None, None) => true,
            _ => false,
        };
        index_matches && self.references == current.references
    }
}

impl Index {
    fn open(path: &Path) -> Result<Option<Self>> {
        if !regular(path)? {
            return Ok(None);
        }
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(Self {
            _file: file,
            metadata,
            bytes,
        }))
    }

    fn matches(&self, current: &Self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if self.metadata.dev() != current.metadata.dev()
                || self.metadata.ino() != current.metadata.ino()
            {
                return false;
            }
        }
        self.metadata.modified().ok() == current.metadata.modified().ok()
            && self.bytes == current.bytes
    }
}

fn indexes(root: &Path, mut visit: impl FnMut(String, AppInventory) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(layout::apps(root))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let app = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Invalid app folder name."))?;
        let index = Index::open(&layout::files(root, &app).join("index.json"))?;
        let references = crate::modules::blob_references(root, &app)?;
        if index.is_some() || !references.is_empty() {
            visit(app, AppInventory { index, references })?;
        }
    }
    Ok(())
}

pub(super) struct CheckedFile {
    pub path: String,
    pub size: u64,
    pub digest: String,
    pub legacy: bool,
    version: Option<String>,
}

#[derive(Default)]
pub(super) struct SourceCache {
    directory: Option<super::Staging>,
    files: HashMap<String, CheckedFile>,
}

impl SourceCache {
    pub(super) fn keep(&mut self, staging: &Path, files: Vec<CheckedFile>) -> Result<()> {
        let files: Vec<_> = files
            .into_iter()
            .filter(|file| file.version.is_some())
            .collect();
        if files.is_empty() {
            *self = Self::default();
            return Ok(());
        }
        let parent = staging
            .parent()
            .context("The copy has no staging directory.")?;
        let cache = super::Staging(parent.join(format!("cache-{}", uuid::Uuid::new_v4())));
        // The staging directory already limits access to the files. Keep a single generation.
        let mut options = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut options, 0o700);
        options.create(&cache.0)?;
        let mut entries = HashMap::new();
        for file in files {
            let target = cache.0.join(&file.path);
            fs::create_dir_all(
                target
                    .parent()
                    .context("The cached file has no directory.")?,
            )?;
            fs::hard_link(staging.join(&file.path), target)?;
            entries.insert(file.path.clone(), file);
        }
        *self = Self {
            directory: Some(cache),
            files: entries,
        };
        Ok(())
    }

    fn evict_if_space_is_low(&mut self, available: u64) {
        let retained = self.files.values().map(|file| file.size).sum::<u64>();
        if available / 2 < retained {
            *self = Self::default();
        }
    }

    async fn reuse(
        &self,
        state: &AppState,
        app: &str,
        staging: &Path,
        file: &mut Attachment,
    ) -> bool {
        let Some(cached) = self.files.get(&file.path) else {
            return false;
        };
        let Some(version) = cached.version.as_deref() else {
            return false;
        };
        let Some(directory) = &self.directory else {
            return false;
        };
        if !file
            .expected
            .as_ref()
            .is_none_or(|expected| expected == &cached.digest)
            || !state
                .files
                .store
                .confirms_version(app, &file.id, version, file.size)
                .await
        {
            return false;
        }
        let source = directory.0.join(&file.path);
        let target = staging.join(&file.path);
        let size = file.size;
        let digest = cached.digest.clone();
        let reused = tokio::task::spawn_blocking(move || -> Result<bool> {
            if super::checksum_file(&source)? != (size, digest) {
                return Ok(false);
            }
            fs::hard_link(source, target)?;
            Ok(true)
        })
        .await
        .is_ok_and(|outcome| outcome.unwrap_or_default());
        if reused {
            file.digest = Some(cached.digest.clone());
            file.version = Some(version.to_owned());
        }
        reused
    }
}

struct Prepared {
    scratch: super::Staging,
    indexes: HashMap<String, AppInventory>,
    blobs: Result<Vec<Attachment>>,
}

impl Prepared {
    async fn read(
        state: &AppState,
        staging: &Path,
        prior: &HashMap<&str, &str>,
        bytes: u64,
        cache: &SourceCache,
    ) -> Result<Self> {
        let started = Instant::now();
        let scratch = super::Staging(staging.join(format!(".prepare-{}", uuid::Uuid::new_v4())));
        let staging = &scratch.0;
        fs::create_dir_all(layout::apps(staging))?;
        let mut captured = HashMap::new();
        indexes(&state.data_root, |app, inventory| {
            captured.insert(app, inventory);
            Ok(())
        })?;
        let blobs = tokio::time::timeout(Duration::from_secs(60), async {
            let mut pending = Vec::new();
            let mut remaining = bytes;
            for (app, inventory) in &captured {
                let index = inventory.index.as_ref().map_or(&b"[]"[..], |index| index.bytes.as_slice());
                let files = attachments(app, index, prior, inventory.references.clone())?;
                for file in &files {
                    remaining = remaining
                        .checked_sub(file.size)
                        .context("Insufficient staging space for the copy.")?;
                }
                if !files.is_empty() {
                    fs::create_dir_all(layout::files(staging, app))?;
                }
                pending.extend(files.into_iter().map(|file| (app.clone(), file)));
            }
            stream::iter(pending)
                .map(|(app, mut file)| async move {
                    if !cache.reuse(state, &app, staging, &mut file).await {
                        file.open(state, &app).await?;
                        file.save(staging).await?;
                    }
                    Ok(file)
                })
                .buffer_unordered(8)
                .try_collect::<Vec<_>>()
                .await
        })
        .await
        .context("Timed out preparing files from the file store.")
        .and_then(|outcome| outcome);
        if let Ok(blobs) = &blobs {
            println!(
                "Copy prepared {} remote files ({} bytes) in {} ms.",
                blobs.len(),
                blobs.iter().map(|file| file.size).sum::<u64>(),
                started.elapsed().as_millis()
            );
        }
        Ok(Self {
            scratch,
            indexes: captured,
            blobs,
        })
    }

    fn install(&mut self, staging: &Path) -> Result<Vec<Attachment>> {
        let blobs = std::mem::replace(&mut self.blobs, Ok(Vec::new()))?;
        let apps = layout::apps(staging);
        fs::remove_dir(&apps)?;
        fs::rename(layout::apps(&self.scratch.0), apps)?;
        Ok(blobs)
    }

    fn matches(&self, root: &Path, deadline: Instant) -> Result<bool> {
        let mut count = 0;
        let mut matches = true;
        indexes(root, |app, current| {
            ensure!(
                Instant::now() < deadline,
                "The copy exceeded its staging pause."
            );
            count += 1;
            matches &= self.indexes.get(&app).is_some_and(|old| old.matches(&current));
            Ok(())
        })?;
        Ok(matches && count == self.indexes.len())
    }
}

fn limit(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => Ok(value
            .parse()
            .with_context(|| format!("Expected an unsigned integer for {name}."))?),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

async fn volume(root: &Path) -> Result<File> {
    let volume = File::open(root)?;
    loop {
        match volume.try_lock() {
            Ok(()) => return Ok(volume),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

struct Views {
    at: u64,
    cut: Cut,
    imports: File,
    generations: tokio::sync::OwnedRwLockWriteGuard<()>,
}

async fn pin(
    state: &AppState,
    staging: &Path,
    prior: &HashMap<&str, &str>,
    bytes: u64,
    readers: usize,
    prepared: Option<Prepared>,
    pause: Duration,
) -> Result<Option<Views>> {
    let mut prepared = prepared;
    let start = Instant::now();
    let deadline = start + pause;
    let until = tokio::time::Instant::from_std(deadline);
    let drain = async {
        let gate = state.copies.operations.write().await;
        let external = volume(&state.data_root).await?;
        let imports = volume(&layout::apps(&state.data_root)).await?;
        let generations = state.copies.generations.clone().write_owned().await;
        ensure!(
            tokio::task::spawn_blocking(crate::usage::flush_status).await?,
            "Could not drain Usage."
        );
        Ok::<_, anyhow::Error>((gate, external, imports, generations))
    };
    let (gate, external, imports, generations) = tokio::time::timeout_at(until, drain)
        .await
        .context("The copy timed out draining operations.")??;
    let drained = start.elapsed();
    super::operation::check(&state.data_root)?;
    (state.installation.check_cut)(&state.data_root)?;
    // A file writer publishes a new index after storing its bytes. A failed
    // replacement leaves an operation record, which the cut checks above.
    let remote = prepared.is_some();
    let blobs = if let Some(prepared) = prepared.as_mut() {
        let matches = prepared.matches(&state.data_root, deadline)?;
        ensure!(
            Instant::now() < deadline,
            "The copy exceeded its staging pause."
        );
        if !matches {
            drop((gate, external, imports, generations));
            println!("Copy released writers after {} ms (drain {} ms). The file index changed during preparation.",
                start.elapsed().as_millis(), drained.as_millis());
            return Ok(None);
        }
        prepared.indexes.clear();
        prepared.install(staging)?
    } else {
        Vec::new()
    };
    let at = crate::now_ms();
    let mut cut = Cut {
        budget: Budget {
            deadline,
            bytes,
            readers,
        },
        images: vec![],
        blobs,
    };
    for file in &cut.blobs {
        cut.budget.reserve(file.size)?;
    }
    tokio::time::timeout_at(until, async {
        for db in &state.installation.databases {
            let source = state.data_root.join(db.file);
            if regular(&source)? {
                cut.database(&source, staging.join(db.file))?;
            }
        }
        for entry in fs::read_dir(layout::apps(&state.data_root))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let app = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid app folder name."))?;
            cut.deployment(&entry.path(), &layout::folder(staging, &app))?;
            if !remote {
                cut.attachments(state, staging, &app, prior).await?;
            }
        }
        cut.budget.check()
    })
    .await
    .context("The copy timed out staging the common cut.")??;
    drop((gate, external));
    let paused = start.elapsed();
    drop(prepared);
    println!(
        "Copy cut admitted writers after {} ms (drain {} ms, {} databases, {} open readers).",
        paused.as_millis(),
        drained.as_millis(),
        cut.images.len(),
        readers - cut.budget.readers
    );

    Ok(Some(Views {
        at,
        cut,
        imports,
        generations,
    }))
}

impl Views {
    async fn write(self, staging: &Path) -> Result<Vec<CheckedFile>> {
        let Self {
            cut,
            imports,
            generations,
            ..
        } = self;
        // Replacements wait until every SQLite handle has closed, including WAL and SHM.
        // Cancellation closes the sender even when the blocking task is still running.
        let (cancel, stopped) = tokio::sync::oneshot::channel::<()>();
        let compact = tokio::task::spawn_blocking(move || -> Result<_> {
            let _guards = (imports, generations);
            let mut stopped = stopped;
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut paths = Vec::new();
            for image in cut.images {
                if let Some(pin) = image.wal {
                    pin.materialize(&image.raw, || {
                        ensure!(
                            stopped.try_recv()
                                == Err(tokio::sync::oneshot::error::TryRecvError::Empty),
                            "The copy was cancelled."
                        );
                        ensure!(Instant::now() < deadline, "Timed out copying WAL pages.");
                        Ok(())
                    })?;
                }
                paths.push((image.raw, image.target));
            }
            Ok(paths)
        })
        .await??;
        drop(cancel);
        for (raw, target) in compact {
            tokio::task::spawn_blocking(move || -> Result<()> {
                konae_backup::vacuum_into(&raw, &target)?;
                fs::remove_file(raw)?;
                Ok(())
            })
            .await??;
        }
        let mut checked = Vec::new();
        for mut file in cut.blobs {
            if file.reader.is_some() {
                file.save(staging).await?;
            }
            checked.push(CheckedFile {
                path: file.path,
                size: file.size,
                digest: file.digest.context("Missing staged digest.")?,
                legacy: file.legacy,
                version: file.version,
            });
        }
        Ok(checked)
    }
}

pub(super) async fn stage(
    state: &AppState,
    staging: &Path,
    prior: &HashMap<&str, &str>,
    cache: &mut SourceCache,
) -> Result<(u64, Vec<CheckedFile>)> {
    cache.evict_if_space_is_low(crate::package::free_bytes(staging)?);
    let bytes =
        limit("KONAE_COPY_STAGE_BYTES", u64::MAX)?.min(crate::package::free_bytes(staging)?);
    let readers = usize::try_from(limit("KONAE_COPY_READERS", 512)?)?;
    let remote = state.files.store.kind() != "local";
    let mut pause = PAUSE;
    let mut pinned = None;
    for _ in 0..3 {
        let prepared = if remote {
            Some(Prepared::read(state, staging, prior, bytes, cache).await?)
        } else {
            None
        };
        let start = Instant::now();
        pinned = pin(state, staging, prior, bytes, readers, prepared, pause).await?;
        if pinned.is_some() {
            break;
        }
        pause = pause.saturating_sub(start.elapsed());
        ensure!(!pause.is_zero(), "The copy exceeded its staging pause.");
    }
    let views =
        pinned.context("The file indexes kept changing while the copy prepared the files.")?;
    let at = views.at;
    let start = Instant::now();
    let files = tokio::time::timeout(Duration::from_secs(60), views.write(staging))
        .await
        .context("Timed out materializing the pinned copy.")??;
    println!(
        "Copy materialized the pinned database and file views in {} ms.",
        start.elapsed().as_millis()
    );
    Ok((at, files))
}

#[cfg(test)]
pub(super) fn test_deployment(source: &Path, target: &Path) -> Result<()> {
    let budget = Budget {
        deadline: Instant::now() + PAUSE,
        bytes: u64::MAX,
        readers: 32,
    };
    Cut {
        budget,
        images: vec![],
        blobs: vec![],
    }
    .deployment(source, target)
}

#[cfg(test)]
mod tests;
