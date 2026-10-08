use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "board/cli.rs"]
mod board_cli;

const THREAD_A: &str = "01a0d637-75e2-71b2-8634-2be0c08a9adc";
const THREAD_B: &str = "01a0d639-dafc-7571-a0aa-bbc2bbbc04ce";

fn unique_root() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let root = std::path::Path::new("/tmp").join(format!(
        "ct-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The single binary-selection seam for this consumer fixture. `cargo test`
/// builds and runs the debug binary by default; a canonical installed binary
/// can be substituted with `COLLAB_TEST_BINARY` so the same public entry point
/// drives identical installed bytes. The fallback is test-only and never a
/// product behavior.
fn binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

/// Seed the project baseline marker. `collab context` resolves its project root
/// from the `.agent-collab` baseline or a git root; a temp fixture is neither,
/// so it seeds the marker instead of reaching into daemon state.
fn seed_baseline(project: &std::path::Path) {
    std::fs::create_dir_all(project.join(".agent-collab")).expect("seed .agent-collab baseline");
}

/// Walk a parsed context snapshot and fail if any object key carries a secret.
fn assert_no_secret_surface(label: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert_ne!(key, "token", "{label}: raw token key leaked");
                assert_ne!(key, "identity_receipt", "{label}: private receipt leaked");
                assert_no_secret_surface(label, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_secret_surface(label, item);
            }
        }
        _ => {}
    }
}

/// The public `collab context` display must never expose the private credential
/// or the internal identity receipt, in raw text or in any parsed object.
fn assert_context_display_is_public(label: &str, output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("identity_receipt"),
        "{label}: context printed the private identity receipt: {stdout}"
    );
    assert!(
        !stdout.contains("\"token\""),
        "{label}: context printed a token field: {stdout}"
    );
    let parsed: Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("{label}: context emits JSON: {error}: stdout={stdout} stderr={stderr}")
    });
    assert_no_secret_surface(label, &parsed);
}

struct AppFixture {
    root: PathBuf,
    project: PathBuf,
    host_state: PathBuf,
    app_socket: PathBuf,
    state: Arc<Mutex<HashMap<String, ThreadState>>>,
    appserver_connections: Arc<AtomicU64>,
    initialized: bool,
    _server_thread: thread::JoinHandle<()>,
}

impl AppFixture {
    fn new() -> Self {
        Self::with_managed_socket(false)
    }

    fn new_desktop_managed() -> Self {
        Self::with_managed_socket(true)
    }

