include!("support/context_operation_fixture.rs");

struct DesktopBridgeFixture {
    endpoint: String,
    sends: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    disconnect_send: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DesktopBridgeFixture {
    fn start(root: &Path) -> Self {
        let socket = root.join(format!(
            "desktop-{}.sock",
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let sends = Arc::new(Mutex::new(Vec::new()));
        let stopping = stop.clone();
        let sent = sends.clone();
        let disconnect_send = Arc::new(AtomicBool::new(false));
        let disconnecting = disconnect_send.clone();
        let cwd = root.canonicalize().unwrap();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut header = [0; 4];
                        if stream.read_exact(&mut header).is_err() {
                            continue;
                        }
                        let length = u32::from_le_bytes(header) as usize;
                        assert!(length <= 8 * 1024 * 1024);
                        let mut bytes = vec![0; length];
                        stream.read_exact(&mut bytes).unwrap();
                        let request: Value = serde_json::from_slice(&bytes).unwrap();
                        let native = if request["op"] == "send" {
                            sent.lock()
                                .unwrap()
                                .push(request["text"].as_str().unwrap().to_owned());
                            if disconnecting.load(Ordering::SeqCst) {
                                continue;
                            }
                            json!({"success":true,"contentItems":[{"type":"inputText","text":"{}"}]})
                        } else {
                            json!({"success":true,"contentItems":[{"type":"inputText","text":json!({
                                "schemaVersion":1,"thread":{"id":"desktop-thread","cwd":cwd,"status":{"type":"idle"}},
                                "turns":[{"id":"original-turn","status":"completed"}]
                            }).to_string()}]})
                        };
                        let response = json!({"id":request["id"],"ok":true,"session_id":"desktop-session","thread_id":"desktop-thread","native":native});
                        let bytes = serde_json::to_vec(&response).unwrap();
                        stream
                            .write_all(&(bytes.len() as u32).to_le_bytes())
                            .unwrap();
                        stream.write_all(&bytes).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("bridge fixture accept: {error}"),
                }
            }
        });
        Self {
            endpoint: format!("unix://{}", socket.display()),
            sends,
            stop,
            disconnect_send,
            worker: Some(worker),
        }
    }
}

impl Drop for DesktopBridgeFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        let _ = std::fs::remove_file(self.endpoint.strip_prefix("unix://").unwrap());
    }
}

fn desktop_command(fixture: &Fixture, args: &[&str]) -> Output {
    fixture
        .configured_command(args, None)
        .env("CODEX_SESSION_ID", "desktop-session")
        .env("CODEX_THREAD_ID", "desktop-thread")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap()
}

