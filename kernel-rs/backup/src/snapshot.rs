use anyhow::{bail, Result};
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::{path::Path, time::Duration};

/// Holds the read transaction that selected the image's first page.
pub struct SqliteSnapshot {
    source: Connection,
    pub wal: bool,
    pub bytes: u64,
}

impl SqliteSnapshot {
    pub fn pin(path: &Path) -> Result<Self> {
        let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        source.busy_timeout(Duration::ZERO)?;
        source.execute_batch("BEGIN")?;
        source.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |_| Ok(()))?;
        let mode: String = source.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let pages: u64 = source.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let size: u64 = source.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        Ok(Self {
            source,
            wal: mode == "wal",
            bytes: pages * size,
        })
    }

    /// The caller compacts the private image before including it in a repository.
    pub fn materialize(self, path: &Path, mut check: impl FnMut() -> Result<()>) -> Result<()> {
        let mut target = Connection::open(path)?;
        let backup = Backup::new(&self.source, &mut target)?;
        loop {
            check()?;
            match backup.step(64)? {
                StepResult::Done => break,
                StepResult::More => {}
                _ => bail!("SQLite could not copy the pinned pages."),
            }
        }
        drop(backup);
        // A WAL source can give the image a WAL header. Seal it as a single file.
        target.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()))?;
        Ok(())
    }
}