    fn with_managed_socket(managed_socket: bool) -> Self {
        let root = unique_root();
        let project = root.join("project");
        let host_state = root.join("h");
        let app_socket = if managed_socket {
            root.join("home/app-server-control/app-server-control.sock")
        } else {
            root.join("app.sock")
        };
        std::fs::create_dir_all(&project).expect("create project root");
        std::fs::create_dir_all(&host_state).expect("create host state root");
        std::fs::create_dir_all(app_socket.parent().unwrap())
            .expect("create AppServer socket root");
        seed_baseline(&project);
        let project_str = std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let listener = UnixListener::bind(&app_socket).expect("bind app server fixture");
        let shared = Arc::new(Mutex::new(HashMap::<String, ThreadState>::new()));
        let server_shared = Arc::clone(&shared);
        let connections = Arc::new(AtomicU64::new(0));
        let server_connections = Arc::clone(&connections);
        let server_scope = project_str.clone();
        let server_thread = thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        server_connections.fetch_add(1, Ordering::Relaxed);
                        let shared = Arc::clone(&server_shared);
                        let scope = server_scope.clone();
                        thread::spawn(move || serve_connection(stream, shared, &scope));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            root,
            project,
            host_state,
            app_socket,
            state: shared,
            appserver_connections: connections,
            initialized: false,
            _server_thread: server_thread,
        }
    }

    fn command_public(&self, args: &[&str], thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI")
    }

    fn command_without_thread(&self, args: &[&str], session: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", session)
            .env_remove("CODEX_THREAD_ID")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI with a continuously missing thread fact")
    }

    fn command_desktop(&self, args: &[&str], thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run Desktop collab CLI")
    }

    fn command_tui_without_explicit_endpoint(
        &self,
        args: &[&str],
        thread: &str,
    ) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run TUI collab CLI without an explicit endpoint")
    }

    /// No automatically observed native facts: no AppServer socket, no
    /// session/thread, no tmux, and an originator the runtime cannot map to a
    /// namespace. The caller must supply every fact through `--provide`.
    fn command_without_native_facts(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex future host")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI without native facts")
    }

    fn run_public(&self, args: &[&str], thread: &str) -> Value {
        let output = self.command_public(args, thread);
        assert!(
            output.status.success(),
            "collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
    }

    fn run_ok_desktop(&self, args: &[&str], thread: &str) -> Value {
        let output = self.command_desktop(args, thread);
        assert!(
            output.status.success(),
            "Desktop collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
    }

    /// The daemon owns the worker id. Fixtures must read it back from the
    /// registration receipt rather than assume a `COLLAB_WORKER`-derived name.
    fn worker_id(&self, thread: &str) -> String {
        let context = self.run_public(&["context"], thread);
        context["identity"]["worker_id"]
            .as_str()
            .expect("context receipt names the daemon-owned worker")
            .to_owned()
    }
}

impl Drop for AppFixture {
    fn drop(&mut self) {
        if self.initialized {
            let _ = Command::new(binary())
                .arg("down")
                .current_dir(&self.project)
                .env("COLLAB_STATE_DIR", &self.host_state)
                .output();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && self.host_state.join("server.sock").exists() {
                thread::sleep(Duration::from_millis(50));
            }
        }
        if std::thread::panicking() {
            eprintln!(
                "preserving failed app e2e fixture for diagnosis: root={}",
                self.root.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[derive(Clone, Debug)]
struct ThreadState {
    status: String,
    active_turn: Option<String>,
    turns: Vec<Value>,
    tool_namespaces: Vec<Option<String>>,
    seq: u64,
    queued: u64,
}

impl ThreadState {
    fn new() -> Self {
        Self {
            status: "idle".into(),
            active_turn: None,
            turns: Vec::new(),
            tool_namespaces: Vec::new(),
            seq: 0,
            queued: 0,
        }
    }

    fn next_turn(&mut self) -> String {
        self.seq += 1;
        format!("turn-{}", self.seq)
    }
}

fn serve_connection(
    mut stream: UnixStream,
    state: Arc<Mutex<HashMap<String, ThreadState>>>,
    project_root: &str,
) {
    if !handshake(&mut stream) {
        return;
    }
    loop {
        let payload = match read_client_frame(&mut stream) {
            Ok(payload) => payload,
            Err(_) => break,
        };
        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let response = dispatch(method, params, id, &state, project_root);
        if write_frame(&mut stream, &response).is_err() {
            break;
        }
    }
}

fn dispatch(
    method: &str,
    params: Value,
    id: Value,
    state: &Mutex<HashMap<String, ThreadState>>,
    project_root: &str,
) -> Value {
    match method {
        "initialize" => json!({"id": id, "result": {}}),
        "thread/loaded/list" => json!({"id": id, "result": {"data": []}}),
        "thread/resume" => {
            let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
            json!({"id": id, "result": {"thread": {"id": thread_id}}})
        }
        "thread/read" => thread_read(params, id, state, project_root),
        "thread/items/list" => {
            json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
        }
        "thread/turns/list" => thread_turns(params, id, state),
        "turn/start" => turn_start(params, id, state),
        "turn/steer" => turn_steer(params, id, state),
        "thread/queue/add" => queue_add(params, id, state),
        "thread/archive" => json!({"id": id, "result": {"archived": true}}),
        _ => json!({"id": id, "error": {"code": -32601, "message": "unsupported"}}),
    }
}

fn thread_read(
    params: Value,
    id: Value,
    state: &Mutex<HashMap<String, ThreadState>>,
    project_root: &str,
) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked
        .entry(thread_id.clone())
        .or_insert_with(ThreadState::new);
    json!({
        "id": id,
        "result": {
            "thread": {
                "id": thread_id,
                "sessionId": thread_id.clone(),
                "cwd": project_root,
                "status": {"type": entry.status}
            }
        }
    })
}

fn thread_turns(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let data = state
        .lock()
        .unwrap()
        .get(&thread_id)
        .cloned()
        .unwrap_or_else(ThreadState::new)
        .turns;
    json!({"id": id, "result": {"data": data}})
}

fn turn_start(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked
        .entry(thread_id.clone())
        .or_insert_with(ThreadState::new);
    entry.tool_namespaces.push(
        params["toolOutput"]["namespace"]
            .as_str()
            .map(str::to_owned),
    );
    let turn_id = entry.next_turn();
    entry.status = "active".into();
    entry.active_turn = Some(turn_id.clone());
    entry
        .turns
        .push(json!({"id": turn_id, "status": "inProgress"}));
    json!({
        "id": id,
        "result": {"turn": {"id": turn_id, "status": "inProgress"}}
    })
}

fn turn_steer(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let expected = params["expectedTurnId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked.entry(thread_id).or_insert_with(ThreadState::new);
    if entry.active_turn.as_deref() != Some(expected.as_str()) {
        return json!({
            "id": id,
            "error": {"code": -32602, "message": "turn identity mismatch"}
        });
    }
    json!({"id": id, "result": {"turnId": expected}})
}

fn queue_add(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked.entry(thread_id).or_insert_with(ThreadState::new);
    entry.queued += 1;
    json!({
        "id": id,
        "result": {"queuedSubmission": {"id": format!("qs-{}", entry.queued)}}
    })
}

fn handshake(stream: &mut UnixStream) -> bool {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match stream.read_exact(&mut byte) {
            Ok(()) => {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return false,
        }
    }
    stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .is_ok()
}

fn read_client_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7f) as usize;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes)?;
        length = u16::from_be_bytes(bytes) as usize;
    }
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(payload)
}

fn write_frame(stream: &mut UnixStream, payload: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(payload).expect("json encode");
    let mut frame = Vec::with_capacity(bytes.len() + 4);
    frame.push(0x81);
    match bytes.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        _ => unimplemented!("large test frames"),
    }
    frame.extend_from_slice(&bytes);
    stream.write_all(&frame)
}

