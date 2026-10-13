static SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch(name: &str) -> PathBuf {
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("collab-dsh-{name}-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create dsh fixture directory");
    path
}

/// A fake dsh-gateway control plane: NDJSON over a UNIX socket.
///
/// `respond` receives one parsed request and returns the single reply line, or
/// `None` to accept the connection and stay silent. `None` is how a hanging
/// gateway is modelled; the connection is held open until the client gives up.
struct Gateway {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Gateway {
    fn start(
        name: &str,
        respond: impl Fn(serde_json::Value) -> Option<String> + Send + 'static,
    ) -> Self {
        let dir = scratch(name);
        let socket = dir.join("control.sock");
        let listener = StdUnixListener::bind(&socket).expect("bind control socket");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
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
                let request: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("control request is JSON");
                match respond(request) {
                    Some(reply) => {
                        let mut stream = &stream;
                        let _ = stream.write_all(reply.as_bytes());
                        let _ = stream.write_all(b"\n");
                        let _ = stream.flush();
                    }
                    None => {
                        // Hold the connection open and never answer: the client
                        // must decide on its own timeout.
                        let mut buffer = [0u8; 64];
                        let source = reader.get_mut();
                        while matches!(source.read(&mut buffer), Ok(read) if read > 0) {}
                    }
                }
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
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake a pending accept so the loop can observe the stop flag.
        let _ = StdUnixStream::connect(&self.socket);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn candidate(socket: &Path, root: &Path) -> DshCandidate {
    candidate_ids(socket, root, "rt-1", "ses-1")
}

fn candidate_ids(socket: &Path, root: &Path, runtime_id: &str, agent_id: &str) -> DshCandidate {
    DshCandidate {
        endpoint: format!("unix://{}", socket.display()),
        runtime_id: runtime_id.into(),
        agent_id: agent_id.into(),
        session_id: agent_id.into(),
        cwd: root.display().to_string(),
    }
}

/// A gateway answering `agent-facts` for whichever agent was asked about, the
/// way a real gateway projects the registry record for the requested id.
fn facts_reply(request: &serde_json::Value, cwd: &Path) -> String {
    let nonce = request
        .pointer("/params/nonce")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let runtime_id = request
        .pointer("/params/runtimeId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("rt-1");
    let agent_id = request
        .pointer("/params/agentId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ses-1");
    serde_json::json!({
        "ok": true,
        "result": {
            "nonce": nonce,
            "runtimeId": runtime_id,
            "agentId": agent_id,
            // The design projects sessionId from agentId; it is not stored twice.
            "sessionId": agent_id,
            "cwd": cwd.display().to_string(),
            "status": "running",
        }
    })
    .to_string()
}

fn registration_challenge(request: &serde_json::Value) -> Option<serde_json::Value> {
    let text = request
        .pointer("/params/content/0/text")
        .and_then(serde_json::Value::as_str)?;
    let raw = text.strip_prefix("COLLAB_REGISTER_CHALLENGE ")?;
    serde_json::from_str(raw).ok()
}

fn is_registration_enqueue(request: &serde_json::Value) -> bool {
    request["method"] == "enqueue" && registration_challenge(request).is_some()
}

fn is_wake_enqueue(request: &serde_json::Value) -> bool {
    request["method"] == "enqueue" && !is_registration_enqueue(request)
}

/// A gateway that answers the real registration challenge and can also serve
/// wake traffic. The challenge marker and participant identity come from the
/// actual enqueue request; no private pending record is read.
fn registration_gateway<W>(
    name: &str,
    root: &Path,
    seen: Option<Arc<Mutex<Vec<serde_json::Value>>>>,
    wake: W,
) -> Gateway
where
    W: Fn() -> Option<String> + Send + 'static,
{
    let root = root.to_path_buf();
    let registrations: Arc<Mutex<HashMap<String, (String, String, String)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    Gateway::start(name, move |request| {
        if let Some(seen) = &seen {
            seen.lock().unwrap().push(request.clone());
        }
        match request["method"].as_str().unwrap_or_default() {
            "agent-facts" => Some(facts_reply(&request, &root)),
            "enqueue" => {
                if let Some(challenge) = registration_challenge(&request) {
                    let message_id = request["params"]["messageId"].as_str()?.to_owned();
                    let runtime_id = request["params"]["runtimeId"].as_str()?.to_owned();
                    let agent_id = request["params"]["agentId"].as_str()?.to_owned();
                    let marker = challenge["challenge"].as_str()?.to_owned();
                    registrations.lock().unwrap().insert(
                        message_id.clone(),
                        (runtime_id.clone(), agent_id.clone(), marker),
                    );
                    Some(
                        serde_json::json!({
                            "ok": true,
                            "result": {
                                "messageId": message_id,
                                "runtimeId": runtime_id,
                                "agentId": agent_id,
                            }
                        })
                        .to_string(),
                    )
                } else {
                    wake()
                }
            }
            "agent-reply" => {
                let message_id = request["params"]["messageId"].as_str()?.to_owned();
                let registration = registrations.lock().unwrap().get(&message_id).cloned();
                match registration {
                    Some((_runtime_id, agent_id, marker)) => Some(
                        serde_json::json!({
                            "ok": true,
                            "result": {
                                // `dsh::agent_reply` reads the gateway result and
                                // then its nested `result`, matching the genuine
                                // participant fixture in
                                // `collab/tests/native_register_late_return.rs`.
                                "result": {
                                    "state": "replied",
                                    "agentId": agent_id,
                                    "messageId": message_id,
                                    "sessionId": agent_id,
                                    "turn": 1,
                                    "step": 1,
                                    "inputSeq": 1,
                                    "outputSeq": 2,
                                    "completedSeq": 3,
                                    "text": marker,
                                }
                            }
                        })
                        .to_string(),
                    ),
                    None => Some(
                        serde_json::json!({
                            "ok": true,
                            "result": {"result": {"state": "pending"}}
                        })
                        .to_string(),
                    ),
                }
            }
            _ => None,
        }
    })
}

fn wake_reply(message_id: &str) -> Option<String> {
    Some(
        serde_json::json!({
            "ok": true,
            "result": {"messageId": message_id, "runtimeId": "rt-1", "agentId": "ses-1"}
        })
        .to_string(),
    )
}

fn test_server(root: &Path) -> Arc<Server> {
    let server_dir = root.join(".agent-collab").join("server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal_path = server_dir.join("journal.jsonl");
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
        .unwrap();
    Arc::new(Server {
        config: crate::config::Config::default(),
        root: root.to_path_buf(),
        storage_root: root.to_path_buf(),
        journal_path,
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        // dsh validation must never consult the App Server candidate check.
        appserver_candidate_check: Arc::new(|_| {
            Err("APPSERVER_CHECK_REACHED: dsh validation must not consult it".into())
        }),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
            Ok(serde_json::json!({"accepted": true}))
        }),
        appserver_thread_status: Arc::new(|_, _| Ok(serde_json::json!({}))),
        appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({}))),
        mailbox_notify: Notify::new(),
    })
}
