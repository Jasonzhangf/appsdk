//! Public local cross-project receipt round trip. No external transport or tmux.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}

impl Fixture {
    fn command(&self, who: &str, args: &[&str]) -> Command {
        let project = if who == "stranger" { "target" } else { who };
        let mut command = Command::new(env!("CARGO_BIN_EXE_collab"));
        command
            .args(args)
            .current_dir(self.root.join(project))
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env("COLLAB_STATE_DIR", self.root.join("host"))
            .env("CODEX_SESSION_ID", format!("{who}-session"))
            .env("CODEX_THREAD_ID", format!("{who}-thread"));
        for key in [
            "TMUX",
            "TMUX_PANE",
            "REX_SERVER",
            "REX_SESSION",
            "REX_BLOCK",
            "COLLAB_SOCKET_PATH",
            "COLLAB_HOST_SOCKET",
            "COLLAB_LOCK_PATH",
            "COLLAB_HOST_LOCK",
            "COLLAB_APPSERVER_SOCKET",
            "CODEX_APP_SERVER_SOCKET",
            "COLLAB_APPSERVER_NAMESPACE",
            "DSH_SESSION_ID",
            "AGY_SESSION_ID",
        ] {
            command.env_remove(key);
        }
        command
    }

    fn run(&self, who: &str, args: &[&str]) -> Output {
        self.command(who, args).output().unwrap()
    }

