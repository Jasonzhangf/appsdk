use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

fn binaries() -> (PathBuf, PathBuf) {
    let explicit_cli = std::env::var_os("COLLAB_TEST_BINARY");
    let cli = explicit_cli
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")));
    let mcp = std::env::var_os("COLLAB_TEST_MCP_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if explicit_cli.is_some() {
                cli.parent().expect("CLI has a parent").join("collab-mcp")
            } else {
                PathBuf::from(env!("CARGO_BIN_EXE_collab-mcp"))
            }
        });
    assert!(cli.is_file(), "CLI binary missing: {}", cli.display());
    assert!(
        mcp.is_file(),
        "paired MCP binary missing: {}",
        mcp.display()
    );
    (cli, mcp)
}

struct Fixture {
    root: PathBuf,
    cli: PathBuf,
    mcp: PathBuf,
    server_pid: String,
    pane: String,
    finished: bool,
}

impl Fixture {
    fn new() -> Self {
        let (cli, mcp) = binaries();
        let root = std::env::temp_dir().join(format!("mc{}", std::process::id()));
        std::fs::create_dir(&root).expect("create exclusively owned fixture root");
        std::fs::create_dir(root.join("h")).unwrap();
        std::fs::create_dir(root.join(".agent-collab")).unwrap();
        let mut fixture = Self {
            root,
            cli,
            mcp,
            server_pid: String::new(),
            pane: String::new(),
            finished: false,
        };
        fixture.tmux(&["new-session", "-d", "-s", "mcp-authority", "sleep 600"]);
        let observation = fixture.tmux(&["display-message", "-p", "#{pid} #{pane_id}"]);
        let observation = String::from_utf8(observation.stdout).unwrap();
        let mut facts = observation.split_whitespace();
        fixture.server_pid = facts.next().unwrap().to_owned();
        fixture.pane = facts.next().unwrap().to_owned();
        eprintln!(
            "owned MCP fixture root={} tmux_pid={} pane={}",
            fixture.root.display(),
            fixture.server_pid,
            fixture.pane
        );
        fixture
    }

    fn tmux(&self, args: &[&str]) -> Output {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(self.root.join("t.sock"))
            .args(args)
            .output()
            .expect("run owned tmux server operation");
        assert!(output.status.success(), "tmux {args:?}: {output:?}");
        output
    }

    fn environment(&self, command: &mut Command) {
        command
            .current_dir(&self.root)
            .env("COLLAB_STATE_DIR", self.root.join("h"))
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("COLLAB_WORKER")
            .env_remove("DSH_SESSION_ID")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env(
                "TMUX",
                format!(
                    "{},{},0",
                    self.root.join("t.sock").display(),
                    self.server_pid
                ),
            )
            .env("TMUX_PANE", &self.pane)
            .env("CODEX_SESSION_ID", "mcp-fixture-session")
            .env("CODEX_THREAD_ID", "mcp-fixture-thread");
    }

    fn cli(&self, args: &[&str]) -> Value {
        let mut command = Command::new(&self.cli);
        self.environment(&mut command);
        let output = command.args(args).output().expect("run actual CLI");
        assert!(output.status.success(), "CLI {args:?}: {output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON response");
        value
            .get("result")
            .and_then(|result| result.get("snapshot"))
            .cloned()
            .unwrap_or(value)
    }

    fn start_mcp(&self) -> Mcp {
        let mut command = Command::new(&self.mcp);
        self.environment(&mut command);
        let mut child = command
            .env("COLLAB_BIN", &self.cli)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch actual paired MCP");
        eprintln!("owned MCP process pid={}", child.id());
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let reply = line.map_err(|error| error.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|error| error.to_string())
                });
                if sender.send(reply).is_err() {
                    break;
                }
            }
        });
        Mcp {
            child,
            stdin: Some(stdin),
            replies,
            reader: Some(reader),
        }
    }

    fn finish(mut self) {
        self.cli(&["down"]);
        assert!(
            !self.root.join("h/server.sock").exists(),
            "daemon socket not released"
        );
        self.tmux(&["kill-server"]);
        std::fs::remove_dir_all(&self.root).expect("remove only owned successful fixture");
        assert!(!self.root.exists());
        self.finished = true;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut down = Command::new(&self.cli);
        self.environment(&mut down);
        match down.arg("down").output() {
            Ok(output) if output.status.success() => {}
            result => eprintln!("failed fixture daemon cleanup: {result:?}"),
        }
        match Command::new("tmux")
            .arg("-S")
            .arg(self.root.join("t.sock"))
            .arg("kill-server")
            .output()
        {
            Ok(output) if output.status.success() => {}
            result => eprintln!("failed fixture tmux cleanup: {result:?}"),
        }
        eprintln!(
            "preserved owned failed MCP fixture: {}",
            self.root.display()
        );
    }
}

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    reader: Option<JoinHandle<()>>,
}

