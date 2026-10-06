use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from("/tmp").join(format!("cms-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn cli_output(project_root: &Path, state_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_collab"))
        .args(["master", "status"])
        .current_dir(project_root)
        .env("COLLAB_STATE_DIR", state_root)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .output()
        .unwrap()
}

fn register_route(project_root: &Path, state_root: &Path) {
    std::fs::create_dir_all(project_root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(state_root).unwrap();
    let root = project_root.canonicalize().unwrap();
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": "appserver-cli",
        "project_scope": root,
        "canonical_root": root,
        "storage_root": root,
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
}

#[test]
fn master_status_uses_registered_cwd_without_tmux() {
    let root = temp_root("registered");
    let project_root = root.join("project");
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&project_root).unwrap();
    register_route(&project_root, &state_root);

    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let expected_root = project_root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let responder = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(&stream)
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["op"], "MasterStatus");
        assert_eq!(request["project_context"]["app_scope_id"], "appserver-cli");
        assert_eq!(request["project_context"]["canonical_root"], expected_root);
        stream
            .write_all(br#"{"ok":true,"master":{"worker_id":"master-peer","endpoint_live":true}}"#)
            .unwrap();
        request
    });

    let output = cli_output(&project_root, &state_root);
    let request = responder.join().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(request["op"], "MasterStatus");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["master"]["worker_id"], "master-peer");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_status_without_registered_cwd_fails_closed_before_daemon_call() {
    let root = temp_root("unregistered");
    let project_root = root.join("project");
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();

    let output = cli_output(&project_root, &state_root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no registered Collab route contains cwd"),
        "unexpected error: {stderr}"
    );
    assert!(!stderr.contains("TMUX_ENDPOINT_MISSING"), "{stderr}");
    assert!(!state_root.join("server.sock").exists());

    std::fs::remove_dir_all(root).unwrap();
}