    fn ok(&self, who: &str, args: &[&str]) -> Value {
        let output = self.run(who, args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn rejects(&self, who: &str, args: &[&str], code: &str) {
        let output = self.run(who, args);
        let text = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success() && text.contains(code),
            "{args:?}: {text}"
        );
    }

    fn state(&self, project: &str) -> Value {
        let routes: Vec<Value> =
            serde_json::from_slice(&fs::read(self.root.join("host/routes.json")).unwrap()).unwrap();
        let root = self.root.join(project).canonicalize().unwrap();
        let route = routes
            .iter()
            .find(|route| route["canonical_root"] == root.to_str().unwrap())
            .unwrap();
        let path = PathBuf::from(route["storage_root"].as_str().unwrap())
            .join(".agent-collab/server/state.json");
        let doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        doc["state"].clone()
    }

    fn start() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "cross-receipt-20261115-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("host")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fixture = Self { root, child: None };
        fs::write(
            fixture.root.join("host/host-state.json"),
            br#"{"schema_version":1,"state":{}}"#,
        )
        .unwrap();
        let mut routes = Vec::new();
        let mut slots = Vec::new();
        for project in ["source", "target"] {
            let path = fixture.root.join(project);
            fs::create_dir_all(path.join(".agent-collab/server")).unwrap();
            let mut workers = serde_json::Map::new();
            let mut bindings = serde_json::Map::new();
            let mut occupancies = serde_json::Map::new();
            let people = if project == "source" {
                vec!["source"]
            } else {
                vec!["target", "stranger"]
            };
            for who in people {
                let worker = format!("{who}-worker");
                let binding = json!({"project_scope":path,"app_scope_id":"appserver-cli",
                    "agent_id":worker,"runtime_id":format!("{who}-runtime"),
                    "binding_id":format!("{who}-binding"),"session_id":format!("{who}-session"),
                    "native_thread_id":format!("{who}-thread")});
                let transport = json!({"kind":"appserver","endpoint":"unix:///tmp/unused-receipt-fixture.sock",
                    "namespace":"codex_tui","session_id":format!("{who}-session"),
                    "thread_id":format!("{who}-thread"),"capabilities":[],"self_check":"static no-transport fixture"});
                workers.insert(
                    worker.clone(),
                    json!({"id":worker,"token":format!("{who}-token"),
                    "cwd":path,"registered_ms":1,"transport":transport}),
                );
                bindings.insert(format!("{who}-binding"), binding.clone());
                let name = if who == "stranger" { "Bob" } else { "Alice" };
                occupancies.insert(
                    name.into(),
                    json!({"name":name, "worker_id":worker,
                    "session_id":format!("{who}-session"), "occupancy_revision":1, "updated_ms":1}),
                );
                slots.push(json!({"identity_key":{"project_scope":path,"app_scope_id":"appserver-cli",
                    "name":if who == "stranger" {"Bob"} else {"Alice"}},
                    "display_slot":if who == "stranger" {3} else {2},"identity_status":"active","generation":1,"previous":null,
                    "current":{"binding":binding,"transport":transport,
                        "tab_id":{"project_scope":path,"app_scope_id":"appserver-cli"},
                        "host":{"host_type":"native","agent_kind":null,"profile":null},
                        "invocation_cwd":path,"physical_name":null}}));
                let identity = fixture.root.join("host/identities").join(&worker);
                fs::create_dir_all(&identity).unwrap();
                fs::write(
                    identity.join("identity.json"),
                    serde_json::to_vec(&json!({
                        "worker_id":worker,"token":format!("{who}-token"),"project_scope":path
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            fs::write(path.join(".agent-collab/server/state.json"), serde_json::to_vec(&json!({"schema_version":1,"state":{
                "workers":workers,"name_occupancies":{(path.to_str().unwrap()):occupancies},"global":{"epoch":1,"sequence":0,"revision":0,"projects":{
                    (path.to_str().unwrap()):{"project_scope":path,
                        "registrations":{"appserver-cli":{"project_scope":path,"app_scope_id":"appserver-cli"}},
                        "runtime_bindings":bindings}}
                }}})).unwrap()).unwrap();
            routes.push(json!({"project_scope":path,"canonical_root":path,"app_scope_id":"appserver-cli","storage_root":path,"registered_ms":1}));
        }
        fs::write(
            fixture.root.join("host/routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
        fs::write(
            fixture.root.join("host/identity-map.json"),
            serde_json::to_vec(&json!({"schema_version":1,"slots":slots})).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.root.join("neutral")).unwrap();
        let log = fs::File::create(fixture.root.join("host/test-daemon.log")).unwrap();
        fixture.child = Some(
            fixture
                .command("source", &["serve"])
                .current_dir(fixture.root.join("neutral"))
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                fixture
                    .child
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none(),
                "isolated daemon exited"
            );
            if let Ok(mut socket) = UnixStream::connect(fixture.root.join("host/server.sock")) {
                socket
                    .set_read_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                socket.write_all(b"{\"op\":\"Ping\"}\n").unwrap();
                let mut line = String::new();
                if BufReader::new(socket).read_line(&mut line).is_ok()
                    && serde_json::from_str::<Value>(&line).is_ok_and(|v| v["ok"] == true)
                {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "isolated daemon readiness timeout"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if std::thread::panicking() {
            eprintln!("fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn cross_project_sender_receipt_round_trip() {
    let f = Fixture::start();
    let sent = f.ok(
        "source",
        &[
            "send",
            "target.Alice",
            "--subject",
            "receipt test",
            "original body",
        ],
    );
    let id = sent["msg_id"].as_str().unwrap();
    // Before the fix this uses the returned short ID and exposes MESSAGE_NOT_FOUND.
    let reference = sent["message_ref"].as_str().unwrap_or(id);
    let status = f.ok("source", &["msg", reference]);
    assert_eq!(status["body"], "original body");
    assert_eq!(status["answered"], false);
    assert_eq!(status["consumed_by_recv"], false);
    assert!(reference.starts_with("collab-ref:"));
    let credential_path = f.root.join("host/identities/source-worker/identity.json");
    let credential_bytes = fs::read(&credential_path).unwrap();
    let mut credential: Value = serde_json::from_slice(&credential_bytes).unwrap();
    credential["token"] = json!("wrong-token");
    fs::write(&credential_path, serde_json::to_vec(&credential).unwrap()).unwrap();
    f.rejects("source", &["msg", reference], "token mismatch");
    f.rejects(
        "source",
        &["send", "target.Alice", "--subject", "bad credential", "no"],
        "token mismatch",
    );
    fs::write(&credential_path, credential_bytes).unwrap();
    assert!(
        f.state("source")["msgs"][id].is_null(),
        "target-only original mailbox"
    );
    assert_eq!(f.state("target")["msgs"][id]["body"], "original body");
    f.rejects("source", &["msg", id], "MESSAGE_NOT_FOUND");
    f.rejects("stranger", &["msg", reference], "CALLER_NOT_PERMITTED");
    f.rejects("stranger", &["msg", id], "CALLER_NOT_PERMITTED");
    assert_eq!(f.ok("target", &["msg", reference])["id"], id);
    assert_eq!(f.ok("target", &["msg", id])["id"], id);
    f.rejects(
        "stranger",
        &[
            "send",
            "source.Alice",
            "--subject",
            "forged reply",
            "no",
            "--in-reply-to",
            reference,
        ],
        "IN_REPLY_TO_PARTICIPANT_MISMATCH",
    );
    f.rejects(
        "target",
        &[
            "send",
            "source.Alice",
            "--subject",
            "bad reply",
            "no",
            "--in-reply-to",
            "missing-message",
        ],
        "IN_REPLY_TO_NOT_FOUND",
    );
    let mut wrong_scope: Value =
        serde_json::from_str(reference.strip_prefix("collab-ref:").unwrap()).unwrap();
    wrong_scope["value"]["project_scope"] = json!(f.root.join("source"));
    let wrong_scope = format!("collab-ref:{wrong_scope}");
    f.rejects(
        "target",
        &[
            "send",
            "source.Alice",
            "--subject",
            "wrong owner",
            "no",
            "--in-reply-to",
            &wrong_scope,
        ],
        "IN_REPLY_TO_OWNER_MISMATCH",
    );
    assert_eq!(
        f.state("source")["msgs"].as_object().map_or(0, |m| m.len()),
        0,
        "rejected sends commit no replies"
    );
    assert_eq!(
        f.state("target")["msgs"].as_object().unwrap().len(),
        1,
        "rejected sends commit no originals"
    );
    assert_eq!(f.ok("source", &["msg", reference])["answered"], false);
    assert_eq!(
        f.ok("source", &["msg", reference])["consumed_by_recv"],
        false
    );
    let received = f.ok("target", &["recv"]);
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == id),
        "{received}"
    );
    assert_eq!(
        f.ok("source", &["msg", reference])["consumed_by_recv"],
        true
    );
    let reply = f.ok(
        "target",
        &[
            "send",
            "source.Alice",
            "--subject",
            "valid reply",
            "reply body",
            "--in-reply-to",
            reference,
        ],
    );
    let reply_id = reply["msg_id"].as_str().unwrap();
    let reply_ref = reply["message_ref"].as_str().unwrap();
    assert_eq!(
        f.state("source")["msgs"][reply_id]["in_reply_to"],
        reference
    );
    assert!(
        f.state("target")["msgs"][reply_id].is_null(),
        "reverse target-only reply mailbox"
    );
    assert_eq!(f.ok("target", &["msg", reply_ref])["body"], "reply body");
    for who in ["source", "target"] {
        assert_eq!(f.ok(who, &["msg", reference])["answered"], true);
    }
    assert_eq!(f.ok("target", &["msg", id])["answered"], true);
    f.rejects("stranger", &["msg", reply_ref], "CALLER_NOT_PERMITTED");
    let received_reply = f.ok("source", &["recv"]);
    assert!(received_reply["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == reply_id));
    assert_eq!(
        f.ok("target", &["msg", reply_ref])["consumed_by_recv"],
        true
    );
    // Legacy raw reply IDs remain valid, with the same participant validation.
    let legacy = f.ok(
        "target",
        &[
            "send",
            "source.Alice",
            "--subject",
            "legacy reply",
            "legacy body",
            "--in-reply-to",
            id,
        ],
    );
    assert_eq!(
        f.state("source")["msgs"][legacy["msg_id"].as_str().unwrap()]["in_reply_to"],
        id
    );
}
