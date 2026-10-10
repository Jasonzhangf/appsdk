include!("support/tmux_cli_fixture.rs");

use std::os::unix::fs::PermissionsExt;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread::JoinHandle;

static TMUX_FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn mcp_binary() -> std::path::PathBuf {
    let explicit_cli = std::env::var_os("COLLAB_TEST_BINARY");
    let cli = explicit_cli
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_collab")));
    std::env::var_os("COLLAB_TEST_MCP_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            if explicit_cli.is_some() {
                cli.parent()
                    .expect("CLI has a parent directory")
                    .join("collab-mcp")
            } else {
                std::path::PathBuf::from(env!("CARGO_BIN_EXE_collab-mcp"))
            }
        })
}

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    reader: Option<JoinHandle<()>>,
}

impl Mcp {
    fn start(fixture: &Fixture, pane: Option<&Pane>) -> Self {
        let mut command = Command::new(mcp_binary());
        command
            .current_dir(&fixture.root)
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("CODEX_HOME", fixture.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("COLLAB_WORKER")
            .env_remove("DSH_SESSION_ID")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("COLLAB_BIN", &fixture.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(pane) = pane {
            command
                .env(
                    "TMUX",
                    format!("{},{},0", fixture.tmux_socket.display(), pane.server_pid),
                )
                .env("TMUX_PANE", &pane.pane_id)
                .env("CODEX_SESSION_ID", &pane.session_anchor)
                .env("CODEX_THREAD_ID", &pane.thread_anchor);
        } else {
            command
                .env_remove("TMUX")
                .env_remove("TMUX_PANE")
                .env_remove("CODEX_SESSION_ID")
                .env_remove("CODEX_THREAD_ID");
        }
        let mut child = command.spawn().expect("launch actual paired collab-mcp");
        let stdin = child.stdin.take().expect("MCP stdin");
        let stdout = child.stdout.take().expect("MCP stdout");
        let (sender, replies) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let reply = line.map_err(|error| error.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|error| error.to_string())
                });
                if sender.send(reply).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            replies,
            reader: Some(reader),
        }
    }

    fn call(&mut self, id: u64, arguments: Value) -> Value {
        let stdin = self.stdin.as_mut().expect("MCP stdin is open");
        serde_json::to_writer(
            &mut *stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{"name":"collab_subagent","arguments":arguments}
            }),
        )
        .unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
        let reply = self
            .replies
            .recv_timeout(Duration::from_secs(20))
            .expect("MCP returns a bounded response")
            .expect("MCP returns valid JSON");
        assert_eq!(reply["id"], id, "{reply}");
        reply["result"].clone()
    }

    fn finish(mut self) {
        self.stdin.take();
        let status = self.child.wait().expect("wait for MCP process");
        assert!(status.success(), "MCP process must exit cleanly: {status}");
        if let Some(reader) = self.reader.take() {
            reader.join().expect("join MCP stdout reader");
        }
    }
}

fn contract_stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn create_daemon_for_subagent_cli(name: &str) -> (Fixture, Pane, String) {
    let (fixture, pane) = single_pane_fixture(name);
    let context = fixture.run_ok(&["context"], Some(&pane));
    let snapshot = &context["result"]["snapshot"];
    assert_eq!(
        snapshot["registered"], true,
        "context must contain a registered daemon snapshot: {context}"
    );
    assert_eq!(
        context["bootstrap"]["registered"], snapshot["registered"],
        "bootstrap registration state must match the daemon snapshot: {context}"
    );
    assert_eq!(
        context["bootstrap"]["identity"], "daemon-owned",
        "registered bootstrap identity must be daemon-owned: {context}"
    );
    let worker_id = snapshot["identity"]["worker_id"]
        .as_str()
        .expect("daemon-owned worker id")
        .to_owned();
    (fixture, pane, worker_id)
}