impl Mcp {
    fn send(&mut self, request: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &request).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
        let response = self
            .replies
            .recv_timeout(Duration::from_secs(20))
            .expect("MCP must return a bounded protocol response")
            .expect("MCP must return valid JSON");
        assert_eq!(response["id"], id, "{response}");
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn clear(&mut self, id: u64, approval: &str) -> Value {
        self.request(
            id,
            "tools/call",
            json!({
                "name":"collab_master", "arguments":{"action":"clear", "approval":approval}
            }),
        )
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().expect("wait after stdin EOF").success());
        self.reader.take().unwrap().join().unwrap();
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                if let Err(error) = self.child.kill() {
                    eprintln!("owned MCP pid {} cleanup failed: {error}", self.child.id());
                }
                if let Err(error) = self.child.wait() {
                    eprintln!("owned MCP wait failed: {error}");
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            if reader.join().is_err() {
                eprintln!("owned MCP stdout reader panicked");
            }
        }
    }
}

#[test]
fn mcp_tools_call_clears_authority_and_refuses_invalid_approval() {
    let fixture = Fixture::new();
    let registered = fixture.cli(&["context"]);
    let holder = registered["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(registered["binding"]["app_scope_id"], "appserver-cli");
    let scope = json!({
        "project_scope":std::fs::canonicalize(&fixture.root).unwrap(),
        "app_scope_id":"appserver-cli"
    });
    let promoted = fixture.cli(&[
        "master",
        "promote",
        "--approval",
        "fixture user approved master",
    ]);
    assert_eq!(promoted["master"], holder);
    assert_eq!(promoted["scope"], scope);
    let mut mcp = fixture.start_mcp();
    let initialized = mcp.request(
        1,
        "initialize",
        json!({
            "protocolVersion":"2025-03-26", "capabilities":{},
            "clientInfo":{"name":"master-authority-public-consumer", "version":"1"}
        }),
    );
    assert_eq!(initialized["serverInfo"]["name"], "collab");
    mcp.send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));

    for (id, approval, expected) in [
        (2, "", "master clear requires explicit user approval"),
        (
            3,
            "approved\nforged",
            "master clear approval must not contain control characters",
        ),
    ] {
        let rejected = mcp.clear(id, approval);
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert!(
            rejected["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{rejected}"
        );
        let unchanged = fixture.cli(&["master", "status"]);
        assert_eq!(unchanged["master"]["worker_id"], holder, "{unchanged}");
        assert_eq!(unchanged["scope"], scope);
    }

    let cleared = mcp.clear(4, "fixture user approved clear");
    assert_eq!(cleared["isError"], false, "{cleared}");
    let receipt: Value =
        serde_json::from_str(cleared["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(receipt["master"].is_null(), "{receipt}");
    assert_eq!(receipt["previous_worker_id"], holder);
    assert_eq!(receipt["was_empty"], false);
    assert_eq!(receipt["scope"], scope);
    for args in [["master", "status"].as_slice(), ["context"].as_slice()] {
        let observed = fixture.cli(args);
        assert!(observed["master"].is_null(), "{observed}");
        assert_eq!(observed["scope"], scope);
    }
    let repeated = mcp.clear(5, "fixture user approved clear");
    assert_eq!(repeated["isError"], false, "{repeated}");
    let receipt: Value =
        serde_json::from_str(repeated["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["was_empty"], true);
    assert_eq!(receipt["scope"], scope);
    mcp.finish();
    fixture.finish();
}
