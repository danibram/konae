use super::*;

async fn test_repository() -> (S3Client, String, String) {
    let settings = crate::file_store::S3Settings::copies(&|name| {
        std::env::var(name.replacen("KONAE_", "KONAE_TEST_", 1)).ok()
    }).unwrap();
    let (client, bucket) = match settings {
        Some(settings) => (crate::file_store::s3_client(&settings), settings.bucket),
        None => (fake_s3_client().await, "data".into()),
    };
    (client, bucket, format!("test-{}/copies/", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn indexed_bytes_and_pre_cut_deletion() {
    let root = scratch("indexed-bytes");
    let copies = root.join("copies");
    let live = root.join("live");
    let (deployment, _) = seed(&live);
    let files = crate::layout::files(&live, &deployment.id);
    let mut index: Value = serde_json::from_slice(&std::fs::read(files.join("index.json")).unwrap()).unwrap();
    let id = index[0]["id"].as_str().unwrap().to_owned();
    let blob = files.join(format!("{id}.blob"));
    let config = CopyConfig { targets: vec![Target::Directory(copies.clone())], interval: Duration::from_secs(3600) };
    let mut state = state(&live);
    state.copies = Arc::new(CopyStatus::open(Some(&config), &root.join("reports"), "", ""));
    let first = run_copy(&state, &config.targets).await.unwrap();

    for (bytes, reason) in [(None, "missing"), (Some(&b"short"[..]), "requires 10"), (Some(&b"1234567890"[..]), "SHA-256")] {
        match bytes {
            None => std::fs::remove_file(&blob).unwrap(),
            Some(bytes) => std::fs::write(&blob, bytes).unwrap(),
        }
        let error = run_copy(&state, &config.targets).await.unwrap_err().to_string();
        assert!(error.contains(&id) && error.contains(reason), "{error}");
        assert_eq!(state.copies.health()["targets"][0]["lastCopyId"], first);
        assert_eq!(Source::Directory(copies.clone()).complete_copies().await.unwrap(), [first.clone()]);
    }
    // An upload with an indexed digest also detects substitution on its first copy.
    std::fs::remove_file(live.join(".copy-state.json")).unwrap();
    assert!(run_copy(&state, &config.targets).await.unwrap_err().to_string().contains(&first));
    index[0]["sha256"] = json!(hex::encode(Sha256::digest(b"attachment")));
    std::fs::write(files.join("index.json"), index.to_string()).unwrap();
    assert!(run_copy(&state, &config.targets).await.unwrap_err().to_string().contains("SHA-256"));

    let from = Source::Directory(copies.clone());
    let restored = root.join("restored");
    restore(&from, Some(&first), &restored, None, &state.installation).await.unwrap();
    assert_eq!(std::fs::read(crate::layout::files(&restored, &deployment.id).join(format!("{id}.blob"))).unwrap(), b"attachment");

    std::fs::write(&blob, b"attachment").unwrap();
    let prior_blob = crate::layout::files(&copies.join(&first), &deployment.id).join(format!("{id}.blob"));
    for replacement in [None, Some(&b"123"[..]), Some(&b"abcdefghij"[..])] {
        match replacement {
            None => std::fs::remove_file(&prior_blob).unwrap(),
            Some(bytes) => std::fs::write(&prior_blob, bytes).unwrap(),
        }
        let error = run_copy(&state, &config.targets).await.unwrap_err().to_string();
        assert!(error.contains(&id), "{error}");
        assert_eq!(state.copies.health()["targets"][0]["lastCopyId"], first);
        assert_eq!(from.complete_copies().await.unwrap(), [first.clone()]);
    }
    std::fs::write(&prior_blob, b"attachment").unwrap();
    std::fs::write(files.join("index.json"), "[]").unwrap();
    let second = run_copy(&state, &config.targets).await.unwrap();
    assert!(blob.exists());
    let copied = crate::layout::files(&copies.join(&second), &deployment.id);
    assert!(!copied.join(format!("{id}.blob")).exists());
    assert_eq!(std::fs::read(copied.join("index.json")).unwrap(), b"[]");
    assert!(state.copies.health()["alert"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn s3_failure_is_local_to_the_target_and_survives_restart() {
    let root = scratch("partial-success");
    let live = root.join("live");
    let copies = root.join("copies");
    let reports = root.join("health");
    let (deployment, _) = seed(&live);
    let (client, bucket, prefix) = test_repository().await;
    let source = Source::S3 { client: client.clone(), bucket: bucket.clone(), prefix: prefix.clone() };
    let config = CopyConfig {
        targets: vec![Target::Directory(copies.clone()), Target::S3 { prefix: prefix.clone() }],
        interval: Duration::from_secs(3600),
    };
    let open = || {
        let mut status = CopyStatus::open(Some(&config), &reports, &crate::file_store::env_var("KONAE_S3_ENDPOINT").unwrap_or_default(), &bucket);
        status.key = Some(Arc::new(CopyKey::derive(TEST_SECRET)));
        Arc::new(status)
    };
    let mut state = state(&live);
    state.s3 = Some(client.clone());
    state.data_bucket = bucket.clone();
    state.copies = open();
    let first = run_copy(&state, &config.targets).await.unwrap();
    let manifest = read_manifest(&source, &first).await.unwrap().unwrap();
    let blob = manifest.files.iter().find(|file| file.path.ends_with(".blob")).unwrap();
    let object = format!("{prefix}{first}/{}", blob.path);
    let original = source.read(&first, &blob.path).await.unwrap();
    restore_with_receipt(&source, Some(&first), &root.join("trial"), state.copies.key(), &state.installation, &reports).await.unwrap();
    let verified = state.copies.health()["targets"][1]["lastVerifiedRestoreAt"].as_u64().unwrap();

    for damaged in [None, Some(vec![0; original.len() - 1]), Some(vec![0; original.len()])] {
        match damaged {
            None => { client.delete_object().bucket(&bucket).key(&object).send().await.unwrap(); }
            Some(bytes) => { client.put_object().bucket(&bucket).key(&object).body(ByteStream::from(bytes)).send().await.unwrap(); }
        }
        let error = run_copy(&state, &config.targets).await.unwrap_err().to_string();
        assert!(error.contains("s3") && error.contains(&blob.path), "{error}");
        assert_eq!(source.complete_copies().await.unwrap(), [first.clone()]);
        assert!(restore_with_receipt(&source, Some(&first), &root.join("failed-trial"), state.copies.key(), &state.installation, &reports).await.is_err());
        state.copies = open();
        let health = state.copies.health();
        assert_ne!(health["targets"][0]["lastCopyId"], first);
        assert!(health["targets"][0]["alert"].is_null());
        assert!(health["targets"][0]["lastVerifiedRestoreAt"].is_null());
        let remote = &health["targets"][1];
        assert_eq!(remote["lastCopyId"], first);
        assert_eq!(remote["lastCutAt"], manifest.created_at);
        assert_eq!(remote["lastVerifiedRestoreAt"], verified);
        assert_eq!(remote["lastVerifiedRestoreCopyId"], first);
        assert!(remote["lastAttemptAt"].as_u64().unwrap() > manifest.created_at);
        assert!(remote["lastError"].as_str().unwrap().contains(&blob.path));
    }

    client.put_object().bucket(&bucket).key(&object).body(ByteStream::from(original)).send().await.unwrap();
    let recovered = run_copy(&state, &config.targets).await.unwrap();
    assert_ne!(recovered, first);
    assert!(state.copies.health()["alert"].is_null());
    let restored = root.join("recovered");
    restore_with_receipt(&source, Some(&recovered), &restored, state.copies.key(), &state.installation, &reports).await.unwrap();
    assert_eq!(std::fs::read(restored.join(&blob.path)).unwrap(), b"attachment");
    let direct = Source::Directory(copies.join(&recovered));
    restore_with_receipt(&direct, None, &root.join("local-trial"), None, &state.installation, &reports).await.unwrap();
    for target in state.copies.health()["targets"].as_array().unwrap() {
        assert_eq!(target["lastCopyId"], recovered);
        assert_eq!(target["lastVerifiedRestoreCopyId"], recovered);
    }
    assert!(crate::layout::folder(&restored, &deployment.id).exists());

    let other = CopyConfig { targets: vec![Target::S3 { prefix: "elsewhere/".into() }], interval: config.interval };
    let health = CopyStatus::open(Some(&other), &reports, &crate::file_store::env_var("KONAE_S3_ENDPOINT").unwrap_or_default(), &bucket).health();
    assert!(health["targets"][0]["lastCopyId"].is_null());
    assert!(health["targets"][0]["lastVerifiedRestoreAt"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn dead_worker_ages_from_the_cut_and_restart_preserves_the_interrupted_attempt() {
    let root = scratch("worker");
    let live = root.join("live");
    let (deployment, _) = seed(&live);
    let config = CopyConfig { targets: vec![Target::Directory(root.join("copies"))], interval: Duration::from_secs(3600) };
    let open = || Arc::new(CopyStatus::open(Some(&config), &root.join("reports"), "", ""));
    let mut state = state(&live);
    state.copies = open();
    assert_eq!(state.copies.health()["targets"][0]["stale"], true);
    let first = run_copy(&state, &config.targets).await.unwrap();
    let report = &state.copies.health()["targets"][0];
    let cut = report["lastCutAt"].as_u64().unwrap();
    let completed = report["lastSuccessAt"].as_u64().unwrap();
    assert!(completed > cut);
    let deadline = cut + 3_900_000;
    assert_eq!(state.copies.health_at(deadline)["targets"][0]["stale"], false);
    let stale = &state.copies.health_at(deadline + 1)["targets"][0];
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["cutAgeMs"], 3_900_001);
    assert_eq!(stale["lastAttemptAt"], cut);
    assert!(stale["alert"].is_string());

    tokio::time::sleep(Duration::from_millis(2)).await;
    let lock = database_lock(&state, &deployment.id).await;
    let worker = tokio::spawn({
        let state = state.clone();
        let targets = config.targets.clone();
        async move { run_copy(&state, &targets).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !open().health()["targets"][0]["lastError"].as_str().is_some_and(|error| error.contains("stopped")) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    drop(lock);
    state.copies = open();
    let report = &state.copies.health_at(deadline + 1)["targets"][0];
    assert_eq!(report["stale"], true);
    assert_eq!(report["lastCopyId"], first);
    assert_eq!(report["lastCutAt"], cut);
    assert_eq!(report["lastSuccessAt"], completed);
    assert!(report["lastAttemptAt"].as_u64().unwrap() > completed);
    assert!(report["lastError"].as_str().unwrap().contains("stopped"));
    run_copy(&state, &config.targets).await.unwrap();
    assert!(open().health()["alert"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

fn manifest_with_final_size(id: &str, length: usize, version: u32, key: Option<&CopyKey>) -> Manifest {
    let digest = hex::encode(Sha256::digest(b"x"));
    let files = (0..16_000).map(|i| ManifestFile {
        path: format!("apps/app/app/file{i}"), size: 1, sha256: digest.clone(),
        home_copy: (version == 2).then(|| id.to_string()),
    }).collect();
    let mut manifest = Manifest { version, copy_id: id.into(), created_at: 1, files, encryption: None, mac: None };
    let unsigned_length = serde_json::to_vec_pretty(&manifest).unwrap().len();
    let original_length = key.map_or(unsigned_length, |key| serde_json::to_vec_pretty(&sealed_manifest(&manifest, key)).unwrap().len());
    let extra = length.checked_sub(original_length).unwrap();
    let count = manifest.files.len();
    for (i, file) in manifest.files.iter_mut().enumerate() {
        file.path.push_str(&"x".repeat(extra / count + usize::from(i < extra % count)));
    }
    match key {
        Some(key) => sealed_manifest(&manifest, key),
        None => manifest,
    }
}

#[tokio::test]
async fn manifest_reader_and_writer_agree_at_the_final_byte() {
    let root = scratch("manifest-boundary");
    let key = CopyKey::derive(TEST_SECRET);
    let (client, bucket, prefix) = test_repository().await;
    let remote = Source::S3 { client: client.clone(), bucket: bucket.clone(), prefix: prefix.clone() };
    let directory = Source::Directory(root.clone());
    for (version, signing_key) in [(1, None), (2, None), (2, Some(&key))] {
        for delta in [-1, 0, 1] {
            let id = copy_id(1_790_000_000_000 + (delta + 1) as u64 + if signing_key.is_some() { 8 } else { version as u64 * 3 });
            let size = (MAX_MANIFEST_BYTES as i64 + delta) as usize;
            let manifest = manifest_with_final_size(&id, size, version, signing_key);
            let wire = serde_json::to_vec_pretty(&manifest).unwrap();
            assert_eq!(wire.len(), size);
            let encoded = encode_manifest(&Installation::for_test(), &id, &manifest);
            if delta <= 0 {
                assert_eq!(encoded.unwrap(), wire);
            } else {
                assert!(encoded.unwrap_err().to_string().contains("4194305"));
                if version == 1 {
                    assert!(send_to_directory(&root.join("unreadable-staging"), &root, &id, &manifest, &HashMap::new(), &Installation::for_test()).unwrap_err().to_string().contains("4194305"));
                    assert!(!root.join(&id).join(MANIFEST).exists());
                } else {
                    let plain = Manifest { mac: None, encryption: None, ..manifest.clone() };
                    if signing_key.is_some() { assert!(encode_manifest(&Installation::for_test(), &id, &plain).is_ok()); }
                    let target = S3CopyTarget { client: &client, bucket: &bucket, prefix: &prefix, key: signing_key };
                    let err = send_to_s3(target, &root.join("unreadable-staging"), &plain, &id, &mut ManifestCache::new(), &HashMap::new(), &Installation::for_test()).await.err().unwrap();
                    assert!(err.to_string().contains("4194305"), "{err:#}");
                    assert!(!remote.complete_copies().await.unwrap().contains(&id));
                }
            }
            let source = if version == 1 {
                std::fs::create_dir_all(root.join(&id)).unwrap();
                std::fs::write(root.join(&id).join(MANIFEST), &wire).unwrap();
                &directory
            } else {
                client.put_object().bucket(&bucket).key(format!("{prefix}{id}/{MANIFEST}")).body(ByteStream::from(wire)).send().await.unwrap();
                &remote
            };
            let read = read_manifest(source, &id).await.unwrap();
            if delta > 0 {
                assert!(read.err().unwrap().contains("larger than 4 MiB"));
            } else {
                let read = read.unwrap();
                assert_eq!(read.files.len(), 16_000);
                check_sealed(&id, &read, signing_key).unwrap();
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn plain_s3_rejects_a_same_size_substitution_at_the_object_home() {
    let root = scratch("plain-s3-bytes");
    let path = "apps/demo/data/files/test.blob";
    std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
    std::fs::write(root.join(path), b"abc").unwrap();
    let (client, bucket, prefix) = test_repository().await;
    let repository = || S3CopyTarget { client: &client, bucket: &bucket, prefix: &prefix, key: None };
    let first = copy_id(now_ms());
    let manifest = Manifest {
        version: 1, copy_id: first.clone(), created_at: now_ms(), files: list_files(&root).unwrap(), mac: None, encryption: None,
    };
    let mut cache = ManifestCache::new();
    send_to_s3(repository(), &root, &manifest, &first, &mut cache, &HashMap::new(), &Installation::for_test()).await.unwrap();
    client.put_object().bucket(&bucket).key(format!("{prefix}{first}/{path}")).body(ByteStream::from_static(b"def")).send().await.unwrap();
    let second = copy_id(parse_copy_id(&first).unwrap() + 1);
    let manifest = Manifest { copy_id: second.clone(), ..manifest };
    let error = send_to_s3(repository(), &root, &manifest, &second, &mut cache, &HashMap::new(), &Installation::for_test()).await.unwrap_err();
    assert!(format!("{error:#}").contains("SHA-256"));
    let from = Source::S3 { client, bucket, prefix };
    assert_eq!(from.complete_copies().await.unwrap(), [first]);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn directory_reuse_requires_real_directories_and_regular_files() {
    use std::os::unix::fs::symlink;
    let root = scratch("copy-file-types");
    let live = root.join("live");
    let copies = root.join("copies");
    let (deployment, _) = seed(&live);
    let config = CopyConfig { targets: vec![Target::Directory(copies.clone())], interval: Duration::from_secs(3600) };
    let mut state = state(&live);
    state.copies = Arc::new(CopyStatus::new(Some(&config)));
    let good = run_copy(&state, &config.targets).await.unwrap();
    let reference = copy_id(parse_copy_id(&good).unwrap() + 1);
    let fixture = copies.join(&reference);
    copy_tree(&copies.join(&good), &fixture).unwrap();
    let mut manifest: Manifest = serde_json::from_slice(&std::fs::read(fixture.join(MANIFEST)).unwrap()).unwrap();
    manifest.copy_id = reference.clone();
    std::fs::write(fixture.join(MANIFEST), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let dist = crate::layout::folder(&fixture, &deployment.id).join("dist");
    let saved = fixture.join("saved-dist");
    std::fs::rename(&dist, &saved).unwrap();
    symlink(&saved, &dist).unwrap();
    let source = Source::Directory(copies.clone());
    for expected in ["Expected a directory", "Expected a regular file"] {
        let error = run_copy(&state, &config.targets).await.unwrap_err().to_string();
        assert!(error.contains(expected) && error.contains("dist/index.html"), "{error}");
        assert_eq!(state.copies.health()["targets"][0]["lastCopyId"], good);
        assert_eq!(source.complete_copies().await.unwrap(), [good.clone(), reference.clone()]);
        if expected == "Expected a directory" {
            std::fs::remove_file(&dist).unwrap();
            std::fs::create_dir(&dist).unwrap();
            symlink(saved.join("index.html"), dist.join("index.html")).unwrap();
        }
    }
    let restored = root.join("restored");
    restore(&source, Some(&good), &restored, None, &state.installation).await.unwrap();
    assert_eq!(std::fs::read(crate::layout::folder(&restored, &deployment.id).join("dist/index.html")).unwrap(), b"<main>copied app</main>");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn writer_and_restore_refuse_the_same_manifest_paths_and_references() {
    let root = scratch("manifest-paths");
    let id = copy_id(now_ms());
    let installation = Installation::kernel();
    let repository = root.join("repo");
    let stored = repository.join(&id);
    std::fs::create_dir_all(&stored).unwrap();
    let (client, bucket, base) = test_repository().await;
    let older = copy_id(parse_copy_id(&id).unwrap() - 1);
    for (path, home, reason) in [
        ("outside.sqlite", None, "outside a data directory"),
        ("apps/demo/dist/label\\name", None, "outside a data directory"),
        ("apps/demo/dist/index.html", Some("invalid-id"), "invalid object reference"),
    ] {
        let manifest = Manifest {
            version: 2, copy_id: id.clone(), created_at: 0, encryption: None, mac: None,
            files: vec![ManifestFile { path: path.into(), size: 0, sha256: hex::encode(Sha256::digest([])), home_copy: home.map(str::to_owned) }],
        };
        let fresh = root.join("new-target");
        let write = send_to_directory(&root.join("staging"), &fresh, &id, &manifest, &HashMap::new(), &installation).unwrap_err();
        assert!(write.to_string().contains(reason), "{write:#}");
        assert!(!fresh.join(&id).join(MANIFEST).exists());
        let prefix = format!("{base}{}/", uuid::Uuid::new_v4());
        let previous = Manifest { copy_id: older.clone(), ..manifest.clone() };
        client.put_object().bucket(&bucket).key(format!("{prefix}{older}/{MANIFEST}"))
            .body(ByteStream::from(serde_json::to_vec(&previous).unwrap())).send().await.unwrap();
        let target = S3CopyTarget { client: &client, bucket: &bucket, prefix: &prefix, key: None };
        let write_s3 = send_to_s3(target, &root.join("staging"), &manifest, &id, &mut ManifestCache::new(), &HashMap::new(), &installation).await.unwrap_err();
        assert_eq!(write_s3.to_string(), write.to_string());
        assert!(list_s3_keys(&client, &bucket, &format!("{prefix}{id}/")).await.unwrap().is_empty());
        std::fs::write(stored.join(MANIFEST), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let read = restore(&Source::Directory(repository.clone()), Some(&id), &root.join("restored"), None, &installation).await.err().unwrap();
        assert_eq!(write.to_string(), read.to_string());
    }
    std::fs::remove_dir_all(root).unwrap();
}
