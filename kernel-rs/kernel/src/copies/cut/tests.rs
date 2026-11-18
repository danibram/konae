use super::*;
use crate::file_store::{BoxFuture, FileStore, StoredObject};
use rusqlite::Connection;
use std::{ops::Range, sync::Arc};
use tokio::sync::Notify;

struct Fixture {
    root: PathBuf,
    state: AppState,
    file: String,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("cut-{}", uuid::Uuid::new_v4()));
        let live = root.join("live");
        let app = layout::folder(&live, "app");
        for dir in ["app", "dist", "data/files"] {
            fs::create_dir_all(app.join(dir)).unwrap();
        }
        fs::write(app.join("app/code"), "draft one").unwrap();
        fs::write(app.join("dist/index.html"), "release one").unwrap();
        let file = uuid::Uuid::new_v4().to_string();
        fs::write(app.join(format!("data/files/{file}.blob")), b"old").unwrap();
        fs::write(
            app.join("data/files/index.json"),
            serde_json::json!([{"id": file, "size": 3}]).to_string(),
        )
        .unwrap();
        for (path, mode) in [
            (live.join("calls.sqlite"), "DELETE"),
            (app.join("data/database.sqlite"), "WAL"),
        ] {
            Connection::open(path)
                .unwrap()
                .execute_batch(&format!(
                    "PRAGMA journal_mode={mode}; CREATE TABLE seq(n); INSERT INTO seq VALUES(1)"
                ))
                .unwrap();
        }
        fs::create_dir_all(root.join("stage/apps")).unwrap();
        Self {
            root,
            state: AppState::for_test(&live),
            file,
        }
    }

    fn stage(&self) -> PathBuf {
        self.root.join("stage")
    }

    fn released(&self) {
        assert!(self.state.copies.operations.try_write().is_ok());
        assert!(self.state.copies.generations.try_read().is_ok());
        for path in [
            self.state.data_root.clone(),
            layout::apps(&self.state.data_root),
        ] {
            File::open(path).unwrap().try_lock().unwrap();
        }
        let writer = Connection::open(layout::database(&self.state.data_root, "app")).unwrap();
        writer.busy_timeout(Duration::ZERO).unwrap();
        let busy: i64 = writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy, 0);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn seq(path: PathBuf) -> i64 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("SELECT n FROM seq", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn common_cut_keeps_wal_rollback_journal_and_reused_file_id() {
    let f = Fixture::new();
    let stage = f.stage();
    let app = layout::folder(&f.state.data_root, "app");
    let before: i64 = 1;
    let writer = Connection::open(app.join("data/database.sqlite")).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; UPDATE seq SET n=2")
        .unwrap();
    let views = pin(&f.state, &stage, &HashMap::new(), u64::MAX, 32, None, PAUSE)
        .await
        .unwrap()
        .unwrap();
    assert!(f.state.copies.operations.try_read().is_ok());
    assert!(f.state.copies.generations.try_read().is_err());
    assert!(File::open(layout::apps(&f.state.data_root))
        .unwrap()
        .try_lock_shared()
        .is_err());
    writer.execute_batch("COMMIT").unwrap();
    Connection::open(f.state.data_root.join("calls.sqlite"))
        .unwrap()
        .execute("UPDATE seq SET n=2", [])
        .unwrap();
    let blob = app.join(format!("data/files/{}.blob", f.file));
    fs::remove_file(&blob).unwrap();
    fs::write(blob, "new").unwrap();
    for (path, text) in [
        ("app/code", "draft two"),
        ("dist/index.html", "release two"),
        ("data/files/index.json", "[]"),
    ] {
        fs::write(app.join(path), text).unwrap();
    }
    views.write(&stage).await.unwrap();
    f.released();
    let before_app = layout::folder(&stage, "app");
    assert_eq!(seq(before_app.join("data/database.sqlite")), before);
    assert_eq!(seq(stage.join("calls.sqlite")), before);
    assert_eq!(
        fs::read_to_string(before_app.join(format!("data/files/{}.blob", f.file))).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(before_app.join("app/code")).unwrap(),
        "draft one"
    );
    assert_eq!(
        fs::read_to_string(before_app.join("dist/index.html")).unwrap(),
        "release one"
    );
    assert!(fs::read_to_string(before_app.join("data/files/index.json"))
        .unwrap()
        .contains(&f.file));
    assert!(!layout::identity(&f.state.data_root, "app").exists());
}

#[tokio::test]
async fn common_cut_releases_partial_inventory_on_space_and_reader_limits() {
    for (bytes, readers, diagnostic) in [(u64::MAX, 1, "reader limit"), (1, 32, "staging space")] {
        let f = Fixture::new();
        let error = pin(
            &f.state,
            &f.stage(),
            &HashMap::new(),
            bytes,
            readers,
            None,
            PAUSE,
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains(diagnostic), "{error}");
        f.released();
    }
}

struct DelayedObject {
    opened: Notify,
    release: Notify,
}

impl FileStore for DelayedObject {
    fn kind(&self) -> &'static str {
        "unversioned-test"
    }
    fn get<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Option<Range<u64>>,
    ) -> BoxFuture<'a, Result<Option<Blob>>> {
        Box::pin(async {
            self.opened.notify_one();
            self.release.notified().await;
            Ok(Some(Blob {
                length: 3,
                reader: Box::pin(&b"old"[..]),
            }))
        })
    }
    fn put<'a>(&'a self, _: &'a str, _: &'a str, _: &'a Path) -> BoxFuture<'a, Result<()>> {
        unreachable!()
    }
    fn delete<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<()>> {
        unreachable!()
    }
    fn objects<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Vec<StoredObject>>> {
        unreachable!()
    }
    fn deployments(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        unreachable!()
    }
}

