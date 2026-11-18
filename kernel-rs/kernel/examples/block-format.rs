//! Exercises one format operation per process so the lab can measure its traffic and peak RSS.
use anyhow::{ensure, Result};
use kernel::{
    file_store::{s3_client, S3Settings},
    installation::Installation,
};
use konae_backup::{blocks::Repository, copy_id, CopyKey, Manifest, Source};
use serde_json::json;
use std::{env, path::Path};

#[cfg(target_os = "macos")]
fn disk_bytes() -> Result<(u64, u64)> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::uninit();
    // Darwin fills the supplied v2 structure on success.
    unsafe {
        let status = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        );
        ensure!(status == 0, "proc_pid_rusage failed.");
        let usage = usage.assume_init();
        Ok((usage.ri_diskio_bytesread, usage.ri_diskio_byteswritten))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [action, backend, repository, path, timestamp, pages] = args.as_slice() else {
        anyhow::bail!(
            "Usage: block-format publish|restore DIRECTORY|s3 REPOSITORY TREE UNIX_MS PAGES"
        );
    };
    let source = if backend == "s3" {
        let endpoint = env::var("KONAE_TEST_S3_ENDPOINT")?;
        let url = url::Url::parse(&endpoint)?;
        let bucket = env::var("KONAE_TEST_S3_BUCKET")?;
        ensure!(
            url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && bucket == "konae-test",
            "Use a disposable loopback MinIO."
        );
        let client = s3_client(&S3Settings {
            endpoint,
            bucket: bucket.clone(),
            region: "us-east-1".into(),
            prefix: String::new(),
            path_style: true,
            access_key: env::var("KONAE_TEST_S3_ACCESS_KEY")?,
            secret_key: env::var("KONAE_TEST_S3_SECRET_KEY")?,
        });
        Source::S3 {
            client,
            bucket,
            prefix: format!("copies-v3/{repository}/"),
        }
    } else {
        Source::Directory(backend.into())
    };
    let key = CopyKey::derive("synthetic-format-lab-without-live-data");
    let repo = Repository::new(&source, repository, &key)?;
    let timestamp = timestamp.parse()?;
    let scope = Installation::kernel();
    let path = Path::new(path);
    #[cfg(target_os = "macos")]
    let before = disk_bytes()?;
    let result = match action.as_str() {
        "publish" => {
            let manifest = Manifest::capture(path, timestamp)?;
            let stats = repo
                .publish(path, &manifest, &scope, pages.parse()?, &Default::default())
                .await?;
            let inventory_bytes: u64 = manifest.files.iter().map(|f| f.size).sum();
            json!({"inventoryBytesRead":inventory_bytes,"publication":stats})
        }
        "restore" => {
            let id = copy_id(timestamp);
            let point = repo.open(&id, &scope).await?;
            repo.restore(&point, path).await?;
            let restored: u64 = point.inventory().files.iter().map(|f| f.size).sum();
            let directory_read = match source {
                Source::Directory(root) => {
                    restored
                        + point.objects().count() as u64 * 16
                        + root.join("points").join(id).metadata()?.len()
                }
                Source::S3 { .. } => 0,
            };
            json!({"restoredBytesWritten":restored,"directoryBytesRead":directory_read})
        }
        _ => anyhow::bail!("Choose publish or restore."),
    };
    #[cfg(target_os = "macos")]
    let result = {
        let after = disk_bytes()?;
        let mut result = result;
        result["darwinDiskBytesRead"] = (after.0 - before.0).into();
        result["darwinDiskBytesWritten"] = (after.1 - before.1).into();
        result
    };
    println!("{result}");
    Ok(())
}
