use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    ServerArtifact,
    wasm_validation::{PrepareStatus, WasmManifest, helper_refusal, verify_component},
};

const PREPARE_LIMIT: Duration = Duration::from_secs(90);
const COMPONENT_LIMIT: usize = 64 * 1024 * 1024;
const NATIVE_RESERVATION_LIMIT: u64 = 512 * 1024 * 1024;
const DEFAULT_CACHE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const NATIVE_VERIFY_READERS: usize = 4;
static CACHE_PREPARATION_LOCK: Mutex<()> = Mutex::new(());
static CACHE_PINS: OnceLock<Mutex<std::collections::HashMap<PathBuf, usize>>> = OnceLock::new();

struct CacheFileLock(std::fs::File);
impl Drop for CacheFileLock {
    fn drop(&mut self) { unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN); } }
}

pub struct PreparedPin(Vec<PathBuf>);
impl Drop for PreparedPin {
    fn drop(&mut self) {
        let mut pins = CACHE_PINS.get_or_init(Default::default).lock().unwrap();
        for path in &self.0 {
            if let Some(count) = pins.get_mut(path) {
                *count -= 1;
                if *count == 0 { pins.remove(path); }
            }
        }
    }
}

pub fn pin_prepared(paths: &std::collections::BTreeMap<String, PathBuf>) -> PreparedPin {
    let objects = paths.values().cloned().collect::<Vec<_>>();
    let mut pins = CACHE_PINS.get_or_init(Default::default).lock().unwrap();
    for path in &objects { *pins.entry(path.clone()).or_default() += 1; }
    PreparedPin(objects)
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

pub async fn check_preparation(_state: &crate::AppState, artifact: &ServerArtifact) -> Result<()> {
    prepare_artifact(Some(artifact)).await.map(|_| ())
}

/// Builds and verifies every Wasm component before a release can serve it.
pub async fn prepare_artifact(artifact: Option<&ServerArtifact>) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    if std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() != Ok("wasm") {
        return Ok(std::collections::BTreeMap::new());
    }
    let Some(artifact) = artifact else {
        return Ok(std::collections::BTreeMap::new());
    };
    super::validate_artifact(Some(&artifact.manifest), Some(artifact))?;

    let bundles = artifact.bundles.clone();
    tokio::task::spawn_blocking(move || prepare_bundles_blocking(&bundles))
        .await
        .context("Wasm preparation task stopped")?
}

/// Prepares the exact source map passed to a Wasm worker and returns pinned cache paths.
pub fn prepare_bundles(bundles: &std::collections::BTreeMap<String, String>) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    ensure!(std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() == Ok("wasm"), "Wasm runner kind is required for prepared bundles");
    prepare_bundles_blocking(bundles)
}

