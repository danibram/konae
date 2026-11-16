
use std::path::{Path, PathBuf};

/// The folder that holds one folder per app.
pub fn apps(root: &Path) -> PathBuf {
    root.join("apps")
}

/// One app's folder.
pub fn folder(root: &Path, id: &str) -> PathBuf {
    apps(root).join(id)
}

/// `konae.json`: the app's settings.
pub fn settings(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join(SETTINGS_FILE)
}

pub const SETTINGS_FILE: &str = "konae.json";

/// `app/`: the saved sources.
pub fn sources(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("app")
}

/// `dist/`: the built static files the Kernel serves.
pub fn dist(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("dist")
}

/// `data/`: the app's databases, uploaded files and secrets.
pub fn data(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("data")
}

pub fn database(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("database.sqlite")
}

pub fn identity(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("identity.sqlite")
}

pub fn usage(root: &Path, id: &str) -> PathBuf {
    data(root, id).join(crate::usage::USAGE_DATABASE)
}

/// The uploaded files: `index.json` and, in a local store, the bytes.
pub fn files(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("files")
}

/// The signing secrets and the app's Google client. They stay with the app
/// on this installation and in its copies, and an export never carries them.
pub fn secrets(root: &Path, id: &str) -> PathBuf {
    data(root, id).join(SECRETS_DATABASE)
}

pub const SECRETS_DATABASE: &str = "secrets.sqlite";

/// `.konae/`: what this installation keeps about the app and never hands
/// over: its Deployment record, address markers, maintenance marker,
/// backups and the editor's metadata of the saved sources.
pub fn private(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join(".konae")
}

/// The installation's record of the published app.
pub fn record(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("deployment.json")
}

pub fn backups(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("backups")
}
