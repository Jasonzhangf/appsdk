//! Public JSON-RPC acceptance. Only fixture identity/resource setup bypasses MCP.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/daemon_readiness.rs"]
mod daemon_readiness;

const MASTER: &str = "01a0d637-75e2-71b2-8634-2be0c08a9adc";
const PEER: &str = "01a0d639-dafc-7571-a0aa-bbc2bbbc04ce";
const OTHER: &str = "01a0d643-1d55-7c4d-9c53-3f4b8f610001";
static FIXTURE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fixture_temp_base() -> PathBuf {
    let private_tmp = std::path::Path::new("/private/tmp");
    if private_tmp.is_dir() {
        private_tmp.to_path_buf()
    } else {
        std::env::temp_dir()
    }
}

struct Fixture {
    root: PathBuf,
    daemon: Option<Child>,
    stop: Arc<AtomicBool>,
    streams: Arc<Mutex<Vec<UnixStream>>>,
    host: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_temp_base().join(format!(
            "mqp-{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("project/.agent-collab")).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let listener = UnixListener::bind(root.join("app.sock")).unwrap_or_else(|error| {
            std::fs::remove_dir_all(&root).unwrap();
            panic!("bind isolated native fixture: {error}");
        });
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let streams = Arc::new(Mutex::new(Vec::new()));
        let accepted_streams = Arc::clone(&streams);
        let project = root.join("project").canonicalize().unwrap();
        let host = thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Ok(cloned) = stream.try_clone() {
                            accepted_streams
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push(cloned);
                        }
                        let stop = Arc::clone(&stopped);
                        let project = project.clone();
                        connections.push(thread::spawn(move || native_host(stream, stop, project)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept native fixture: {error}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        let mut fixture = Self {
            root,
            daemon: None,
            stop,
            streams,
            host: Some(host),
        };
        fixture.start();
        for session in [MASTER, PEER, OTHER] {
            fixture.cli(session, &["context"]);
        }
        fixture.cli(
            MASTER,
            &[
                "master",
                "promote",
                "--approval",
                "isolated MCP parity fixture",
            ],
        );
        fixture
    }

    fn command(&self, binary: &str, session: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("TMPDIR", fixture_temp_base())
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env("COLLAB_STATE_DIR", self.root.join("state"))
            .env("COLLAB_APPSERVER_SOCKET", self.root.join("app.sock"))
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", session)
            .env("CODEX_THREAD_ID", session)
            .env("COLLAB_TEST_ITERM2_ATTACHMENT", "fixture")
            .env("COLLAB_BIN", env!("CARGO_BIN_EXE_collab"))
            .current_dir(self.root.join("project"));
        command
    }

    fn start(&mut self) {
        let log = std::fs::File::create(self.root.join("daemon.log")).unwrap();
        let mut command = self.command(env!("CARGO_BIN_EXE_collab"), MASTER);
        command
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .arg("serve");
        self.daemon = Some(
            command
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        daemon_readiness::wait_for_ping(
            self.daemon.as_mut().unwrap(),
            &self.root.join("state/server.sock"),
            Duration::from_secs(30),
        );
    }

    fn stop_daemon_checked(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(mut child) = self.daemon.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let pid = child.id() as libc::pid_t;
                    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() != Some(libc::ESRCH) {
                            errors.push(format!("send SIGTERM to daemon pid {pid}: {err}"));
                        }
                    }
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) if std::time::Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(20));
                            }
                            Ok(None) => {
                                errors.push(format!(
                                    "daemon pid {pid} did not exit within 5s after SIGTERM; sending SIGKILL"
                                ));
                                let _ = child.kill();
                                let kill_deadline =
                                    std::time::Instant::now() + Duration::from_secs(2);
                                loop {
                                    match child.try_wait() {
                                        Ok(Some(_)) => break,
                                        Ok(None) if std::time::Instant::now() < kill_deadline => {
                                            thread::sleep(Duration::from_millis(20));
                                        }
                                        Ok(None) => {
                                            errors.push(format!(
                                                "daemon pid {pid} did not exit after SIGKILL"
                                            ));
                                            break;
                                        }
                                        Err(err) => {
                                            errors.push(format!(
                                                "wait for daemon pid {pid} after SIGKILL: {err}"
                                            ));
                                            break;
                                        }
                                    }
                                }
                                break;
                            }
                            Err(err) => {
                                errors.push(format!("wait for daemon pid {pid}: {err}"));
                                break;
                            }
                        }
                    }
                }
                Err(err) => errors.push(format!("check daemon child status: {err}")),
            }
        }
        let sock = self.root.join("state/server.sock");
        if sock.exists() {
            errors.push(format!(
                "daemon socket still exists after shutdown: {}",
                sock.display()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn stop_daemon(&mut self) {
        self.stop_daemon_checked().unwrap();
    }

    fn cli(&self, session: &str, argv: &[&str]) -> Value {
        let output = self
            .command(env!("CARGO_BIN_EXE_collab"), session)
            .args(argv)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{argv:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        value.pointer("/result/snapshot").cloned().unwrap_or(value)
    }

    fn rpc(&self, session: &str, method: &str, params: Value) -> Value {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_collab-mcp"), session)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(
            child.stdin.as_mut().unwrap(),
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
        )
        .unwrap();
        child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["id"], 1);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn raw(&self, session: &str, name: &str, arguments: Value) -> Value {
        self.rpc(
            session,
            "tools/call",
            json!({"name":name,"arguments":arguments}),
        )
    }

    fn call(&self, session: &str, name: &str, arguments: Value) -> Value {
        let result = self.raw(session, name, arguments);
        assert_eq!(result["isError"], false, "{name}: {result}");
        let value: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        value.pointer("/result/snapshot").cloned().unwrap_or(value)
    }

    fn row(&self, id: &str) -> Value {
        self.row_as(MASTER, id)
    }

    fn row_as(&self, session: &str, id: &str) -> Value {
        self.call(session, "collab_board_show", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap()
            .clone()
    }

    fn publish(&self, session: &str, id: &str, dependencies: &[&str]) -> Value {
        self.call(session, "collab_board_publish", json!({"id":id,"title":id,"description":"MCP public queue acceptance","delivery_condition":"observable receipt","test_condition":"public assertions","depends_on":dependencies}))
    }

    fn publish_for(&self, session: &str, id: &str, executor: &str, dependencies: &[&str]) -> Value {
        self.call(session, "collab_board_publish", json!({"id":id,"title":id,"description":"MCP public queue acceptance","delivery_condition":"observable receipt","test_condition":"public assertions","executor":executor,"depends_on":dependencies}))
    }

    fn claim_start(&self, session: &str, id: &str) -> Value {
        let revision = self.row_as(session, id)["revision"].clone();
        self.call(
            session,
            "collab_board_claim",
            json!({"id":id,"expected_revision":revision}),
        );
        let row = self.row_as(session, id);
        self.call(session, "collab_task_start", json!({"id":id,"expected_revision":row["revision"],"attempt":row["execute"]["attempt_generation"]}));
        self.row_as(session, id)
    }

    fn state_path(&self) -> PathBuf {
        let routes: Value =
            serde_json::from_slice(&std::fs::read(self.root.join("state/routes.json")).unwrap())
                .unwrap();
        let project = self.root.join("project").canonicalize().unwrap();
        let route = routes
            .as_array()
            .unwrap()
            .iter()
            .find(|route| {
                route["project_scope"] == project.to_str().unwrap()
                    && route["app_scope_id"] == "appserver-cli"
            })
            .unwrap();
        PathBuf::from(route["storage_root"].as_str().unwrap())
            .join(".agent-collab/server/state.json")
    }

    fn state(&self) -> Value {
        serde_json::from_slice::<Value>(&std::fs::read(self.state_path()).unwrap()).unwrap()
            ["state"]
            .clone()
    }

    fn seed_resources(&mut self, update: impl FnOnce(&mut Value)) {
        self.stop_daemon();
        let path = self.state_path();
        let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        update(&mut document["state"]);
        std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
        std::fs::remove_file(self.root.join("state/DOWN")).ok();
        self.start();
    }

    fn observe_peer(&self, session: &str) -> Value {
        let context = self.call(session, "collab_context", json!({}));
        let worker_id = context["identity"]["worker_id"].as_str().unwrap();
        self.cli(MASTER, &["worker", "snapshot", worker_id]);
        context
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut errors = Vec::new();
        if let Err(err) = self.stop_daemon_checked() {
            errors.push(err);
        }
        self.stop.store(true, Ordering::Relaxed);
        for s in self
            .streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
        {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(host) = self.host.take() {
            if let Err(panic) = host.join() {
                errors.push(format!("mock host thread panicked: {panic:?}"));
            }
        }
        if self.root.exists() {
            if let Err(err) = std::fs::remove_dir_all(&self.root) {
                errors.push(format!(
                    "remove fixture root {}: {err}",
                    self.root.display()
                ));
            } else if self.root.exists() {
                errors.push(format!(
                    "fixture root still exists after removal: {}",
                    self.root.display()
                ));
            }
        }
        if !errors.is_empty() {
            let summary = errors.join("; ");
            if std::thread::panicking() {
                eprintln!("fixture teardown failed during unwind: {summary}");
            } else {
                panic!("fixture teardown failed: {summary}");
            }
        }
    }
}

fn read_exact_with_stop(
    stream: &mut UnixStream,
    mut buf: &mut [u8],
    stop: &AtomicBool,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while !buf.is_empty() {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return false;
        }
        match stream.read(buf) {
            Ok(0) => return false,
            Ok(read) => buf = &mut buf[read..],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => return false,
        }
    }
    true
}

// The native endpoint reports real fixture session/cwd facts. It never supplies
// Collab identity, task, mailbox or scheduler results.
fn native_host(mut stream: UnixStream, stop: Arc<AtomicBool>, project: PathBuf) {
    if stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_millis(200)))
            .is_err()
    {
        return;
    }
    let mut header = Vec::new();
    let mut byte = [0];
    let handshake_deadline = Instant::now() + Duration::from_secs(1);
    while !header.ends_with(b"\r\n\r\n") {
        let Some(remaining) = handshake_deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        if !read_exact_with_stop(&mut stream, &mut byte, &stop, remaining) {
            return;
        }
        header.push(byte[0]);
    }
    if stop.load(Ordering::Relaxed)
        || stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            )
            .is_err()
    {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        let mut header = [0; 2];
        match stream.read(&mut header) {
            Ok(0) => return,
            Ok(1) => {
                if !read_exact_with_stop(
                    &mut stream,
                    &mut header[1..],
                    &stop,
                    Duration::from_secs(1),
                ) {
                    return;
                }
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return,
        }
        let mut length = usize::from(header[1] & 127);
        if length == 126 {
            let mut extended = [0; 2];
            if !read_exact_with_stop(&mut stream, &mut extended, &stop, Duration::from_secs(1)) {
                return;
            }
            length = u16::from_be_bytes(extended) as usize;
        }
        let mut mask = [0; 4];
        if header[1] & 128 != 0
            && !read_exact_with_stop(&mut stream, &mut mask, &stop, Duration::from_secs(1))
        {
            return;
        }
        let mut payload = vec![0; length];
        if !read_exact_with_stop(&mut stream, &mut payload, &stop, Duration::from_secs(1)) {
            return;
        }
        if header[1] & 128 != 0 {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        let Some(id) = request.get("id") else {
            continue;
        };
        let session = request["params"]["threadId"].as_str().unwrap_or("");
        let result = match request["method"].as_str().unwrap_or("") {
            "thread/read" => {
                json!({"thread":{"id":session,"sessionId":session,"cwd":project,"status":{"type":"idle"},"turns":[]}})
            }
            "thread/turns/list" | "thread/loaded/list" => json!({"data":[]}),
            "thread/resume" => json!({"thread":{"id":session}}),
            "turn/start" => json!({"turn":{"id":"fixture-turn","status":"inProgress"}}),
            "thread/queue/add" => json!({"queuedSubmission":{"id":"fixture-queue"}}),
            _ => json!({}),
        };
        let bytes = serde_json::to_vec(&json!({"id":id,"result":result})).unwrap();
        let mut frame = vec![0x81];
        if bytes.len() < 126 {
            frame.push(bytes.len() as u8);
        } else {
            frame.push(126);
            frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(&bytes);
        if stop.load(Ordering::Relaxed) || stream.write_all(&frame).is_err() {
            return;
        }
    }
}

#[test]
fn public_mcp_queue_contract() {
    let fixture = Fixture::new();
    let catalog = fixture.rpc(PEER, "tools/list", json!({}));
    let tools = catalog["tools"].as_array().unwrap();
    let recv = tools
        .iter()
        .find(|tool| tool["name"] == "collab_recv")
        .unwrap();
    assert_eq!(recv["inputSchema"]["required"], json!([]));
    assert!(recv["description"]
        .as_str()
        .unwrap()
        .contains("consume once"));
    for name in [
        "collab_task_start",
        "collab_task_deliver",
        "collab_board_invite",
    ] {
        assert!(tools.iter().any(|tool| tool["name"] == name));
    }

    fixture.call(
        MASTER,
        "collab_task_register",
        json!({"id":"parent","next":"produce input"}),
    );
    let published = fixture.publish(PEER, "dependent", &["parent"]);
    assert_eq!(published["ticket"]["task_id"], "dependent");
    assert_eq!(published["ticket"]["phases"][0]["phase"], "execute");
    assert_eq!(
        published["ticket"]["phases"][0]["status"],
        "waiting_assignment"
    );
    assert_eq!(
        published["ticket"]["phases"][0]["depends_on"],
        json!(["parent"])
    );
    assert_eq!(
        fixture.state()["board_details"]["dependent"]["admitted"],
        true
    );
    let dependent = fixture.row("dependent");
    assert_eq!(dependent["depends_on"], json!(["parent"]));
    assert_eq!(
        fixture.row_as(PEER, "dependent")["depends_on"],
        json!(["parent"])
    );
    let refused = fixture.raw(
        PEER,
        "collab_board_claim",
        json!({"id":"dependent","expected_revision":dependent["revision"]}),
    );
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused.to_string().contains("BOARD_DEPENDENCY_UNSATISFIED"),
        "{refused}"
    );
    for status in ["verifying", "reviewed"] {
        fixture.call(
            MASTER,
            "collab_task_update",
            json!({"id":"parent","status":status}),
        );
    }
    fixture.call(
        MASTER,
        "collab_task_deliver",
        json!({"id":"parent","no_candidate":true,"evidence":"input produced"}),
    );
    let revision = fixture.row("dependent")["revision"].clone();
    fixture.call(
        PEER,
        "collab_board_claim",
        json!({"id":"dependent","expected_revision":revision}),
    );
    let claimed = fixture.row_as(PEER, "dependent");
    assert_eq!(claimed["admitted"], true);
    assert_eq!(claimed["execute"]["status"], "claimed");
    let attempt = claimed["execute"]["attempt_generation"].as_u64().unwrap();
    let revision = claimed["revision"].as_u64().unwrap();
    for (args, expected_error) in [
        (
            json!({"id":"dependent","expected_revision":revision-1,"attempt":attempt}),
            "BOARD_STALE_REVISION",
        ),
        (
            json!({"id":"dependent","expected_revision":revision,"attempt":attempt+1}),
            "TICKET_STALE_ATTEMPT",
        ),
    ] {
        let refused = fixture.raw(PEER, "collab_task_start", args);
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(
            refused["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected_error),
            "expected {expected_error} in {refused}"
        );
        assert_eq!(fixture.row_as(PEER, "dependent"), claimed);
    }
    fixture.call(
        PEER,
        "collab_task_start",
        json!({"id":"dependent","expected_revision":revision,"attempt":attempt}),
    );
    let running = fixture.row_as(PEER, "dependent");
    assert_eq!(running["execute"]["status"], "running");
    let stale_delivery = fixture.raw(
        PEER,
        "collab_task_deliver",
        json!({"id":"dependent","attempt":attempt+1,"no_candidate":true,"evidence":"stale"}),
    );
    assert_eq!(stale_delivery["isError"], true, "{stale_delivery}");
    assert!(
        stale_delivery["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TICKET_STALE_ATTEMPT"),
        "{stale_delivery}"
    );
    assert_eq!(fixture.row_as(PEER, "dependent"), running);
    fixture.call(PEER, "collab_task_deliver", json!({"id":"dependent","attempt":attempt,"no_candidate":true,"evidence":"dependent output"}));
    assert_eq!(
        fixture.call(PEER, "collab_task_status", json!({"id":"dependent"}))["status"],
        "delivered"
    );
}

#[test]
fn public_mcp_wait_and_block_continuation() {
    let mut fixture = Fixture::new();
    let other_name = fixture.call(OTHER, "collab_context", json!({}))["identity"]["display_name"]
        .as_str()
        .unwrap()
        .to_owned();
    let peer_name = fixture.call(PEER, "collab_context", json!({}))["identity"]["display_name"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.publish_for(MASTER, "resource-owner", &other_name, &[]);
    fixture.claim_start(OTHER, "resource-owner");
    fixture.publish_for(MASTER, "waiter", &peer_name, &[]);
    fixture.claim_start(PEER, "waiter");
    fixture.publish_for(MASTER, "queued", &peer_name, &[]);
    let blocked_claim = fixture.raw(
        PEER,
        "collab_board_claim",
        json!({"id":"queued","expected_revision":fixture.row("queued")["revision"]}),
    );
    assert_eq!(blocked_claim["isError"], true, "{blocked_claim}");
    assert!(
        blocked_claim["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TICKET_ADMISSION_SLOT_OCCUPIED"),
        "{blocked_claim}"
    );
    let before = fixture.row_as(PEER, "waiter");
    for (blocker, expected_error) in [
        ("missing", "blocking task missing not found"),
        (
            "resource-owner",
            "blocking task does not hold a matching active resource",
        ),
        ("waiter", "WAIT_CYCLE_DETECTED"),
    ] {
        let rejected = fixture.raw(
            PEER,
            "collab_task_wait",
            json!({"id":"waiter","blocking_task":blocker}),
        );
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert!(
            rejected["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected_error),
            "expected {expected_error} in {rejected}"
        );
        assert_eq!(fixture.row_as(PEER, "waiter"), before);
    }
    fixture.seed_resources(|state| {
        state["tasks"]["waiter"]["feature_id"] = json!("shared-resource");
        state["tasks"]["resource-owner"]["feature_id"] = json!("shared-resource");
    });
    fixture.observe_peer(PEER);
    fixture.call(
        PEER,
        "collab_task_wait",
        json!({"id":"waiter","blocking_task":"resource-owner"}),
    );
    assert_eq!(fixture.row("waiter")["status"], "waiting");
    assert_eq!(
        fixture.row_as(PEER, "queued")["execute"]["status"],
        "claimed"
    );
    let owner_before = fixture.row_as(OTHER, "resource-owner");
    let cycle = fixture.raw(
        OTHER,
        "collab_task_wait",
        json!({"id":"resource-owner","blocking_task":"waiter"}),
    );
    assert_eq!(cycle["isError"], true);
    assert!(cycle.to_string().contains("WAIT_CYCLE_DETECTED"), "{cycle}");
    assert_eq!(fixture.row_as(OTHER, "resource-owner"), owner_before);

    let row = fixture.row_as(PEER, "queued");
    fixture.call(PEER, "collab_task_start", json!({"id":"queued","expected_revision":row["revision"],"attempt":row["execute"]["attempt_generation"]}));
    fixture.publish_for(MASTER, "after-block", &peer_name, &[]);
    let applied = fixture.call(
        PEER,
        "collab_task_block",
        json!({"id":"queued","next":"external business decision"}),
    );
    assert_eq!(applied["block_request"]["state"], "pending");
    assert_eq!(fixture.row("queued")["status"], "working");
    assert_ne!(
        fixture.row_as(PEER, "after-block")["execute"]["status"],
        "claimed"
    );
    let self_approval = fixture.raw(
        PEER,
        "collab_task_update",
        json!({"id":"queued","blocker_action":"approve","decision_reason":"self approval"}),
    );
    assert_eq!(self_approval["isError"], true, "{self_approval}");
    assert!(
        self_approval["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TASK_BLOCK_SELF_DECISION_DENIED"),
        "{self_approval}"
    );
    assert_eq!(fixture.row("queued")["status"], "working");
    let peer_context = fixture.call(PEER, "collab_context", json!({}));
    assert!(
        peer_context["management"]["block_requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|request| request["task_id"] == "queued" && request["state"] == "pending"),
        "{peer_context}"
    );
    assert_eq!(
        fixture.state()["task_block_requests"]["queued"]["state"],
        "pending"
    );
    let rejected = fixture.call(MASTER, "collab_task_update", json!({"id":"queued","blocker_action":"reject","decision_reason":"continue available work"}));
    assert_eq!(rejected["block_request"]["state"], "rejected");
    assert_eq!(fixture.row("queued")["status"], "working");
    fixture.call(
        PEER,
        "collab_task_block",
        json!({"id":"queued","next":"confirmed external blocker"}),
    );
    let approved = fixture.call(MASTER, "collab_task_update", json!({"id":"queued","blocker_action":"approve","decision_reason":"external owner must act"}));
    assert_eq!(approved["block_request"]["state"], "approved");
    assert_eq!(fixture.row("queued")["status"], "blocked");
    assert_eq!(
        fixture.row_as(PEER, "after-block")["execute"]["status"],
        "claimed"
    );
}

#[test]
fn public_mcp_invitation_context_and_receive() {
    let fixture = Fixture::new();
    let peer = fixture.call(PEER, "collab_context", json!({}));
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.publish(MASTER, "one-shot", &[]);
    fixture.observe_peer(PEER);
    let subscriptions = fixture.state()["notification_subscriptions"].clone();
    let invited = fixture.call(MASTER, "collab_board_invite", json!({"id":"one-shot","to":peer_id,"expected_revision":fixture.row("one-shot")["revision"],"one_shot":true}));
    assert!(invited["goal_schedule"]["subscription"].is_null());
    assert_eq!(fixture.state()["notification_subscriptions"], subscriptions);
    fixture.call(MASTER, "collab_board_withdraw", json!({"id":"one-shot","expected_revision":fixture.row("one-shot")["revision"],"reason":"test the declared periodic invitation"}));
    fixture.call(MASTER, "collab_board_publish", json!({"id":"long-horizon","title":"long-horizon","description":"MCP public queue acceptance","delivery_condition":"observable receipt","test_condition":"public assertions","long_horizon_content":"Continue the exact queue acceptance goal","long_horizon_every_ms":3600000,"long_horizon_repeat":2,"long_horizon_ttl_seconds":7200}));
    fixture.observe_peer(PEER);
    let invited = fixture.call(MASTER, "collab_board_invite", json!({"id":"long-horizon","to":peer_id,"expected_revision":fixture.row("long-horizon")["revision"]}));
    assert_eq!(invited["goal_schedule"]["armed"], true);
    let subscription = &invited["goal_schedule"]["subscription"];
    assert_eq!(subscription["status"], "armed");
    assert_eq!(subscription["subject"], "goal:task:long-horizon");
    assert_eq!(subscription["periodic_declared"], true);
    assert_eq!(subscription["interval_ms"], 3600000);
    assert_eq!(subscription["repeat_count"], 2);
    assert_eq!(
        subscription["goal_content"]["source"],
        json!({"kind": "board-task", "task_id": "long-horizon"})
    );
    assert_eq!(subscription["goal_content"]["ttl_seconds"], 7200);
    assert_eq!(
        invited["task"]["long_horizon"]["content"],
        "Continue the exact queue acceptance goal"
    );
    assert_eq!(invited["task"]["long_horizon"]["interval_ms"], 3600000);
    assert_eq!(invited["task"]["long_horizon"]["repeat_count"], 2);
    assert_eq!(invited["task"]["long_horizon"]["ttl_seconds"], 7200);
    let horizon = fixture.row("long-horizon")["long_horizon"].clone();
    assert!(
        horizon
            .to_string()
            .contains("Continue the exact queue acceptance goal"),
        "{horizon}"
    );
    assert!(horizon.to_string().contains("3600000"), "{horizon}");
    fixture.call(PEER, "collab_task_register", json!({"id":"no-next"}));
    let sent = fixture.call(MASTER, "collab_sendmessage", json!({"to":peer_id,"subject":"parity-unread","body":"Keep this original body","delivery":"queued"}));
    let before = fixture.state();
    for _ in 0..2 {
        let context = fixture.call(PEER, "collab_context", json!({}));
        let actions = context["next_actions"].as_array().unwrap();
        assert!(
            actions
                .iter()
                .any(|action| action.as_str().unwrap().contains("collab recv")),
            "{context}"
        );
        assert!(
            actions.iter().any(|action| action
                .as_str()
                .unwrap()
                .contains("collab task status no-next")),
            "{context}"
        );
    }
    let after = fixture.state();
    for field in ["tasks", "msgs", "task_lifecycle"] {
        assert_eq!(after[field], before[field], "context mutated {field}");
    }
    let received = fixture.call(
        PEER,
        "collab_recv",
        json!({"timeout":0,"receive_id":"trace-once"}),
    );
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .unwrap();
    assert_eq!(message["body"], "Keep this original body");
    for args in [
        json!({"timeout":0,"receive_id":"trace-once"}),
        json!({"timeout":0}),
    ] {
        let received = fixture.call(PEER, "collab_recv", args);
        assert!(
            received["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message["id"] != sent["msg_id"]),
            "{received}"
        );
    }
    let sent = fixture.call(
        MASTER,
        "collab_sendmessage",
        json!({"to":peer_id,"subject":"parity-rejected-trace","body":"Still unread after a rejected call","delivery":"queued"}),
    );
    let rejected = fixture.raw(PEER, "collab_recv", json!({"timeout":0,"receive_id":" "}));
    assert_eq!(rejected["isError"], true, "{rejected}");
    let received = fixture.call(PEER, "collab_recv", json!({"timeout":0}));
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["id"] == sent["msg_id"]));
}
