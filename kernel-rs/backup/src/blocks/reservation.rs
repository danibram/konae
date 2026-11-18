//! Each S3 participant registers before checking for incompatible participants.
//! Strongly consistent PUT and LIST make at least one side observe the other.
#[cfg(feature = "s3")]
use super::store;
use super::Source;
use anyhow::{Context, Result};
use std::fs::File;

pub(super) enum Reservation {
    Directory(File),
    #[cfg(feature = "s3")]
    Remote {
        client: aws_sdk_s3::Client,
        bucket: String,
        key: String,
    },
}

impl Reservation {
    pub(super) async fn acquire(source: &Source, exclusive: bool) -> Result<Self> {
        match source {
            Source::Directory(root) => {
                let file = File::open(root)?;
                if exclusive {
                    file.try_lock()
                } else {
                    file.try_lock_shared()
                }
                .context("The repository is busy or cannot lock its directory.")?;
                Ok(Self::Directory(file))
            }
            #[cfg(feature = "s3")]
            Source::S3 {
                client,
                bucket,
                prefix,
            } => {
                let group = if exclusive { "collectors" } else { "users" };
                let name = uuid::Uuid::new_v4().to_string();
                anyhow::ensure!(
                    store::put(source, group, &name, b"1").await?,
                    "S3 reservation name already exists."
                );
                let key = format!("{prefix}{group}/{name}");
                let reservation = Self::Remote {
                    client: client.clone(),
                    bucket: bucket.clone(),
                    key: key.clone(),
                };
                let check = async {
                    let collectors =
                        crate::list_s3_keys(client, bucket, &format!("{prefix}collectors/"))
                            .await?;
                    anyhow::ensure!(
                        collectors.iter().all(|k| exclusive && k == &key),
                        "The repository has a collector reservation."
                    );
                    if exclusive {
                        anyhow::ensure!(
                            collectors.contains(&key),
                            "S3 omitted the collector's reservation."
                        );
                        let users =
                            crate::list_s3_keys(client, bucket, &format!("{prefix}users/")).await?;
                        anyhow::ensure!(
                            users.is_empty(),
                            "The repository has active reader or writer reservations."
                        );
                    }
                    Ok(())
                }
                .await;
                if let Err(e) = check {
                    reservation.release().await?;
                    return Err(e);
                }
                Ok(reservation)
            }
        }
    }

    pub(super) async fn release(self) -> Result<()> {
        match self {
            Self::Directory(file) => drop(file),
            #[cfg(feature = "s3")]
            Self::Remote {
                client,
                bucket,
                key,
            } => {
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(&key)
                    .send()
                    .await
                    .with_context(|| format!("Could not release reservation {key}"))?;
            }
        }
        Ok(())
    }
}

// Only a reader can release its reservation from Drop. A cancelled remote
// mutation may still reach S3, so its reservation survives cancellation.
pub(super) struct Reader(pub(super) Option<Reservation>);

impl Drop for Reader {
    fn drop(&mut self) {
        #[cfg(feature = "s3")]
        if let Some(remote @ Reservation::Remote { .. }) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                if let Err(error) = remote.release().await {
                    eprintln!("Could not close v3 reader: {error:#}");
                }
            });
        }
    }
}