#[test]
fn two_tui_appserver_receipt_flow() {
    let fixture = AppFixture::new();

    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker B")
        .to_owned();
    let mut app_fixture = fixture;
    app_fixture.initialized = true;

    let context_a = app_fixture.run_public(&["context"], THREAD_A);
    assert_eq!(context_a["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_a["liveness"]["presence"], "present");
    let context_b = app_fixture.run_public(&["context"], THREAD_B);
    assert_eq!(context_b["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_b["liveness"]["presence"], "present");
    assert_eq!(context_a["identity"]["worker_id"], worker_a);
    assert_eq!(context_b["identity"]["worker_id"], worker_b);

    let sent = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver immediate",
            "immediate body",
        ],
        THREAD_A,
    );
    let immediate_id = sent["msg_id"].as_str().unwrap().to_string();
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);

    let steered = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver steer",
            "steer body",
        ],
        THREAD_A,
    );
    let steered_id = steered["msg_id"].as_str().unwrap().to_string();
    assert_eq!(steered["notification"], "appserver-input-submitted");
    assert_eq!(steered["consumed"], false);

    let queued = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver queued",
            "queued body",
        ],
        THREAD_A,
    );
    let queued_id = queued["msg_id"].as_str().unwrap().to_string();
    assert_eq!(queued["notification"], "appserver-input-submitted");
    assert_eq!(queued["consumed"], false);

    let received = app_fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "appserver-receive-1",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 3);
    let ids = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(ids.contains(&immediate_id));
    assert!(ids.contains(&steered_id));
    assert!(ids.contains(&queued_id));
    assert_eq!(received["receive_id"], "appserver-receive-1");

    let msg_state = app_fixture.run_public(&["msg", &immediate_id], THREAD_A);
    assert_eq!(msg_state["state"], "read");

    let _down = app_fixture.command_public(&["down"], THREAD_B);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && app_fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    let _up = app_fixture.command_public(&["up"], THREAD_B);
    let replay_context = app_fixture.run_public(&["context"], THREAD_B);
    assert_eq!(replay_context["inbox"]["unread"], 0);
    assert_eq!(replay_context["identity"]["transport"]["kind"], "appserver");
    assert_eq!(replay_context["identity"]["worker_id"], worker_b);
    let replayed_msg = app_fixture.run_public(&["msg", &immediate_id], THREAD_A);
    assert_eq!(replayed_msg["state"], "read");

    drop(app_fixture);
}

