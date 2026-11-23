use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use super::ServerArtifact;

const PREPARE_LIMIT: Duration = Duration::from_secs(90);

/// Prepares every bundle through the configured Wasm runner before a release
/// or an exported app folder can serve it. The runner owns the engine cache.
pub async fn prepare_artifact(artifact: Option<&ServerArtifact>) -> Result<()> {
    if std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() != Ok("wasm") {
        return Ok(());
    }
    let Some(artifact) = artifact else {
        return Ok(());
    };
    super::validate_artifact(Some(&artifact.manifest), Some(artifact))?;

    let runner = PathBuf::from(
        std::env::var_os("KONAE_FUNCTIONS_RUNNER")
            .context("KONAE_FUNCTIONS_RUNNER must name the Wasm runner")?,
    );
    ensure!(
        runner.is_absolute() && runner.is_file(),
        "KONAE_FUNCTIONS_RUNNER must name an absolute executable file"
    );
    let cache = PathBuf::from(
        std::env::var_os("KONAE_STARLING_CACHE")
            .context("KONAE_STARLING_CACHE must name the operator-owned Wasm cache")?,
    );
    ensure!(cache.is_absolute(), "KONAE_STARLING_CACHE must be absolute");

    let runner_for_build = runner.clone();
    let cache_for_build = cache.clone();
    let artifact = artifact.clone();
    tokio::task::spawn_blocking(move || {
        let staging = Staging::new()?;
        for (name, source) in &artifact.bundles {
            let path = staging.write_bundle(name, source)?;
            prepare_bundle(&runner_for_build, &cache_for_build, &path)?;
        }
        Ok(())
    })
    .await
    .context("Wasm preparation task stopped")?
}

struct Staging(PathBuf);

impl Staging {
    fn new() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("konae-wasm-prepare-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).context("Create Wasm preparation staging directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn write_bundle(&self, name: &str, source: &str) -> Result<PathBuf> {
        let path = self.0.join(format!("{name}.mjs"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        std::io::Write::write_all(&mut file, source.as_bytes())?;
        file.sync_all()?;
        Ok(path)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn prepare_bundle(runner: &Path, cache: &Path, bundle: &Path) -> Result<()> {
    let mut command = Command::new(runner);
    command
        .arg("--prepare")
        .arg(bundle)
        .env_clear()
        .env("KONAE_STARLING_CACHE", cache)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in [
        "KONAE_STARLING_BUILDER",
        "KONAE_STARLING_TOOLS",
        "KONAE_STARLING_WASMTIME",
        "KONAE_STARLING_NODE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let start = Instant::now();
    let mut child = command
        .spawn()
        .context("Start the Wasm component preparer")?;
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "Wasm component preparation failed");
            return Ok(());
        }
        if start.elapsed() >= PREPARE_LIMIT {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            child.kill()?;
            let _ = child.wait();
            bail!("Wasm component preparation exceeded its 90 second deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