/// Resolves cache entries without invoking either compiler.
pub fn prepared_bundles(
    bundles: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, File>> {
    let runner = required_executable("KONAE_FUNCTIONS_RUNNER")?;
    let helper = required_executable("KONAE_FUNCTIONS_PREPARER")?;
    let cache = cache_directory()?;
    check_cache_directory(&cache)?;
    let identity: HelperIdentity = json_command(&helper, &["identity"], PREPARE_LIMIT)?;
    let target: NativeTarget = json_command(&runner, &["--native-target"], PREPARE_LIMIT)?;
    let entries: Vec<_> = bundles
        .iter()
        .map(|(name, source)| (name, alias_key(source, &identity, &target)))
        .collect();
    if entries.len() <= NATIVE_VERIFY_READERS {
        return entries
            .iter()
            .map(|(name, key)| prepared_file(&cache, name, key))
            .collect();
    }

    let chunk_size = entries.len().div_ceil(NATIVE_VERIFY_READERS);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = entries
            .chunks(chunk_size)
            .map(|chunk| {
                let cache = &cache;
                std::thread::Builder::new().spawn_scoped(scope, move || {
                    chunk
                        .iter()
                        .map(|(name, key)| prepared_file(cache, name, key))
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect::<io::Result<_>>()?;
        let mut files = std::collections::BTreeMap::new();
        for task in tasks {
            let verified = task
                .join()
                .map_err(|_| anyhow::anyhow!("Wasm cache verification stopped"))??;
            files.extend(verified);
        }
        Ok(files)
    })
}

fn prepared_file(cache: &Path, name: &str, key: &str) -> Result<(String, File)> {
    let (_, file) = cached_object(cache, key)?.with_context(|| {
        format!("Prepared Wasm component for {name} is missing or corrupt; prepare the app before serving")
    })?;
    Ok((name.to_owned(), file))
}

fn prepare_bundles_blocking(bundles: &std::collections::BTreeMap<String, String>) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let runner = required_executable("KONAE_FUNCTIONS_RUNNER")?;
    let helper = required_executable("KONAE_FUNCTIONS_PREPARER")?;
    let cache = cache_directory()?;
    fs::create_dir_all(&cache).context("Create the operator-owned Wasm cache")?;
    prepare_blocking(&runner, &helper, &cache, bundles)
}

fn cache_directory() -> Result<PathBuf> {
    let cache = PathBuf::from(std::env::var_os("KONAE_FUNCTIONS_WASM_CACHE")
        .context("KONAE_FUNCTIONS_WASM_CACHE must name the operator-owned Wasm cache")?);
    ensure!(cache.is_absolute(), "KONAE_FUNCTIONS_WASM_CACHE must be absolute");
    Ok(cache)
}

fn check_cache_directory(cache: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(cache)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
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

fn prepare_blocking(
    runner: &Path,
    helper: &Path,
    cache: &Path,
    bundles: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let _cache_serial = CACHE_PREPARATION_LOCK.lock().unwrap();
    check_cache_directory(cache)?;
    let lock_path = cache.join(".prepare.lock");
    let lock_file = OpenOptions::new().read(true).write(true).create(true).open(lock_path)?;
    ensure!(unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock_file), libc::LOCK_EX) } == 0, "Could not lock the Wasm preparation cache");
    let _cache_file_lock = CacheFileLock(lock_file);
    let identity: HelperIdentity = json_command(helper, &["identity"], PREPARE_LIMIT)?;
    ensure!(
        identity.abi == "konae-functions-wasm/1",
        "The configured preparer uses an unsupported Wasm ABI"
    );
    let target: NativeTarget = json_command(runner, &["--native-target"], PREPARE_LIMIT)?;
    ensure!(
        target.wasmtime == "38.0.4",
        "The configured runner uses an unsupported Wasmtime version"
    );

    let mut prepared = std::collections::BTreeMap::new();
    let mut active_pin = pin_prepared(&prepared);
    for (name, source) in bundles {
        let alias = alias_key(source, &identity, &target);
        if let Some((path, _)) = cached_object(cache, &alias)? {
            prepared.insert(name.clone(), path);
            active_pin = pin_prepared(&prepared);
            continue;
        }

        let staging = Staging::new(cache)?;
        let input = staging.write(name, source.as_bytes())?;
        let output = staging.path.join("component.wasm");
        let reply: HelperReply = json_command(
            helper,
            &[
                "prepare",
                "--input",
                input.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ],
            PREPARE_LIMIT,
        )?;
        if reply.status != "ok" {
            let status = parse_prepare_status(&reply.status)?;
            let _private_diagnostic = reply.diagnostic;
            return Err(helper_refusal(status, name));
        }
        let manifest = reply
            .manifest
            .context("Wasm preparer returned no manifest")?;
        let component = fs::read(&output).context("Read prepared Wasm component")?;
        ensure!(
            component.len() <= COMPONENT_LIMIT,
            "Prepared Wasm component exceeds 64 MiB"
        );
        let input_sha256 = format!("{:x}", Sha256::digest(source.as_bytes()));
        verify_component(&manifest, &input_sha256, &component)?;
        let mut key_hash = Sha256::new();
        for value in [
            &input_sha256,
            &manifest.component_sha256,
            &identity.engine_sha256,
            &identity.builder_fingerprint,
            &target.wasmtime,
            &target.target_fingerprint,
        ] {
            key_hash.update(value.as_bytes());
            key_hash.update([0]);
        }
        let key = format!("{:x}", key_hash.finalize());
        let installed = cache.join(format!("{key}.cwasm"));
        let native = staging.path.join("component.cwasm");
        let cache_limit = cache_limit()?;
        let reservation = (component.len() as u64)
            .saturating_mul(8)
            .saturating_add(8 * 1024 * 1024)
            .min(NATIVE_RESERVATION_LIMIT);
        ensure_cache_room(cache, reservation, cache_limit)?;
        compile_component(runner, &native, &component)?;
        let native_size = fs::metadata(&native)?.len();
        ensure_cache_room(cache, native_size, cache_limit)?;
        let mut file = File::open(&native)?;
        let digest = native_digest(&mut file)?.context("Invalid native component size")?;
        file.sync_all()?;
        fs::rename(&native, &installed)?;
        install_alias(cache, &alias, &installed, &digest)?;
        prepared.insert(name.clone(), installed);
        active_pin = pin_prepared(&prepared);
    }
    drop(active_pin);
    Ok(prepared)
}

