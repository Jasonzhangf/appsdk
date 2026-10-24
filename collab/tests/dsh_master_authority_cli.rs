use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

mod native_support {
    #![allow(dead_code)]
    include!("support/context_operation_fixture.rs");
}

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "d-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn collab_binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnrollMode {
    Accept,
    Reject,
    MismatchBinding,
}

struct Gateway {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    enrolled: Arc<Mutex<HashSet<(String, String)>>>,
    register_peers: Arc<Mutex<Vec<Value>>>,
    enroll_mode: Arc<Mutex<EnrollMode>>,
    current_runtime: Arc<Mutex<Option<String>>>,
}

impl Gateway {
    fn start(socket: PathBuf, root: PathBuf, agents: Vec<String>) -> Self {
        Self::start_with_enrollment_gate(socket, root, agents, false)
    }

    fn start_requiring_enrollment(socket: PathBuf, root: PathBuf, agents: Vec<String>) -> Self {
        Self::start_with_enrollment_gate(socket, root, agents, true)
    }

    fn start_with_enrollment_gate(
        socket: PathBuf,
        root: PathBuf,
        agents: Vec<String>,
        require_enrollment: bool,
    ) -> Self {
        let listener = UnixListener::bind(&socket).expect("bind gateway socket");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let enrolled = Arc::new(Mutex::new(HashSet::new()));
        let register_peers = Arc::new(Mutex::new(Vec::new()));
        let enroll_mode = Arc::new(Mutex::new(EnrollMode::Accept));
        let thread_enrolled = Arc::clone(&enrolled);
        let thread_requests = Arc::clone(&register_peers);
        let thread_mode = Arc::clone(&enroll_mode);
        let current_runtime = Arc::new(Mutex::new(None::<String>));
        let thread_runtime = current_runtime.clone();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: Value = serde_json::from_str(line.trim()).expect("gateway JSON");
                let method = request["method"].as_str().unwrap_or_default();
                let runtime_id = request["params"]["runtimeId"].as_str().unwrap_or_default();
                let agent_id = request["params"]["agentId"].as_str().unwrap_or_default();
                let known = agents.iter().any(|agent| agent == agent_id);
                let key = (runtime_id.to_owned(), agent_id.to_owned());
                let current = thread_runtime.lock().unwrap().clone();
                let reply = if method == "resolve-agent" && known {
                    json!({"ok":true,"result":{
                        "nonce":request["params"]["nonce"],
                        "runtimeId":current.as_deref().unwrap_or("gateway-runtime"),
                        "agentId":agent_id,"sessionId":agent_id,"cwd":root,"status":"running"
                    }})
                } else if current
                    .as_deref()
                    .is_some_and(|current| current != runtime_id)
                {
                    json!({"ok":false,"error":{"code":"unknown-runtime","message":"old runtime disconnected"}})
                } else if method == "agent-facts" && known {
                    json!({"ok":true,"result":{
                        "nonce":request["params"]["nonce"],
                        "runtimeId":runtime_id,
                        "agentId":agent_id,
                        "sessionId":agent_id,
                        "cwd":root,
                        "status":"running"
                    }})
                } else if method == "register-peer" && known {
                    thread_requests.lock().unwrap().push(request.clone());
                    let mode = *thread_mode.lock().unwrap();
                    let binding = &request["params"]["binding"];
                    let runtime_context = &binding["runtimeContext"];
                    if !binding.is_object() || !runtime_context.is_object() {
                        json!({"ok":false,"error":{
                            "code":"invalid-params",
                            "message":"register-peer requires adopted runtimeContext"
                        }})
                    } else if mode == EnrollMode::Reject {
                        json!({"ok":false,"error":{
                            "code":"enrollment-rejected",
                            "message":"fixture rejected receive-loop enrollment"
                        }})
                    } else {
                        let binding_id = if mode == EnrollMode::MismatchBinding {
                            json!("mismatched-binding-id")
                        } else {
                            thread_enrolled.lock().unwrap().insert(key.clone());
                            runtime_context["binding_id"].clone()
                        };
                        json!({"ok":true,"result":{
                            "runtimeId":request["params"]["runtimeId"],
                            "agentId":request["params"]["agentId"],
                            "workerId":binding["workerId"],
                            "projectScope":binding["projectScope"],
                            "bindingId":binding_id,
                            "endpointGeneration":runtime_context["endpoint_generation"]
                        }})
                    }
                } else if method == "enqueue" && known {
                    if require_enrollment && !thread_enrolled.lock().unwrap().contains(&key) {
                        json!({"ok":false,"error":{
                            "code":"collab-not-registered",
                            "message":"fixture has no canonical receive loop for agent"
                        }})
                    } else {
                        json!({"ok":true,"result":{
                            "messageId":request["params"]["messageId"],
                            "runtimeId":runtime_id,
                            "agentId":agent_id
                        }})
                    }
                } else {
                    json!({"ok":false,"error":{"code":"unknown-agent","message":"fixture unknown agent"}})
                };
                let mut stream = &stream;
                let _ = writeln!(stream, "{reply}");
                let _ = stream.flush();
            }
        });
        Self {
            socket,
            stop,
            handle: Some(handle),
            enrolled,
            register_peers,
            enroll_mode,
            current_runtime,
        }
    }

    fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    fn register_peer_requests(&self) -> Vec<Value> {
        self.register_peers.lock().unwrap().clone()
    }

    fn set_enroll_mode(&self, mode: EnrollMode) {
        *self.enroll_mode.lock().unwrap() = mode;
    }

    fn is_enrolled(&self, runtime_id: &str, agent_id: &str) -> bool {
        self.enrolled
            .lock()
            .unwrap()
            .contains(&(runtime_id.to_owned(), agent_id.to_owned()))
    }

    /// Simulate a gateway restart whose local enrollment cache is empty while
    /// the control socket stays reachable.
    fn clear_enrollment(&self) {
        self.enrolled.lock().unwrap().clear();
    }

    fn wait_enrolled(&self, runtime_id: &str, agent_id: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_enrolled(runtime_id, agent_id) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.is_enrolled(runtime_id, agent_id)
    }

    fn shutdown(&mut self) -> thread::Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = UnixStream::connect(&self.socket);
            handle.join()
        } else {
            Ok(())
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            eprintln!("owned gateway thread failed: {}", self.socket.display());
        }
    }
}

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    binary: PathBuf,
    gateway: Gateway,
    daemon: Option<Child>,
    finished: bool,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_enrollment_gate(false)
    }

    fn new_requiring_enrollment() -> Self {
        Self::new_with_enrollment_gate(true)
    }

    fn new_with_enrollment_gate(require_enrollment: bool) -> Self {
        let root = unique_root();
        std::fs::create_dir_all(root.join("h")).expect("create host state");
        std::fs::create_dir_all(root.join(".agent-collab")).expect("create baseline marker");
        std::fs::create_dir_all(root.join("home")).expect("create isolated home");
        let root = root.canonicalize().expect("canonical project root");
        let gateway_dir = root.join("gateway");
        std::fs::create_dir_all(&gateway_dir).expect("create gateway directory");
        let socket = gateway_dir.join("control.sock");
        let agents = vec!["agentA".into(), "agentB".into()];
        let gateway = if require_enrollment {
            Gateway::start_requiring_enrollment(socket, root.clone(), agents)
        } else {
            Gateway::start(socket, root.clone(), agents)
        };
        Self {
            state: root.join("h"),
            root,
            binary: collab_binary(),
            gateway,
            daemon: None,
            finished: false,
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        self.configured_command(args)
            .output()
            .expect("run collab CLI")
    }

    fn configured_command(&self, args: &[&str]) -> Command {
        self.configured_program(&self.binary, args)
    }

    fn configured_program(&self, binary: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(binary);
        command
            .args(args)
            .current_dir(&self.root)
            .env("COLLAB_STATE_DIR", &self.state)
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_WORKER")
            .env_remove("DSH_SESSION_ID");
        command
    }

    fn request(&self, value: Value) -> Value {
        let mut stream =
            UnixStream::connect(self.state.join("server.sock")).expect("daemon socket");
        writeln!(stream, "{value}").expect("write daemon request");
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("read daemon reply");
        serde_json::from_str(&line).expect("daemon JSON reply")
    }

    fn dsh_command(&self, args: &[&str], session: &str) -> Output {
        self.configured_command(args)
            .env("DSH_SESSION_ID", session)
            .output()
            .unwrap()
    }

    fn registrations(&self) -> usize {
        fn count(path: &Path) -> usize {
            if path.is_dir() {
                return std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| count(&entry.unwrap().path()))
                    .sum();
            }
            if path.file_name().is_none_or(|name| name != "journal.jsonl") {
                return 0;
            }
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|event| event["ev"] == "Registered")
                .count()
        }
        count(&self.state)
    }

    fn wait_socket(&self, exists: bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if self.state.join("server.sock").exists() == exists {
                if exists {
                    // A bound socket path can precede listen readiness.
                    if let Ok(mut stream) = UnixStream::connect(self.state.join("server.sock")) {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(200)))
                            .unwrap();
                        if writeln!(stream, "{}", json!({"op":"Ping"})).is_ok() {
                            let mut line = String::new();
                            if BufReader::new(stream).read_line(&mut line).is_ok()
                                && serde_json::from_str::<Value>(&line)
                                    .is_ok_and(|reply| reply["ok"] == true)
                            {
                                return;
                            }
                        }
                    }
                } else {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "daemon socket did not reach exists={exists}: {}",
            self.state.display()
        );
    }

    fn stop_daemon(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = std::fs::remove_file(self.state.join("server.sock"));
        self.wait_socket(false);
    }

    fn start_daemon(&mut self) {
        let daemon_log = std::fs::File::create(self.root.join("test-daemon.log"))
            .expect("create owned serve log");
        self.daemon = Some(
            self.configured_command(&["serve"])
                .stdin(std::process::Stdio::null())
                .stdout(daemon_log.try_clone().expect("clone owned serve log"))
                .stderr(daemon_log)
                .spawn()
                .expect("spawn owned serve child"),
        );
        self.wait_socket(true);
    }

    fn restart_daemon(&mut self) {
        self.stop_daemon();
        self.start_daemon();
    }

    fn finish(mut self) {
        self.stop_daemon();
        self.gateway
            .shutdown()
            .expect("join gateway before removing its socket directory");
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
        if self.daemon.is_some() {
            self.stop_daemon();
        }
        if self.gateway.shutdown().is_err() {
            eprintln!("owned failed fixture gateway thread panicked");
        }
        eprintln!(
            "preserving failed DSH fixture root={} state={} gateway={}",
            self.root.display(),
            self.state.display(),
            self.gateway.socket.display()
        );
    }
}

