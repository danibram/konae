use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::{
    ServerArtifact,
    wasm_validation::{PrepareStatus, WasmManifest, helper_refusal, verify_component},
};

const PREPARE_LIMIT: Duration = Duration::from_secs(90);
const COMPONENT_LIMIT: usize = 64 * 1024 * 1024;
const NATIVE_RESERVATION_LIMIT: u64 = 512 * 1024 * 1024;
const DEFAULT_CACHE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const NATIVE_VERIFY_READERS: usize = 4;
const PREPARATION_JOBS: usize = 4;
static CONFIGURATION: OnceLock<Result<Option<Preparation>, String>> = OnceLock::new();
static QUEUE: Semaphore = Semaphore::const_new(PREPARATION_JOBS);
static SERIAL: Mutex<()> = Mutex::new(());
static JOB_KEYS: OnceLock<Mutex<HashSet<JobKey>>> = OnceLock::new();

#[derive(Clone, Eq, Hash, PartialEq)]
enum JobKey {
    Candidate(String),
    Repair(String),
}

struct PreparationJob {
    _permit: tokio::sync::SemaphorePermit<'static>,
    key: Option<JobKey>,
    queue_deadline: Instant,
}

impl PreparationJob {
    fn claim(key: Option<JobKey>) -> Result<Self> {
        let permit = QUEUE
            .try_acquire()
            .context("Function preparation queue is full")?;
        if let Some(key) = &key {
            ensure!(
                JOB_KEYS
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .insert(key.clone()),
                "Function preparation is already running for this app"
            );
        }
        Ok(Self {
            _permit: permit,
            key,
            queue_deadline: Instant::now() + PREPARE_LIMIT,
        })
    }
}

impl Drop for PreparationJob {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            JOB_KEYS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .remove(key);
        }
    }
}

/// Shared file locks keep verified native objects in the cache until release.
#[derive(Default)]
pub struct PreparedPin(Vec<File>);

type ReleasePin = Arc<Mutex<PreparedPin>>;

pub(super) struct Preparation {
    helper: PathBuf,
    identity: HelperIdentity,
    native: Option<NativeCache>,
}

