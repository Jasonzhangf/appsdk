use std::process::Command;

#[test]
fn init_does_not_reject_a_directory_named_playground() {
    let root = std::env::temp_dir().join(format!("collab-init-path-{}", std::process::id()));
    let work = root.join("playground").join("project");
    let ordinary = root.join("ordinary");
    let state = root.join("state");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&ordinary).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("DOWN"), "").unwrap();
    for directory in [&ordinary, &work] {
        let output = Command::new(env!("CARGO_BIN_EXE_collab"))
            .arg("init")
            .current_dir(directory)
            .env("COLLAB_STATE_DIR", &state)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(stderr.contains("DAEMON_UNAVAILABLE"), "{stderr}");
        assert!(!directory.join(".agent-collab").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn collab_rejects_an_external_state_root_before_using_its_socket() {
    use std::os::unix::fs::MetadataExt;
    let external = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let system = std::path::Path::new("/System/Volumes/Data");
    if std::fs::metadata(external).unwrap().dev() == std::fs::metadata(system).unwrap().dev() {
        return;
    }
    let root = external.join(format!(
        "state-root-must-not-be-created-{}",
        std::process::id()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_collab"))
        .arg("init")
        .env("COLLAB_STATE_DIR", &root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("COLLAB_SYSTEM_DISK_REQUIRED"), "{stderr}");
    assert!(!root.exists());
    let status = Command::new(env!("CARGO_BIN_EXE_collab"))
        .arg("status")
        .env("COLLAB_STATE_DIR", &root)
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("COLLAB_SYSTEM_DISK_REQUIRED"));
    assert!(!root.exists());

    let link_dir = std::env::temp_dir().join(format!("collab-volume-link-{}", std::process::id()));
    std::fs::create_dir_all(&link_dir).unwrap();
    std::os::unix::fs::symlink(external, link_dir.join("external")).unwrap();
    let linked_root = link_dir.join("external").join(format!(
        "state-root-must-not-be-created-{}",
        std::process::id()
    ));
    let linked = Command::new(env!("CARGO_BIN_EXE_collab"))
        .arg("init")
        .env("COLLAB_STATE_DIR", &linked_root)
        .output()
        .unwrap();
    assert!(!linked.status.success());
    assert!(String::from_utf8_lossy(&linked.stderr).contains("COLLAB_SYSTEM_DISK_REQUIRED"));
    assert!(!linked_root.exists());
    std::fs::remove_dir_all(link_dir).unwrap();
}
