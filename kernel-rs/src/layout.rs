//! Where each part of an app lives under the Kernel's data directory.
//!
//! Every path to an app's record, built files, databases, uploaded files,
//! backups and markers goes through this module, so the layout is written
//! down once.

use std::path::{Path, PathBuf};

/// The folder that holds one folder per app.
pub(crate) fn apps(root: &Path) -> PathBuf {
    root.join("deployments")
}

/// One app's folder.
pub(crate) fn folder(root: &Path, id: &str) -> PathBuf {
    apps(root).join(id)
}

/// The app's data: its databases and uploaded files.
pub(crate) fn data(root: &Path, id: &str) -> PathBuf {
    folder(root, id)
}

pub(crate) fn database(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("database.sqlite")
}

pub(crate) fn identity(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("identity.sqlite")
}

pub(crate) fn usage(root: &Path, id: &str) -> PathBuf {
    data(root, id).join(crate::usage::USAGE_DATABASE)
}

/// The uploaded files: `index.json` and, in a local store, the bytes.
pub(crate) fn files(root: &Path, id: &str) -> PathBuf {
    data(root, id).join("files")
}

/// What this installation keeps about the app and never hands over: its
/// record, address markers, maintenance marker and backups.
pub(crate) fn private(root: &Path, id: &str) -> PathBuf {
    folder(root, id)
}

/// The installation's record of the published app.
pub(crate) fn record(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("deployment.json")
}

pub(crate) fn backups(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("backups")
}

/// The built static files the Kernel serves.
pub(crate) fn artifact(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("artifact")
}