#[test]
fn desktop_managed_socket_registration_persists_and_uses_codex_app_namespace() {
    let mut fixture = AppFixture::new_desktop_managed();

    let context_a = fixture.run_ok_desktop(&["context"], THREAD_A);
    assert_eq!(context_a["identity"]["transport"]["namespace"], "codex_app");
    assert_eq!(
        context_a["identity"]["transport"]["endpoint"],
        format!("unix://{}", fixture.app_socket.display())
    );
    let context_b = fixture.run_ok_desktop(&["context"], THREAD_B);
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names Desktop worker B")
        .to_owned();
    assert_eq!(context_b["identity"]["transport"]["namespace"], "codex_app");
    fixture.initialized = true;

    let context_b = fixture.run_ok_desktop(&["context"], THREAD_B);
    assert_eq!(context_b["identity"]["transport"]["namespace"], "codex_app");
    assert_eq!(
        context_b["identity"]["transport"]["endpoint"],
        format!("unix://{}", fixture.app_socket.display())
    );

    let sent = fixture.run_ok_desktop(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "desktop namespace",
            "desktop body",
        ],
        THREAD_A,
    );
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);
    assert_eq!(
        fixture.state.lock().unwrap()[THREAD_B]
            .tool_namespaces
            .last()
            .and_then(Option::as_deref),
        Some("codex_app")
    );

    let received = fixture.run_ok_desktop(
        &[
            "recv",
            "--receive-id",
            "desktop-receive-1",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);

    let down = fixture.command_desktop(&["down"], THREAD_B);
    assert!(down.status.success());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    let up = fixture.command_desktop(&["up"], THREAD_B);
    assert!(up.status.success());
    let replay_context = fixture.run_ok_desktop(&["context"], THREAD_B);
    assert_eq!(
        replay_context["identity"]["transport"]["namespace"],
        "codex_app"
    );
}

#[test]
fn tui_does_not_borrow_the_desktop_managed_socket() {
    let mut fixture = AppFixture::new_desktop_managed();
    fixture.initialized = true;

    let output = fixture.command_tui_without_explicit_endpoint(
        &["init"],
        THREAD_A,
    );
    assert!(
        !output.status.success(),
        "TUI init must not synthesize a managed Desktop endpoint: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.appserver_connections.load(Ordering::Relaxed),
        0,
        "TUI must not connect to the Desktop managed AppServer socket"
    );
}

