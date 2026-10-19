//! Late participant return belongs only to explicit communication restoration.
include!("support/context_operation_fixture.rs");

struct NativeParticipant {
    socket: PathBuf,
    received: Receiver<String>,
    dispatches: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl NativeParticipant {
    fn start(fixture: &Fixture) -> Self {
        let socket = fixture.root.join("native.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let root = fixture.root.canonicalize().unwrap();
        let (send, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let dispatches = Arc::new(AtomicU64::new(0));
        let worker_stop = stop.clone();
        let worker_dispatches = dispatches.clone();
        let handle = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if worker_stop.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    if stream.read_exact(&mut byte).is_err() {
                        return;
                    }
                    header.push(byte[0]);
                }
                stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
                while let Ok(Some(bytes)) = read_appserver_frame(&mut stream) {
                    let request: Value = serde_json::from_slice(&bytes).unwrap();
                    let params = &request["params"];
                    let result = match request["method"].as_str().unwrap_or("") {
                        "initialize" => json!({"capabilities":{}}),
                        "thread/read" => {
                            json!({"thread":{"id":"native-thread","sessionId":"native-session","cwd":root,"status":{"type":"idle"},"turns":[]}})
                        }
                        "items/list" => json!({"items":[]}),
                        "turn/start" | "turn/steer" if params["threadId"] == "" => json!({}),
                        "turn/start" | "turn/steer" => {
                            assert_eq!(params["threadId"], "native-thread");
                            worker_dispatches.fetch_add(1, Ordering::SeqCst);
                            let prompt = format!(
                                "{}\n{}",
                                params
                                    .pointer("/input/0/text")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                                params
                                    .pointer("/toolOutput/output")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                            );
                            send.send(prompt).unwrap();
                            json!({"turn":{"id":"manual-turn","status":"inProgress"}})
                        }
                        _ if request.get("id").is_none() => continue,
                        _ => json!({}),
                    };
                    if write_appserver_frame(
                        &mut stream,
                        &serde_json::to_vec(&json!({"id":request["id"],"result":result})).unwrap(),
                    )
                    .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            socket,
            received,
            dispatches,
            stop,
            handle: Some(handle),
        }
    }

    fn return_reply(&self, fixture: &Fixture, challenge: &Value) -> Value {
        let output = fixture
            .configured_command(
                &["context", "--return-challenge", &challenge.to_string()],
                None,
            )
            .env("CODEX_SESSION_ID", "native-session")
            .env("CODEX_THREAD_ID", "native-thread")
            .env("COLLAB_APPSERVER_SOCKET", &self.socket)
            .env("COLLAB_APPSERVER_NAMESPACE", "codex_tui")
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
    }
}

impl Drop for NativeParticipant {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
        if self.handle.take().unwrap().join().is_err() {
            eprintln!("native fixture failed; preserve original panic");
        }
    }
}

#[test]
fn appserver_manual_late_return_preserves_restored_identity_and_binding() {
    let mut fixture = Fixture::without_tmux("native-manual");
    let native = NativeParticipant::start(&fixture);
    let facts = json!({"session_id":"native-session","thread_id":"native-thread",
        "endpoint":format!("unix://{}", native.socket.display()),"namespace":"codex_tui"});
    let registered = fixture.context_provide("native-initial", &facts, None);
    assert_eq!(registered["result"]["outcome"], "completed", "{registered}");
    let identity = registered["result"]["snapshot"]["identity"].clone();
    let binding = registered["result"]["snapshot"]["binding"].clone();
    assert_eq!(native.dispatches.load(Ordering::SeqCst), 0);
    assert!(!fixture
        .host_state
        .join("register-reply-evidence.jsonl")
        .exists());
    let journal_path = fixture.project_journal();
    let registration_events = || {
        std::fs::read_to_string(&journal_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| {
                matches!(
                    event["ev"].as_str(),
                    Some("Registered" | "GlobalRuntimeBound")
                )
            })
            .collect::<Vec<_>>()
    };
    let before = registration_events();
    assert_eq!(before.len(), 2);
    let output = fixture.command(
        &[
            "context",
            "--op",
            "native-manual",
            "--recover-communication",
            "--provide",
            &facts.to_string(),
        ],
        None,
    );
    let pending: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(pending["ok"], false, "{pending}");
    let prompt = native
        .received
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let start = prompt.find("COLLAB_REGISTER_CHALLENGE ").unwrap();
    let challenge =
        serde_json::Deserializer::from_str(&prompt[start + "COLLAB_REGISTER_CHALLENGE ".len()..])
            .into_iter::<Value>()
            .next()
            .unwrap()
            .unwrap();
    assert_eq!(challenge["purpose"], "manual_communication_restore");
    // A prior daemon wrote these retired registration fields to the same
    // evidence journal. They must not revive registration or block manual restore.
    let evidence_path = fixture.host_state.join("register-reply-evidence.jsonl");
    let mut legacy: Value = serde_json::from_str(
        std::fs::read_to_string(&evidence_path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    legacy["completion"] = Value::Null;
    legacy["publication_outcome"] = Value::Null;
    let mut retired = legacy.clone();
    retired["purpose"] = json!("initial_register");
    retired["operation_id"] = json!("retired-initial");
    let mut history = std::fs::OpenOptions::new()
        .append(true)
        .open(&evidence_path)
        .unwrap();
    writeln!(history, "{retired}\n{legacy}").unwrap();
    history.sync_all().unwrap();
    let mut wrong = challenge.clone();
    wrong["challenge"] = json!("wrong-nonce");
    assert_eq!(native.return_reply(&fixture, &wrong)["ok"], false);
    fixture.restart_daemon();
    let returned = native.return_reply(&fixture, &challenge);
    assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
    assert_eq!(
        native.return_reply(&fixture, &challenge)["result"]["outcome"],
        "completed"
    );
    let after = fixture.context_provide("native-after", &facts, None);
    assert_eq!(after["result"]["snapshot"]["identity"], identity);
    assert_eq!(after["result"]["snapshot"]["binding"], binding);
    assert_eq!(registration_events(), before);
    assert_eq!(native.dispatches.load(Ordering::SeqCst), 1);
}
