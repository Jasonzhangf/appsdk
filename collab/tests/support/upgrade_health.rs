use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("COLLAB_VERSION");
/// Keep each controlled listener alive for the product's five-second check
/// budget plus one second for host process startup.
pub const LISTENER_ACCEPT_TIMEOUT: Duration = Duration::from_secs(6);

static TEST_EXECUTION_LOCK: Mutex<()> = Mutex::new(());

/// Serialize one whole fixture test, including controlled-listener setup.
/// Without this, a queued test's responder starts its accept window before the
/// serialized child launches and the controlled socket expires prematurely.
pub struct TestGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

pub fn test_guard() -> TestGuard {
    TestGuard {
        _guard: TEST_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    }
}

pub fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from("/tmp").join(format!("cuh-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

pub fn copy_pair(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cli = bin.join("collab");
    std::fs::copy(env!("CARGO_BIN_EXE_collab"), &cli).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_collab-mcp"), bin.join("collab-mcp")).unwrap();
    cli
}

pub fn run_check(cli: &Path, project_root: &Path, state_root: &Path, local: bool) -> Output {
    run_check_with_dsh_session(cli, project_root, state_root, local, None)
}

pub fn run_check_with_dsh_session(
    cli: &Path,
    project_root: &Path,
    state_root: &Path,
    local: bool,
    dsh_session_id: Option<&str>,
) -> Output {
    let home = state_root.join("fixture-home");
    let cargo_home = state_root.join("fixture-cargo");
    let build_state = state_root.join("fixture-build");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cargo_home).unwrap();
    std::fs::create_dir_all(&build_state).unwrap();
    let mut command = Command::new(cli);
    command.args(["status", "--check"]);
    if local {
        command.arg("--local");
    }
    command
        .current_dir(project_root)
        .env("HOME", &home)
        .env("CARGO_HOME", &cargo_home)
        .env("COLLAB_STATE_DIR", state_root)
        .env("COLLAB_BUILD_STATE_DIR", &build_state)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("DSH_SESSION_ID")
        .env_remove("COLLAB_BIN");
    if let Some(session_id) = dsh_session_id {
        command.env("DSH_SESSION_ID", session_id);
    }
    command.output().unwrap()
}

pub fn snapshot(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return entries;
    };
    if metadata.file_type().is_symlink() {
        entries.push((
            path.to_path_buf(),
            std::fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_bytes()
                .to_vec(),
        ));
        return entries;
    }
    if metadata.is_dir() {
        let Ok(children) = std::fs::read_dir(path) else {
            return entries;
        };
        let mut children: Vec<_> = children.flatten().collect();
        children.sort_by_key(|entry| entry.path());
        for child in children {
            entries.extend(snapshot(&child.path()));
        }
        return entries;
    }
    entries.push((path.to_path_buf(), std::fs::read(path).unwrap_or_default()));
    entries
}

pub fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn ping_response(version: Option<&str>, capability: Option<&str>) -> Vec<u8> {
    let mut response = json!({
        "ok": true,
        "workers": 0,
        "messages": 0,
        "tasks": 0,
        "now": "2026-11-08T00:00:00Z",
    });
    if let Some(version) = version {
        response["daemon_version"] = json!(version);
    }
    if let Some(capability) = capability {
        response["capabilities"] = json!([capability]);
    }
    format!("{response}\n").into_bytes()
}

pub fn readiness_response() -> Vec<u8> {
    format!(
        "{}\n",
        json!({
            "ok": true,
            "status": "ready",
            "context": "passed",
            "board": "passed",
            "receive": {"status": "passed", "owner": "gateway"},
            "send": "passed",
            "transport": {"kind": "dsh", "receive_owner": "gateway"}
        })
    )
    .into_bytes()
}

pub fn connect(listener: &UnixListener, timeout: Duration) -> std::os::unix::net::UnixStream {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "status --check did not connect to the controlled listener"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept status --check connection: {error}"),
        }
    }
}

pub fn request(stream: &mut std::os::unix::net::UnixStream) -> Value {
    let mut line = String::new();
    let count = BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(count > 0, "controlled daemon did not receive a request");
    serde_json::from_str(line.trim()).unwrap()
}

pub fn create_project(root: &Path) -> PathBuf {
    let project = root.join("project");
    std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
    project
}

pub fn retained_dsh_identity(project: &Path, state_root: &Path) {
    let canonical = std::fs::canonicalize(project).unwrap();
    let project_scope = canonical.to_string_lossy();
    std::fs::create_dir_all(state_root.join("identities/readiness-worker")).unwrap();
    let transport = json!({
        "kind": "dsh",
        "endpoint": "unix:///tmp/collab-readiness-gateway.sock",
        "namespace": "readiness-gateway",
        "session_id": "readiness-session",
        "thread_id": "readiness-thread",
        "capabilities": ["send_message_to_thread", "probe_thread"],
        "self_check": "readiness gateway"
    });
    let binding = json!({
        "project_scope": project_scope,
        "app_scope_id": "appserver-cli",
        "agent_id": "readiness-worker",
        "runtime_id": "readiness-runtime",
        "binding_id": "readiness-binding",
        "session_id": "readiness-session"
    });
    let identity = json!({
        "worker_id": "readiness-worker",
        "token": "readiness-token",
        "project_scope": project_scope,
        "runtime": {
            "agent_id": "readiness-worker",
            "runtime_id": "readiness-runtime",
            "appserver_id": "appserver-cli",
            "binding_id": "readiness-binding",
            "session_id": "readiness-session"
        },
        "transport": transport.clone()
    });
    std::fs::write(
        state_root.join("identities/readiness-worker/identity.json"),
        serde_json::to_vec_pretty(&identity).unwrap(),
    )
    .unwrap();
    let map = json!({
        "schema_version": 1,
        "slots": [{
            "identity_key": {
                "project_scope": project_scope,
                "app_scope_id": "appserver-cli",
                "name": "Alice"
            },
            "display_slot": 2,
            "identity_status": "active",
            "generation": 1,
            "current": {
                "binding": binding,
                "transport": transport,
                "tab_id": {
                    "project_scope": project_scope,
                    "app_scope_id": "appserver-cli"
                },
                "host": {
                    "agent_kind": "codex",
                    "host_type": "dsh",
                    "profile": null
                },
                "invocation_cwd": project_scope,
                "physical_name": "Alice",
                "layout_binding_id": null
            },
            "previous": null
        }]
    });
    std::fs::write(
        state_root.join("identity-map.json"),
        serde_json::to_vec_pretty(&map).unwrap(),
    )
    .unwrap();
}

pub fn scripted_mcp(cli: &Path, script: &str) {
    let path = cli.parent().unwrap().join("collab-mcp");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