fn register(fixture: &Fixture, worker: &str, token: &str, agent: &str) -> Value {
    fixture.request(json!({
        "op":"Register", "worker_id":worker, "token":token,
        "cwd":fixture.root,
        "candidates":{"dsh":{
            "endpoint":fixture.gateway.endpoint(), "runtime_id":"gateway-runtime",
            "agent_id":agent, "session_id":agent, "cwd":fixture.root
        }},
        "project_context":{"app_scope_id":"appserver-cli", "canonical_root":fixture.root,
            "project_scope":fixture.root}
    }))
}

fn binding_context(receipt: &Value, root: &Path) -> Value {
    let binding = &receipt["command"]["binding"];
    json!({
        "app_scope_id":binding["app_scope_id"], "canonical_root":root,
        "project_scope":binding["project_scope"],
        "runtime_context":{
            "agent_id":binding["agent_id"], "runtime_id":binding["runtime_id"],
            "appserver_id":binding["app_scope_id"],
            "endpoint_generation":binding["endpoint_generation"],
            "binding_id":binding["binding_id"], "session_id":binding["session_id"],
            "native_thread_id":binding["native_thread_id"]
        }
    })
}

fn authority(
    fixture: &Fixture,
    op: &str,
    worker: &str,
    token: &str,
    context: &Value,
    extra: Value,
) -> Value {
    let mut request = json!({"op":op,"worker_id":worker,"token":token,"project_context":context});
    if let (Some(dst), Some(src)) = (request.as_object_mut(), extra.as_object()) {
        dst.extend(src.clone());
    }
    fixture.request(request)
}

fn status(fixture: &Fixture, root: &Path) -> Value {
    fixture.request(json!({"op":"MasterStatus","project_context":{
        "app_scope_id":"appserver-cli","canonical_root":root,"project_scope":root
    }}))
}

