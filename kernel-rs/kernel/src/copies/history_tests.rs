use super::{history::*, *};
use crate::{AppState, layout};
use std::fs;
use uuid::Uuid;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("konae-history-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn record(dir: &Path, id: &str, holds: &[&str], key: &str, size: u64) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(format!("{id}.json")), json!({
        "id":id, "key":key, "holds":holds, "sizeBytes":size, "createdAt":1, "label":"Private owner label"
    }).to_string()).unwrap();
}

#[test]
fn history_copies_only_the_components_covered_by_each_record() {
    let tmp = Scratch::new();
    let dir = tmp.0.join("backups");
    let id = "before-import";
    let folder = dir.join(id);
    fs::create_dir_all(folder.join("scratch")).unwrap();
    fs::write(folder.join("sources.json"), b"null").unwrap();
    for name in ["database.sqlite", "files.tar", "secrets.sqlite", "scratch/sources.json"] {
        fs::write(folder.join(name), b"private scratch").unwrap();
    }
    fs::write(dir.join("orphan.sqlite"), b"private scratch").unwrap();
    record(&dir, id, &["sources"], id, 4);
    let target = tmp.0.join("copied");
    copy_records(&dir, &target).unwrap();
    assert_eq!(fs::read_dir(&target).unwrap().count(), 2);
    assert_eq!(fs::read_dir(target.join(id)).unwrap().count(), 1);
    assert_eq!(fs::read(target.join(id).join("sources.json")).unwrap(), b"null");
    record(&dir, id, &["sources"], "../elsewhere", 4);
    assert!(checked_record(&dir, id).err().unwrap().to_string().contains("Invalid internal backup key"));
    record(&dir, id, &["sources"], id, 4);
    fs::remove_file(folder.join("sources.json")).unwrap();
    assert!(checked_record(&dir, id).err().unwrap().to_string().contains("sources.json"));
    fs::write(folder.join("sources.json"), b"{}").unwrap();
    assert!(checked_record(&dir, id).err().unwrap().to_string().contains("2 of 4"));
}

#[test]
fn history_reads_a_flat_record_without_holds_and_checks_both_parts_of_an_import() {
    let tmp = Scratch::new();
    let dir = tmp.0.join("backups");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("legacy.json"), br#"{"id":"legacy","key":"legacy.sqlite","sizeBytes":5,"label":"Saved","createdAt":1}"#).unwrap();
    fs::write(dir.join("legacy.sqlite"), b"saved").unwrap();
    assert!(checked_record(&dir, "legacy").unwrap().holds.is_empty());
    fs::remove_file(dir.join("legacy.sqlite")).unwrap();
    assert!(checked_record(&dir, "legacy").err().unwrap().to_string().contains("legacy.sqlite"));
    record(&dir, "both", &["data", "sources"], "both", 9);
    fs::create_dir(dir.join("both")).unwrap();
    fs::write(dir.join("both/database.sqlite"), b"saved").unwrap();
    fs::write(dir.join("both/sources.json"), b"null").unwrap();
    assert!(checked_record(&dir, "both").is_ok());
    fs::remove_file(dir.join("both/database.sqlite")).unwrap();
    assert!(checked_record(&dir, "both").err().unwrap().to_string().contains("4 of 9"));
}

