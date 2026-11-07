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