#[test]
fn start_is_rejected_before_identity_coordination() {
    let root = unique_root();
    std::fs::create_dir_all(&root).expect("create isolated project root");
    seed_baseline(&root);
    let fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state: root.join("h"),
        tmux_socket: root.join("t.sock"),
        initialized: false,
    };

    let output = fixture.command(&["subagent", "start", "--id", "child"], None);
    assert!(!output.status.success(), "Start must fail: {output:?}");
    assert!(
        output.stdout.is_empty(),
        "Start must not print empty success"
    );
    let stderr = contract_stderr(&output);
    assert!(
        stderr.contains("MANAGED_SUBAGENT_UNSUPPORTED"),
        "Start must keep its typed refusal: {stderr}"
    );
    assert!(
        !fixture.host_state.join("server.sock").exists(),
        "Start must terminate before identity coordination or daemon start"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn list_and_unknown_status_use_observe_without_mutation_token() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane, _worker_id) =
        create_daemon_for_subagent_cli("collab-subagent-public-observe");

    let listed = fixture.run_ok(&["subagent", "list"], Some(&pane));
    assert!(
        listed["subagents"].as_array().is_some(),
        "List must return a structural array: {listed}"
    );

    let unknown = fixture.command(&["subagent", "status", "missing-child"], Some(&pane));
    assert!(!unknown.status.success(), "unknown Status must fail");
    assert!(
        unknown.stdout.is_empty(),
        "unknown Status must not be empty success"
    );
    let stderr = contract_stderr(&unknown);
    assert!(
        stderr.contains("unknown subagent"),
        "Status must preserve the daemon typed failure: {stderr}"
    );

    drop(fixture);
}

#[test]
fn mutating_actions_for_missing_child_fail_nonempty_at_cli_and_mcp() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane, _worker_id) =
        create_daemon_for_subagent_cli("collab-subagent-public-missing-child");

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("snapshot", vec!["subagent", "snapshot", "missing-child"]),
        ("rearm", vec!["subagent", "rearm", "missing-child"]),
        (
            "send",
            vec![
                "subagent",
                "send",
                "missing-child",
                "--subject",
                "subject",
                "body",
            ],
        ),
        ("ready", vec!["subagent", "ready", "missing-child"]),
        ("working", vec!["subagent", "working", "missing-child"]),
        ("close", vec!["subagent", "close", "missing-child"]),
    ];
    for (label, args) in cases {
        let output = fixture.command(&args, Some(&pane));
        assert!(!output.status.success(), "{label} must fail: {output:?}");
        assert!(
            output.stdout.is_empty(),
            "{label} must not emit empty success stdout"
        );
        let stderr = contract_stderr(&output);
        assert!(
            !stderr.trim().is_empty(),
            "{label} must return a non-empty typed error"
        );
        assert!(
            stderr.contains("collab response:") || stderr.contains("unknown subagent"),
            "{label} must preserve the daemon response or typed error: {stderr}"
        );
    }

    let mut mcp = Mcp::start(&fixture, Some(&pane));
    let result = mcp.call(
        7,
        json!({"action":"send","id":"missing-child","subject":"subject","body":"body"}),
    );
    assert_eq!(result["isError"], true, "MCP must mark failure: {result}");
    assert!(
        result["content"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["text"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()))),
        "MCP failure must have non-empty content: {result}"
    );
    mcp.finish();

    drop(fixture);
}

#[test]
fn mcp_rejects_empty_or_invalid_success_output_for_subagent_only() {
    let root = unique_root();
    let mut fixture = Fixture {
        binary: std::path::PathBuf::from("/usr/bin/true"),
        root: root.clone(),
        host_state: root.join("h"),
        tmux_socket: root.join("t.sock"),
        initialized: false,
    };
    std::fs::create_dir_all(&fixture.root).expect("create MCP test root");

    let mut mcp = Mcp::start(&fixture, None);
    let empty = mcp.call(1, json!({"action":"list"}));
    assert_eq!(
        empty["isError"], true,
        "empty successful CLI stdout must be a protocol error: {empty}"
    );
    assert!(
        empty["content"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["text"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()))),
        "protocol error must be non-empty: {empty}"
    );
    mcp.finish();

    let invalid_cli = root.join("invalid-cli");
    std::fs::write(&invalid_cli, "#!/bin/sh\nprintf 'not-json\\n'\n")
        .expect("write isolated invalid-output CLI");
    std::fs::set_permissions(&invalid_cli, std::fs::Permissions::from_mode(0o700))
        .expect("make invalid-output CLI executable");
    fixture.binary = invalid_cli;
    let mut mcp = Mcp::start(&fixture, None);
    let invalid = mcp.call(2, json!({"action":"list"}));
    assert_eq!(
        invalid["isError"], true,
        "invalid successful CLI JSON must be a protocol error: {invalid}"
    );
    assert!(
        invalid["content"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["text"]
                .as_str()
                .is_some_and(|text| text.contains("COLLAB_MCP_INVALID_RESULT")))),
        "protocol error must identify invalid output: {invalid}"
    );
    mcp.finish();

    std::fs::remove_dir_all(&fixture.root).ok();
}