#[cfg(unix)]
#[test]
fn history_refuses_symlinks_and_unreadable_catalog_records() {
    use std::os::unix::fs::symlink;
    let tmp = Scratch::new();
    let dir = tmp.0.join("backups");
    let target = tmp.0.join("staging");
    let id = "before-import";
    record(&dir, id, &["data"], id, 5);
    let private = tmp.0.join("private");
    fs::create_dir(&private).unwrap();
    fs::write(private.join("database.sqlite"), b"other").unwrap();
    symlink(&private, dir.join(id)).unwrap();
    assert!(copy_records(&dir, &target).is_err());
    fs::remove_file(dir.join(id)).unwrap();
    fs::create_dir(dir.join(id)).unwrap();
    symlink(private.join("database.sqlite"), dir.join(id).join("database.sqlite")).unwrap();
    assert!(copy_records(&dir, &target).is_err());
    fs::remove_file(dir.join(format!("{id}.json"))).unwrap();
    symlink(private.join("database.sqlite"), dir.join(format!("{id}.json"))).unwrap();
    assert!(copy_records(&dir, &target).is_err());
    fs::remove_file(dir.join(format!("{id}.json"))).unwrap();
    fs::write(dir.join("bad.json"), "{}").unwrap();
    assert!(format!("{:#}", copy_records(&dir, &target).unwrap_err()).contains("bad"));
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[tokio::test]
async fn history_empty_data_survives_a_file_only_manifest() {
    let tmp = Scratch::new();
    let live = tmp.0.join("live");
    let repo = tmp.0.join("repo");
    let dir = layout::backups(&live, "app");
    fs::create_dir_all(dir.join("empty")).unwrap();
    record(&dir, "empty", &["data"], "empty", 0);
    let copy = run_copy(&AppState::for_test(&live), &[Target::Directory(repo.clone())]).await.unwrap();
    let source = Source::Directory(repo);
    assert!(audit(&source, &copy, None).await.unwrap().1);
    let recovered = tmp.0.join("restored");
    restore(&source, Some(&copy), &recovered, None, &Installation::for_test()).await.unwrap();
    assert!(checked_record(&layout::backups(&recovered, "app"), "empty").is_ok());
}

#[tokio::test]
async fn history_reports_three_missing_payloads_and_writes_a_separate_recovery_point() {
    let tmp = Scratch::new();
    let preserved = tmp.0.join("preserved");
    let historical = tmp.0.join("historical");
    let repo = tmp.0.join("repo");
    let old = copy_id(now_ms() - 2000);
    let app = "workshop";
    let dir = layout::backups(&historical, app);
    let data = layout::data(&historical, app);
    fs::create_dir_all(&data).unwrap();
    Connection::open(data.join("database.sqlite")).unwrap().execute_batch("CREATE TABLE notes(body); INSERT INTO notes VALUES ('current')").unwrap();

    for id in ["first", "second", "third"] {
        let folder = preserved.join(id);
        fs::create_dir_all(&folder).unwrap();
        for name in ["database.sqlite", "identity.sqlite", "usage.sqlite"] {
            vacuum_into(&data.join("database.sqlite"), &folder.join(name)).unwrap();
        }
        crate::package::folder_files_tar(&folder, &folder.join("files.tar")).unwrap();
        let size = list_files(&folder).unwrap().iter().map(|f| f.size).sum();
        record(&dir, id, &["data"], id, size);
    }
    let manifest = Manifest { version: 1, copy_id: old.clone(), created_at: now_ms(), files: list_files(&historical).unwrap(), encryption: None, mac: None };
    fs::write(historical.join(MANIFEST), serde_json::to_vec(&manifest).unwrap()).unwrap();
    send_to_directory(&historical, &repo, &old, &manifest, &HashMap::new(), &Installation::for_test()).unwrap();
    let original_hash = checksum_file(&repo.join(&old).join(MANIFEST)).unwrap();
    let source = Source::Directory(repo.clone());
    let (report, ok) = audit(&source, &old, None).await.unwrap();
    assert!(!ok);
    assert_eq!(report["currentApp"], "unchecked");
    let rows = report["internalBackups"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for (row, id) in rows.iter().zip(["first", "second", "third"]) {
        assert_eq!(row["backup"], id);
        assert_eq!(row["scope"], "data");
        assert_eq!(row["status"], "missing");
    }
    assert!(!report.to_string().contains("Private owner label"));

    let restored = tmp.0.join("restored");
    restore(&source, Some(&old), &restored, None, &Installation::for_test()).await.unwrap();
    let state = AppState::for_test(&restored);
    let db = Connection::open(layout::database(&restored, app)).unwrap();
    assert_eq!(db.query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0)).unwrap(), "current");
    drop(db);
    assert!(crate::restore_backup(&state, app, "first").await.is_err());
    let target = Target::Directory(repo.clone());
    let refusal = run_copy(&state, std::slice::from_ref(&target)).await.unwrap_err().to_string();
    assert!(refusal.contains("Backup") && refusal.contains("(data)"), "{refusal}");
    assert_eq!(source.complete_copies().await.unwrap(), [old.clone()]);
    let dir = layout::backups(&restored, app);
    for id in ["first", "second", "third"] {
        copy_tree(&preserved.join(id), &dir.join(id)).unwrap();
    }
    let new = run_copy(&state, &[target]).await.unwrap();
    assert_ne!(old, new);
    let (report, ok) = audit(&source, &new, None).await.unwrap();
    assert!(ok);
    assert_eq!(report["internalBackups"].as_array().unwrap().len(), 3);
    assert!(report["internalBackups"].as_array().unwrap().iter().all(|r| r["status"] == "recovered"));
    assert_eq!(checksum_file(&repo.join(&old).join(MANIFEST)).unwrap(), original_hash);
    assert!(!audit(&source, &old, None).await.unwrap().1);
    let copied = layout::backups(&repo.join(new.clone()), app);
    fs::remove_file(copied.join("first/files.tar")).unwrap();
    fs::write(copied.join("second/files.tar"), "damaged").unwrap();
    let (damaged, ok) = audit(&source, &new, None).await.unwrap();
    assert!(!ok);
    let states: Vec<_> = damaged["internalBackups"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(states, ["missing", "unchecked", "recovered"]);
}
