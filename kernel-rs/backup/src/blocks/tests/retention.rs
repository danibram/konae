//! The Python runner starts this worker once per operation with a fresh runtime.
use super::*;
use anyhow::{bail, Context};
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use std::env;
use std::path::PathBuf;

const HOUR: u64 = 3_600_000;
const HOURS: usize = 45 * 24;

fn setting(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("Missing test setting {name}"))
}

fn source(root: &Path) -> Result<Source> {
    if setting("KONAE_LAB_TARGET")? == "directory" {
        return Ok(Source::Directory(root.join("copies-v3").join(REPO)));
    }
    let endpoint = setting("KONAE_TEST_S3_ENDPOINT")?;
    let bucket = setting("KONAE_TEST_S3_BUCKET")?;
    ensure!(
        endpoint
            .strip_prefix("http://127.0.0.1:")
            .and_then(|p| p.parse::<u16>().ok())
            .is_some()
            && bucket == "konae-test",
        "Expected the disposable loopback bucket."
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(Credentials::new(
            setting("KONAE_TEST_S3_ACCESS_KEY")?,
            setting("KONAE_TEST_S3_SECRET_KEY")?,
            None,
            None,
            "disposable-lab",
        ))
        .build();
    Ok(Source::S3 {
        client: aws_sdk_s3::Client::from_conf(config),
        bucket,
        prefix: format!("copies-v3/{REPO}/"),
    })
}

#[tokio::test]
#[ignore = "Run tests/smoke/block-retention.py for directory and disposable S3"]
async fn worker() -> Result<()> {
    let root = PathBuf::from(setting("KONAE_LAB_ROOT")?);
    let destination = env::var_os("KONAE_LAB_DESTINATION").map(PathBuf::from);
    let source = source(destination.as_deref().unwrap_or(&root))?;
    let old = CopyKey::derive(SECRET);
    let key = CopyKey::derive("synthetic-second-epoch-retention-test");
    let first = Repository::new(&source, REPO, &old)?;
    let repo = Repository::new(&source, REPO, &key)?.with_previous(&[old]);
    let stage = root.join(format!("stage-{}", std::process::id()));
    fs::create_dir(&stage)?;
    let start: u64 = setting("KONAE_LAB_START")?.parse()?;
    let at = |i: usize| start + i as u64 * HOUR;
    let now = at(HOURS - 1);
    let last = copy_id(now);
    let scope = |p: &str| p == "asset" || p == "hour";
    let phase = setting("KONAE_LAB_PHASE")?;
    match phase.as_str() {
        "capacity" | "abandon" => {
            let mut asset = fs::File::create(stage.join("asset"))?;
            let blocks = if phase == "capacity" { 8 } else { 1 };
            for i in 0..blocks {
                asset.write_all(&vec![i; ASSET_BLOCK])?;
            }
            drop(asset);
            repo.publish(
                &stage,
                &Manifest::capture(&stage, at(HOURS + 1))?,
                &scope,
                16,
                &HashMap::new(),
            )
            .await?;
        }
        "grace" => {
            let orphan = "b".repeat(64);
            assert!(store::put(&source, "objects", &orphan, b"interrupted upload").await?);
            let modified = store::list(&source, "objects")
                .await?
                .into_iter()
                .find(|o| o.name == orphan)
                .unwrap()
                .modified_ms;
            repo.prune(&scope, modified + crate::DAY_MS - 1, &last)
                .await?;
            assert_eq!(
                store::read(&source, "objects", &orphan, 32).await?,
                b"interrupted upload"
            );
            repo.prune(&scope, modified + crate::DAY_MS, &last).await?;
            assert!(!store::list(&source, "objects")
                .await?
                .iter()
                .any(|o| o.name == orphan));
        }
        "seed" | "retry" | "next" => {
            let (begin, end) = match phase.as_str() {
                "seed" => (0, HOURS),
                "retry" => (HOURS - 1, HOURS),
                _ => (HOURS, HOURS + 1),
            };
            for i in begin..end {
                fs::write(
                    stage.join("asset"),
                    b"shared bytes first stored in this epoch's oldest copy",
                )?;
                fs::write(stage.join("hour"), (i.min(HOURS - 1) as u64).to_le_bytes())?;
                let writer = if i < HOURS / 2 { &first } else { &repo };
                let stats = writer
                    .publish(
                        &stage,
                        &Manifest::capture(&stage, at(i))?,
                        &scope,
                        16,
                        &HashMap::new(),
                    )
                    .await?;
                if phase == "next" {
                    assert_eq!((stats.new_objects, stats.reused_objects), (0, 2));
                }
            }
        }
        "prune" | "next-prune" | "no-key" => {
            let writer = if phase == "no-key" {
                Repository::new(&source, REPO, &key)?
            } else {
                repo
            };
            let time = if phase == "next-prune" {
                at(HOURS)
            } else {
                now
            };
            let count = writer.prune(&scope, time, &copy_id(time)).await?;
            println!("Expired roots {count}");
        }
        "inventory" => {
            let ids = repo.points().await?;
            fs::write(root.join("listed.json"), serde_json::to_vec(&ids)?)?;
            let entries = store::list(&source, "objects").await?;
            let mut names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
            names.sort();
            fs::write(root.join("objects.json"), serde_json::to_vec(&names)?)?;
        }
        "reachable" => {
            let mut expected = HashSet::new();
            for id in repo.points().await? {
                let point = repo.open(&id, &scope).await?;
                expected.extend(point.objects().map(str::to_owned));
                point.close().await?;
            }
            let actual = store::list(&source, "objects")
                .await?
                .into_iter()
                .map(|e| e.name)
                .collect::<HashSet<_>>();
            assert_eq!(actual, expected);
            println!("Reachable objects {}", expected.len());
        }
        "oldest" | "latest" | "original" | "newest" => {
            let i = match phase.as_str() {
                "oldest" => HOURS - 1 - 30 * 24,
                "latest" => HOURS - 1,
                "original" => 0,
                _ => HOURS,
            };
            let point = repo.open(&copy_id(at(i)), &scope).await?;
            let output = stage.join("restored");
            repo.restore(&point, &output).await?;
            assert_eq!(
                fs::read(output.join("hour"))?,
                (i.min(HOURS - 1) as u64).to_le_bytes()
            );
            assert_eq!(
                fs::read(output.join("asset"))?,
                b"shared bytes first stored in this epoch's oldest copy"
            );
            point.close().await?;
        }
        "legacy" => match &source {
            Source::Directory(root) => {
                let parent = root.parent().unwrap().parent().unwrap();
                for i in 0..25 {
                    let dir = parent.join(copy_id(at(i)));
                    fs::create_dir(&dir)?;
                    fs::write(dir.join(crate::MANIFEST), b"{}")?;
                }
                assert_eq!(crate::prune_directory(parent, now)?, 1);
            }
            Source::S3 { client, bucket, .. } => {
                // The legacy collector scans a shared ancestor. It needs an authentic legacy root.
                let m = Manifest::capture(&stage, now)?;
                let target = crate::S3CopyTarget {
                    client,
                    bucket,
                    prefix: "",
                    key: Some(&key),
                };
                let mut cache = crate::ManifestCache::new();
                crate::send_to_s3(
                    target,
                    &stage,
                    &m,
                    &last,
                    &mut cache,
                    &HashMap::new(),
                    &scope,
                )
                .await?;
                // An interrupted v2 upload and an expired v1 root share the ancestor.
                let abandoned = format!("{}/payload", copy_id(0));
                client
                    .put_object()
                    .bucket(bucket)
                    .key(&abandoned)
                    .body(vec![0].into())
                    .send()
                    .await?;
                for i in 0..24 {
                    let mut legacy = m.clone();
                    legacy.created_at = at(i);
                    legacy.copy_id = copy_id(at(i));
                    legacy.version = 1;
                    let sealed = crate::sealed_manifest(&legacy, &key);
                    let bytes = crate::encode_manifest(&scope, &legacy.copy_id, &sealed)?;
                    client
                        .put_object()
                        .bucket(bucket)
                        .key(format!("{}/{}", legacy.copy_id, crate::MANIFEST))
                        .body(bytes.into())
                        .send()
                        .await?;
                }
                assert_eq!(
                    crate::prune_s3_cached(client, bucket, "", &last, now, &mut cache).await?,
                    1
                );
                assert!(crate::list_s3_keys(client, bucket, &abandoned)
                    .await?
                    .is_empty());
            }
        },
        _ => bail!("Unknown retention test operation"),
    }
    fs::remove_dir_all(stage)?;
    Ok(())
}