struct NativeCache {
    runner: PathBuf,
    directory: PathBuf,
    target: NativeTarget,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperIdentity {
    abi: String,
    engine_sha256: String,
    builder_fingerprint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperReply {
    status: String,
    manifest: Option<WasmManifest>,
    diagnostic: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeTarget {
    wasmtime: String,
    target_fingerprint: String,
}

#[derive(Debug)]
pub(crate) struct ConfigurationError(String);
impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigurationError {}

pub(super) fn configuration() -> Result<Option<&'static Preparation>> {
    CONFIGURATION
        .get_or_init(|| Preparation::from_env().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map(Option::as_ref)
        .map_err(|error| {
            let message = if super::runtime::wasm_selected() {
                format!("Server functions need a Wasm runner, preparer and cache. Set KONAE_FUNCTIONS_RUNNER, KONAE_FUNCTIONS_PREPARER and KONAE_FUNCTIONS_WASM_CACHE, or set KONAE_FUNCTIONS_RUNNER_KIND=deno to use the deno runner. {error}")
            } else {
                error.clone()
            };
            ConfigurationError(message).into()
        })
}

impl Preparation {
    fn from_env() -> Result<Option<Self>> {
        let wasm = super::runtime::wasm_selected();
        let packaged = Path::new("/usr/local/lib/konae/functions-prepare");
        let helper = if std::env::var_os("KONAE_FUNCTIONS_PREPARER").is_some() {
            required_executable("KONAE_FUNCTIONS_PREPARER")?
        } else if packaged.is_file() {
            packaged.to_owned()
        } else if wasm {
            bail!("The Wasm preparer is missing");
        } else {
            return Ok(None);
        };
        let deadline = Instant::now() + PREPARE_LIMIT;
        let identity: HelperIdentity = json_command(&helper, &["identity"], deadline)?;
        ensure!(
            identity.abi == "konae-functions-wasm/1",
            "The configured preparer uses an unsupported Wasm ABI"
        );
        let native = if wasm {
            let runner = super::runtime::runner_path(true).context("The Wasm runner is missing")?;
            ensure!(
                runner.is_absolute() && runner.is_file(),
                "KONAE_FUNCTIONS_RUNNER must name an absolute executable file"
            );
            if std::env::var("KONAE_FUNCTIONS_DEVELOPMENT").as_deref() != Ok("1") {
                #[cfg(not(target_os = "linux"))]
                bail!(
                    "The Wasm production worker policy requires Linux; use KONAE_FUNCTIONS_DEVELOPMENT=1 for local development"
                );
                #[cfg(target_os = "linux")]
                konae_functions_executor::validate_linux_worker_identity()
                    .map_err(|_| anyhow::anyhow!("Set KONAE_FUNCTIONS_UID and KONAE_FUNCTIONS_GID to a separate worker identity and grant the Kernel CAP_KILL, CAP_SETUID and CAP_SETGID"))?;
            }
            let directory = cache_directory()?;
            check_cache_directory(&directory)?;
            cache_limit()?;
            let target: NativeTarget = json_command(&runner, &["--native-target"], deadline)?;
            ensure!(
                target.wasmtime == "38.0.4",
                "The configured runner uses an unsupported Wasmtime version"
            );
            Some(NativeCache {
                runner,
                directory,
                target,
            })
        } else {
            None
        };
        Ok(Some(Self {
            helper,
            identity,
            native,
        }))
    }

    fn prepare(
        &self,
        bundles: &BTreeMap<String, String>,
        job: PreparationJob,
    ) -> Result<PreparedPin> {
        let deadline = job.queue_deadline;
        let _serial = loop {
            check_deadline(deadline)?;
            if let Ok(lock) = SERIAL.try_lock() {
                break lock;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let cache = self.native.as_ref();
        let _cache_lock = cache.map(|cache| cache.lock(deadline)).transpose()?;
        let mut pins = PreparedPin::default();
        let mut aliases = HashSet::new();
        for (name, source) in bundles {
            let alias = cache.map(|cache| alias_key(source, &self.identity, &cache.target));
            if let (Some(cache), Some(alias)) = (cache, &alias) {
                if !aliases.insert(alias.clone()) {
                    continue;
                }
                if let Some(file) = cached_object(&cache.directory, alias)? {
                    pin_file(&mut pins, file)?;
                    continue;
                }
            }

            let scratch = match cache {
                Some(cache) => cache.directory.clone(),
                None => std::env::temp_dir().canonicalize()?,
            };
            let staging = Staging::new(&scratch)?;
            let (manifest, component) = self.portable_component(&staging, name, source)?;
            if let (Some(cache), Some(alias)) = (cache, alias) {
                let file = cache
                    .install(&staging, &alias, &manifest, &component)
                    .with_context(|| format!("Prepare native component for {name}"))?;
                pin_file(&mut pins, file)?;
            }
        }
        Ok(pins)
    }

    fn portable_component(
        &self,
        staging: &Staging,
        name: &str,
        source: &str,
    ) -> Result<(WasmManifest, File)> {
        let input = staging.write(name, source.as_bytes())?;
        let output = staging.path.join("component.wasm");
        let reply: HelperReply = json_command(
            &self.helper,
            &[
                "prepare",
                "--input",
                input.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ],
            Instant::now() + PREPARE_LIMIT,
        )
        .with_context(|| format!("Prepare portable component for {name}"))?;
        if reply.status != "ok" {
            let status = parse_prepare_status(&reply.status)?;
            if let Some(diagnostic) = reply.diagnostic {
                eprintln!(
                    "konae: function {name} preparation returned {}: {diagnostic:?}",
                    reply.status
                );
            }
            return Err(helper_refusal(status, name));
        }
        let manifest = reply
            .manifest
            .context("Wasm preparer returned no manifest")?;
        let mut file = File::open(output).context("Read prepared Wasm component")?;
        let mut component = Vec::new();
        (&mut file)
            .take(COMPONENT_LIMIT as u64 + 1)
            .read_to_end(&mut component)?;
        ensure!(
            component.len() <= COMPONENT_LIMIT,
            "Prepared Wasm component exceeds 64 MiB"
        );
        let input_sha256 = format!("{:x}", Sha256::digest(source.as_bytes()));
        verify_component(&manifest, &input_sha256, &component)?;
        file.rewind()?;
        Ok((manifest, file))
    }

    pub(super) fn prepared_bundles(
        &'static self,
        bundles: &BTreeMap<String, String>,
        pin: Option<ReleasePin>,
    ) -> Result<BTreeMap<String, File>> {
        let cache = self
            .native
            .as_ref()
            .context("Wasm runner requires a native component cache")?;
        check_cache_directory(&cache.directory)?;
        let entries: Vec<_> = bundles
            .iter()
            .map(|(name, source)| (name, alias_key(source, &self.identity, &cache.target)))
            .collect();
        let open_entry = |(name, key): &(&String, String)| -> Result<_> {
            Ok((
                name.to_string(),
                key.clone(),
                cached_object(&cache.directory, key)?,
            ))
        };
        let opened = if entries.len() <= NATIVE_VERIFY_READERS {
            entries.iter().map(open_entry).collect::<Result<Vec<_>>>()?
        } else {
            let chunk_size = entries.len().div_ceil(NATIVE_VERIFY_READERS);
            std::thread::scope(|scope| {
                let readers = entries
                    .chunks(chunk_size)
                    .map(|chunk| {
                        scope.spawn(|| chunk.iter().map(open_entry).collect::<Result<Vec<_>>>())
                    })
                    .collect::<Vec<_>>();
                let mut opened = Vec::new();
                for reader in readers {
                    opened.extend(
                        reader
                            .join()
                            .map_err(|_| anyhow::anyhow!("Wasm cache verification stopped"))??,
                    );
                }
                Ok::<_, anyhow::Error>(opened)
            })?
        };
        let mut files = BTreeMap::new();
        for (name, key, file) in opened {
            let Some(file) = file else {
                self.repair(key, bundles, pin);
                return Err(konae_functions_executor::WorkerError::new("cache_busy").into());
            };
            files.insert(name, file);
        }
        Ok(files)
    }

    fn repair(
        &'static self,
        key: String,
        bundles: &BTreeMap<String, String>,
        pin: Option<ReleasePin>,
    ) {
        let Ok(job) = PreparationJob::claim(Some(JobKey::Repair(key))) else {
            return;
        };
        let bundles = bundles.clone();
        tokio::task::spawn_blocking(move || match self.prepare(&bundles, job) {
            Ok(prepared) => {
                if let Some(pin) = pin {
                    *pin.lock().unwrap() = prepared;
                }
            }
            Err(error) => eprintln!("konae: Wasm cache repair failed: {error:#}"),
        });
    }
}

pub async fn check_preparation(
    state: &crate::AppState,
    project: &str,
    artifact: &ServerArtifact,
) -> Result<bool> {
    if !state.functions.preparation_enabled() {
        return Ok(false);
    }
    Ok(
        prepare_artifact_with(artifact, Some(JobKey::Candidate(project.to_owned())))
            .await?
            .is_some(),
    )
}

/// Verifies initialization and, when serving Wasm, pins the native components.
pub async fn prepare_artifact(
    state: &crate::AppState,
    artifact: Option<&ServerArtifact>,
) -> Result<PreparedPin> {
    let Some(artifact) = artifact else {
        return Ok(PreparedPin::default());
    };
    if !state.functions.preparation_enabled() || artifact.bundles.is_empty() {
        return Ok(PreparedPin::default());
    }
    Ok(prepare_artifact_with(artifact, None)
        .await?
        .unwrap_or_default())
}

async fn prepare_artifact_with(
    artifact: &ServerArtifact,
    key: Option<JobKey>,
) -> Result<Option<PreparedPin>> {
    let job = PreparationJob::claim(key)?;
    super::validate_artifact(Some(&artifact.manifest), Some(artifact))?;
    let bundles = artifact.bundles.clone();
    tokio::task::spawn_blocking(move || {
        let Some(preparation) = configuration()? else {
            return Ok(None);
        };
        preparation.prepare(&bundles, job).map(Some)
    })
    .await
    .context("Wasm preparation task stopped")?
}

#[cfg(test)]
pub(super) fn prepare_test_bundles(bundles: &BTreeMap<String, String>) -> Result<PreparedPin> {
    configuration()?
        .context("No preparer configured")?
        .prepare(bundles, PreparationJob::claim(None)?)
}

fn pin_file(pins: &mut PreparedPin, file: File) -> Result<()> {
    lock_file(&file, libc::LOCK_SH | libc::LOCK_NB).context("Pin prepared Wasm object")?;
    pins.0.push(file);
    Ok(())
}

fn lock_file(file: &File, operation: i32) -> io::Result<()> {
    if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn check_deadline(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "Function preparation exceeded its deadline"
    );
    Ok(())
}

fn cache_directory() -> Result<PathBuf> {
    let cache = PathBuf::from(
        std::env::var_os("KONAE_FUNCTIONS_WASM_CACHE")
            .context("KONAE_FUNCTIONS_WASM_CACHE must name the operator-owned Wasm cache")?,
    );
    ensure!(
        cache.is_absolute(),
        "KONAE_FUNCTIONS_WASM_CACHE must be absolute"
    );
    Ok(cache)
}

fn check_cache_directory(cache: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(cache)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o722 == 0o700,
        "Unsafe Wasm cache directory"
    );
    Ok(())
}

fn required_executable(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(
        std::env::var_os(name)
            .with_context(|| format!("{name} must name an operator-installed executable"))?,
    );
    ensure!(
        path.is_absolute() && path.is_file(),
        "{name} must name an absolute executable file"
    );
    Ok(path)
}

impl NativeCache {
    fn lock(&self, deadline: Instant) -> Result<File> {
        let cache = &self.directory;
        check_cache_directory(cache)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(cache.join(".prepare.lock"))?;
        ensure!(
            safe_cache_metadata(&lock.metadata()?),
            "Unsafe Wasm cache lock"
        );
        loop {
            check_deadline(deadline)?;
            match lock_file(&lock, libc::LOCK_EX | libc::LOCK_NB) {
                Ok(()) => return Ok(lock),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error).context("Lock the Wasm preparation cache"),
            }
        }
    }

    fn install(
        &self,
        staging: &Staging,
        alias: &str,
        manifest: &WasmManifest,
        component: &File,
    ) -> Result<File> {
        let cache = &self.directory;
        let mut key_hash = Sha256::new();
        for value in [alias, &manifest.component_sha256] {
            key_hash.update(value.as_bytes());
            key_hash.update([0]);
        }
        let key = format!("{:x}", key_hash.finalize());
        let installed = cache.join(format!("{key}.cwasm"));
        let native = staging.path.join("component.cwasm");
        let cache_limit = cache_limit()?;
        let reservation = component
            .metadata()?
            .len()
            .saturating_mul(8)
            .saturating_add(8 * 1024 * 1024)
            .min(NATIVE_RESERVATION_LIMIT);
        ensure_cache_room(cache, reservation, cache_limit)?;
        compile_component(
            &self.runner,
            &native,
            component,
            Instant::now() + PREPARE_LIMIT,
        )?;
        let mut file =
            open_cache_file(&native)?.context("Native compiler wrote an unsafe component")?;
        let digest = native_digest(&mut file)?.context("Invalid native component size")?;
        if let Some(mut existing) = open_cache_file(&installed)? {
            if native_digest(&mut existing)?.as_deref() == Some(digest.as_str()) {
                install_alias(cache, alias, &installed, &digest)?;
                return Ok(existing);
            }
        }
        let replaced = fs::symlink_metadata(&installed)
            .ok()
            .filter(|entry| entry.is_file())
            .map_or(0, |entry| entry.len());
        ensure_cache_room(
            cache,
            file.metadata()?.len().saturating_sub(replaced),
            cache_limit,
        )?;
        replace_cache_entry(&native, &installed)?;
        install_alias(cache, alias, &installed, &digest)?;
        Ok(file)
    }
}

fn cache_limit() -> Result<u64> {
    match std::env::var("KONAE_FUNCTIONS_WASM_CACHE_BYTES") {
        Ok(value) => {
            let bytes = value
                .parse::<u64>()
                .context("KONAE_FUNCTIONS_WASM_CACHE_BYTES must be a positive byte count")?;
            ensure!(
                bytes > 0,
                "KONAE_FUNCTIONS_WASM_CACHE_BYTES must be a positive byte count"
            );
            Ok(bytes)
        }
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_CACHE_LIMIT),
        Err(error) => Err(error.into()),
    }
}

fn ensure_cache_room(cache: &Path, reserve: u64, limit: u64) -> Result<()> {
    let mut objects = Vec::new();
    let mut used = 0u64;
    for entry in fs::read_dir(cache)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "cwasm")
        {
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_file() {
                used = used.saturating_add(metadata.len());
                objects.push((
                    entry.path(),
                    metadata.len(),
                    metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                ));
            }
        }
    }
    if used.saturating_add(reserve) <= limit {
        return Ok(());
    }
    objects.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in objects {
        let file = open_cache_file(&path)?;
        if let Some(file) = &file {
            match lock_file(file, libc::LOCK_EX | libc::LOCK_NB) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let Some(filename) = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
        else {
            continue;
        };
        fs::remove_file(&path)?;
        used = used.saturating_sub(size);
        for entry in fs::read_dir(cache)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".alias-")
                && alias_target(&entry.path(), &filename)?
            {
                fs::remove_file(entry.path())?;
            }
        }
        if used.saturating_add(reserve) <= limit {
            return Ok(());
        }
    }
    anyhow::bail!("Wasm component cache budget is exhausted by pinned objects")
}