#[test]
fn wrong_appserver_endpoint_fails_closed() {
    let root = unique_root();
    let project = root.join("project");
    let host_state = root.join("h");
    let missing = root.join("missing.sock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&host_state).unwrap();
    let output = Command::new(binary())
        .arg("init")
        .current_dir(&project)
        .env("COLLAB_STATE_DIR", &host_state)
        .env("COLLAB_APPSERVER_SOCKET", &missing)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", THREAD_A)
        .env("CODEX_THREAD_ID", THREAD_A)
        .env("CODEX_HOME", root.join("home"))
        .output()
        .expect("run collab init");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(
        combined.contains("APPSERVER_ENDPOINT_REJECTED") || combined.contains("endpoint"),
        "{combined}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// B02/B11: an explicit approved promotion replaces the recorded holder even
/// when the incumbent is unreachable, the replaced holder loses control, and
/// the AppServer communication path keeps working for the new holder.
#[test]
fn master_authority_appserver_explicit_replacement_keeps_holder_control() {
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names TUI worker B")
        .to_owned();
    let project_scope = context_a["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = context_a["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();
    fixture.initialized = true;

    let promoted_a = fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user approved TUI worker A",
        ],
        THREAD_A,
    );
    assert_eq!(promoted_a["master"], worker_a);
    assert_eq!(promoted_a["scope"]["project_scope"], project_scope);
    assert_eq!(promoted_a["scope"]["app_scope_id"], app_scope_id);

    let promoted_b = fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user approved TUI worker B",
        ],
        THREAD_B,
    );
    assert_eq!(promoted_b["master"], worker_b);
    assert_eq!(promoted_b["scope"]["project_scope"], project_scope);
    assert_eq!(promoted_b["scope"]["app_scope_id"], app_scope_id);

    let status = fixture.run_public(&["master", "status"], THREAD_B);
    assert_eq!(status["master"]["worker_id"], worker_b);
    assert_eq!(status["scope"]["project_scope"], project_scope);
    assert_eq!(status["scope"]["app_scope_id"], app_scope_id);

    // The replaced holder no longer controls the board; the current holder does.
    let rejected = fixture.command_public(
        &[
            "board",
            "publish",
            "a-after-replacement",
            "--title",
            "t",
            "--description",
            "d",
            "--delivery-condition",
            "c",
            "--test-condition",
            "t",
            "--priority",
            "p2",
        ],
        THREAD_A,
    );
    assert!(
        !rejected.status.success(),
        "the replaced holder must not keep board control: stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    let published = fixture.run_public(
        &[
            "board",
            "publish",
            "b-after-replacement",
            "--title",
            "t",
            "--description",
            "d",
            "--delivery-condition",
            "c",
            "--test-condition",
            "t",
            "--priority",
            "p2",
        ],
        THREAD_B,
    );
    assert_eq!(published["task"]["id"], "b-after-replacement");

    // The replaced holder keeps its own peer row, task and mailbox while it
    // loses only control authority.
    fixture.run_public(
        &[
            "task",
            "register",
            "a-preserved-task",
            "--next",
            "survive replacement",
        ],
        THREAD_A,
    );
    let preserved = fixture.run_public(
        &[
            "send",
            "--to",
            &worker_a,
            "--subject",
            "preserve replaced holder",
            "keep the mailbox",
        ],
        THREAD_B,
    );
    let preserved_message = preserved["msg_id"]
        .as_str()
        .or_else(|| preserved["message_id"].as_str())
        .expect("send names the durable message")
        .to_owned();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        context_a["identity"]["worker_id"], worker_a,
        "replacement must preserve the replaced holder's peer identity: {context_a}"
    );
    assert!(
        context_a["tasks"]
            .as_array()
            .expect("context lists tasks")
            .iter()
            .any(|task| task["id"] == "a-preserved-task"),
        "replacement must preserve the replaced holder's task: {context_a}"
    );
    assert!(
        context_a["inbox"]["messages"]
            .as_array()
            .expect("context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(preserved_message.as_str())),
        "replacement must preserve the replaced holder's mailbox: {context_a}"
    );

    // The AppServer delivery path still works after the authority change.
    let sent = fixture.run_public(
        &[
            "send",
            "--to",
            &worker_a,
            "--subject",
            "after replacement",
            "deliver to the replaced peer",
        ],
        THREAD_B,
    );
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);

    drop(fixture);
}

#[test]
fn repeated_appserver_context_reuses_the_same_identity_and_binding() {
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    assert!(
        context_a["identity"]["worker_id"].as_str().is_some(),
        "daemon receipt must name the AppServer worker"
    );
    fixture.initialized = true;
    let repeated = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(repeated["identity"]["worker_id"], context_a["identity"]["worker_id"]);
    assert_eq!(repeated["binding"], context_a["binding"]);
    assert_eq!(fixture.run_public(&["who"], THREAD_A)["workers"].as_array().unwrap().len(), 1);
    drop(fixture);
}

