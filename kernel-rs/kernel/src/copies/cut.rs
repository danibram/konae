use super::{history, DEPLOYMENT_DATABASES};
use crate::{
    file_store::{Blob, CopyBlob},
    layout, AppState,
};
use anyhow::{ensure, Context, Result};
use konae_backup::SqliteSnapshot;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Leave time for the task that releases the permits to run before two seconds.
const PAUSE: Duration = Duration::from_millis(1850);

fn regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(meta.is_file(), "Expected a regular file at {}.", path.display());
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
        ensure!(Instant::now() < self.deadline, "The copy exceeded its staging pause.");
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
    path: String,
    size: u64,
    expected: Option<String>,
    legacy: bool,
    reader: Option<Blob>,
    digest: Option<String>,
}

impl Attachment {
    async fn save(&mut self, staging: &Path) -> Result<()> {
        let blob = self.reader.take().context("The attachment has no open reader.")?;
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
        let pin = SqliteSnapshot::pin(source).with_context(|| format!("Could not pin {}", source.display()))?;
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
                Some("app" | "dist") if kind.is_dir() => self.tree(&from, &to)?,
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
                        history::stage_records(&history_dir, &to.join("backups"), |a, b| self.budget.file(a, b))?;
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
        prior: &std::collections::HashMap<&str, &str>,
    ) -> Result<()> {
        let index = layout::files(staging, app).join("index.json");
        if !index.try_exists()? {
            return Ok(());
        }
        let entries: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(index)?)?;
        let mut seen = HashSet::new();
        for entry in &entries {
            let id = entry["id"]
                .as_str()
                .filter(|id| crate::valid_file_id(id))
                .context("The file index has an invalid ID.")?;
            ensure!(seen.insert(id), "The file index lists {id} twice.");
            let size = entry["size"].as_u64().context("An indexed file has no size.")?;
            self.budget.reserve(size)?;
            let path = format!("apps/{app}/data/files/{id}.blob");
            let indexed = entry
                .get("sha256")
                .map(|v| v.as_str().context("Invalid indexed SHA-256."))
                .transpose()?;
            let mut file = Attachment {
                expected: indexed.or_else(|| prior.get(path.as_str()).copied()).map(str::to_owned),
                path,
                size,
                legacy: indexed.is_none(),
                reader: None,
                digest: None,
            };
            let object = state
                .files
                .store
                .open_for_copy(app, id)
                .await?
                .with_context(|| format!("Indexed file {id} is missing from the file store."))?;
            let pinned = matches!(object, CopyBlob::Pinned(_));
            let blob = match object {
                CopyBlob::Pinned(blob) | CopyBlob::Unversioned(blob) => blob,
            };
            ensure!(
                blob.length == size,
                "File {id} has {} bytes in the store. The index requires {size}.",
                blob.length
            );
            file.reader = Some(blob);
            if pinned {
                self.budget.reader()?;
            } else {
                file.save(staging).await?;
            }
            self.blobs.push(file);
        }
        Ok(())
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
            Err(std::fs::TryLockError::WouldBlock) => tokio::time::sleep(Duration::from_millis(5)).await,
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
    prior: &std::collections::HashMap<&str, &str>,
    bytes: u64,
    readers: usize,
) -> Result<Views> {
    let start = Instant::now();
    let deadline = start + PAUSE;
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
    let at = crate::now_ms();
    let mut cut = Cut {
        budget: Budget {
            deadline,
            bytes,
            readers,
        },
        images: vec![],
        blobs: vec![],
    };
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
            cut.attachments(state, staging, &app, prior).await?;
        }
        cut.budget.check()
    })
    .await
    .context("The copy timed out staging the common cut.")??;
    drop((gate, external));
    println!(
        "Copy cut admitted writers after {} ms (drain {} ms, {} databases, {} open readers).",
        start.elapsed().as_millis(),
        drained.as_millis(),
        cut.images.len(),
        readers - cut.budget.readers
    );

    Ok(Views {
        at,
        cut,
        imports,
        generations,
    })
}

impl Views {
    async fn write(self, staging: &Path) -> Result<Vec<(String, u64, String, bool)>> {
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
                            stopped.try_recv() == Err(tokio::sync::oneshot::error::TryRecvError::Empty),
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
            checked.push((
                file.path,
                file.size,
                file.digest.context("Missing staged digest.")?,
                file.legacy,
            ));
        }
        Ok(checked)
    }
}

pub(super) async fn stage(
    state: &AppState,
    staging: &Path,
    prior: &std::collections::HashMap<&str, &str>,
) -> Result<(u64, Vec<(String, u64, String, bool)>)> {
    let bytes = limit("KONAE_COPY_STAGE_BYTES", u64::MAX)?.min(crate::package::free_bytes(staging)?);
    let readers = usize::try_from(limit("KONAE_COPY_READERS", 512)?)?;
    let views = pin(state, staging, prior, bytes, readers).await?;
    let at = views.at;
    let files = tokio::time::timeout(Duration::from_secs(60), views.write(staging))
        .await
        .context("Timed out materializing the pinned copy.")??;
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