fn alias_key(source: &str, identity: &HelperIdentity, target: &NativeTarget) -> String {
    let mut hash = Sha256::new();
    let source_sha256 = format!("{:x}", Sha256::digest(source.as_bytes()));
    for value in [
        source_sha256.as_str(),
        &identity.abi,
        &identity.engine_sha256,
        &identity.builder_fingerprint,
        &target.wasmtime,
        &target.target_fingerprint,
    ] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

fn cached_object(cache: &Path, key: &str) -> Result<Option<File>> {
    let Some(alias) = open_cache_file(&cache.join(format!(".alias-{key}")))? else {
        return Ok(None);
    };
    let mut record = Vec::new();
    alias.take(256).read_to_end(&mut record)?;
    let Ok(record) = std::str::from_utf8(&record) else {
        return Ok(None);
    };
    let Some((filename, digest)) = record.trim_end().split_once(' ') else {
        return Ok(None);
    };
    if !filename.strip_suffix(".cwasm").is_some_and(valid_digest) || !valid_digest(digest) {
        return Ok(None);
    }
    let path = cache.join(filename);
    let Some(mut file) = open_cache_file(&path)? else {
        return Ok(None);
    };
    if native_digest(&mut file)?.as_deref() != Some(digest) {
        return Ok(None);
    }
    Ok(Some(file))
}

fn open_cache_file(path: &Path) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) || matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENXIO)) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    Ok(safe_cache_metadata(&file.metadata()?).then_some(file))
}