fn assert_ok(reply: &Value) {
    assert_eq!(reply["ok"], true, "unexpected daemon refusal: {reply}");
}

fn rendered(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn only_identity(fixture: &Fixture) -> (String, Value) {
    let identities = fixture.state.join("identities");
    let entries = std::fs::read_dir(&identities)
        .unwrap_or_else(|error| panic!("read {}: {error}", identities.display()))
        .collect::<Result<Vec<_>, _>>()
        .expect("collect persisted identities");
    assert_eq!(entries.len(), 1, "expected one identity: {entries:?}");
    let worker = entries[0].file_name().to_string_lossy().into_owned();
    let path = entries[0].path().join("identity.json");
    let identity: Value = serde_json::from_slice(&std::fs::read(&path).expect("read identity"))
        .expect("identity JSON");
    (worker, identity)
}

fn assert_runtime_context(params: &Value, runtime_binding: &Value) {
    let context = &params["binding"]["runtimeContext"];
    assert!(
        context.is_object(),
        "register-peer binding must carry authoritative runtimeContext: {params}"
    );
    assert_eq!(context["agent_id"], runtime_binding["agent_id"]);
    assert_eq!(context["runtime_id"], runtime_binding["runtime_id"]);
    assert_eq!(context["appserver_id"], runtime_binding["appserver_id"]);
    assert_eq!(context["binding_id"], runtime_binding["binding_id"]);
    assert_eq!(
        context["endpoint_generation"],
        runtime_binding["endpoint_generation"]
    );
    assert_eq!(context["session_id"], runtime_binding["session_id"]);
    assert_eq!(
        context["native_thread_id"],
        runtime_binding["native_thread_id"]
    );
    assert_eq!(
        context["agent_id"], params["binding"]["workerId"],
        "runtimeContext agent_id is the committed collab worker id"
    );
    assert_ne!(
        context["runtime_id"], params["runtimeId"],
        "collab runtime id must stay distinct from the gateway runtime id"
    );
    assert_ne!(
        context["agent_id"], params["agentId"],
        "collab agent id must stay distinct from the gateway agent id"
    );
    assert_eq!(
        context["native_thread_id"], params["agentId"],
        "gateway agent addressing comes from native_thread_id"
    );
}

fn assert_scope(reply: &Value, root: &Path) {
    assert_eq!(
        reply["scope"]["project_scope"],
        root.to_string_lossy().as_ref(),
        "{reply}"
    );
    assert_eq!(reply["scope"]["app_scope_id"], "appserver-cli", "{reply}");
}

fn clear(fixture: &Fixture, worker: &str, token: &str, context: &Value, approval: &str) -> Value {
    authority(
        fixture,
        "MasterClear",
        worker,
        token,
        context,
        json!({"approval":approval}),
    )
}

fn must_refuse(reply: &Value) {
    assert_eq!(reply["ok"], false, "expected refusal: {reply}");
    assert!(
        reply["error"].is_string(),
        "missing explicit error: {reply}"
    );
}

#[test]
fn dsh_context_registers_from_typed_gateway_facts() {
    let mut fixture = Fixture::new();
    fixture.start_daemon();
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),
        "runtime_id":"gateway-runtime", "agent_id":"agentA",
        "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let output = fixture.command(&["context", "--provide", &facts]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("context JSON");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let snapshot = &response["result"]["snapshot"];
    assert_eq!(
        snapshot["identity"]["transport"]["kind"], "dsh",
        "{response}"
    );
    let worker = snapshot["identity"]["worker_id"].as_str().unwrap();
    let binding = snapshot["binding"].clone();
    assert!(binding["binding_id"].is_string(), "{response}");
    let registered = fixture.registrations();
    assert!(registered > 0, "context must commit a real registration");
    let saved_path = fixture
        .state
        .join("identities")
        .join(worker)
        .join("identity.json");
    let saved = std::fs::read(&saved_path).unwrap();
    let repeated = fixture.dsh_command(&["context", "--provide", &facts], "agentA");
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(
        fixture.registrations(),
        registered,
        "ordinary context must not Register again"
    );
    let leases = fixture.dsh_command(&["notify", "status"], "agentA");
    assert!(
        leases.status.success(),
        "{}",
        String::from_utf8_lossy(&leases.stderr)
    );
    assert!(
        String::from_utf8_lossy(&leases.stdout).contains("direct-message"),
        "{}",
        String::from_utf8_lossy(&leases.stdout)
    );
    let conflicting = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(), "runtime_id":"different-api",
        "agent_id":"agentA", "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let rejected = fixture.dsh_command(&["context", "--provide", &conflicting], "agentA");
    assert!(
        !rejected.status.success(),
        "explicit API conflict must fail: {}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert_eq!(
        std::fs::read(&saved_path).unwrap(),
        saved,
        "conflict must preserve the credential and binding"
    );
    assert_eq!(fixture.registrations(), registered);
    fixture.restart_daemon();
    let restored = fixture.dsh_command(&["context"], "agentA");
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["result"]["snapshot"]["binding"], binding);
    assert_eq!(
        fixture.registrations(),
        registered,
        "restart recovery must not Register"
    );
    let sent = fixture.dsh_command(
        &[
            "sendmessage",
            "--to",
            worker,
            "--subject",
            "dsh-context-message",
            "real registered message",
        ],
        "agentA",
    );
    assert!(
        sent.status.success(),
        "{} {}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr)
    );
    let sent: Value = serde_json::from_slice(&sent.stdout).unwrap();
    assert_eq!(sent["durable"], true, "{sent}");
    let received = fixture.dsh_command(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "dsh-context-receive",
        ],
        "agentA",
    );
    assert!(
        received.status.success(),
        "{}",
        String::from_utf8_lossy(&received.stderr)
    );
    let received: Value = serde_json::from_slice(&received.stdout).unwrap();
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["subject"] == "dsh-context-message"),
        "{received}"
    );
    let message_id = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "dsh-context-message")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let reply = fixture.dsh_command(
        &[
            "send",
            "--to",
            worker,
            "--subject",
            "dsh-context-reply",
            "--in-reply-to",
            message_id,
            "handled registered message",
        ],
        "agentA",
    );
    assert!(
        reply.status.success(),
        "{}",
        String::from_utf8_lossy(&reply.stderr)
    );
    let ack = fixture.dsh_command(&["ack", message_id], "agentA");
    assert!(
        ack.status.success(),
        "{}",
        String::from_utf8_lossy(&ack.stderr)
    );
    let status = fixture.dsh_command(&["msg", message_id], "agentA");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["answered"], true, "{status}");
    assert_eq!(status["consumed_by_recv"], true, "{status}");
    assert_eq!(status["state"], "read", "{status}");
    assert_eq!(
        status["wake_transport_evidence"]["queued"], true,
        "{status}"
    );
    assert_eq!(
        status["wake_transport_evidence"]["consumed"], false,
        "gateway acceptance alone is not consumption: {status}"
    );
    fixture.finish();
}

