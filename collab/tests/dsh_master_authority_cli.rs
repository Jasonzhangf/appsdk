use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "dshma-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn collab_binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

struct Gateway {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Gateway {
    fn start(socket: PathBuf, root: PathBuf, agents: Vec<String>) -> Self {
        let listener = UnixListener::bind(&socket).expect("bind gateway socket");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
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
                let reply = if method == "agent-facts"
                    && agents.iter().any(|agent| agent == agent_id)
                {
                    json!({"ok":true,"result":{
                        "nonce":request["params"]["nonce"],
                        "runtimeId":runtime_id,
                        "agentId":agent_id,
                        "sessionId":agent_id,
                        "cwd":root,
                        "status":"running"
                    }})
                } else if method == "enqueue" && agents.iter().any(|agent| agent == agent_id) {
                    json!({"ok":true,"result":{
                        "messageId":request["params"]["messageId"],
                        "runtimeId":runtime_id,
                        "agentId":agent_id
                    }})
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
        }
    }

    fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
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
    daemon_owned: bool,
    finished: bool,
}

impl Fixture {
    fn new() -> Self {
        let root = unique_root();
        std::fs::create_dir_all(root.join("h")).expect("create host state");
        std::fs::create_dir_all(root.join(".agent-collab")).expect("create baseline marker");
        std::fs::create_dir_all(root.join("home")).expect("create isolated home");
        let root = root.canonicalize().expect("canonical project root");
        let gateway_dir = root.join("gateway");
        std::fs::create_dir_all(&gateway_dir).expect("create gateway directory");
        let gateway = Gateway::start(
            gateway_dir.join("control.sock"),
            root.clone(),
            vec!["agentA".into(), "agentB".into()],
        );
        Self {
            state: root.join("h"),
            root,
            binary: collab_binary(),
            gateway,
            daemon_owned: false,
            finished: false,
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(&self.binary)
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
            .env_remove("DSH_SESSION_ID")
            .output()
            .expect("run collab CLI")
    }

    fn daemon(&mut self, args: &[&str]) -> Output {
        let output = self.command(args);
        if args == ["up"] && output.status.success() {
            self.daemon_owned = true;
        }
        if args == ["down"] && output.status.success() {
            self.daemon_owned = false;
        }
        output
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

    fn wait_socket(&self, exists: bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if self.state.join("server.sock").exists() == exists {
                if exists {
                    let reply = self.request(json!({"op":"Ping"}));
                    if reply["ok"] == true {
                        return;
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

    fn finish(mut self) {
        let down = self.daemon(&["down"]);
        assert!(down.status.success(), "final daemon down: {down:?}");
        self.wait_socket(false);
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
        if self.daemon_owned {
            let down = self.daemon(&["down"]);
            if !down.status.success() {
                eprintln!("owned failed fixture daemon cleanup: {down:?}");
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && self.state.join("server.sock").exists() {
                thread::sleep(Duration::from_millis(30));
            }
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
fn dsh_master_authority_public_wire_and_restart_preserve_peer_state() {
    let mut fixture = Fixture::new();
    let up = fixture.daemon(&["up"]);
    assert!(
        up.status.success(),
        "up stdout={} stderr={}",
        String::from_utf8_lossy(&up.stdout),
        String::from_utf8_lossy(&up.stderr)
    );
    fixture.wait_socket(true);

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
    let down = fixture.daemon(&["down"]);
    assert!(
        down.status.success(),
        "down stdout={} stderr={}",
        String::from_utf8_lossy(&down.stdout),
        String::from_utf8_lossy(&down.stderr)
    );
    fixture.wait_socket(false);
    let restart = fixture.daemon(&["up"]);
    assert!(
        restart.status.success(),
        "restart up stdout={} stderr={}",
        String::from_utf8_lossy(&restart.stdout),
        String::from_utf8_lossy(&restart.stderr)
    );
    fixture.wait_socket(true);

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
