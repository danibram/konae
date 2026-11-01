//! Where each part of an app lives under the Kernel's data directory.
//!
//! An app is one folder, `apps/<id>/`, laid out the way `konae init` writes
//! it and an export hands it over:
//!
//! ```text
//! apps/<id>/
//!   konae.json   name, visibility, Signup Policy, sign-in methods, contact email, Kernel version
//!   app/         the saved sources: pages, actions.json, schema.sql
//!   dist/        the built static files the Kernel serves
//!   data/        database.sqlite, identity.sqlite, usage.sqlite, files/, secrets.sqlite
//!   .konae/      what this installation keeps and never hands over
//! ```
//!
//! The app's ID names the folder and its Deployment. Every path into it
//! goes through this module.

use std::path::{Path, PathBuf};

/// The folder that holds one folder per app.
pub(crate) fn apps(root: &Path) -> PathBuf {
    root.join("apps")
}

/// One app's folder.
pub(crate) fn folder(root: &Path, id: &str) -> PathBuf {
    apps(root).join(id)
}

/// `konae.json`: the app's settings.
pub(crate) fn settings(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join(SETTINGS_FILE)
}

pub(crate) const SETTINGS_FILE: &str = "konae.json";

/// `app/`: the saved sources.
pub(crate) fn sources(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("app")
}

/// `dist/`: the built static files the Kernel serves.
pub(crate) fn dist(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("dist")
}

/// `data/`: the app's databases, uploaded files and secrets.
pub(crate) fn data(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join("data")
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

/// The signing secrets and the app's Google client. They stay with the app
/// on this installation and in its copies, and an export never carries them.
pub(crate) fn secrets(root: &Path, id: &str) -> PathBuf {
    data(root, id).join(SECRETS_DATABASE)
}

pub(crate) const SECRETS_DATABASE: &str = "secrets.sqlite";

/// `.konae/`: what this installation keeps about the app and never hands
/// over: its Deployment record, address markers, maintenance marker,
/// backups and the editor's metadata of the saved sources.
pub(crate) fn private(root: &Path, id: &str) -> PathBuf {
    folder(root, id).join(".konae")
}

/// The installation's record of the published app.
pub(crate) fn record(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("deployment.json")
}

pub(crate) fn backups(root: &Path, id: &str) -> PathBuf {
    private(root, id).join("backups")
}

/// In `.konae/`: the editor's name, slug, manifest and folders of the saved
/// sources in `app/`.
pub(crate) const SOURCES_METADATA: &str = "sources.json";
