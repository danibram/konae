use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
    prepare_artifact(Some(artifact)).await
}

/// Builds and verifies every Wasm component before a release can serve it.
pub async fn prepare_artifact(artifact: Option<&ServerArtifact>) -> Result<()> {
    if std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() != Ok("wasm") {
        return Ok(());
    }
    let Some(artifact) = artifact else {
        return Ok(());
    };
    super::validate_artifact(Some(&artifact.manifest), Some(artifact))?;

    let bundles = artifact.bundles.clone();
    tokio::task::spawn_blocking(move || prepare_bundles_blocking(&bundles))
        .await
        .context("Wasm preparation task stopped")??;
    Ok(())
}

/// Prepares the exact source map passed to a Wasm worker and returns pinned cache paths.
pub fn prepare_bundles(bundles: &std::collections::BTreeMap<String, String>) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    ensure!(std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() == Ok("wasm"), "Wasm runner kind is required for prepared bundles");
    prepare_bundles_blocking(bundles)
}

fn prepare_bundles_blocking(bundles: &std::collections::BTreeMap<String, String>) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let runner = required_executable("KONAE_FUNCTIONS_RUNNER")?;
    let helper = required_executable("KONAE_FUNCTIONS_PREPARER")?;
    let cache = PathBuf::from(
        std::env::var_os("KONAE_FUNCTIONS_WASM_CACHE")
            .context("KONAE_FUNCTIONS_WASM_CACHE must name the operator-owned Wasm cache")?,
    );
    ensure!(cache.is_absolute(), "KONAE_FUNCTIONS_WASM_CACHE must be absolute");
    fs::create_dir_all(&cache).context("Create the operator-owned Wasm cache")?;
    prepare_blocking(&runner, &helper, &cache, bundles)
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
    for (name, source) in bundles {
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
        if installed.is_file() {
            prepared.insert(name.clone(), installed);
            continue;
        }
        let native = staging.path.join("component.cwasm");
        compile_component(runner, &native, &component)?;
        let file = OpenOptions::new().read(true).open(&native)?;
        file.sync_all()?;
        match fs::rename(&native, &installed) {
            Ok(()) => {}
            Err(_error) if installed.is_file() => {
                let _ = fs::remove_file(native);
            }
            Err(error) => return Err(error.into()),
        }
        let _ = error_if_empty(&installed)?;
        prepared.insert(name.clone(), installed);
    }
    Ok(prepared)
}

fn error_if_empty(path: &Path) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() > 0,
        "Native component compilation produced an empty file"
    );
    Ok(())
}

fn parse_prepare_status(status: &str) -> Result<PrepareStatus> {
    serde_json::from_value(serde_json::Value::String(status.to_owned()))
        .context("Wasm preparer returned an unknown refusal status")
}

fn json_command<T: for<'de> Deserialize<'de>>(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<T> {
    let start = Instant::now();
    let output = Command::new(program)
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
    let mut child = Command::new(runner)
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
