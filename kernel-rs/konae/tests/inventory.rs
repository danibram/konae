use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Volume(PathBuf);
impl Volume {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("konae-inventory-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("apps")).unwrap();
        let v = Self(root);
        v.file("owners.sqlite");
        v
    }

    fn file(&self, path: &str) {
        let file = self.0.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        // An inventory that opens these as databases or JSON will fail.
        fs::write(file, b"metadata only").unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_konae"))
            .env("KONAE_DATA_DIR", &self.0)
            .arg("inventory")
            .args(args)
            .output()
            .unwrap()
    }

    fn report(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn refused(&self, expected: &str) {
        let out = self.run(&[]);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let diagnostic = String::from_utf8(out.stderr).unwrap();
        assert!(diagnostic.contains(expected), "{diagnostic}");
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut items = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        items.insert(entry.path(), (metadata.len(), metadata.modified().unwrap()));
        if metadata.is_dir() {
            items.extend(snapshot(&entry.path()));
        }
    }
    items
}

#[test]
fn production_sample_counts_flows_objects_and_nested_history() {
    let v = Volume::new();
    for name in [
        "mcp.sqlite",
        "calls.sqlite",
        "mail.sqlite",
        "transfers.sqlite",
        "google.sqlite",
        "google.sqlite-wal",
    ] {
        v.file(name);
    }
    for n in 0..6 {
        let app = format!("apps/app_{n}");
        for file in [
            "konae.json",
            ".konae/deployment.json",
            "app/schema.sql",
            "dist/index.html",
            "data/database.sqlite",
            "data/identity.sqlite",
            "data/usage.sqlite",
            "data/secrets.sqlite",
        ] {
            v.file(&format!("{app}/{file}"));
        }
    }
    v.file("apps/app_0/data/files/index.json");
    for n in 0..255 {
        v.file(&format!("apps/app_0/data/files/00000000-0000-4000-8000-{n:012x}.blob"));
    }
    for n in 0..3 {
        let record = format!("apps/app_0/.konae/backups/old-{n}");
        v.file(&format!("{record}.json"));
        for part in ["database.sqlite", "identity.sqlite", "usage.sqlite", "files.tar"] {
            v.file(&format!("{record}/{part}"));
        }
    }
    let before = snapshot(&v.0);
    let report = v.report(&[]);
    let components = &report["components"];
    assert_eq!(components["apps/{app}"], 6);
    for name in ["database.sqlite", "identity.sqlite", "usage.sqlite", "secrets.sqlite"] {
        assert_eq!(components[format!("data/{name}")], 6);
    }
    assert_eq!(components["root/google.sqlite"], 1);
    assert_eq!(components["files/{uuid}.blob"], 255);
    assert_eq!(components["backups/{backup}"], 3);
    for part in ["database.sqlite", "identity.sqlite", "usage.sqlite", "files.tar"] {
        assert_eq!(components[format!("payload/{part}")], 3);
    }
    assert_eq!(before, snapshot(&v.0));
    fs::remove_dir_all(v.0.join("apps/app_0/.konae/backups/old-1")).unwrap();
    v.refused("Missing internal backup payload");
}

#[test]
fn local_sample_keeps_the_draft_and_absent_optional_stores() {
    let v = Volume::new();
    for i in 0..4 {
        let app = format!("apps/project_{i}");
        for part in ["konae.json", "app/index.html", "data/secrets.sqlite"] {
            v.file(&format!("{app}/{part}"));
        }
        if i < 3 {
            for part in [
                "data/database.sqlite",
                "data/identity.sqlite",
                "data/usage.sqlite",
                "dist/index.html",
                ".konae/deployment.json",
            ] {
                v.file(&format!("{app}/{part}"));
            }
        }
    }
    v.file("apps/project_3/.konae/sources.json");
    v.file("apps/project_0/.konae/backups/no-modules.json");
    v.file("apps/project_0/.konae/backups/no-modules/.empty-data");
    assert!(!v.0.join("mail-key").exists());
    assert!(v.report(&[])["components"].get("root/mail-key").is_none());
    assert!(!v.0.join("mail-key").exists());
    v.file("mail-key");
    let before = snapshot(&v.0);
    let report = v.report(&[]);
    let components = &report["components"];
    assert_eq!(components["root/mail-key"], 1);
    assert_eq!(components["payload/.empty-data"], 1);
    assert_eq!(components["app/app"], 4);
    assert_eq!(components["app/dist"], 3);
    assert_eq!(components["data/secrets.sqlite"], 4);
    for name in ["database.sqlite", "identity.sqlite", "usage.sqlite"] {
        assert_eq!(components[format!("data/{name}")], 3);
    }
    assert_eq!(components["private/sources.json"], 1);
    assert!(components.get("files/index.json").is_none());
    assert_eq!(report["binary"], "konae");
    assert_eq!(before, snapshot(&v.0));
}

#[test]
fn unclassified_components_wrong_types_and_missing_settings_refuse_certification() {
    let v = Volume::new();
    v.file("apps/test/konae.json");
    for name in [
        "new.sqlite",
        "new.sqlite-wal",
        "google.sqlite-wal",
        "apps/test/data/new.store",
        "apps/test/.konae/notes.json",
        "apps/test/.konae/backups/unpaired.sqlite",
        "apps/test/data/files/unknown.blob",
    ] {
        v.file(name);
        v.refused(name);
        fs::remove_file(v.0.join(name)).unwrap();
    }
    fs::create_dir(v.0.join("google.sqlite")).unwrap();
    v.refused("google.sqlite");
    fs::remove_dir(v.0.join("google.sqlite")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(v.0.join("owners.sqlite"), v.0.join("google.sqlite")).unwrap();
        v.refused("google.sqlite");
        fs::remove_file(v.0.join("google.sqlite")).unwrap();
    }
    fs::remove_file(v.0.join("apps/test/konae.json")).unwrap();
    v.refused("apps/test/konae.json");
}

#[test]
fn policy_uses_the_real_konae_registry_and_needs_no_data_directory() {
    let v = Volume::new();
    let unused = v.0.join("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_konae"))
        .env("KONAE_DATA_DIR", &unused)
        .args(["inventory", "--policy"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let policies = report["policies"].as_array().unwrap();
    assert!(policies.iter().any(|row| row["component"] == "owners.sqlite"));
    let mail_key = policies.iter().find(|row| row["component"] == "mail-key").unwrap();
    assert_eq!(mail_key["binary"], "both");
    assert_eq!(mail_key["presence"], "optional");
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        report["operations"]["installationCopy"]["recover"],
        "restore --from <copy repository>"
    );
    assert_eq!(
        report["operations"]["appExport"]["recover"],
        "project_import or kernel import"
    );
    assert_eq!(
        report["operations"]["internalBackup"]["recover"],
        "database_execute restore-backup"
    );
    assert!(!unused.exists());
}