#[test]
fn dsh_context_first_registration_enrolls_gateway_receive_loop() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),
        "runtime_id":"gateway-runtime", "agent_id":"agentA",
        "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let output = fixture.command(&["context", "--provide", &facts]);
    let output_text = rendered(&output);
    assert!(output.status.success(), "{output_text}");
    assert!(
        !output_text.contains("\"token\""),
        "context must not echo credentials: {output_text}"
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("context JSON");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let worker = response["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("worker id")
        .to_owned();
    let (persisted_worker, identity) = only_identity(&fixture);
    assert_eq!(persisted_worker, worker);
    let runtime_binding = identity["runtime"].clone();
    let requests = fixture.gateway.register_peer_requests();
    assert_eq!(requests.len(), 1, "expected one enrollment");
    let params = &requests[0]["params"];
    assert_eq!(params["runtimeId"], "gateway-runtime");
    assert_eq!(params["agentId"], "agentA");
    assert_eq!(
        params["binding"]["workerId"].as_str(),
        Some(worker.as_str())
    );
    assert_eq!(
        params["binding"]["projectScope"],
        fixture.root.to_string_lossy().as_ref()
    );
    assert_runtime_context(params, &runtime_binding);
    assert!(
        params["binding"]["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "the selected gateway socket must receive the committed credential"
    );
    assert!(
        fixture.gateway.is_enrolled("gateway-runtime", "agentA"),
        "first registration must start the canonical receive loop"
    );
    let sent = fixture.dsh_command(
        &[
            "sendmessage",
            "--to",
            &worker,
            "--subject",
            "enrolled-loop",
            "message for the canonical receive loop",
        ],
        "agentA",
    );
    let sent_text = rendered(&sent);
    assert!(sent.status.success(), "{sent_text}");
    let sent: Value = serde_json::from_slice(&sent.stdout).expect("sendmessage JSON");
    assert_eq!(sent["notification"], "dsh-wake-enqueued", "{sent}");
    fixture.finish();
}

#[test]
fn dsh_context_enrollment_rejection_preserves_identity_and_retry_recovers() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    fixture.gateway.set_enroll_mode(EnrollMode::Reject);
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),
        "runtime_id":"gateway-runtime", "agent_id":"agentA",
        "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let failed = fixture.command(&["context", "--provide", &facts]);
    let failed_text = rendered(&failed);
    assert!(
        !failed.status.success(),
        "a rejected receive-loop enrollment must not report completion: {failed_text}"
    );
    assert!(
        failed_text.contains("DSH_ENDPOINT_REJECTED"),
        "the enrollment failure must be explicit: {failed_text}"
    );
    assert_eq!(
        fixture.registrations(),
        1,
        "the committed registration must stay durable"
    );
    let (worker, before) = only_identity(&fixture);
    let token = before["token"].as_str().expect("credential").to_owned();
    assert!(
        !failed_text.contains(&token),
        "credentials must never appear in context output: {failed_text}"
    );
    let binding_id = before["runtime"]["binding_id"]
        .as_str()
        .expect("binding id")
        .to_owned();
    let generation = before["runtime"]["endpoint_generation"]
        .as_u64()
        .expect("generation");
    assert_eq!(fixture.gateway.register_peer_requests().len(), 1);
    assert!(!fixture.gateway.is_enrolled("gateway-runtime", "agentA"));

    fixture.gateway.set_enroll_mode(EnrollMode::Accept);
    let retry = fixture.dsh_command(&["context"], "agentA");
    let retry_text = rendered(&retry);
    assert!(retry.status.success(), "{retry_text}");
    let retry: Value = serde_json::from_slice(&retry.stdout).expect("retry JSON");
    assert_eq!(retry["result"]["outcome"], "completed", "{retry}");
    assert_eq!(
        retry["result"]["snapshot"]["identity"]["worker_id"],
        worker.as_str(),
        "{retry}"
    );
    assert_eq!(
        retry["result"]["snapshot"]["binding"]["binding_id"],
        binding_id.as_str(),
        "{retry}"
    );
    assert_eq!(
        retry["result"]["snapshot"]["binding"]["endpoint_generation"], generation,
        "{retry}"
    );
    assert_eq!(
        fixture.registrations(),
        1,
        "retrying enrollment must not Register again"
    );
    assert_eq!(fixture.gateway.register_peer_requests().len(), 2);
    assert!(
        fixture.gateway.is_enrolled("gateway-runtime", "agentA"),
        "retry must start the canonical receive loop"
    );
    let (_, after) = only_identity(&fixture);
    assert_eq!(after["token"], token.as_str(), "credential changed");
    assert_eq!(after["runtime"]["binding_id"], binding_id.as_str());
    assert_eq!(
        after["runtime"]["endpoint_generation"], generation,
        "generation changed"
    );
    fixture.finish();
}

#[test]
fn dsh_context_enrollment_receipt_mismatch_is_explicit_and_retry_recovers() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    fixture.gateway.set_enroll_mode(EnrollMode::MismatchBinding);
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),
        "runtime_id":"gateway-runtime", "agent_id":"agentA",
        "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let failed = fixture.command(&["context", "--provide", &facts]);
    let failed_text = rendered(&failed);
    assert!(
        !failed.status.success(),
        "a mismatched enrollment receipt must not report completion: {failed_text}"
    );
    assert!(
        failed_text.contains("DSH_ENDPOINT_REJECTED"),
        "the receipt mismatch must be explicit: {failed_text}"
    );
    assert_eq!(fixture.registrations(), 1);
    let (_, before) = only_identity(&fixture);
    assert!(!fixture.gateway.is_enrolled("gateway-runtime", "agentA"));

    fixture.gateway.set_enroll_mode(EnrollMode::Accept);
    let retry = fixture.dsh_command(&["context"], "agentA");
    let retry_text = rendered(&retry);
    assert!(retry.status.success(), "{retry_text}");
    assert_eq!(
        fixture.registrations(),
        1,
        "retrying enrollment must not Register again"
    );
    assert_eq!(fixture.gateway.register_peer_requests().len(), 2);
    assert!(
        fixture.gateway.is_enrolled("gateway-runtime", "agentA"),
        "retry must start the canonical receive loop"
    );
    let (_, after) = only_identity(&fixture);
    assert_eq!(after["token"], before["token"]);
    assert_eq!(after["runtime"], before["runtime"]);
    fixture.finish();
}

