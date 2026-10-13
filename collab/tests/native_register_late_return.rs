//! Isolated native participants. Challenges arrive only through the selected
//! production adaptor; returns carry this consumer's own control facts.
include!("support/context_operation_fixture.rs");

struct NativeParticipant {
    socket: PathBuf,
    kind: &'static str,
    root: PathBuf,
    session: String,
    thread: String,
    received: Receiver<String>,
    dispatches: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl NativeParticipant {
    fn start(fixture: &Fixture, kind: &'static str) -> Self {
        Self::start_named(fixture, kind, kind)
    }

    fn start_named(fixture: &Fixture, kind: &'static str, name: &str) -> Self {
        let socket = fixture.root.join(format!("{name}.sock"));
        let listener = UnixListener::bind(&socket).expect("bind isolated selected native adaptor");
        let root = fixture.root.canonicalize().unwrap();
        let session = format!("{name}-session");
        let thread = format!("{name}-participant");
        let (send, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let dispatches = Arc::new(AtomicU64::new(0));
        let worker_stop = stop.clone();
        let worker_dispatches = dispatches.clone();
        let worker_root = root.clone();
        let worker_session = session.clone();
        let worker_thread = thread.clone();
        let handle = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if worker_stop.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                if kind == "appserver" {
                    let mut header = Vec::new();
                    let mut byte = [0];
                    while !header.ends_with(b"\r\n\r\n") {
                        if stream.read_exact(&mut byte).is_err() {
                            return;
                        }
                        header.push(byte[0]);
                    }
                    stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
                }
                loop {
                    let request: Value = if kind == "appserver" {
                        match read_appserver_frame(&mut stream) {
                            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap(),
                            _ => break,
                        }
                    } else {
                        let mut line = String::new();
                        if BufReader::new(&stream).read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        serde_json::from_str(&line).unwrap()
                    };
                    let params = &request["params"];
                    let result = match request["method"].as_str().unwrap_or("") {
                        "initialize" => json!({"capabilities":{}}),
                        "thread/read" => {
                            json!({"thread":{"id":worker_thread,"sessionId":worker_session,
                            "cwd":worker_root,"status":{"type":"idle"},"turns":[]}})
                        }
                        "items/list" => json!({"items":[]}),
                        // Admission checks method existence using empty
                        // threadId. It is not a participant dispatch.
                        "turn/start" | "turn/steer" if params["threadId"] == "" => json!({}),
                        "turn/start" | "turn/steer" => {
                            assert_eq!(params["threadId"], worker_thread);
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
                            worker_dispatches.fetch_add(1, Ordering::SeqCst);
                            send.send(prompt).unwrap();
                            json!({"turn":{"id":"selected-turn","status":"inProgress"}})
                        }
                        "agent-facts" => {
                            json!({"nonce":params["nonce"],"runtimeId":"selected-runtime",
                            "agentId":worker_thread,"sessionId":worker_session,"cwd":worker_root,"status":"running"})
                        }
                        "enqueue" => {
                            assert_eq!(params["agentId"], worker_thread);
                            worker_dispatches.fetch_add(1, Ordering::SeqCst);
                            send.send(params["content"][0]["text"].as_str().unwrap().to_owned())
                                .unwrap();
                            json!({"messageId":params["messageId"],"runtimeId":"selected-runtime","agentId":worker_thread})
                        }
                        "agent-reply" => json!({"result":{"state":"pending"}}),
                        _ if request.get("id").is_none() => continue,
                        _ => json!({}),
                    };
                    let response = if kind == "appserver" {
                        json!({"id":request["id"],"result":result})
                    } else {
                        json!({"ok":true,"result":result})
                    };
                    if kind == "appserver" {
                        if write_appserver_frame(
                            &mut stream,
                            &serde_json::to_vec(&response).unwrap(),
                        )
                        .is_err()
                        {
                            break;
                        }
                    } else {
                        writeln!(stream, "{response}").unwrap();
                        break;
                    }
                }
            }
        });
        Self {
            socket,
            kind,
            root,
            session,
            thread,
            received,
            dispatches,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    fn native_facts(&self) -> Value {
        if self.kind == "appserver" {
            json!({"kind":"app_server","candidate":{
            "endpoint":self.endpoint(),"namespace":"codex_tui","session_id":self.session,"thread_id":self.thread,"cwd":self.root}})
        } else {
            json!({"kind":"dsh","candidate":{"endpoint":self.endpoint(),"runtime_id":"selected-runtime",
            "session_id":self.session,"agent_id":self.thread,"cwd":self.root}})
        }
    }

    fn challenge(&self) -> Value {
        let prompt = self
            .received
            .recv_timeout(Duration::from_secs(10))
            .expect("selected native participant receives challenge");
        let start = prompt
            .find("COLLAB_REGISTER_CHALLENGE ")
            .expect("native challenge must permit a future typed return");
        let raw = &prompt[start + "COLLAB_REGISTER_CHALLENGE ".len()..];
        let mut values = serde_json::Deserializer::from_str(raw).into_iter::<Value>();
        values.next().unwrap().unwrap()
    }

    fn return_reply(&self, fixture: &Fixture, challenge: &Value) -> Value {
        if self.kind == "appserver" {
            let output = fixture
                .configured_command(
                    &["context", "--return-challenge", &challenge.to_string()],
                    None,
                )
                .env("CODEX_SESSION_ID", &self.session)
                .env("CODEX_THREAD_ID", &self.thread)
                .env("COLLAB_APPSERVER_SOCKET", &self.socket)
                .env("COLLAB_APPSERVER_NAMESPACE", "codex_tui")
                .output()
                .expect("selected AppServer participant returns through public CLI");
            return serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| panic!("native CLI reply JSON {error}: {output:?}"));
        }
        let mut reply = challenge.clone();
        reply
            .as_object_mut()
            .unwrap()
            .insert("native".into(), self.native_facts());
        public_request(
            fixture,
            json!({"op":"RegisterReply","reply":reply,"project_context":project_context(fixture)}),
        )
    }
}

