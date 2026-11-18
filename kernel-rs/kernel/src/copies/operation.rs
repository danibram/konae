use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::{ErrorKind, Write}, path::{Path, PathBuf}};

const DIRECTORY: &str = ".copy-operations";

#[derive(Serialize, Deserialize)]
struct Record {
    subject: String,
    phase: String,
}

/// Install the record before the first live write. A crash before the rename
/// leaves only a temporary file and the operation has yet to start.
pub struct Pending(PathBuf);

impl Pending {
    pub fn begin(root: &Path, subject: &str, phase: &str) -> Result<Self> {
        let directory = root.join(DIRECTORY);
        match fs::create_dir(&directory) {
            Ok(()) => fs::File::open(root)?.sync_all()?,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let id = uuid::Uuid::new_v4();
        let path = directory.join(format!("{id}.json"));
        let temp = path.with_extension("tmp");
        let record = Record { subject: subject.into(), phase: phase.into() };
        let mut file = fs::File::create_new(&temp)?;
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()?;
        fs::rename(temp, &path)?;
        fs::File::open(directory)?.sync_all()?;
        Ok(Self(path))
    }

    pub fn finish(self) -> Result<()> {
        fs::remove_file(&self.0)?;
        fs::File::open(self.0.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

pub(super) fn check(root: &Path) -> Result<()> {
    let entries = match fs::read_dir(root.join(DIRECTORY)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "tmp") { continue; }
        let record: Record = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("Cannot verify operation record {}", path.display()))?;
        anyhow::bail!("Unfinished operation for {} during {}. Inspect {}. Recover a complete installation point into an empty volume if the commits cannot be reconciled.",
            record.subject, record.phase, path.display());
    }
    Ok(())
}

pub(crate) async fn action(
    state: &crate::AppState, app: &str, name: &str, action: &crate::ActionDefinition,
    work: impl Future<Output = axum::response::Response>,
) -> axum::response::Response {
    if action.kind != "mutation" || (action.signal.is_none() && action.notify.is_none()) {
        return work.await;
    }
    let result = async {
        let failures = crate::usage::write_failures();
        let pending = Pending::begin(&state.data_root, &format!("{app}/{name}"), "writing the action row and its declared effects")?;
        let response = work.await;
        ensure!(tokio::task::spawn_blocking(crate::usage::flush_status).await? && crate::usage::write_failures() == failures,
            "Could not confirm the action's queued writes. Inspect the app row and its effects before retrying.");
        pending.finish()?;
        Ok::<_,anyhow::Error>(response)
    }.await;
    result.unwrap_or_else(|err| crate::error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()))
}