#[test]
fn dsh_context_recovers_a_gateway_registered_binding_without_cli_archive() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    let registered = register(
        &fixture,
        "gateway-owned-worker",
        "gateway-owned-token",
        "agentA",
    );
    assert_ok(&registered);
    let context = binding_context(&registered, &fixture.root);
    assert_ok(&authority(
        &fixture,
        "MasterPromote",
        "gateway-owned-worker",
        "gateway-owned-token",
        &context,
        json!({"approval":"isolated DSH gateway recovery acceptance"}),
    ));
    assert!(!fixture
        .state
        .join("identities/gateway-owned-worker/identity.json")
        .exists());
    assert!(
        fixture.gateway.register_peer_requests().is_empty(),
        "the raw Register operation is not the enrollment path"
    );
    let count = fixture.registrations();
    let binding = registered["command"]["binding"].clone();
    let restored = fixture.dsh_command(&["context"], "agentA");
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        "gateway-owned-worker"
    );
    assert_eq!(
        restored["result"]["snapshot"]["binding"]["binding_id"],
        binding["binding_id"]
    );
    assert_eq!(fixture.registrations(), count);
    assert_eq!(
        status(&fixture, &fixture.root)["master"]["worker_id"],
        "gateway-owned-worker"
    );
    let requests = fixture.gateway.register_peer_requests();
    assert!(!requests.is_empty(), "recovery must enroll the loop");
    let params = &requests[0]["params"];
    assert_eq!(params["runtimeId"], "gateway-runtime");
    assert_eq!(params["agentId"], "agentA");
    assert_eq!(params["binding"]["workerId"], "gateway-owned-worker");
    assert_eq!(
        params["binding"]["projectScope"],
        fixture.root.to_string_lossy().as_ref()
    );
    let (_, identity) = only_identity(&fixture);
    assert_runtime_context(params, &identity["runtime"]);
    assert_eq!(
        params["binding"]["token"], "gateway-owned-token",
        "the committed credential must be the one handed to the gateway"
    );
    assert!(
        fixture.gateway.is_enrolled("gateway-runtime", "agentA"),
        "recovery must start the canonical receive loop"
    );
    fixture.restart_daemon();
    let restored = fixture.dsh_command(&["context"], "agentA");
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).expect("recovery JSON");
    assert_eq!(fixture.registrations(), count);
    assert_eq!(
        status(&fixture, &fixture.root)["master"]["worker_id"],
        "gateway-owned-worker"
    );
    // Startup and maintenance now restore loops without an agent command, so
    // handoff count is not a registration count. Every retry must carry the
    // exact original identity/epoch; Registered and master assertions above
    // still prohibit a new identity or authority mutation.
    let repeated_requests = fixture.gateway.register_peer_requests();
    assert!(
        repeated_requests.len() >= 2,
        "repeated recovery must re-enroll"
    );
    for request in repeated_requests {
        assert_runtime_context(&request["params"], &identity["runtime"]);
        assert_eq!(
            request["params"]["binding"]["workerId"],
            "gateway-owned-worker"
        );
        assert_eq!(request["params"]["binding"]["token"], "gateway-owned-token");
    }
    assert_eq!(
        restored["result"]["snapshot"]["binding"], binding,
        "recovery must keep the same binding and generation"
    );
    fixture.finish();
}

#[test]
fn dsh_session_recovery_ignores_a_registered_codex_launcher() {
    let mut fixture = Fixture::new();
    fixture.start_daemon();
    let native =
        native_support::TestAppServer::start(&fixture.root, "launcher-session", "launcher-thread");
    let native_facts = json!({"session_id":"launcher-session","thread_id":"launcher-thread",
        "endpoint":native.endpoint(),"namespace":"codex_tui"})
    .to_string();
    let launcher = fixture
        .configured_command(&["context", "--provide", &native_facts])
        .env("COLLAB_APPSERVER_NAMESPACE", "codex_tui")
        .output()
        .unwrap();
    assert!(
        launcher.status.success(),
        "{} {}",
        String::from_utf8_lossy(&launcher.stdout),
        String::from_utf8_lossy(&launcher.stderr)
    );
    let launcher: Value = serde_json::from_slice(&launcher.stdout).unwrap();
    let launcher_worker = &launcher["result"]["snapshot"]["identity"]["worker_id"];
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(), "runtime_id":"gateway-runtime",
        "agent_id":"agentA", "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let command = |args: &[&str]| {
        fixture
            .configured_command(args)
            .env("DSH_SESSION_ID", "agentA")
            .env("CODEX_SESSION_ID", "launcher-session")
            .env("CODEX_THREAD_ID", "launcher-thread")
            .env(
                "CODEX_APP_SERVER_SOCKET",
                native.endpoint().strip_prefix("unix://").unwrap(),
            )
            .env("COLLAB_APPSERVER_NAMESPACE", "codex_tui")
            .output()
            .unwrap()
    };
    let registered = command(&["context", "--provide", &facts]);
    assert!(
        registered.status.success(),
        "{} {}",
        String::from_utf8_lossy(&registered.stdout),
        String::from_utf8_lossy(&registered.stderr)
    );
    let registered: Value = serde_json::from_slice(&registered.stdout).unwrap();
    let dsh_binding = registered["result"]["snapshot"]["binding"].clone();
    assert_ne!(
        registered["result"]["snapshot"]["identity"]["worker_id"],
        *launcher_worker
    );
    let count = fixture.registrations();
    let restored = command(&["context"]);
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["result"]["snapshot"]["binding"], dsh_binding);
    assert_eq!(fixture.registrations(), count);
    fixture.restart_daemon();
    let restored = fixture
        .configured_command(&["context"])
        .env("DSH_SESSION_ID", "agentA")
        .env("CODEX_SESSION_ID", "launcher-session")
        .env("CODEX_THREAD_ID", "launcher-thread")
        .env(
            "CODEX_APP_SERVER_SOCKET",
            native.endpoint().strip_prefix("unix://").unwrap(),
        )
        .env("COLLAB_APPSERVER_NAMESPACE", "codex_tui")
        .output()
        .unwrap();
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["result"]["snapshot"]["binding"], dsh_binding);
    assert_eq!(fixture.registrations(), count);
    drop(native);
    fixture.finish();
}