#[tokio::test]
async fn slow_remote_get_leaves_admission_open_and_the_cut_includes_the_write() {
    let mut f = Fixture::new();
    let slow = Arc::new(DelayedObject {
        opened: Notify::new(),
        release: Notify::new(),
    });
    f.state.files.store = slow.clone();
    let worker = tokio::spawn({
        let (state, staging) = (f.state.clone(), f.stage());
        async move {
            stage(
                &state,
                &staging,
                &HashMap::new(),
                &mut SourceCache::default(),
            )
            .await
        }
    });
    slow.opened.notified().await;
    let permission = f.state.copies.operations.try_read().unwrap();
    Connection::open(layout::database(&f.state.data_root, "app"))
        .unwrap()
        .execute("UPDATE seq SET n=2", [])
        .unwrap();
    drop(permission);
    slow.release.notify_one();
    worker.await.unwrap().unwrap();
    assert_eq!(seq(layout::database(&f.stage(), "app")), 2);
    assert_eq!(
        fs::read(layout::files(&f.stage(), "app").join(format!("{}.blob", f.file))).unwrap(),
        b"old"
    );
    f.released();
}

#[tokio::test]
async fn a_replaced_index_discards_the_remote_preparation_and_retries() {
    let mut f = Fixture::new();
    let slow = Arc::new(DelayedObject {
        opened: Notify::new(),
        release: Notify::new(),
    });
    f.state.files.store = slow.clone();
    let worker = tokio::spawn({
        let (state, staging) = (f.state.clone(), f.stage());
        async move {
            stage(
                &state,
                &staging,
                &HashMap::new(),
                &mut SourceCache::default(),
            )
            .await
        }
    });
    slow.opened.notified().await;
    let permission = f.state.copies.operations.try_read().unwrap();
    let index = layout::files(&f.state.data_root, "app").join("index.json");
    // Even a byte-for-byte rewrite publishes a new index generation.
    crate::write_atomic(&index, &fs::read(&index).unwrap())
        .await
        .unwrap();
    drop(permission);
    slow.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), slow.opened.notified())
        .await
        .unwrap();
    let permission = f.state.copies.operations.try_read().unwrap();
    fs::remove_file(index).unwrap();
    drop(permission);
    slow.release.notify_one();
    let (_, files) = worker.await.unwrap().unwrap();
    assert!(files.is_empty());
    assert!(!layout::files(&f.stage(), "app").exists());
    f.released();
}

#[tokio::test]
async fn abort_while_preparing_remote_files_leaves_admission_open() {
    let mut f = Fixture::new();
    let slow = Arc::new(DelayedObject {
        opened: Notify::new(),
        release: Notify::new(),
    });
    f.state.files.store = slow.clone();
    let worker = tokio::spawn({
        let (state, staging) = (f.state.clone(), f.stage());
        async move {
            stage(
                &state,
                &staging,
                &HashMap::new(),
                &mut SourceCache::default(),
            )
            .await
        }
    });
    slow.opened.notified().await;
    worker.abort();
    assert!(worker.await.err().unwrap().is_cancelled());
    f.released();
}

