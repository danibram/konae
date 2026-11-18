use super::*;
use serde_json::json;

const REPO: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "0123456789abcdef0123456789abcdee";
const SECRET: &str = "synthetic-format-lab-without-live-data";

#[test]
fn independent_hkdf_hmac_and_aes_gcm_vectors() {
    let key = CopyKey::derive(SECRET);
    let keys = Keys::new(&key, REPO);
    let address = keys.address(b"abc");
    assert_eq!(
        address,
        "b2eca1d49cdd28b6a6530d031d11058d06d602a2b763037244b44bcb8bd1a3a1"
    );
    let bytes = keys.seal_block(&address, b"abc".to_vec()).unwrap();
    assert_eq!(
        hex::encode(&bytes),
        "3ba4bc82c106806c2ef0963896ce65b0f11c15"
    );
    let foreign = Keys::new(&key, OTHER);
    assert_ne!(address, foreign.address(b"abc"));
    assert!(foreign.open_block(&address, bytes.clone()).is_err());
    assert!(keys.open_block(&keys.address(b"abd"), bytes).is_err());

    let sealed = hex::decode(concat!(
        "4b435033070707070707070707070707c8d009700810aa02f4d628b027917a2b39",
        "21c1681abc9cac73609d249050d2d7906fa3e66a0929962010b7ed10aa32362e",
        "eaa4cb4f0b9aac073c9dc0bf7c06f35f9520e69de304109be25aa95ace93bf1",
        "d25ae92b9116844fedc17f8eb128d67be34fc0ccb8955c1fef816d11e215a0f",
        "aa1b8190ddd91d3dfc21ab958401324d011fff766622422f2ba86fac9f70a1c",
        "13073f2c7801b1b3a0aa2e7a578ed"
    ))
    .unwrap();
    let source = Source::Directory("unused".into());
    let repo = Repository::new(&source, REPO, &key).unwrap();
    let id = copy_id(0);
    let scope = |_: &str| false;
    assert_eq!(
        repo.decode(&id, &sealed, &scope).unwrap().objects().count(),
        0
    );
    assert!(repo.decode(&copy_id(1), &sealed, &scope).is_err());
    assert!(repo
        .decode(&id, &sealed[..sealed.len() - 1], &scope)
        .is_err());
    assert!(repo
        .decode(&id, &vec![0; MAX_MANIFEST_BYTES as usize + 1], &scope)
        .is_err());
}

struct Sqlite;
impl PathScope for Sqlite {
    fn allows(&self, path: &str) -> bool {
        path == "demo"
    }
    fn sqlite_image(&self, _: &str) -> bool {
        true
    }
}

#[tokio::test]
async fn signed_geometry_order_length_and_independent_points() -> Result<()> {
    let scratch = std::env::temp_dir().join(format!("block-format-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&scratch)?;
    let stage = scratch.join("stage");
    fs::create_dir(&stage)?;
    let source = Source::Directory(scratch.join("repository"));
    let key = CopyKey::derive(SECRET);
    let repo = Repository::new(&source, REPO, &key)?;
    let mut image = vec![0; 3 * 65536];
    image[..16].copy_from_slice(b"SQLite format 3\0");
    image[17] = 1;
    image[65536] = 42;
    fs::write(stage.join("demo"), &image)?;
    let manifest = Manifest::capture(&stage, 0)?;
    let first = repo
        .publish(&stage, &manifest, &Sqlite, 1, &HashMap::new())
        .await?;
    assert_eq!(first.new_objects, 3);
    let again = repo
        .publish(&stage, &manifest, &Sqlite, 1, &HashMap::new())
        .await?;
    assert_eq!(again.directory_bytes_written, 0);
    let id = &manifest.copy_id;
    let point = repo.open(id, &Sqlite).await?;
    let bytes = serde_json::to_vec(&point.point)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    for (name, wrong) in [
        ("size", json!(65537)),
        ("pageSize", json!(0)),
        ("pageSize", json!(513)),
        ("objects", json!([])),
        ("objects", json!(["../outside"])),
        ("path", json!("../outside")),
    ] {
        let mut bad = value.clone();
        bad["files"][0][name] = wrong;
        let sealed = repo.keys.seal_point(id, serde_json::to_vec(&bad)?)?;
        assert!(repo.decode(id, &sealed, &Sqlite).is_err(), "{name}");
    }
    for (field, bad) in [
        ("version", json!(4)),
        ("repository", json!(OTHER)),
        ("createdAt", json!(1)),
        ("pagesPerObject", json!(65)),
    ] {
        let mut altered = value.clone();
        altered[field] = bad;
        assert!(repo
            .decode(
                id,
                &repo.keys.seal_point(id, serde_json::to_vec(&altered)?)?,
                &Sqlite
            )
            .is_err());
    }
    let mut swapped = value.clone();
    swapped["files"][0]["objects"]
        .as_array_mut()
        .unwrap()
        .swap(1, 2);
    let swapped = repo.decode(
        id,
        &repo.keys.seal_point(id, serde_json::to_vec(&swapped)?)?,
        &Sqlite,
    )?;
    let restored = scratch.join("restore");
    assert!(repo.restore(&swapped, &restored).await.is_err());
    assert!(!restored.exists());
    repo.restore(&point, &restored).await?;
    assert_eq!(fs::read(restored.join("demo"))?, image);
    fs::remove_dir_all(scratch)?;
    Ok(())
}

#[tokio::test]
async fn a_partial_directory_upload_retries_without_replacing_a_block() -> Result<()> {
    let scratch = std::env::temp_dir().join(format!("block-retry-{}", uuid::Uuid::new_v4()));
    let stage = scratch.join("stage");
    fs::create_dir_all(&stage)?;
    fs::write(stage.join("a"), b"first")?;
    fs::write(stage.join("b"), b"second")?;
    let source = Source::Directory(scratch.join("copies"));
    let master = CopyKey::derive(SECRET);
    let repo = Repository::new(&source, REPO, &master)?;
    let scope = |_: &str| true;
    let inventory = Manifest::capture(&stage, 1)?;
    fs::remove_file(stage.join("b"))?;
    assert!(repo
        .publish(&stage, &inventory, &scope, 16, &HashMap::new())
        .await
        .is_err());
    assert!(repo.points().await?.is_empty());
    let paths: Vec<_> =
        fs::read_dir(scratch.join("copies/objects"))?.collect::<std::io::Result<_>>()?;
    assert_eq!(paths.len(), 1);
    let object = paths[0].path();
    let modified = object.metadata()?.modified()?;
    fs::write(stage.join("b"), b"second")?;
    let stats = repo
        .publish(&stage, &inventory, &scope, 16, &HashMap::new())
        .await?;
    assert_eq!((stats.new_objects, stats.reused_objects), (1, 1));
    assert_eq!(object.metadata()?.modified()?, modified);

    fs::write(stage.join("b"), b"third")?;
    let next = Manifest::capture(&stage, 2)?;
    repo.publish(&stage, &next, &scope, 16, &HashMap::new())
        .await?;
    fs::remove_dir_all(stage)?;
    fs::remove_file(scratch.join("copies/points").join(inventory.copy_id))?;
    let point = repo.open(&next.copy_id, &scope).await?;
    repo.restore(&point, &scratch.join("recovered")).await?;
    assert_eq!(fs::read(scratch.join("recovered/a"))?, b"first");
    assert_eq!(fs::read(scratch.join("recovered/b"))?, b"third");
    fs::remove_dir_all(scratch)?;
    Ok(())
}