fn safe_cache_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o022 == 0
}

fn alias_target(path: &Path, filename: &str) -> Result<bool> {
    let Some(file) = open_cache_file(path)? else {
        return Ok(false);
    };
    let mut record = Vec::new();
    file.take(256).read_to_end(&mut record)?;
    Ok(std::str::from_utf8(&record)
        .ok()
        .and_then(|text| text.split_whitespace().next())
        == Some(filename))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn native_digest(file: &mut File) -> Result<Option<String>> {
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut reader = (&mut *file).take(NATIVE_RESERVATION_LIMIT + 1);
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let bytes = NATIVE_RESERVATION_LIMIT + 1 - reader.limit();
    file.rewind()?;
    Ok((bytes > 0 && bytes <= NATIVE_RESERVATION_LIMIT)
        .then(|| hex::encode(hash.finish().as_ref())))
}

fn install_alias(cache: &Path, key: &str, object: &Path, digest: &str) -> Result<()> {
    let filename = object.file_name().context("Prepared object has no name")?;
    let temporary = cache.join(format!(".alias-{key}-{}", uuid::Uuid::new_v4()));
    let destination = cache.join(format!(".alias-{key}"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    writeln!(file, "{} {digest}", filename.to_string_lossy())?;
    file.sync_all()?;
    let result = replace_cache_entry(&temporary, &destination);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

fn parse_prepare_status(status: &str) -> Result<PrepareStatus> {
    serde_json::from_value(serde_json::Value::String(status.to_owned()))
        .context("Wasm preparer returned an unknown refusal status")
}

fn replace_cache_entry(source: &Path, destination: &Path) -> io::Result<()> {
    if fs::symlink_metadata(destination).is_ok_and(|entry| entry.is_dir()) {
        fs::remove_dir_all(destination)?;
    }
    fs::rename(source, destination)
}

fn isolate_preparation_command(command: &mut Command) {
    // Wizer inherits the preparer's process group so both can be stopped together.
    command.process_group(0);
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(|| {
            // Keep Rust's exec error pipe open until exec, but pass only stdio to the tool.
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC)
                != 0
            {
                return Err(io::Error::last_os_error());
            }
            konae_functions_executor::sandbox::drop_privileges()
        });
    }
    #[cfg(target_os = "macos")]
    unsafe {
        command.pre_exec(|| {
            let mut descriptors = [std::mem::MaybeUninit::<libc::proc_fdinfo>::uninit(); 128];
            let capacity = std::mem::size_of_val(&descriptors) as libc::c_int;
            let bytes = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                descriptors.as_mut_ptr().cast(),
                capacity,
            );
            let mark_close_on_exec = |fd| {
                if fd >= 3 && libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::EBADF) {
                        return Err(error);
                    }
                }
                Ok(())
            };
            if bytes > 0 && bytes < capacity {
                let count = bytes as usize / std::mem::size_of::<libc::proc_fdinfo>();
                for entry in &descriptors[..count] {
                    mark_close_on_exec(entry.assume_init_ref().proc_fd)?;
                }
            } else {
                for fd in 3..libc::getdtablesize() {
                    mark_close_on_exec(fd)?;
                }
            }
            Ok(())
        });
    }
}

