//! The catalog owns a SQLite database and a directory of image files.
use anyhow::{Result, bail, ensure};
use konae_backup::{CopyReader, Manifest, Source, send_to_directory, vacuum_into};
use std::{fs, path::Path, time::SystemTime};

fn catalog_path(path: &str) -> bool {
    path == "catalog.sqlite" || path.starts_with("assets/")
}

fn capture(live: &Path, repository: &Path) -> Result<String> {
    let staging = std::env::temp_dir().join(format!("catalog-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging)?;
    let result = (|| {
        vacuum_into(&live.join("catalog.sqlite"), &staging.join("catalog.sqlite"))?;
        let assets = staging.join("assets");
        fs::create_dir(&assets)?;
        for entry in fs::read_dir(live.join("assets"))? {
            let entry = entry?;
            ensure!(entry.file_type()?.is_file(), "Expected an asset file.");
            fs::copy(entry.path(), assets.join(entry.file_name()))?;
        }
        let created_at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_millis();
        let manifest = Manifest::capture(&staging, created_at.try_into()?)?;
        send_to_directory(&staging, repository, &manifest.copy_id, &manifest, &Default::default(), &catalog_path)?;
        Ok(manifest.copy_id)
    })();
    fs::remove_dir_all(staging)?;
    result
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, live, repository] if command == "capture" => {
            println!("{}", capture(live.as_ref(), repository.as_ref())?);
        }
        [command, repository, id] if command == "verify" => {
            let source = Source::Directory(repository.into());
            CopyReader::open(&source, id, None, &catalog_path).await?.verify().await?;
        }
        [command, repository, id, destination] if command == "restore" => {
            let source = Source::Directory(repository.into());
            CopyReader::open(&source, id, None, &catalog_path).await?.restore(destination.as_ref()).await?;
        }
        _ => bail!("Usage: catalog capture LIVE REPOSITORY | verify REPOSITORY ID | restore REPOSITORY ID DESTINATION"),
    }
    Ok(())
}