fn cache_limit() -> Result<u64> {
    match std::env::var("KONAE_FUNCTIONS_WASM_CACHE_BYTES") {
        Ok(value) => {
            let bytes = value.parse::<u64>().context("KONAE_FUNCTIONS_WASM_CACHE_BYTES must be a positive byte count")?;
            ensure!(bytes > 0, "KONAE_FUNCTIONS_WASM_CACHE_BYTES must be a positive byte count");
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
        if entry.path().extension().is_some_and(|extension| extension == "cwasm") {
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                used = used.saturating_add(metadata.len());
                objects.push((entry.path(), metadata.len(), metadata.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
        }
    }
    if used.saturating_add(reserve) <= limit { return Ok(()); }
    let pins = CACHE_PINS.get_or_init(Default::default).lock().unwrap();
    objects.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in objects {
        if pins.get(&path).copied().unwrap_or_default() != 0 { continue; }
        let Some(filename) = path.file_name().map(|value| value.to_string_lossy().into_owned()) else { continue };
        fs::remove_file(&path)?;
        used = used.saturating_sub(size);
        for entry in fs::read_dir(cache)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".alias-")
                && fs::read_to_string(entry.path()).is_ok_and(|target| target.split_whitespace().next() == Some(filename.as_str()))
            {
                fs::remove_file(entry.path())?;
            }
        }
        if used.saturating_add(reserve) <= limit { return Ok(()); }
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

fn cached_object(cache: &Path, key: &str) -> Result<Option<(PathBuf, File)>> {
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
    Ok(Some((path, file)))
}

fn open_cache_file(path: &Path) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "Unsafe prepared component cache entry"
    );
    Ok(Some(file))
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
    fs::rename(&temporary, &destination)?;
    Ok(())
}

fn parse_prepare_status(status: &str) -> Result<PrepareStatus> {
    serde_json::from_value(serde_json::Value::String(status.to_owned()))
        .context("Wasm preparer returned an unknown refusal status")
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
    timeout: Duration,
) -> Result<T> {
    let start = Instant::now();
    let mut command = Command::new(program);
    isolate_preparation_command(&mut command);
    let output = command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("Start operator tool {}", program.display()))?;
    ensure!(
        start.elapsed() <= timeout,
        "Operator tool exceeded its deadline"
    );
    ensure!(
        output.status.success(),
        "Operator tool {} failed",
        program.display()
    );
    ensure!(
        output.stdout.len() <= 8 * 1024,
        "Operator tool reply exceeded 8 KiB"
    );
    serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "Operator tool {} returned an invalid reply",
            program.display()
        )
    })
}

fn compile_component(runner: &Path, output: &Path, component: &[u8]) -> Result<()> {
    ensure!(
        output.is_absolute(),
        "Native component staging path must be absolute"
    );
    let mut command = Command::new(runner);
    isolate_preparation_command(&mut command);
    let mut child = command
        .arg("--compile-component")
        .arg(output)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Start Wasm native compiler")?;
    use std::io::Write;
    child
        .stdin
        .take()
        .context("Open native compiler input")?
        .write_all(component)?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "Wasm native compilation failed");
            return Ok(());
        }
        if start.elapsed() > PREPARE_LIMIT {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("Wasm native compilation exceeded its 90 second deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Staging {
    path: PathBuf,
}
impl Staging {
    fn new(cache: &Path) -> Result<Self> {
        let path = cache.join(format!(".prepare-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).context("Create private Wasm preparation staging directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
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
        let (resolved, mut verified) = cached_object(&cache.path, &key).unwrap().unwrap();
        let mut bytes = Vec::new();
        verified.read_to_end(&mut bytes).unwrap();
        assert_eq!(resolved, path);
        assert_eq!(bytes, b"good");

        fs::write(&path, b"bad!").unwrap();
        assert!(cached_object(&cache.path, &key).unwrap().is_none());
    }
}