#[test]
fn dsh_context_rejects_missing_invalid_and_conflicting_facts_without_registration() {
    let mut fixture = Fixture::new();
    fixture.start_daemon();
    let missing = fixture.dsh_command(&["context"], "agentA");
    assert!(!missing.status.success());
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["result"]["outcome"], "missing_facts", "{missing}");
    assert_eq!(
        missing["result"]["requires"]["fields"],
        json!(["dsh"]),
        "{missing}"
    );
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(), "runtime_id":"gateway-runtime",
        "agent_id":"agentA", "session_id":"agentA", "cwd":fixture.root
    }});
    let conflict = fixture.dsh_command(&["context", "--provide", &facts.to_string()], "agentB");
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("IDENTITY_FACT_CONFLICT"));
    for (field, invalid) in [
        ("endpoint", "relative.sock"),
        ("agent_id", "unknown-agent"),
        ("cwd", "/different-project"),
    ] {
        let mut invalid_facts = facts.clone();
        invalid_facts["dsh"][field] = json!(invalid);
        let rejected = fixture.command(&["context", "--provide", &invalid_facts.to_string()]);
        assert!(
            !rejected.status.success(),
            "invalid {field} must fail: {}",
            String::from_utf8_lossy(&rejected.stdout)
        );
        assert_eq!(
            fixture.registrations(),
            0,
            "invalid {field} must not register"
        );
    }
    assert!(
        !fixture.state.join("identities").exists()
            || std::fs::read_dir(fixture.state.join("identities"))
                .unwrap()
                .next()
                .is_none()
    );
    fixture.finish();
}

#[test]
fn dsh_mcp_registers_from_typed_gateway_facts() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    let binary = std::env::var_os("COLLAB_TEST_MCP_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab-mcp")));
    let mut child = fixture
        .configured_program(&binary, &[])
        .env("COLLAB_BIN", &fixture.binary)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"collab_context","arguments":{"provide":{"dsh":{
                "endpoint":fixture.gateway.endpoint(), "runtime_id":"gateway-runtime",
                "agent_id":"agentB", "session_id":"agentB", "cwd":fixture.root
            }}}
        }})
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        replies
            .iter()
            .any(|reply| reply["id"] == 1 && reply["result"].is_object()),
        "{replies:?}"
    );
    let reply = replies.iter().find(|reply| reply["id"] == 2).unwrap();
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let response: Value = serde_json::from_str(text).unwrap();
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    assert_eq!(
        response["result"]["snapshot"]["identity"]["transport"]["kind"],
        "dsh"
    );
    assert!(fixture.registrations() > 0);
    assert_eq!(
        fixture.gateway.register_peer_requests().len(),
        1,
        "MCP first registration must enroll the canonical receive loop"
    );
    assert!(
        fixture.gateway.is_enrolled("gateway-runtime", "agentB"),
        "MCP registration must start the canonical receive loop"
    );
    assert!(
        !text.contains("\"token\""),
        "MCP must not expose stored credentials"
    );
    fixture.finish();
}

