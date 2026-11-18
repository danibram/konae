//! Interrupt one temporary object or point before the repository links its name.
use super::*;
use std::cell::Cell;
use std::io::{self, Write};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    Short,
    WriteZero,
    DiskFull,
    Sync,
}

thread_local! {
    static NEXT: Cell<Option<(&'static str, Fault)>> = const { Cell::new(None) };
}

pub(crate) struct File {
    inner: fs::File,
    fault: Option<Fault>,
    wrote_prefix: bool,
}

impl File {
    pub(crate) fn wrap(inner: fs::File, group: &str) -> Self {
        let fault = NEXT.with(|next| match next.get() {
            Some((target, fault)) if target == group => {
                next.set(None);
                Some(fault)
            }
            _ => None,
        });
        Self {
            inner,
            fault,
            wrote_prefix: false,
        }
    }

    pub(crate) fn sync_all(&self) -> io::Result<()> {
        if self.fault == Some(Fault::Sync) {
            return Err(io::Error::other("injected file sync failure"));
        }
        self.inner.sync_all()
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match (self.fault, self.wrote_prefix) {
            (Some(Fault::Short | Fault::WriteZero | Fault::DiskFull), false) => {
                self.wrote_prefix = true;
                self.inner.write(&buf[..buf.len().div_ceil(2)])
            }
            (Some(Fault::WriteZero), true) => Ok(0),
            (Some(Fault::DiskFull), true) => Err(io::ErrorKind::StorageFull.into()),
            _ => self.inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[tokio::test]
async fn publication_after_a_short_write_or_failed_sync() -> Result<()> {
    let key = CopyKey::derive(SECRET);
    let scope = |p: &str| p == "asset";
    for group in ["objects", "points"] {
        for fault in [Fault::Short, Fault::WriteZero, Fault::DiskFull, Fault::Sync] {
            let root = std::env::temp_dir().join(format!("write-fault-{}", uuid::Uuid::new_v4()));
            let stage = root.join("stage");
            fs::create_dir_all(&stage)?;
            let source = Source::Directory(root.join("repository"));
            let repo = Repository::new(&source, REPO, &key)?;
            let old = b"complete before the failed write";
            let new = b"bytes required by the next point";
            for (at, bytes) in [(0, old), (1, new)] {
                fs::write(stage.join("asset"), bytes)?;
                let manifest = Manifest::capture(&stage, at)?;
                if at == 1 {
                    NEXT.with(|next| next.set(Some((group, fault))));
                }
                let publish = repo
                    .publish(&stage, &manifest, &scope, 16, &HashMap::new())
                    .await;
                assert!(
                    NEXT.with(|next| next.get().is_none()),
                    "unused {group} {fault:?}"
                );
                if at == 1 && fault != Fault::Short {
                    let error = publish
                        .err()
                        .expect("publication survived the injected error");
                    let kind = error.downcast_ref::<io::Error>().map(io::Error::kind);
                    let expected = match fault {
                        Fault::WriteZero => io::ErrorKind::WriteZero,
                        Fault::DiskFull => io::ErrorKind::StorageFull,
                        Fault::Sync => io::ErrorKind::Other,
                        Fault::Short => unreachable!(),
                    };
                    assert_eq!(kind, Some(expected), "{group} {fault:?}: {error:#}");
                    assert_eq!(repo.points().await?, vec![copy_id(0)]);
                    let point = repo.open(&copy_id(0), &scope).await?;
                    let dest = root.join("before-retry");
                    repo.restore(&point, &dest).await?;
                    assert_eq!(fs::read(dest.join("asset"))?, old);
                    point.close().await?;
                    repo.publish(&stage, &manifest, &scope, 16, &HashMap::new())
                        .await?;
                } else {
                    publish?;
                }
            }
            assert_eq!(repo.points().await?, vec![copy_id(0), copy_id(1)]);
            for (at, bytes) in [(0, old), (1, new)] {
                let point = repo.open(&copy_id(at), &scope).await?;
                let dest = root.join(format!("restore-{at}"));
                repo.restore(&point, &dest).await?;
                assert_eq!(fs::read(dest.join("asset"))?, bytes, "{group} {fault:?}");
                point.close().await?;
            }
            fs::remove_dir_all(root)?;
        }
    }
    Ok(())
}