#[test]
fn context_partial_native_facts_require_the_missing_endpoint_fields() {
    let mut fixture = AppFixture::new();

    // No explicit AppServer endpoint: the TUI originator still yields a
    // namespace, but the native socket is absent, so the endpoint is the one
    // fact the caller must supply. The already-observed facts are not
    // re-requested.
    let partial = fixture.command_tui_without_explicit_endpoint(&["context"], THREAD_A);
    assert!(
        partial.status.success(),
        "partial native facts are a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&partial.stdout),
        String::from_utf8_lossy(&partial.stderr)
    );
    let partial: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(partial["registered"], false);
    let update = &partial["requires_identity_update"];
    assert_eq!(update["reason"], "IDENTITY_INFORMATION_REQUIRED");
    assert_eq!(
        update["required_fields"],
        json!(["endpoint"]),
        "observed session/thread facts must not be re-requested: {partial}"
    );
    assert!(update["worker_id"].is_null());

    // Supplying a conflicting session/thread must be rejected before the daemon
    // creates any identity: a supplement may fill gaps, never override the
    // caller's real anchor.
    let invalid = fixture.command_tui_without_explicit_endpoint(
        &[
            "context",
            "--provide",
            "{\"session_id\":\"different\",\"thread_id\":\"different\"}",
        ],
        THREAD_A,
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    assert!(!invalid.status.success(), "{combined}");
    assert!(
        combined.contains("IDENTITY_FACT_CONFLICT"),
        "supplied facts must not override the observed caller anchor: {combined}"
    );
    let identities = fixture.host_state.join("identities");
    let created = identities.exists()
        && std::fs::read_dir(&identities)
            .expect("read isolated identities dir")
            .next()
            .is_some();
    assert!(
        !created,
        "a conflicting supplement must not create an identity: {identities:?}"
    );
    fixture.initialized = true;
    drop(fixture);
}

/// One complete supplement, supplied through the real CLI to the real daemon,
/// must finish the whole bootstrap: identity, runtime binding and default
/// lease. A plain replay of the same actual anchor must then return the same
/// worker and generation with no second identity.
#[test]
fn one_full_supplement_establishes_and_then_replays_the_same_identity() {
    let mut fixture = AppFixture::new();

    let missing = fixture.command_without_native_facts(&["context"]);
    assert!(
        missing.status.success(),
        "no-anchor context is a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&missing.stdout),
        String::from_utf8_lossy(&missing.stderr)
    );
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(
        missing["requires_identity_update"]["required_fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"])
    );

    let endpoint = format!("unix://{}", fixture.app_socket.display());
    let supplement = format!(
        "{{\"session_id\":\"{THREAD_A}\",\"thread_id\":\"{THREAD_A}\",\"endpoint\":\"{endpoint}\",\"namespace\":\"codex_tui\"}}"
    );
    let registered = fixture.command_without_native_facts(&[
        "context",
        "--provide",
        &supplement,
    ]);
    assert_context_display_is_public("full-supplement context", &registered);
    assert!(
        registered.status.success(),
        "a complete real supplement must register: stdout={} stderr={}",
        String::from_utf8_lossy(&registered.stdout),
        String::from_utf8_lossy(&registered.stderr)
    );
    let registered: Value = serde_json::from_slice(&registered.stdout).unwrap();
    assert_eq!(
        registered["registered"], true,
        "complete supplement must register: {registered}"
    );
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .expect("registered receipt names the daemon-owned worker")
        .to_owned();
    assert_eq!(
        registered["identity"]["transport"]["kind"], "appserver",
        "the verified endpoint must be the selected transport: {registered}"
    );
    assert_eq!(
        registered["identity"]["transport"]["namespace"], "codex_tui"
    );
    let generation = registered["binding"]["endpoint_generation"].clone();
    let binding = registered["binding"].clone();
    assert!(
        registered["subscriptions"]
            .as_array()
            .expect("context lists subscriptions")
            .iter()
            .any(|subscription| subscription["event"] == "direct-message"
                && subscription["status"] == "armed"),
        "the default direct-message lease must be armed after registration: {registered}"
    );

    // The same real native anchor replayed through the automatic path returns
    // the same worker and generation, so the supplement is idempotent.
    let replay = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(replay["identity"]["worker_id"], worker);
    assert_eq!(replay["binding"]["endpoint_generation"], generation);
    assert_eq!(replay["binding"], binding);

    fixture.initialized = true;
    drop(fixture);
}

#[test]
fn one_thread_supplement_survives_continuously_missing_environment() {
    let mut fixture = AppFixture::new();
    let missing = fixture.command_without_thread(&["context"], THREAD_A);
    assert!(missing.status.success());
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["requires_identity_update"]["required_fields"], json!(["thread_id"]));
    let supplement = format!("{{\"thread_id\":\"{THREAD_A}\"}}");
    let registered = fixture.command_without_thread(&["context", "--provide", &supplement], THREAD_A);
    assert_context_display_is_public("thread supplement", &registered);
    assert!(registered.status.success(), "{}", String::from_utf8_lossy(&registered.stderr));
    let registered: Value = serde_json::from_slice(&registered.stdout).unwrap();
    assert_eq!(registered["registered"], true);
    for args in [vec!["context"], vec!["task", "status"]] {
        let output = fixture.command_without_thread(&args, THREAD_A);
        assert!(output.status.success(), "{args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if args == ["context"] {
            assert_eq!(value["registered"], true);
            assert_eq!(value["identity"], registered["identity"]);
            assert_eq!(value["binding"], registered["binding"]);
        } else {
            assert_eq!(value["tasks"], json!([]));
        }
    }
    let anonymous = fixture.command_without_native_facts(&["context"]);
    assert!(anonymous.status.success());
    let anonymous: Value = serde_json::from_slice(&anonymous.stdout).unwrap();
    assert_eq!(anonymous["registered"], false);
    assert_eq!(anonymous["requires_identity_update"]["required_fields"], json!(["session_id", "thread_id", "endpoint", "namespace"]));
    fixture.initialized = true;
    drop(fixture);
}