#[test]
fn dsh_master_authority_public_wire_and_restart_preserve_peer_state() {
    let mut fixture = Fixture::new();
    fixture.start_daemon();

    let reg_a = register(&fixture, "workerA", "tokenA", "agentA");
    assert_ok(&reg_a);
    assert_eq!(reg_a["typed"], true, "{reg_a}");
    assert_eq!(reg_a["worker_id"], "workerA", "{reg_a}");
    assert_eq!(reg_a["transport_selected"]["kind"], "dsh", "{reg_a}");
    let ctx_a = binding_context(&reg_a, &fixture.root);
    let reg_b = register(&fixture, "workerB", "tokenB", "agentB");
    assert_ok(&reg_b);
    assert_eq!(reg_b["transport_selected"]["kind"], "dsh", "{reg_b}");
    let ctx_b = binding_context(&reg_b, &fixture.root);

    let empty = status(&fixture, &fixture.root);
    assert_ok(&empty);
    assert!(empty["master"].is_null(), "{empty}");
    assert_scope(&empty, &fixture.root);

    let promoted = authority(
        &fixture,
        "MasterPromote",
        "workerA",
        "tokenA",
        &ctx_a,
        json!({"approval":"fixture approved promotion"}),
    );
    assert_ok(&promoted);
    assert_eq!(promoted["master"], "workerA", "{promoted}");
    assert_eq!(
        promoted["mode"], "user_approved_self_promotion",
        "{promoted}"
    );
    assert_scope(&promoted, &fixture.root);
    let assigned = status(&fixture, &fixture.root);
    assert_eq!(assigned["master"]["worker_id"], "workerA", "{assigned}");
    assert_scope(&assigned, &fixture.root);

    for approval in ["", "bad\ncontrol"] {
        let refusal = clear(&fixture, "workerB", "tokenB", &ctx_b, approval);
        must_refuse(&refusal);
        let unchanged = status(&fixture, &fixture.root);
        assert_eq!(unchanged["master"], assigned["master"], "{unchanged}");
        assert_scope(&unchanged, &fixture.root);
    }
    let cleared_by_nonmaster = clear(
        &fixture,
        "workerB",
        "tokenB",
        &ctx_b,
        "fixture approved clear",
    );
    assert_ok(&cleared_by_nonmaster);
    assert_eq!(
        cleared_by_nonmaster["previous_worker_id"], "workerA",
        "{cleared_by_nonmaster}"
    );
    assert_eq!(
        cleared_by_nonmaster["was_empty"], false,
        "{cleared_by_nonmaster}"
    );
    assert_scope(&cleared_by_nonmaster, &fixture.root);
    let empty_again = clear(
        &fixture,
        "workerB",
        "tokenB",
        &ctx_b,
        "fixture approved clear",
    );
    assert_ok(&empty_again);
    assert_eq!(
        empty_again["previous_worker_id"],
        Value::Null,
        "{empty_again}"
    );
    assert_eq!(empty_again["was_empty"], true, "{empty_again}");
    assert_scope(&empty_again, &fixture.root);

    assert_ok(&authority(
        &fixture,
        "MasterPromote",
        "workerA",
        "tokenA",
        &ctx_a,
        json!({"approval":"fixture approved promotion"}),
    ));
    let delegated = authority(
        &fixture,
        "MasterDelegate",
        "workerA",
        "tokenA",
        &ctx_a,
        json!({"target_id":"workerB"}),
    );
    assert_ok(&delegated);
    assert_eq!(delegated["master"], "workerB", "{delegated}");
    assert_scope(&delegated, &fixture.root);
    let before_refusals = status(&fixture, &fixture.root);
    let wrong_token = clear(
        &fixture,
        "workerA",
        "wrong-token",
        &ctx_a,
        "fixture approved clear",
    );
    must_refuse(&wrong_token);
    assert_eq!(
        status(&fixture, &fixture.root)["master"],
        before_refusals["master"]
    );
    let mut stale_context = ctx_a.clone();
    stale_context["runtime_context"]["endpoint_generation"] = json!(
        ctx_a["runtime_context"]["endpoint_generation"]
            .as_u64()
            .unwrap()
            + 1
    );
    let stale = clear(
        &fixture,
        "workerA",
        "tokenA",
        &stale_context,
        "fixture approved clear",
    );
    must_refuse(&stale);
    assert_eq!(
        status(&fixture, &fixture.root)["master"],
        before_refusals["master"]
    );

    // Preserve publicly observable peer and business records across authority clearing and restart.
    let workers = fixture.request(json!({"op":"Workers","project_context":{
        "app_scope_id":"appserver-cli","canonical_root":fixture.root,"project_scope":fixture.root
    }}));
    assert_ok(&workers);
    let task_id = format!("task-{}", std::process::id());
    let task = fixture.request(json!({"op":"TaskRegister","task_id":task_id,
        "priority":"p2","next_step":"survive clear","worker_id":"workerA","token":"tokenA",
        "project_context":ctx_a}));
    assert_ok(&task);
    let sent = fixture.request(json!({"op":"Send","from":"workerA","worker_id":"workerA",
        "token":"tokenA","to":"workerB","type":"notify","subject":"preservation",
        "body":"preserve me","delivery":"queued","message_id":"fixture-message-id",
        "command":{"command_id":"cmd-preserve","operation_id":"op-preserve",
            "actor_binding_id":reg_a["command"]["binding"]["binding_id"],
            "endpoint_generation":reg_a["command"]["binding"]["endpoint_generation"],
            "scope":{"app_scope_id":"appserver-cli","project_scope_id":fixture.root}},
        "project_context":ctx_a}));
    assert_ok(&sent);
    assert!(
        sent["msg_id"].is_string(),
        "missing authoritative message id: {sent}"
    );
    assert_eq!(sent["notification"], "dsh-wake-enqueued", "{sent}");
    let msg_id = sent["msg_id"].as_str().unwrap().to_owned();

    fixture.gateway.shutdown().expect("stop owned gateway");
    let unreachable = status(&fixture, &fixture.root);
    assert_ok(&unreachable);
    assert_eq!(
        unreachable["master"]["worker_id"], "workerB",
        "{unreachable}"
    );
    assert_ne!(
        unreachable["master"]["endpoint_live"], true,
        "an unreachable gateway must not be reported live: {unreachable}"
    );
    assert_scope(&unreachable, &fixture.root);

    let clear_a = clear(
        &fixture,
        "workerA",
        "tokenA",
        &ctx_a,
        "fixture approved clear",
    );
    assert_ok(&clear_a);
    assert_eq!(clear_a["previous_worker_id"], "workerB", "{clear_a}");
    fixture.restart_daemon();

    let after_restart = status(&fixture, &fixture.root);
    assert_ok(&after_restart);
    assert!(
        after_restart["master"].is_null(),
        "authority resurrected: {after_restart}"
    );
    assert_scope(&after_restart, &fixture.root);
    let workers_after = fixture.request(json!({"op":"Workers","project_context":{
        "app_scope_id":"appserver-cli","canonical_root":fixture.root,"project_scope":fixture.root
    }}));
    assert_ok(&workers_after);
    for worker in ["workerA", "workerB"] {
        assert!(
            workers_after.to_string().contains(worker),
            "peer missing after restart: {workers_after}"
        );
    }
    let task_after = fixture.request(json!({"op":"TaskStatus","task_id":task_id,
        "project_context":ctx_a}));
    assert_ok(&task_after);
    assert!(
        task_after.to_string().contains("survive clear"),
        "task missing: {task_after}"
    );
    let mailbox = fixture.request(json!({"op":"MailboxRead","all":true,"worker_id":"workerB",
        "token":"tokenB","project_context":ctx_b}));
    assert_ok(&mailbox);
    assert!(
        mailbox.to_string().contains(&msg_id),
        "message missing: {mailbox}"
    );
    assert!(
        mailbox.to_string().contains("preserve me"),
        "message body missing: {mailbox}"
    );

    let msg_status = fixture.request(json!({"op":"MsgStatus","msg_id":msg_id,
        "project_context":ctx_a}));
    assert_ok(&msg_status);
    assert_eq!(msg_status["consumed_by_recv"], false, "{msg_status}");
    fixture.finish();
}

