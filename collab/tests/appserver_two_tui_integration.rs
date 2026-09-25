use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_collab"))
}

struct AppFixture {
    root: PathBuf,
    project: PathBuf,
    host_state: PathBuf,
    app_socket: PathBuf,
    initialized: bool,
    _server_thread: thread::JoinHandle<()>,
}

impl AppFixture {
    fn new() -> Self {
        let root = unique_root();
        let project = root.join("project");
        let host_state = root.join("h");
        let app_socket = root.join("app.sock");
        std::fs::create_dir_all(&project).expect("create project root");
        std::fs::create_dir_all(&host_state).expect("create host state root");
        let project_str = std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let listener = UnixListener::bind(&app_socket).expect("bind app server fixture");
        let shared = Arc::new(Mutex::new(HashMap::<String, ThreadState>::new()));
        let server_shared = Arc::clone(&shared);
        let server_scope = project_str.clone();
        let server_thread = thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
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
            initialized: false,
            _server_thread: server_thread,
        }
    }

    fn command(&self, args: &[&str], thread: &str, worker: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env("COLLAB_WORKER", worker)
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI")
    }

    fn run_ok(&self, args: &[&str], thread: &str, worker: &str) -> Value {
        let output = self.command(args, thread, worker);
        assert!(
            output.status.success(),
            "collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
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
    seq: u64,
    queued: u64,
}

impl ThreadState {
    fn new() -> Self {
        Self {
            status: "idle".into(),
            active_turn: None,
            turns: Vec::new(),
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

    fixture.run_ok(&["init"], THREAD_A, "codex-a");
    fixture.run_ok(&["init"], THREAD_B, "codex-b");
    let mut app_fixture = fixture;
    app_fixture.initialized = true;

    let context_a = app_fixture.run_ok(&["context", "--worker", "codex-a"], THREAD_A, "codex-a");
    assert_eq!(context_a["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_a["liveness"]["presence"], "present");
    let context_b = app_fixture.run_ok(&["context", "--worker", "codex-b"], THREAD_B, "codex-b");
    assert_eq!(context_b["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_b["liveness"]["presence"], "present");

    let sent = app_fixture.run_ok(
        &[
            "send",
            "--from",
            "codex-a",
            "--to",
            "codex-b",
            "--subject",
            "appserver immediate",
            "immediate body",
        ],
        THREAD_A,
        "codex-a",
    );
    let immediate_id = sent["msg_id"].as_str().unwrap().to_string();
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);

    let steered = app_fixture.run_ok(
        &[
            "send",
            "--from",
            "codex-a",
            "--to",
            "codex-b",
            "--subject",
            "appserver steer",
            "steer body",
        ],
        THREAD_A,
        "codex-a",
    );
    let steered_id = steered["msg_id"].as_str().unwrap().to_string();
    assert_eq!(steered["notification"], "appserver-input-submitted");
    assert_eq!(steered["consumed"], false);

    let queued = app_fixture.run_ok(
        &[
            "send",
            "--from",
            "codex-a",
            "--to",
            "codex-b",
            "--subject",
            "appserver queued",
            "queued body",
        ],
        THREAD_A,
        "codex-a",
    );
    let queued_id = queued["msg_id"].as_str().unwrap().to_string();
    assert_eq!(queued["notification"], "appserver-input-submitted");
    assert_eq!(queued["consumed"], false);

    let received = app_fixture.run_ok(
        &[
            "recv",
            "--worker",
            "codex-b",
            "--receive-id",
            "appserver-receive-1",
            "--timeout",
            "0",
        ],
        THREAD_B,
        "codex-b",
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

    let msg_state = app_fixture.run_ok(&["msg", &immediate_id], THREAD_A, "codex-a");
    assert_eq!(msg_state["state"], "read");

    let _down = app_fixture.command(&["down"], THREAD_B, "codex-b");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && app_fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    let _up = app_fixture.command(&["up"], THREAD_B, "codex-b");
    let replay_context =
        app_fixture.run_ok(&["context", "--worker", "codex-b"], THREAD_B, "codex-b");
    assert_eq!(replay_context["inbox"]["unread"], 0);
    assert_eq!(replay_context["identity"]["transport"]["kind"], "appserver");
    let replayed_msg = app_fixture.run_ok(&["msg", &immediate_id], THREAD_A, "codex-a");
    assert_eq!(replayed_msg["state"], "read");

    drop(app_fixture);
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

#[test]
fn appserver_thread_identity_cannot_be_claimed_twice() {
    let mut fixture = AppFixture::new();
    fixture.run_ok(&["init"], THREAD_A, "codex-a");
    fixture.initialized = true;
    let conflict = fixture.command(&["init"], THREAD_A, "codex-a2");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&conflict.stdout),
        String::from_utf8_lossy(&conflict.stderr)
    );
    assert!(!conflict.status.success(), "{combined}");
    assert!(
        combined.contains("CONFLICT")
            || combined.contains("already")
            || combined.contains("session-bound"),
        "{combined}"
    );
    drop(fixture);
}