fn json_command<T: for<'de> Deserialize<'de>>(
    program: &Path,
    args: &[&str],
    deadline: Instant,
) -> Result<T> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let mut child = spawn_tool(command, deadline)
        .with_context(|| format!("Start operator tool {}", program.display()))?;
    let mut stdout = child.stdout.take().unwrap();
    let result = (|| {
        let fd = stdout.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            return Err(io::Error::last_os_error().into());
        }
        let mut reply = Vec::new();
        let mut status = None;
        let mut eof = false;
        while !eof || status.is_none() {
            check_deadline(deadline)?;
            if !eof {
                match (&mut stdout)
                    .take(8193 - reply.len() as u64)
                    .read_to_end(&mut reply)
                {
                    Ok(_) => eof = true,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
                ensure!(reply.len() <= 8192, "Operator tool reply exceeded 8 KiB");
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if !eof || status.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        ensure!(
            status.unwrap().success(),
            "Operator tool {} failed",
            program.display()
        );
        serde_json::from_slice(&reply).with_context(|| {
            format!(
                "Operator tool {} returned an invalid reply",
                program.display()
            )
        })
    })();
    if result.is_err() {
        stop_tool(&mut child);
    }
    result
}

fn compile_component(
    runner: &Path,
    output: &Path,
    component: &File,
    deadline: Instant,
) -> Result<()> {
    let mut command = Command::new(runner);
    command
        .arg("--compile-component")
        .arg(output)
        .stdin(component.try_clone()?)
        .stdout(Stdio::null());
    let mut child = spawn_tool(command, deadline).context("Start Wasm native compiler")?;
    let result = (|| {
        loop {
            check_deadline(deadline)?;
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "Wasm native compilation failed");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        stop_tool(&mut child);
    }
    result
}

fn spawn_tool(mut command: Command, deadline: Instant) -> Result<Child> {
    check_deadline(deadline)?;
    isolate_preparation_command(&mut command);
    Ok(command.env_clear().stderr(Stdio::null()).spawn()?)
}

fn stop_tool(child: &mut Child) {
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.wait();
}

struct Staging {
    path: PathBuf,
}
impl Staging {
    fn new(cache: &Path) -> Result<Self> {
        let path = cache.join(format!(".prepare-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .context("Create private Wasm preparation staging directory")?;
        Ok(Self { path })
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.path.join(format!("{name}.mjs"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o400);
        }
        let mut file = options.open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(path)
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_shell_and_its_child_share_an_isolated_process_group() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-ec",
            r#"
for fd in /dev/fd/[0-9]*; do test "${fd##*/}" -lt 9; done
test "$(ps -o pgid= -p "$$" | tr -d ' ')" = "$$"
if [ -r /proc/$$/status ]; then
    awk '/^Cap(Inh|Prm|Eff|Amb):/ && $2 !~ /^0+$/ { exit 1 }
         /^NoNewPrivs:/ { if ($2 != 1) exit 1; seen = 1 }
         END { if (!seen) exit 1 }' /proc/$$/status
fi
/bin/sh -ec 'test "$(ps -o pgid= -p "$$" | tr -d " ")" = "$1"' child "$$"
"#,
        ]);
        unsafe {
            command.pre_exec(|| {
                let fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let result = libc::fcntl(fd, libc::F_DUPFD, 9);
                libc::close(fd);
                if result < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        isolate_preparation_command(&mut command);
        assert!(command.status().unwrap().success());
    }

    #[test]
    fn a_cache_alias_returns_only_bytes_matching_its_digest() {
        let cache = Staging::new(&std::env::temp_dir()).unwrap();
        let filename = format!("{}.cwasm", "a".repeat(64));
        let path = cache.path.join(filename);
        fs::write(&path, b"good").unwrap();
        let key = "b".repeat(64);
        let digest = format!("{:x}", Sha256::digest(b"good"));
        assert!(cached_object(&cache.path, &key).unwrap().is_none());

        install_alias(&cache.path, &key, &path, &digest).unwrap();
        let mut verified = cached_object(&cache.path, &key).unwrap().unwrap();
        let mut bytes = Vec::new();
        verified.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"good");

        fs::write(&path, b"bad!").unwrap();
        assert!(cached_object(&cache.path, &key).unwrap().is_none());
    }

    #[test]
    fn hung_operator_tool_is_killed_and_reaped() {
        let dir = Staging::new(&std::env::temp_dir()).unwrap();
        let pid_path = dir.path.join("pid");
        let code = r#"echo $$ > "$1"; sleep 30 & wait"#;
        let start = Instant::now();
        let result = json_command::<serde_json::Value>(
            Path::new("/bin/sh"),
            &["-c", code, "sh", pid_path.to_str().unwrap()],
            start + Duration::from_millis(250),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeded its deadline")
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        let pid: i32 = fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn preparation_limits_queue_and_deduplicates_candidate_projects() {
        let candidate = JobKey::Candidate("project".into());
        let first = PreparationJob::claim(Some(candidate.clone())).unwrap();
        assert!(PreparationJob::claim(Some(candidate.clone())).is_err());
        let others = (1..PREPARATION_JOBS)
            .map(|_| PreparationJob::claim(None).unwrap())
            .collect::<Vec<_>>();
        assert!(PreparationJob::claim(None).is_err());
        drop(first);
        assert!(PreparationJob::claim(Some(candidate)).is_ok());
        drop(others);
    }

    #[test]
    fn a_pin_prevents_another_cache_open_from_evicting_the_object() {
        let cache = Staging::new(&std::env::temp_dir()).unwrap();
        let path = cache.path.join(format!("{}.cwasm", "a".repeat(64)));
        fs::write(&path, b"object").unwrap();
        let mut pin = PreparedPin::default();
        pin_file(&mut pin, File::open(&path).unwrap()).unwrap();
        assert!(ensure_cache_room(&cache.path, 1, 1).is_err());
        assert!(path.is_file());
        drop(pin);
        ensure_cache_room(&cache.path, 1, 1).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn unsafe_cache_entries_are_misses() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let cache = Staging::new(&std::env::temp_dir()).unwrap();
        let filename = format!("{}.cwasm", "a".repeat(64));
        let path = cache.path.join(filename);
        let digest = format!("{:x}", Sha256::digest(b"good"));
        let key = "b".repeat(64);
        fs::write(&path, b"good").unwrap();
        install_alias(&cache.path, &key, &path, &digest).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o662)).unwrap();
        assert!(cached_object(&cache.path, &key).unwrap().is_none());

        let target = cache.path.join("target");
        fs::rename(&path, &target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();
        assert!(cached_object(&cache.path, &key).unwrap().is_none());
    }
}
