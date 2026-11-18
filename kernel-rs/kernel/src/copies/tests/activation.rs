use super::*;
use std::{fs, process::Command, time::Instant};

#[tokio::test]
async fn restart_refuses_a_volume_killed_between_two_moves() {
    const CHILD: &str = "KONAE_TEST_ACTIVATION_ROOT";
    if let Some(dir) = std::env::var_os(CHILD) {
        let root = PathBuf::from(dir);
        let _lock = crate::file_store::hold_while_stopped(&root, "restore").unwrap();
        activate(&root.join(".restore-interrupted"), &root, |dir| {
            sync_path(dir)?;
            if !root.join("owners.sqlite").exists() && !root.join("mcp.sqlite").exists() {
                return Ok(());
            }
            fs::write(root.with_extension("ready"), []).unwrap();
            loop { std::thread::park(); }
        }).unwrap();
        return;
    }

    let root = scratch("activation");
    let staging = root.join(".restore-interrupted");
    fs::create_dir_all(&staging).unwrap();
    for name in ["owners.sqlite", "mcp.sqlite"] {
        Connection::open(staging.join(name)).unwrap().execute_batch("CREATE TABLE fixture(id)").unwrap();
    }
    let ready = root.with_extension("ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "copies::tests::activation::restart_refuses_a_volume_killed_between_two_moves"])
        .env(CHILD, &root).spawn().unwrap();
    let start = Instant::now();
    while !ready.exists() && child.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(ready.is_file());
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);
    assert_eq!(["owners.sqlite", "mcp.sqlite"].iter().filter(|name| root.join(name).exists()).count(), 1);

    let mut startup = crate::Startup::from_env().unwrap();
    startup.data_root = root.clone();
    let error = crate::start(&startup, Installation::kernel(), "lab", vec![], None).await.err().unwrap();
    assert!(error.to_string().contains("restore is incomplete"));
    assert!(!root.join("apps").exists());
    fs::remove_file(ready).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_failed_sync_keeps_the_volume_closed() {
    let root = scratch("activation-sync");
    let stage = root.join(".restore-staged");
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("fixture"), b"whole file").unwrap();
    let error = activate(&stage, &root, |path| {
        if root.join("fixture").exists() { bail!("injected sync failure"); }
        sync_path(path)
    }).unwrap_err();
    assert_eq!(error.to_string(), "injected sync failure");
    assert!(check_activation(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_failed_final_sync_restores_the_startup_guard() {
    let root = scratch("activation-final-sync");
    let stage = root.join(".restore-staged");
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("fixture"), b"whole file").unwrap();
    let error = activate(&stage, &root, |dir| {
        if !stage.exists() { bail!("injected final sync failure"); }
        sync_path(dir)
    }).unwrap_err();
    assert_eq!(error.to_string(), "injected final sync failure");
    assert!(check_activation(&root).is_err());
    assert_eq!(fs::read(root.join("fixture")).unwrap(), b"whole file");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn restore_checks_the_cut_before_signing_out() {
    let root = scratch("invalid-cut");
    let live = root.join("live");
    let repo = root.join("copies");
    let (app, _) = seed(&live);
    let identity = crate::layout::identity(&live, &app.id);
    Connection::open(identity).unwrap().execute_batch(
        "PRAGMA foreign_keys=OFF;
         UPDATE app_sessions SET user_id='missing-at-cut';"
    ).unwrap();
    let copy = run_copy(&state(&live), &[Target::Directory(repo.clone())]).await.unwrap();
    let destination = root.join("destination");
    let error = restore(&Source::Directory(repo), Some(&copy), &destination, None, &Installation::for_test())
        .await.err().unwrap();
    assert!(error.to_string().contains("broken foreign key"));
    assert_eq!(installed_entries(&destination), 0);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn the_file_index_must_match_the_bytes_and_accounts_in_its_app() {
    let root = scratch("invalid-file-owner");
    let (app, _) = seed(&root);
    let index = crate::layout::files(&root, &app.id).join("index.json");
    let original: Value = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
    for (field, value, error) in [
        ("ownerId", json!("another-app-user"), "no matching app user"),
        ("size", json!(11), "Indexed size differs"),
        ("sha256", json!("0".repeat(64)), "Indexed SHA-256 differs"),
    ] {
        let mut files = original.clone();
        files[0][field] = value;
        fs::write(&index, files.to_string()).unwrap();
        let refused = validation::check_cut(&root, &Installation::for_test()).unwrap_err();
        assert!(format!("{refused:#}").contains(error), "{refused:#}");
    }
    fs::remove_file(&index).unwrap();
    assert!(format!("{:#}", validation::check_cut(&root, &Installation::for_test()).unwrap_err()).contains("absent from the index"));
    fs::remove_dir_all(root).unwrap();
}