impl Drop for NativeParticipant {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
        if self.handle.take().unwrap().join().is_err() {
            eprintln!("selected native fixture failed; preserving the original panic");
        }
    }
}

fn project_context(fixture: &Fixture) -> Value {
    let root = fixture.root.canonicalize().unwrap();
    json!({"app_scope_id":"appserver-cli","canonical_root":root,"project_scope":root})
}

fn public_request(fixture: &Fixture, request: Value) -> Value {
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn effects(fixture: &Fixture) -> [usize; 2] {
    std::fs::read_to_string(fixture.project_journal())
        .unwrap()
        .lines()
        .fold([0, 0], |mut n, line| {
            let ev: Value = serde_json::from_str(line).unwrap();
            match ev["ev"].as_str() {
                Some("Registered") => n[0] += 1,
                Some("GlobalRuntimeBound") => n[1] += 1,
                _ => {}
            }
            n
        })
}

fn late_return(kind: &'static str, restart: bool, cancel: bool, wrong: bool) {
    let mut fixture = Fixture::without_tmux(kind);
    let native = NativeParticipant::start(&fixture, kind);
    let pending = if kind == "appserver" {
        let facts = json!({"session_id":native.session,"thread_id":native.thread,"endpoint":native.endpoint(),"namespace":"codex_tui"});
        let output = fixture.command(
            &[
                "context",
                "--op",
                "native-late",
                "--provide",
                &facts.to_string(),
            ],
            None,
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    } else {
        fixture.run_ok(&["up"], None);
        public_request(
            &fixture,
            json!({"op":"Register","worker_id":"native-worker","token":"isolated-credential",
            "cwd":fixture.root,"candidates":{"dsh":native.native_facts()["candidate"]},"project_context":project_context(&fixture)}),
        )
    };
    assert_ne!(pending["result"]["outcome"], "completed", "{pending}");
    assert_eq!(effects(&fixture), [0, 0]);
    let challenge = native.challenge();
    if kind == "appserver" {
        let before =
            std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
        let journal = std::fs::read(fixture.project_journal()).unwrap();
        let query = fixture.command(&["context", "--op", "native-late", "--query"], None);
        assert!(!query.stdout.is_empty(), "{query:?}");
        assert_eq!(
            std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap(),
            before
        );
        assert_eq!(std::fs::read(fixture.project_journal()).unwrap(), journal);
    }
    if wrong {
        for field in [
            "operation_id",
            "command_id",
            "purpose",
            "challenge",
            "native",
        ] {
            let mut reply = challenge.clone();
            reply
                .as_object_mut()
                .unwrap()
                .insert("native".into(), native.native_facts());
            if field == "purpose" {
                reply[field] = json!("manual_communication_restore");
            } else if field == "native" {
                reply["native"]["candidate"]["session_id"] = json!("wrong-native-session");
            } else {
                reply[field] = json!("wrong-associated-value");
            }
            let rejected = public_request(
                &fixture,
                json!({"op":"RegisterReply","reply":reply,"project_context":project_context(&fixture)}),
            );
            assert_eq!(rejected["ok"], false, "{field}: {rejected}");
            assert_eq!(effects(&fixture), [0, 0]);
        }
        let target_fields = if kind == "appserver" {
            vec!["endpoint", "namespace", "thread_id", "cwd"]
        } else {
            vec!["endpoint", "runtime_id", "agent_id", "cwd"]
        };
        for field in target_fields {
            let mut reply = challenge.clone();
            reply
                .as_object_mut()
                .unwrap()
                .insert("native".into(), native.native_facts());
            reply["native"]["candidate"][field] = json!(if field == "endpoint" {
                "unix:///tmp/wrong-selected-target.sock"
            } else if field == "cwd" {
                "/tmp"
            } else {
                "wrong-selected-target"
            });
            let rejected = public_request(
                &fixture,
                json!({"op":"RegisterReply","reply":reply,
                "project_context":project_context(&fixture)}),
            );
            assert_eq!(rejected["ok"], false, "{field}: {rejected}");
            assert_eq!(effects(&fixture), [0, 0]);
        }
        let other = fixture.root.join("other");
        seed_baseline(&other);
        let mut reply = challenge.clone();
        reply
            .as_object_mut()
            .unwrap()
            .insert("native".into(), native.native_facts());
        let rejected = public_request(
            &fixture,
            json!({"op":"RegisterReply","reply":reply,"project_context":{
            "app_scope_id":"appserver-cli","canonical_root":other,"project_scope":other}}),
        );
        assert_eq!(rejected["ok"], false, "{rejected}");
        assert_eq!(effects(&fixture), [0, 0]);
    }
    if cancel {
        let proof = fixture.proof("native-late");
        let cancelled = public_request(
            &fixture,
            json!({"op":"IdentityContext","project_context":project_context(&fixture),
            "facts":{},"identity_context":{"operation_id":"native-late","invocation":"automatic",
            "action":"cancel","query_capability":proof["query_capability"]}}),
        );
        assert_eq!(cancelled["ok"], true, "{cancelled}");
    }
    let frozen = std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
    if restart {
        fixture.restart_daemon();
        assert_eq!(
            std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap(),
            frozen
        );
    }
    let returned = native.return_reply(&fixture, &challenge);
    if cancel {
        assert_eq!(returned["ok"], false, "{returned}");
        assert!(
            returned["error"].as_str().unwrap().contains("CANCELLED"),
            "{returned}"
        );
        assert_eq!(effects(&fixture), [0, 0]);
        assert_eq!(native.dispatches.load(Ordering::SeqCst), 1);
        return;
    }
    assert_eq!(returned["ok"], true, "{returned}");
    if kind == "appserver" {
        assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
    }
    assert_eq!(effects(&fixture), [1, 1]);
    assert_eq!(native.return_reply(&fixture, &challenge)["ok"], true);
    assert_eq!(effects(&fixture), [1, 1]);
    assert_eq!(native.dispatches.load(Ordering::SeqCst), 1);
}

#[test]
fn appserver_actual_late_return_completes_original_operation() {
    late_return("appserver", false, false, false);
}

#[test]
fn dsh_actual_late_return_completes_original_operation() {
    late_return("dsh", false, false, false);
}

#[test]
fn appserver_restart_late_return_and_query_preserve_exact_command() {
    late_return("appserver", true, false, false);
}
#[test]
fn dsh_restart_late_return_preserves_exact_command() {
    late_return("dsh", true, false, false);
}
#[test]
fn appserver_cancelled_return_after_restart_never_commits() {
    late_return("appserver", true, true, false);
}
#[test]
fn appserver_wrong_native_association_then_real_return() {
    late_return("appserver", false, false, true);
}
#[test]
fn dsh_wrong_native_association_then_real_return() {
    late_return("dsh", false, false, true);
}

fn pending_native(
    fixture: &Fixture,
    native: &NativeParticipant,
    operation: &str,
    worker: &str,
) -> Value {
    if native.kind == "appserver" {
        let facts = json!({"session_id":native.session,"thread_id":native.thread,"endpoint":native.endpoint(),"namespace":"codex_tui"});
        let output = fixture.command(
            &[
                "context",
                "--op",
                operation,
                "--provide",
                &facts.to_string(),
            ],
            None,
        );
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        public_request(
            fixture,
            json!({"op":"Register","worker_id":worker,"token":format!("isolated-{worker}"),
            "cwd":fixture.root,"candidates":{"dsh":native.native_facts()["candidate"]},"project_context":project_context(fixture)}),
        )
    }
}

fn classified_native_pair(kind: &'static str) {
    let fixture = Fixture::without_tmux("native-classification");
    let first = NativeParticipant::start_named(&fixture, kind, "first");
    let second = NativeParticipant::start_named(&fixture, kind, "second");
    if kind == "dsh" {
        fixture.run_ok(&["up"], None);
    }
    let initial = pending_native(&fixture, &first, "first-native", "first-worker");
    assert_ne!(initial["ok"], true, "{initial}");
    let a = first.challenge();
    let pending = pending_native(&fixture, &second, "second-native", "second-worker");
    assert_ne!(pending["ok"], true, "{pending}");
    let b = second.challenge();
    let frozen =
        std::fs::read_to_string(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
    let original: Value = frozen
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["operation_id"] == b["operation_id"])
        .last()
        .unwrap()["prepared"]
        .clone();
    assert_eq!(first.return_reply(&fixture, &a)["ok"], true);
    assert!(
        wait_until(Duration::from_secs(15), || std::fs::read_to_string(
            fixture.project_journal()
        )
        .unwrap()
        .contains("GlobalRuntimeBindingLedgerClassified")),
        "first native binding classification missing"
    );
    assert_eq!(effects(&fixture), [1, 1]);
    let returned = second.return_reply(&fixture, &b);
    assert_eq!(returned["ok"], true, "{returned}");
    assert_eq!(effects(&fixture), [2, 2]);
    let records =
        std::fs::read_to_string(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
    let final_prepared: Value = records
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["operation_id"] == b["operation_id"])
        .last()
        .unwrap()["prepared"]
        .clone();
    assert_eq!(original, final_prepared);
    assert_eq!(second.return_reply(&fixture, &b)["ok"], true);
    assert_eq!(effects(&fixture), [2, 2]);
    assert_eq!(first.dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(second.dispatches.load(Ordering::SeqCst), 1);
}

#[test]
fn appserver_pending_return_survives_other_binding_classification() {
    classified_native_pair("appserver");
}
#[test]
fn dsh_pending_return_survives_other_binding_classification() {
    classified_native_pair("dsh");
}

#[test]
fn appserver_manual_late_return_preserves_restored_identity_and_binding() {
    let fixture = Fixture::without_tmux("native-manual");
    let native = NativeParticipant::start(&fixture, "appserver");
    let pending = pending_native(&fixture, &native, "native-initial", "unused");
    assert_eq!(pending["ok"], false, "{pending}");
    let initial = native.challenge();
    let registered = native.return_reply(&fixture, &initial);
    assert_eq!(registered["ok"], true, "{registered}");
    assert_eq!(effects(&fixture), [1, 1]);
    let facts = json!({"session_id":native.session,"thread_id":native.thread,
        "endpoint":native.endpoint(),"namespace":"codex_tui"});
    let restored = fixture.run_ok(
        &[
            "context",
            "--op",
            "native-restored",
            "--provide",
            &facts.to_string(),
        ],
        None,
    );
    let identity = restored["result"]["snapshot"]["identity"].clone();
    assert!(identity.is_object(), "{restored}");
    assert_eq!(native.dispatches.load(Ordering::SeqCst), 1);
    let manual = fixture.command(
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
    let pending: Value = serde_json::from_slice(&manual.stdout).unwrap();
    assert_eq!(pending["ok"], false, "{pending}");
    assert_eq!(effects(&fixture), [1, 1]);
    let challenge = native.challenge();
    assert_eq!(challenge["purpose"], "manual_communication_restore");
    let mut wrong = challenge.clone();
    wrong["purpose"] = json!("initial_register");
    assert_eq!(native.return_reply(&fixture, &wrong)["ok"], false);
    assert_eq!(effects(&fixture), [1, 1]);
    let returned = native.return_reply(&fixture, &challenge);
    assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
    let completed = fixture.run_ok(
        &[
            "context",
            "--op",
            "native-after-manual",
            "--provide",
            &facts.to_string(),
        ],
        None,
    );
    assert_eq!(
        completed["result"]["snapshot"]["identity"], identity,
        "{completed}"
    );
    assert_eq!(effects(&fixture), [1, 1]);
    assert_eq!(native.dispatches.load(Ordering::SeqCst), 2);
}