#[test]
fn desktop_context_and_durable_notification_use_bridge_without_appserver_endpoint() {
    let mut fixture = Fixture::without_tmux("desktop-bridge-contract");
    let bridge = DesktopBridgeFixture::start(&fixture.root);
    let facts =
        json!({"session_id":"desktop-session","thread_id":"desktop-thread","desktop_bridge":{
            "endpoint":bridge.endpoint,"namespace":"codex_app","session_id":"desktop-session",
            "thread_id":"desktop-thread","cwd":fixture.root
        }})
        .to_string();
    let restored = desktop_command(&fixture, &["context", "--provide", &facts]);
    assert!(
        restored.status.success(),
        "Desktop context must not require an AppServer endpoint: {} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    let snapshot = &restored["result"]["snapshot"];
    assert_eq!(
        snapshot["identity"]["transport"]["kind"], "desktop_bridge",
        "{restored}"
    );
    let id = snapshot["identity"]["worker_id"].as_str().unwrap();
    let again = desktop_command(&fixture, &["context", "--provide", &facts]);
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["result"]["snapshot"]["identity"]["worker_id"], id);
    assert_eq!(
        again["result"]["snapshot"]["binding"]["endpoint_generation"],
        snapshot["binding"]["endpoint_generation"]
    );
    let mut status = fixture
        .configured_command(&["master", "status"], None)
        .env("CODEX_SESSION_ID", "desktop-session")
        .env("CODEX_THREAD_ID", "desktop-thread")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let returned = wait_until(Duration::from_secs(3), || {
        status.try_wait().unwrap().is_some()
    });
    if !returned {
        status.kill().unwrap();
        // This private fixture's reducer is deliberately wedged by the red
        // case. Reap that owned process before failure evidence collection.
        fixture.daemon.as_mut().unwrap().kill().unwrap();
        fixture.reap_daemon();
    }
    let status = status.wait_with_output().unwrap();
    assert!(
        returned && status.status.success(),
        "Desktop master status must complete without presence observations: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            id,
            "--subject",
            "bridge-contract",
            "registered Desktop notification",
        ],
    );
    assert!(
        sent.status.success(),
        "{} {}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr)
    );
    let sent: Value = serde_json::from_slice(&sent.stdout).unwrap();
    assert_eq!(sent["durable"], true, "{sent}");
    assert_eq!(
        sent["notification"], "desktop-bridge-input-submitted",
        "{sent}"
    );
    assert!(
        wait_until(Duration::from_secs(5), || bridge
            .sends
            .lock()
            .unwrap()
            .iter()
            .any(|text| text.contains("bridge-contract"))),
        "daemon must call the bridge, not queue-only or AppServer"
    );
    let received = desktop_command(
        &fixture,
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "desktop-bridge-read",
        ],
    );
    assert!(
        received.status.success(),
        "{}",
        String::from_utf8_lossy(&received.stderr)
    );
    let received: Value = serde_json::from_slice(&received.stdout).unwrap();
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["subject"] == "bridge-contract"));
    drop(bridge);
    let read = desktop_command(&fixture, &["worker", "read"]);
    assert!(
        read.status.success(),
        "lost transport must not erase identity: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let read: Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(
        read["result"]["projection"]["identity_valid"], true,
        "{read}"
    );
    let replacement = DesktopBridgeFixture::start(&fixture.root);
    let mut new_facts: Value = serde_json::from_str(&facts).unwrap();
    new_facts["desktop_bridge"]["endpoint"] = json!(replacement.endpoint);
    let renewed = desktop_command(&fixture, &["context", "--provide", &new_facts.to_string()]);
    assert!(
        renewed.status.success(),
        "{}",
        String::from_utf8_lossy(&renewed.stderr)
    );
    let renewed: Value = serde_json::from_slice(&renewed.stdout).unwrap();
    assert_eq!(renewed["result"]["snapshot"]["identity"]["worker_id"], id);
    assert!(
        renewed["result"]["snapshot"]["binding"]["endpoint_generation"]
            .as_u64()
            .unwrap()
            > snapshot["binding"]["endpoint_generation"].as_u64().unwrap()
    );
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            id,
            "--subject",
            "new-bridge-route",
            "after restart",
        ],
    );
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(wait_until(Duration::from_secs(5), || replacement
        .sends
        .lock()
        .unwrap()
        .iter()
        .any(|text| text.contains("new-bridge-route"))));
}

#[test]
fn desktop_candidate_mismatch_is_rejected_and_unknown_send_is_not_replayed() {
    let fixture = Fixture::without_tmux("desktop-bridge-failure");
    let bridge = DesktopBridgeFixture::start(&fixture.root);
    let mut facts = json!({"session_id":"desktop-session","thread_id":"desktop-thread","desktop_bridge":{
        "endpoint":bridge.endpoint,"namespace":"codex_app","session_id":"wrong-session",
        "thread_id":"desktop-thread","cwd":fixture.root
    }});
    let bad = desktop_command(&fixture, &["context", "--provide", &facts.to_string()]);
    assert!(
        !bad.status.success(),
        "mismatching session must not register"
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("Desktop candidate must match"),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    facts["desktop_bridge"]["session_id"] = json!("desktop-session");
    facts["desktop_bridge"]["thread_id"] = json!("wrong-thread");
    facts["thread_id"] = json!("wrong-thread");
    let wrong_target = desktop_command(&fixture, &["context", "--provide", &facts.to_string()]);
    assert!(
        !wrong_target.status.success(),
        "bridge response cannot substitute a different thread"
    );
    facts["desktop_bridge"]["thread_id"] = json!("desktop-thread");
    facts["thread_id"] = json!("desktop-thread");
    let restored = desktop_command(&fixture, &["context", "--provide", &facts.to_string()]);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    let id = restored["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    bridge.disconnect_send.store(true, Ordering::SeqCst);
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            id,
            "--subject",
            "unknown-bridge-result",
            "retained durable message",
        ],
    );
    assert!(
        !sent.status.success(),
        "unknown delivery must not report success"
    );
    assert!(
        String::from_utf8_lossy(&sent.stderr).contains("\"durable\":true"),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(wait_until(Duration::from_secs(5), || !bridge
        .sends
        .lock()
        .unwrap()
        .is_empty()));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        bridge.sends.lock().unwrap().len(),
        1,
        "unknown outcome must not trigger another host effect"
    );
    let journal = std::fs::read_to_string(fixture.project_journal()).unwrap();
    let failure = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["ev"] == "NotificationDeliveryFailed")
        .expect("explicit failure receipt");
    assert_eq!(failure["retryable"], false, "{failure}");
    let recv = desktop_command(
        &fixture,
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "unknown-result-consume",
        ],
    );
    assert!(recv.status.success());
    let recv: Value = serde_json::from_slice(&recv.stdout).unwrap();
    assert!(recv["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["subject"] == "unknown-bridge-result"));
}