#[test]
fn dsh_daemon_restart_reenrolls_committed_binding_without_context() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();

    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),
        "runtime_id":"gateway-runtime", "agent_id":"agentA",
        "session_id":"agentA", "cwd":fixture.root
    }})
    .to_string();
    let enrolled = fixture.command(&["context", "--provide", &facts]);
    let enrolled_text = rendered(&enrolled);
    assert!(enrolled.status.success(), "{enrolled_text}");
    let (worker, identity) = only_identity(&fixture);
    let token = identity["token"].as_str().expect("credential").to_owned();
    let committed_binding = identity["runtime"].clone();
    let ctx = json!({
        "app_scope_id": committed_binding["appserver_id"],
        "canonical_root": fixture.root,
        "project_scope": identity["project_scope"],
        "runtime_context": committed_binding
    });
    assert!(fixture.gateway.is_enrolled("gateway-runtime", "agentA"));

    let first = fixture.dsh_command(
        &[
            "sendmessage",
            "--to",
            &worker,
            "--subject",
            "restart-pending",
            "pending across restart",
        ],
        "agentA",
    );
    let first_text = rendered(&first);
    assert!(first.status.success(), "{first_text}");
    let first: Value = serde_json::from_slice(&first.stdout).expect("sendmessage JSON");
    assert_eq!(first["notification"], "dsh-wake-enqueued", "{first}");
    let msg_id = first["msg_id"].as_str().expect("message id").to_owned();

    let registrations_before_restart = fixture.registrations();
    let enrollments_before_restart = fixture.gateway.register_peer_requests().len();

    fixture.stop_daemon();
    // The gateway restarts with an empty local cache while Collab keeps the
    // committed durable bindings in its reducers.
    fixture.gateway.clear_enrollment();
    assert!(!fixture.gateway.is_enrolled("gateway-runtime", "agentA"));
    fixture.start_daemon();

    // No `collab context` or `Register` follows the restart. The daemon must
    // hand its committed binding to the cache-empty gateway on its own.
    assert!(
        fixture
            .gateway
            .wait_enrolled("gateway-runtime", "agentA", Duration::from_secs(8)),
        "daemon restart did not re-enroll the committed DSH binding"
    );

    let requests = fixture.gateway.register_peer_requests();
    assert!(
        requests.len() > enrollments_before_restart,
        "restart produced no register-peer handoff: {requests:?}"
    );
    let replayed = requests
        .iter()
        .skip(enrollments_before_restart)
        .find(|request| request["params"]["agentId"] == "agentA")
        .expect("restart handoff for agentA");
    assert_eq!(replayed["params"]["runtimeId"], "gateway-runtime");
    assert_eq!(
        replayed["params"]["binding"]["workerId"],
        worker.as_str(),
        "{replayed}"
    );
    assert_runtime_context(&replayed["params"], &committed_binding);
    assert_eq!(fixture.registrations(), registrations_before_restart);

    let after_restart = fixture.dsh_command(
        &[
            "sendmessage",
            "--to",
            &worker,
            "--subject",
            "restart-live",
            "delivered after restart",
        ],
        "agentA",
    );
    let after_restart_text = rendered(&after_restart);
    assert!(after_restart.status.success(), "{after_restart_text}");
    let after_restart: Value =
        serde_json::from_slice(&after_restart.stdout).expect("sendmessage after restart JSON");
    assert_eq!(
        after_restart["notification"], "dsh-wake-enqueued",
        "{after_restart}"
    );

    let mailbox = fixture.request(json!({"op":"MailboxRead","all":true,"worker_id":worker,
        "token":token,"project_context":ctx}));
    assert_ok(&mailbox);
    assert!(
        mailbox.to_string().contains(&msg_id),
        "pending message missing after restart: {mailbox}"
    );
    assert!(
        mailbox.to_string().contains("pending across restart"),
        "pending message body missing after restart: {mailbox}"
    );
    let msg_status = fixture.request(json!({"op":"MsgStatus","msg_id":msg_id,
        "project_context":ctx}));
    assert_ok(&msg_status);
    assert_eq!(msg_status["consumed_by_recv"], false, "{msg_status}");

    // The handoff reuses the committed credential; it must not echo it.
    assert!(
        replayed["params"]["binding"].get("token").is_some()
            && replayed["params"]["binding"]["token"] != "",
        "restart handoff must reuse the committed credential"
    );
    assert!(
        replayed.get("result").is_none(),
        "request log must stay a request"
    );

    fixture.finish();
}

#[test]
fn dsh_runtime_uuid_change_rebinds_without_agent_recovery() {
    let mut fixture = Fixture::new_requiring_enrollment();
    fixture.start_daemon();
    let facts = json!({"dsh":{
        "endpoint":fixture.gateway.endpoint(),"runtime_id":"gateway-runtime",
        "agent_id":"agentA","session_id":"agentA","cwd":fixture.root
    }})
    .to_string();
    let initial = fixture.command(&["context", "--provide", &facts]);
    assert!(initial.status.success(), "{}", rendered(&initial));
    let (worker, identity) = only_identity(&fixture);
    let runtime_identity = &identity["runtime"];
    let context = json!({"app_scope_id":runtime_identity["appserver_id"],
        "canonical_root":fixture.root,"project_scope":identity["project_scope"],
        "runtime_context":runtime_identity});
    assert_ok(&authority(
        &fixture,
        "MasterPromote",
        &worker,
        identity["token"].as_str().unwrap(),
        &context,
        json!({"approval":"isolated runtime replacement master preservation"}),
    ));
    fixture.stop_daemon();
    fixture.gateway.clear_enrollment();
    *fixture.gateway.current_runtime.lock().unwrap() = Some("gateway-new".into());
    fixture.start_daemon();
    assert!(
        fixture
            .gateway
            .wait_enrolled("gateway-new", "agentA", Duration::from_secs(8)),
        "same live DSH session was not rebound to new runtime UUID"
    );
    let (restored_worker, restored) = only_identity(&fixture);
    assert_eq!(restored_worker, worker);
    assert_eq!(
        restored["runtime"]["binding_id"],
        identity["runtime"]["binding_id"]
    );
    assert_eq!(
        restored["runtime"]["runtime_id"],
        identity["runtime"]["runtime_id"]
    );
    assert_eq!(
        restored["runtime"]["endpoint_generation"].as_u64().unwrap(),
        identity["runtime"]["endpoint_generation"].as_u64().unwrap() + 1
    );
    assert_eq!(restored["transport"]["namespace"], "gateway-new");
    assert_eq!(
        status(&fixture, &fixture.root)["master"]["worker_id"],
        worker
    );
    // Manual recovery resolves current facts too; no UUID/token arguments.
    let manual = fixture.dsh_command(&["context", "--recover-communication"], "agentA");
    assert!(manual.status.success(), "{}", rendered(&manual));
    assert_eq!(only_identity(&fixture).1["runtime"], restored["runtime"]);
    fixture.finish();
}