#[tokio::test]
async fn common_cut_timeout_and_abort_reopen_admission_in_drain() {
    for abort in [false, true] {
        let f = Fixture::new();
        let admitted = f.state.copies.operations.read().await;
        let start = Instant::now();
        let worker = tokio::spawn({
            let (state, stage) = (f.state.clone(), f.stage());
            async move { pin(&state, &stage, &HashMap::new(), u64::MAX, 32, None, PAUSE).await }
        });
        while f.state.copies.operations.try_read().is_ok() {
            tokio::task::yield_now().await;
        }
        if abort {
            worker.abort();
            assert!(worker.await.err().unwrap().is_cancelled());
        } else {
            let error = worker.await.unwrap().err().unwrap();
            assert!(error.to_string().contains("draining"), "{error}");
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(admitted);
        f.released();
    }
}

#[tokio::test]
#[ignore = "requires an exclusive empty MinIO bucket through with-minio"]
async fn common_cut_minio_reuses_an_id_with_and_without_bucket_versions() {
    use crate::file_store::{S3Settings, S3Store};
    use aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};
    let settings =
        S3Settings::files(&|name| std::env::var(name.replacen("KONAE_", "KONAE_TEST_", 1)).ok())
            .unwrap()
            .expect("Run with-minio to supply the disposable bucket and credentials.");
    assert!(settings.endpoint.starts_with("http://127.0.0.1:"));
    let client = crate::file_store::s3_client(&settings);
    let bucket = &settings.bucket;
    assert!(client
        .list_objects_v2()
        .bucket(bucket)
        .max_keys(1)
        .send()
        .await
        .unwrap()
        .contents()
        .is_empty());
    assert!(client
        .get_bucket_versioning()
        .bucket(bucket)
        .send()
        .await
        .unwrap()
        .status()
        .is_none());
    for versioned in [false, true] {
        if versioned {
            client
                .put_bucket_versioning()
                .bucket(bucket)
                .versioning_configuration(
                    VersioningConfiguration::builder()
                        .status(BucketVersioningStatus::Enabled)
                        .build(),
                )
                .send()
                .await
                .unwrap();
        }
        let mut f = Fixture::new();
        let stage = f.stage();
        let store = Arc::new(S3Store::new(settings.clone()));
        let path = layout::files(&f.state.data_root, "app").join(format!("{}.blob", f.file));
        store.put("app", &f.file, &path).await.unwrap();
        f.state.files.store = store.clone();
        let prepared = Prepared::read(
            &f.state,
            &stage,
            &HashMap::new(),
            u64::MAX,
            &SourceCache::default(),
        )
        .await
        .unwrap();
        let views = pin(
            &f.state,
            &stage,
            &HashMap::new(),
            u64::MAX,
            32,
            Some(prepared),
            PAUSE,
        )
        .await
        .unwrap()
        .unwrap();
        store.delete("app", &f.file).await.unwrap();
        fs::write(&path, b"new").unwrap();
        store.put("app", &f.file, &path).await.unwrap();
        let saved = views.write(&stage).await.unwrap();
        assert_eq!(saved[0].version.is_some(), versioned);
        let mut cache = SourceCache::default();
        cache.keep(&stage, saved).unwrap();
        let next_stage = f.root.join("next");
        fs::create_dir_all(layout::apps(&next_stage)).unwrap();
        let (_, files) = super::stage(&f.state, &next_stage, &HashMap::new(), &mut cache)
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::read(layout::files(&next_stage, "app").join(format!("{}.blob", f.file))).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(layout::files(&stage, "app").join(format!("{}.blob", f.file))).unwrap(),
            b"old"
        );
        f.released();
    }
}

#[cfg(unix)]
#[test]
fn common_cut_releases_readers_after_os_descriptor_exhaustion() {
    const CHILD: &str = "KONAE_DESCRIPTOR_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("copies::cut::tests::common_cut_releases_readers_after_os_descriptor_exhaustion")
            .arg("--exact")
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let f = Fixture::new();
    let files = layout::files(&f.state.data_root, "app");
    let index: Vec<_> = (0..80)
        .map(|_| {
            let id = uuid::Uuid::new_v4().to_string();
            fs::write(files.join(format!("{id}.blob")), []).unwrap();
            serde_json::json!({"id":id, "size":0})
        })
        .collect();
    fs::write(
        files.join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let limit = libc::rlimit {
        rlim_cur: 48,
        rlim_max: 48,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    rt.block_on(async {
        let error = pin(
            &f.state,
            &f.stage(),
            &HashMap::new(),
            u64::MAX,
            512,
            None,
            PAUSE,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error()),
            Some(libc::EMFILE),
            "{error:?}"
        );
        f.released();
    });
}
