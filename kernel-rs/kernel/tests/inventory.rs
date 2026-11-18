use std::{fs, process::Command};

#[test]
fn standalone_inventory_classifies_a_local_mail_key_without_owner_stores() {
    let root = std::env::temp_dir().join(format!("kernel-inventory-{}", uuid::Uuid::new_v4()));
    let inspect = || {
        Command::new(env!("CARGO_BIN_EXE_kernel"))
            .env("KONAE_DATA_DIR", &root)
            .arg("inventory")
            .output()
            .unwrap()
    };
    assert!(!inspect().status.success());
    assert!(!root.exists());
    fs::create_dir_all(root.join("apps")).unwrap();
    fs::write(root.join("mail-key"), "fixture").unwrap();
    let output = inspect();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["binary"], "kernel");
    assert_eq!(report["components"]["root/mail-key"], 1);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
    fs::write(root.join("owners.sqlite"), "").unwrap();
    let output = inspect();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("owners.sqlite"));
    fs::remove_dir_all(root).unwrap();
}