#[test]
fn external_linked_worktree_reuses_the_canonical_identity_without_recovery_calls() {
    let mut fixture = AppFixture::new();
    let canonical = fixture.project.clone();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["-c", "user.name=Collab Test", "-c", "user.email=collab-test@example.invalid", "commit", "--allow-empty", "-q", "-m", "initial"],
    ] {
        assert!(Command::new("git").args(args).current_dir(&canonical).status().unwrap().success());
    }
    let context = fixture.run_public(&["context"], THREAD_A);
    fixture.initialized = true;
    let linked = fixture.root.join("external-linked");
    assert!(Command::new("git").args(["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]).current_dir(&canonical).status().unwrap().success());
    fixture.project = linked;
    let replay = fixture.command_public(&["context"], THREAD_A);
    let tasks = fixture.command_without_thread(&["task", "status"], THREAD_A);
    let worktree_baseline = fixture.project.join(".agent-collab").exists();
    // Restore the daemon's lifecycle cwd before assertions so a failing
    // regression still stops its own isolated daemon in Fixture::drop.
    fixture.project = canonical;
    assert!(replay.status.success(), "{}{}", String::from_utf8_lossy(&replay.stdout), String::from_utf8_lossy(&replay.stderr));
    let replay: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["project_root"], context["project_root"]);
    assert_eq!(replay["identity"], context["identity"]);
    assert_eq!(replay["binding"], context["binding"]);
    assert!(tasks.status.success(), "{}{}", String::from_utf8_lossy(&tasks.stdout), String::from_utf8_lossy(&tasks.stderr));
    let tasks: Value = serde_json::from_slice(&tasks.stdout).unwrap();
    assert_eq!(tasks["tasks"], json!([]));
    assert!(!worktree_baseline, "a worktree must not create a second identity baseline");
    drop(fixture);
}

#[test]
fn forged_appserver_endpoint_and_missing_project_context_fail_closed() {
    let root = unique_root();
    let project = root.join("project");
    let host_state = root.join("h");
    let forged = root.join("forged.sock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&project);

    let output = Command::new(binary())
        .args([
            "context",
            "--provide",
            &format!(
                "{{\"session_id\":\"{THREAD_A}\",\"thread_id\":\"{THREAD_A}\",\"endpoint\":\"unix://{}\",\"namespace\":\"codex_tui\"}}",
                forged.display()
            ),
        ])
        .current_dir(&project)
        .env("COLLAB_STATE_DIR", &host_state)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_HOME", root.join("home"))
        .env_remove("COLLAB_APPSERVER_SOCKET")
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_WORKER")
        .output()
        .expect("run context with forged endpoint");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(
        combined.contains("APPSERVER_ENDPOINT_REJECTED")
            || combined.contains("endpoint")
            || combined.contains("RouteUnavailable"),
        "{combined}"
    );
    assert!(
        !host_state.join("identities").exists(),
        "forged endpoint must not mint an identity"
    );

    // A request envelope that omits ProjectContext is rejected by the daemon
    // before identity selection. This mirrors the public wire contract without
    // reaching into daemon state.
    let server_socket = host_state.join("server.sock");
    if server_socket.exists() {
        let mut stream = UnixStream::connect(&server_socket).expect("connect isolated daemon");
        stream
            .write_all(b"{\"op\":\"IdentityContext\",\"facts\":{}}\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(
            line.contains("PROJECT_CONTEXT_REQUIRED"),
            "missing ProjectContext must fail explicitly: {line}"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
