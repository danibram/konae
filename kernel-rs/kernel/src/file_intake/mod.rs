mod accept;
mod worker;

pub use accept::DECLARATION_FILE;
pub use accept::{AcceptOutcome, accept_staged, validate_consumers, validate_declaration};

pub(crate) fn pin_declaration(
    manifest: &mut serde_json::Value,
    source: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(source) = source {
        manifest["fileIntake"] = serde_json::from_str(source)?;
    } else if let Some(object) = manifest.as_object_mut() {
        object.remove("fileIntake");
    }
    Ok(())
}

pub fn blob_references(database: &std::path::Path) -> anyhow::Result<Vec<(String, u64)>> {
    if !database.is_file() {
        return Ok(Vec::new());
    }
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='__konae_intake_versions')",
        [], |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement =
        connection.prepare("SELECT id,size FROM __konae_intake_versions ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub(crate) use worker::spawn_worker;
pub(crate) use worker::{check_pending, pause_export, prepare_transfer, resume_import};

#[cfg(test)]
mod tests;
