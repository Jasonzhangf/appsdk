include!("support/context_operation_fixture.rs");
#[path = "support/communication_receiver.rs"]
mod communication_receiver;

struct DesktopBridgeFixture {
    endpoint: String,
    sends: Arc<Mutex<Vec<String>>>,
    received_threads: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    disconnect_send: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    receiver: Arc<Mutex<Option<std::sync::mpsc::Sender<Value>>>>,
    worker: Option<JoinHandle<()>>,
}

impl DesktopBridgeFixture {
    fn start(root: &Path) -> Self {
        Self::start_at(root, root)
    }
    fn start_at(root: &Path, sockets: &Path) -> Self {
        Self::start_with_identity(root, sockets, "desktop-session", "desktop-thread")
    }
    fn start_with_identity(root: &Path, sockets: &Path, session: &str, thread: &str) -> Self {
        std::fs::create_dir_all(sockets).unwrap();
        let socket = sockets.join(format!(
            "desktop-{}.sock",
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let sends = Arc::new(Mutex::new(Vec::new()));
        let received_threads = Arc::new(Mutex::new(Vec::new()));
        let received = received_threads.clone();
        let stopping = stop.clone();
        let sent = sends.clone();
        let disconnect_send = Arc::new(AtomicBool::new(false));
        let disconnecting = disconnect_send.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let is_busy = busy.clone();
        let receiver = Arc::new(Mutex::new(None::<std::sync::mpsc::Sender<Value>>));
        let receiving = receiver.clone();
        let cwd = root.canonicalize().unwrap();
        let session = session.to_owned();
        let thread = thread.to_owned();
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
                            received
                                .lock()
                                .unwrap()
                                .push(request["thread_id"].as_str().unwrap_or("").to_owned());
                            if disconnecting.load(Ordering::SeqCst) {
                                continue;
                            }
                            json!({"success":true,"contentItems":[{"type":"inputText","text":"{}"}]})
                        } else {
                            json!({"success":true,"contentItems":[{"type":"inputText","text":json!({
                                "schemaVersion":1,"thread":{"id":thread,"cwd":cwd,"status":{"type":if is_busy.load(Ordering::SeqCst) {"active"} else {"idle"}}},
                                "turns":[{"id":"original-turn","status":"completed"}]
                            }).to_string()}]})
                        };
                        let response = json!({"id":request["id"],"ok":true,"session_id":session,"thread_id":thread,"native":native});
                        let bytes = serde_json::to_vec(&response).unwrap();
                        stream
                            .write_all(&(bytes.len() as u32).to_le_bytes())
                            .unwrap();
                        stream.write_all(&bytes).unwrap();
                        if request["op"] == "send" {
                            if let Some(receiver) = receiving.lock().unwrap().as_ref() {
                                let _ = receiver.send(request);
                            }
                        }
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
            received_threads,
            stop,
            disconnect_send,
            busy,
            receiver,
            worker: Some(worker),
        }
    }
}

#[test]
fn desktop_manual_recovery_requires_independent_receive_and_reply() {
    let mut fixture = Fixture::without_tmux("desktop-manual-recovery");
    let config = fixture.root.join("home/.appsdk");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[timers]\ntick_interval_ms=60000\n",
    )
    .unwrap();
    fixture.restart_daemon();
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &bridge);
    let registered = desktop_command(&fixture, &["context"]);
    assert!(registered.status.success());
    let registered: Value = serde_json::from_slice(&registered.stdout).unwrap();
    let worker = registered["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let socket = PathBuf::from(bridge.endpoint.strip_prefix("unix://").unwrap());
    let offline = socket.with_extension("offline");
    std::fs::rename(&socket, &offline).unwrap();
    let refused = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            worker,
            "--subject",
            "desktop-known-failure",
            "retain current failed message",
        ],
    );
    assert!(!refused.status.success());
    std::fs::rename(offline, socket).unwrap();
    let (tx, native) = std::sync::mpsc::channel();
    *bridge.receiver.lock().unwrap() = Some(tx);
    let configured = fixture.configured_command(&[], None);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.current_dir(&fixture.root).env_clear();
    for (key, value) in configured.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    command
        .env("CODEX_SESSION_ID", "desktop-session")
        .env("CODEX_THREAD_ID", "desktop-thread")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop");
    let mut receiver = communication_receiver::ReceivingProcess::start(command, &fixture.binary);
    let pending = desktop_command(&fixture, &["context", "--recover-communication"]);
    let pending: Value = serde_json::from_slice(&pending.stdout).unwrap();
    let communication = &pending["result"]["owner_readback"]["communication"];
    assert_eq!(communication["state"], "pending", "{communication}");
    assert!(communication["receive_id"].is_null());
    let message_id = communication["message_id"].as_str().unwrap();
    let input = native.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(input["thread_id"], "desktop-thread");
    assert!(input["text"].as_str().unwrap().contains(message_id));
    let held = desktop_command(&fixture, &["context", "--query"]);
    let held: Value = serde_json::from_slice(&held.stdout).unwrap();
    assert_eq!(
        held["result"]["owner_readback"]["communication"]["state"],
        "pending"
    );
    receiver.submit(&input);
    receiver.event("received");
    let reply = receiver.event("replied");
    let query = desktop_command(&fixture, &["context", "--query"]);
    assert!(query.status.success());
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["state"],
        "recovered"
    );
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["reply_message_id"],
        reply["receipt"]["msg_id"]
    );
    assert_eq!(
        bridge.sends.lock().unwrap().len(),
        2,
        "one recovery wake and its ordinary reply wake"
    );
}

#[test]
fn desktop_manual_recovery_does_not_repeat_a_lost_native_response() {
    let fixture = Fixture::without_tmux("desktop-manual-unknown");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &bridge);
    let identity = desktop_command(&fixture, &["context"]);
    assert!(identity.status.success());
    let identity: Value = serde_json::from_slice(&identity.stdout).unwrap();
    let worker = identity["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    bridge.disconnect_send.store(true, Ordering::SeqCst);
    let failed = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            worker,
            "--subject",
            "desktop-ambiguous-return",
            "submitted but response lost",
        ],
    );
    assert!(!failed.status.success());
    bridge.disconnect_send.store(false, Ordering::SeqCst);
    assert_eq!(bridge.sends.lock().unwrap().len(), 1);
    let before = fixture.current_state();
    let response = desktop_command(&fixture, &["context", "--recover-communication"]);
    let response: Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(
        response["result"]["owner_readback"]["communication"]["state"],
        "unknown"
    );
    assert_eq!(bridge.sends.lock().unwrap().len(), 1);
    assert_eq!(fixture.current_state()["msgs"], before["msgs"]);
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

// These review regressions use only the isolated native host and public wire
// protocol. No pane fixture, repository commit or installed service is used.
fn route_review_wire(fixture: &Fixture, request: Value) -> Value {
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{}", request).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn route_review_context(fixture: &Fixture, root: &Path, bridge: &DesktopBridgeFixture) -> Output {
    route_review_context_with_identity(fixture, root, bridge, "desktop-session", "desktop-thread")
}

fn route_review_context_with_identity(
    fixture: &Fixture,
    root: &Path,
    bridge: &DesktopBridgeFixture,
    session: &str,
    thread: &str,
) -> Output {
    let facts = json!({"session_id":session,"thread_id":thread,
        "desktop_bridge":{"endpoint":bridge.endpoint,"namespace":"codex_app",
        "session_id":session,"thread_id":thread,"cwd":root}});
    fixture
        .configured_command(&["context", "--provide", &facts.to_string()], None)
        .current_dir(root)
        .env("CODEX_SESSION_ID", session)
        .env("CODEX_THREAD_ID", thread)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap()
}

#[test]
fn route_review_storage_denials_precede_namespace_and_identity_mutation() {
    route_review_storage_scenario(true);
}

#[test]
fn route_review_storage_collision_precedes_namespace_and_identity_mutation() {
    route_review_storage_scenario(false);
}

fn route_review_storage_scenario(check_permission: bool) {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::without_tmux("route-review-storage");
    let bridge = DesktopBridgeFixture::start(&fixture.root);
    let first = route_review_context(&fixture, &fixture.root, &bridge);
    assert!(first.status.success(), "{first:?}");
    let routes_path = fixture.host_routes();
    let routes: Value = serde_json::from_slice(&std::fs::read(&routes_path).unwrap()).unwrap();
    let first_route = routes.as_array().unwrap().first().unwrap();
    let app = first_route["app_scope_id"].as_str().unwrap();
    let storage = PathBuf::from(first_route["storage_root"].as_str().unwrap());
    let second = fixture.root.join("second-project");
    seed_baseline(&second);
    let second = second.canonicalize().unwrap();
    let bridge_b = DesktopBridgeFixture::start(&second);
    let encoded = |base: &Path, value: &str| {
        let hex: String = value
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        base.join(format!("v1-{}-{hex}", value.len()))
    };
    let prospective = encoded(
        &encoded(
            &fixture.host_state.join("projects"),
            second.to_str().unwrap(),
        )
        .join("apps"),
        app,
    );
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let routes_before = std::fs::read(&routes_path).unwrap();
    if check_permission {
        let blocked = fixture.root.join("blocked-owner");
        std::fs::create_dir(&blocked).unwrap();
        let mut denied_routes = routes.clone();
        denied_routes.as_array_mut().unwrap().push(json!({"app_scope_id":"blocked-app",
        "project_scope":fixture.root.canonicalize().unwrap(),"canonical_root":fixture.root.canonicalize().unwrap(),
        "storage_root":blocked.join("future"),"registered_ms":1}));
        std::fs::write(&routes_path, denied_routes.to_string()).unwrap();
        let denied_bytes = std::fs::read(&routes_path).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = route_review_context(&fixture, &second, &bridge_b);
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!denied.status.success(), "{denied:?}");
        assert!(
            String::from_utf8_lossy(&denied.stderr).contains("Permission denied"),
            "{denied:?}"
        );
        assert!(
            !prospective.exists(),
            "permission denial created a namespace"
        );
        assert_eq!(std::fs::read(&routes_path).unwrap(), denied_bytes);
        assert_eq!(
            std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
            map_before
        );
        std::fs::write(&routes_path, &routes_before).unwrap();
    }

    std::fs::create_dir_all(prospective.parent().unwrap()).unwrap();
    symlink(&storage, &prospective).unwrap();
    let state_path = storage.join(".agent-collab/server/state.json");
    let state_before = std::fs::read(&state_path).unwrap();
    std::fs::remove_file(&state_path).unwrap();
    let collision = route_review_context(&fixture, &second, &bridge_b);
    assert!(!collision.status.success(), "{collision:?}");
    assert!(
        String::from_utf8_lossy(&collision.stderr).contains("already owned by route"),
        "{collision:?}"
    );
    assert!(
        !state_path.exists(),
        "collision recreated another owner's state"
    );
    assert_eq!(std::fs::read(&routes_path).unwrap(), routes_before);
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before
    );
    std::fs::write(&state_path, state_before).unwrap();
    std::fs::remove_file(&prospective).unwrap();
    let allowed = route_review_context(&fixture, &second, &bridge_b);
    assert!(allowed.status.success(), "{allowed:?}");
    let allowed_again = route_review_context(&fixture, &second, &bridge_b);
    assert!(allowed_again.status.success(), "{allowed_again:?}");
    let a: Value = serde_json::from_slice(&allowed.stdout).unwrap();
    let b: Value = serde_json::from_slice(&allowed_again.stdout).unwrap();
    assert_eq!(
        a["result"]["snapshot"]["binding"],
        b["result"]["snapshot"]["binding"]
    );
}

#[test]
fn route_review_native_map_scope_and_goal_binding_loss_are_explicit() {
    let fixture = Fixture::without_tmux("route-review-map-goal");
    let bridge = DesktopBridgeFixture::start(&fixture.root);
    let output = route_review_context(&fixture, &fixture.root, &bridge);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let snapshot = &value["result"]["snapshot"];
    let binding = &snapshot["binding"];
    let app = binding["app_scope_id"].as_str().unwrap();
    let child_bridge = DesktopBridgeFixture::start_with_identity(
        &fixture.root,
        &fixture.root,
        "child-session",
        "child-thread",
    );
    let child_output = route_review_context_with_identity(
        &fixture,
        &fixture.root,
        &child_bridge,
        "child-session",
        "child-thread",
    );
    assert!(child_output.status.success(), "{child_output:?}");
    let child: Value = serde_json::from_slice(&child_output.stdout).unwrap();
    assert_eq!(child["result"]["snapshot"]["binding"]["app_scope_id"], app);
    assert_ne!(
        child["result"]["snapshot"]["binding"]["agent_id"],
        binding["agent_id"]
    );
    let root = fixture.root.canonicalize().unwrap();
    let context = json!({"project_scope":root,"canonical_root":root,"app_scope_id":app,
        "runtime_context":{"agent_id":binding["agent_id"],"runtime_id":binding["runtime_id"],
            "appserver_id":binding["app_scope_id"],"binding_id":binding["binding_id"],
            "session_id":binding["session_id"],"native_thread_id":binding["native_thread_id"]}});
    let state = fixture.current_state_for(&fixture.root, app);
    let worker = binding["agent_id"].as_str().unwrap();
    let token = state["workers"][worker]["token"].as_str().unwrap();
    let request = |op: &str| json!({"op":op,"project_context":context});
    let mut route = request("RouteResolveNative");
    route["session_id"] = json!("desktop-session");
    route["native_thread_id"] = json!("desktop-thread");
    assert_eq!(route_review_wire(&fixture, route.clone())["ok"], true);
    let mut child_list = request("Subagent");
    child_list["worker_id"] = json!(worker);
    child_list["token"] = json!(token);
    child_list["command"] = json!("List");
    let listed = route_review_wire(&fixture, child_list.clone());
    assert_eq!(listed["ok"], true, "{listed}");
    let mut forged = child_list;
    forged["project_context"]["app_scope_id"] = json!("forged-app");
    let before = fixture.current_state_for(&fixture.root, app);
    assert_eq!(route_review_wire(&fixture, forged)["ok"], false);
    assert_eq!(fixture.current_state_for(&fixture.root, app), before);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut goal = request("GoalScheduleReconcile");
    goal.as_object_mut().unwrap().extend(
        json!({"worker_id":worker,"token":token,
        "goal_id":"route-review-capacity","goal_owner":snapshot["name"],"desired":"active",
        "trigger_ms":now+600_000,"ttl_seconds":3600,"interval_ms":600_000,
        "repeat_count":4,"content":"route review capacity"})
        .as_object()
        .unwrap()
        .clone(),
    );
    let positive = route_review_wire(&fixture, goal.clone());
    assert_eq!(positive["ok"], true, "{positive}");
    for id in [
        "route-review-other-a",
        "route-review-other-b",
        "route-review-other-c",
    ] {
        let mut unrelated = goal.clone();
        unrelated["goal_id"] = json!(id);
        assert_eq!(route_review_wire(&fixture, unrelated)["ok"], true);
    }
    let schedules =
        fixture.current_state_for(&fixture.root, app)["notification_subscriptions"].clone();
    assert_eq!(
        route_review_wire(&fixture, goal.clone())["subscription"]["id"],
        positive["subscription"]["id"]
    );
    assert_eq!(
        fixture.current_state_for(&fixture.root, app)["notification_subscriptions"],
        schedules
    );
    let mut invalid = goal.clone();
    invalid["ttl_seconds"] = json!(60);
    let rejected = route_review_wire(&fixture, invalid);
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("before expiry"),
        "{rejected}"
    );
    assert_eq!(
        fixture.current_state_for(&fixture.root, app)["notification_subscriptions"],
        schedules
    );
    let mut unauthorized = goal.clone();
    let child_binding = &child["result"]["snapshot"]["binding"];
    let child_worker = child_binding["agent_id"].as_str().unwrap();
    unauthorized["worker_id"] = json!(child_worker);
    unauthorized["token"] =
        fixture.current_state_for(&fixture.root, app)["workers"][child_worker]["token"].clone();
    unauthorized["project_context"]["runtime_context"] = json!({
        "agent_id":child_binding["agent_id"],"runtime_id":child_binding["runtime_id"],
        "appserver_id":child_binding["app_scope_id"],"binding_id":child_binding["binding_id"],
        "session_id":child_binding["session_id"],"native_thread_id":child_binding["native_thread_id"]});
    let denied = route_review_wire(&fixture, unauthorized);
    assert_eq!(denied["error"], "GOAL_SCHEDULE_OWNER_MISMATCH: only the owner, project master or current CEO may manage this goal", "{denied}");
    assert_eq!(
        fixture.current_state_for(&fixture.root, app)["notification_subscriptions"],
        schedules
    );
    let map_path = fixture.host_state.join("identity-map.json");
    let map_bytes = std::fs::read(&map_path).unwrap();
    let mut stale: Value = serde_json::from_slice(&map_bytes).unwrap();
    let stale_binding = &mut stale["slots"][0]["current"]["binding"];
    stale_binding["agent_id"] = json!("unregistered-stale-agent");
    stale_binding["runtime_id"] = json!("unregistered-stale-runtime");
    stale_binding["binding_id"] = json!("unregistered-stale-binding");
    std::fs::write(&map_path, stale.to_string()).unwrap();
    let stale_bytes = std::fs::read(&map_path).unwrap();
    let stale_response = route_review_wire(&fixture, route.clone());
    assert_eq!(stale_response["ok"], false, "{stale_response}");
    assert!(
        stale_response["error"]
            .as_str()
            .unwrap()
            .contains("ROUTE_RESOLVE_STALE_INDEX"),
        "{stale_response}"
    );
    assert_eq!(std::fs::read(&map_path).unwrap(), stale_bytes);
    std::fs::write(&map_path, &map_bytes).unwrap();
    std::fs::write(&map_path, b"{corrupt").unwrap();
    let corrupt = route_review_wire(&fixture, route.clone());
    assert_eq!(corrupt["ok"], false, "{corrupt}");
    assert!(
        corrupt["error"].as_str().unwrap().contains("IDENTITY_MAP"),
        "{corrupt}"
    );
    assert_eq!(std::fs::read(&map_path).unwrap(), b"{corrupt");
    std::fs::write(&map_path, &map_bytes).unwrap();
    let mut map: Value = serde_json::from_slice(&map_bytes).unwrap();
    let slots = map["slots"].as_array_mut().expect("current map slots");
    slots.clear();
    std::fs::write(&map_path, map.to_string()).unwrap();
    let before = fixture.current_state_for(&fixture.root, app);
    let lost = route_review_wire(&fixture, goal.clone());
    assert_eq!(lost["ok"], false, "{lost}");
    assert_eq!(
        lost["error"], "CEO_SOURCE_NAME_REQUIRED",
        "a missing current name must not borrow reducer authority: {lost}"
    );
    assert_eq!(fixture.current_state_for(&fixture.root, app), before);
    std::fs::write(&map_path, map_bytes).unwrap();
    assert_eq!(route_review_wire(&fixture, route)["ok"], true);
    assert_eq!(route_review_wire(&fixture, goal)["ok"], true);
}

#[test]
fn desktop_appsdk_three_goal_crud_on_real_daemon() {
    let fixture = Fixture::without_tmux("desktop-appsdk-crud");
    let bridge =
        DesktopBridgeFixture::start_at(&fixture.root, &fixture.host_state.join("runtime/desktop"));
    publish_capability(&fixture, &bridge);
    let context = desktop_command(&fixture, &["context"]);
    assert!(context.status.success(), "{:?}", context);
    let sdk = std::env::var_os("APPSDK_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("rust/target/debug/appsdk")
        });
    assert!(
        sdk.is_file(),
        "build the candidate appsdk first: {}",
        sdk.display()
    );
    let run = |args: &[&str]| {
        let configured = fixture.configured_command(&[], None);
        let mut command = Command::new(&sdk);
        command.args(args).current_dir(&fixture.root).env_clear();
        for (key, value) in configured.get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        let mut paths = vec![fixture.binary.parent().unwrap().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("CODEX_SESSION_ID", "desktop-session")
            .env("CODEX_THREAD_ID", "desktop-thread")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop");
        command.output().unwrap()
    };
    let ok = |args: &[&str]| -> Value {
        let output = run(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let mut goals = Vec::new();
    let mut subscriptions = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let path = fixture.root.join(format!("{name}.md"));
        std::fs::write(&path, format!("Deliver {name}")).unwrap();
        let receipt = ok(&[
            "goal",
            "subscribe",
            "--goal",
            path.to_str().unwrap(),
            "--interval",
            "120s",
            "--repeat",
            "5",
            "--json",
        ]);
        assert_eq!(receipt["active"], true, "{receipt}");
        goals.push(receipt["goal_id"].as_str().unwrap().to_owned());
        subscriptions.push(receipt["subscription_id"].as_str().unwrap().to_owned());
    }
    let remote = || -> Value {
        let output = desktop_command(&fixture, &["notify", "status"]);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let selected = |status: &Value, index: usize| -> Value {
        status["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sub| sub["id"] == subscriptions[index])
            .unwrap()
            .clone()
    };
    let before = remote();
    for index in 0..3 {
        assert_eq!(selected(&before, index)["status"], "armed");
    }
    assert_eq!(ok(&["goal", "list", "--json"])["count"], 3);
    for goal in &goals {
        let status = ok(&["goal", "status", "--goal-id", goal, "--json"]);
        assert_eq!(status["goal_id"], *goal);
        ok(&["goal", "prompt", "--goal-id", goal, "--json"]);
    }
    assert_eq!(
        remote()["subscriptions"],
        before["subscriptions"],
        "reads do not mutate remote schedules"
    );
    for args in [
        vec!["goal", "status", "--json"],
        vec![
            "goal",
            "edit",
            "--goal-id",
            "missing",
            "--repeat",
            "4",
            "--json",
        ],
    ] {
        assert!(!run(&args).status.success(), "{args:?}");
    }
    assert_eq!(
        remote()["subscriptions"],
        before["subscriptions"],
        "failures have no schedule effects"
    );
    let edited = ok(&[
        "goal",
        "edit",
        "--goal-id",
        &goals[1],
        "--interval",
        "180s",
        "--repeat",
        "8",
        "--json",
    ]);
    assert_eq!(edited["subscription_id"], subscriptions[1]);
    let after = remote();
    assert_eq!(selected(&after, 1)["interval_ms"], 180000);
    assert_eq!(selected(&after, 1)["repeat_count"], 8);
    assert_eq!(selected(&after, 0), selected(&before, 0));
    assert_eq!(selected(&after, 2), selected(&before, 2));
    let updated = ok(&[
        "goal",
        "update",
        "--goal-id",
        &goals[2],
        "--repeat",
        "6",
        "--json",
    ]);
    assert_eq!(updated["subscription_id"], subscriptions[2]);
    ok(&["goal", "cancel", "--goal-id", &goals[1], "--json"]);
    let terminal = selected(&remote(), 1);
    assert_eq!(terminal["status"], "cancelled");
    let edited = ok(&[
        "goal",
        "edit",
        "--goal-id",
        &goals[1],
        "--repeat",
        "7",
        "--json",
    ]);
    assert_eq!(edited["subscription_id"], subscriptions[1]);
    let after = selected(&remote(), 1);
    assert_eq!(after["status"], "cancelled");
    assert_eq!(after["fired_count"], terminal["fired_count"]);
    assert_eq!(after["trigger_ms"], terminal["trigger_ms"]);
    assert_eq!(after["expires_ms"], terminal["expires_ms"]);
    assert_eq!(ok(&["goal", "list", "--json"])["count"], 3);
}

#[test]
fn desktop_wallclock_summary_two_overdue_goals() {
    let mut fixture = Fixture::without_tmux("desktop-wallclock-summary");
    let bridge =
        DesktopBridgeFixture::start_at(&fixture.root, &fixture.host_state.join("runtime/desktop"));
    publish_capability(&fixture, &bridge);
    let run = |fixture: &Fixture, args: &[&str]| -> Value {
        let output = desktop_command(fixture, args);
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let context = run(&fixture, &["context"]);
    let owner = context["result"]["snapshot"]["name"].as_str().unwrap();
    let epoch = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    };
    let first = epoch() + 3_000;
    let mut ids = Vec::new();
    for goal in ["physical-alpha", "physical-beta"] {
        let armed = run(
            &fixture,
            &[
                "goal",
                "reconcile",
                "--goal-id",
                goal,
                "--owner",
                owner,
                "--desired",
                "active",
                "--at-ms",
                &first.to_string(),
                "--every-ms",
                "20000",
                "--repeat",
                "3",
                "--ttl-seconds",
                "180",
            ],
        );
        ids.push(armed["subscription"]["id"].as_str().unwrap().to_string());
    }
    // Only this owned daemon is paused. Its clock is the real wall clock.
    let paused = fixture.pause_daemon();
    bridge.busy.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(45_000));
    drop(paused);
    std::thread::sleep(Duration::from_millis(1_500));
    let status = run(&fixture, &["notify", "status"]);
    let cursors: Vec<_> = ids
        .iter()
        .map(|id| {
            status["subscriptions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == *id)
                .unwrap()["trigger_ms"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert!(
        cursors.iter().all(|next| *next > epoch()),
        "cursors advance before receive: {status}"
    );
    assert!(
        cursors.iter().all(|next| *next == first + 60_000),
        "missed periods fold without shifting phase"
    );
    bridge.busy.store(false, Ordering::SeqCst);
    let wake_deadline = Instant::now() + Duration::from_secs(3);
    while bridge.sends.lock().unwrap().is_empty() && Instant::now() < wake_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let wakes = bridge.sends.lock().unwrap().len();
    assert_eq!(wakes, 1, "one native wake for the pending summary");
    fixture.restart_daemon();
    std::thread::sleep(Duration::from_millis(1_100));
    assert_eq!(
        bridge.sends.lock().unwrap().len(),
        wakes,
        "restart must not replay an accepted unread summary"
    );
    let ordinary = run(
        &fixture,
        &[
            "sendmessage",
            "--to",
            owner,
            "--subject",
            "ordinary-control",
            "ordinary body unchanged",
        ],
    );
    assert!(ordinary.is_object());
    let received = run(
        &fixture,
        &["recv", "--timeout", "0", "--receive-id", "physical-summary"],
    );
    let summaries: Vec<_> = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["type"] == "notification")
        .collect();
    assert_eq!(summaries.len(), 1, "{received}");
    let body = summaries[0]["body"].as_str().unwrap();
    assert!(
        body.contains("goal:physical-alpha") && body.contains("goal:physical-beta"),
        "{body}"
    );
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["subject"] == "ordinary-control" && m["body"] == "ordinary body unchanged"));
    let replay = run(
        &fixture,
        &["recv", "--timeout", "0", "--receive-id", "physical-summary"],
    );
    assert_eq!(received["messages"], replay["messages"]);
    let status = run(&fixture, &["notify", "status"]);
    for (id, cursor) in ids.iter().zip(cursors) {
        let sub = status["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == *id)
            .unwrap();
        assert_eq!(
            sub["trigger_ms"], cursor,
            "receive cannot shift the physical cursor"
        );
        assert_eq!(sub["fired_count"], 1);
    }
    fixture.restart_daemon();
    std::thread::sleep(Duration::from_millis(1_100));
    assert!(run(&fixture, &["recv", "--timeout", "0"])["messages"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn desktop_overdue_goal_keeps_physical_interval_after_public_receive() {
    // This path owns no tmux sessions. It uses an isolated daemon and a local
    // native bridge fixture; only public CLI reads create consume receipts.
    let fixture = Fixture::without_tmux("desktop-overdue-goal");
    let bridge =
        DesktopBridgeFixture::start_at(&fixture.root, &fixture.host_state.join("runtime/desktop"));
    publish_capability(&fixture, &bridge);
    let run = |args: &[&str]| -> Value {
        let output = desktop_command(&fixture, args);
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let context = run(&["context"]);
    let owner = context["result"]["snapshot"]["name"]
        .as_str()
        .or_else(|| context["name"].as_str())
        .expect("registered stable name");
    let epoch = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    };
    let first = epoch() + 1_000;
    let armed = run(&[
        "goal",
        "reconcile",
        "--goal-id",
        "overdue-public",
        "--owner",
        owner,
        "--desired",
        "active",
        "--at-ms",
        &first.to_string(),
        "--every-ms",
        "2000",
        "--repeat",
        "3",
        "--ttl-seconds",
        "60",
    ]);
    let sub_id = armed["subscription"]["id"].as_str().unwrap();
    std::thread::sleep(Duration::from_millis(6_100));
    let received_at = epoch();
    let received = run(&["recv", "--timeout", "0", "--receive-id", "overdue-first"]);
    let goals = |value: &Value| {
        value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["subject"] == "deadline:summary"
                    && message["body"]
                        .as_str()
                        .is_some_and(|body| body.contains("goal:overdue-public"))
            })
            .count()
    };
    assert_eq!(goals(&received), 1, "concurrent due periods coalesce");
    let status = run(&["notify", "status"]);
    let sub = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sub| sub["id"] == sub_id)
        .unwrap();
    assert_eq!(sub["fired_count"], 1);
    assert!(
        sub["trigger_ms"].as_i64().unwrap() > received_at
            && sub["trigger_ms"].as_i64().unwrap() <= received_at + 2_000,
        "{sub}"
    );
    let replay = run(&["recv", "--timeout", "0", "--receive-id", "overdue-first"]);
    assert_eq!(received["messages"], replay["messages"]);
    let empty = run(&[
        "recv",
        "--timeout",
        "0",
        "--receive-id",
        "overdue-no-backlog",
    ]);
    assert_eq!(goals(&empty), 0);
    std::thread::sleep(Duration::from_millis(2_500));
    let second = run(&["recv", "--timeout", "0", "--receive-id", "overdue-second"]);
    assert_eq!(goals(&second), 1, "regular future delivery remains armed");
    assert_ne!(received["messages"][0]["id"], second["messages"][0]["id"]);
    run(&[
        "goal",
        "reconcile",
        "--goal-id",
        "overdue-public",
        "--owner",
        owner,
        "--desired",
        "ended",
        "--reason",
        "isolated test complete",
    ]);
    std::thread::sleep(Duration::from_millis(2_500));
    assert_eq!(goals(&run(&["recv", "--timeout", "0"])), 0);
    let status = run(&["notify", "status"]);
    let sub = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sub| sub["id"] == sub_id)
        .unwrap();
    assert_eq!(sub["status"], "cancelled");
    assert_eq!(sub["fired_count"], 2);
    let cancelled = sub.clone();
    let next = epoch() + 10_000;
    let renewal_args = [
        "goal",
        "reconcile",
        "--goal-id",
        "overdue-public",
        "--owner",
        owner,
        "--desired",
        "active",
        "--at-ms",
        &next.to_string(),
        "--every-ms",
        "2000",
        "--repeat",
        "3",
        "--ttl-seconds",
        "60",
    ];
    let renewal = run(&renewal_args);
    assert_ne!(renewal["subscription"]["id"], sub_id);
    assert_eq!(renewal["subscription"]["fired_count"], 0);
    let repeated = run(&renewal_args);
    assert_eq!(
        repeated["subscription"]["id"],
        renewal["subscription"]["id"]
    );
    let status = run(&["notify", "status"]);
    let old = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sub| sub["id"] == sub_id)
        .unwrap();
    assert_eq!(old, &cancelled, "renewal preserves the old finite batch");
    assert_eq!(
        status["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|sub| sub["subject"] == "goal:overdue-public" && sub["status"] == "armed")
            .count(),
        1
    );
    let current = run(&["context"]);
    assert_eq!(
        current["result"]["snapshot"]["identity"]["worker_id"],
        context["result"]["snapshot"]["identity"]["worker_id"]
    );
}

fn publish_capability(fixture: &Fixture, bridge: &DesktopBridgeFixture) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let cwd = fixture.root.canonicalize().unwrap();
    let mut directory = fixture.host_state.join("runtime/desktop/capabilities/v1");
    for (label, value) in [
        ("cwd", cwd.to_str().unwrap()),
        ("session", "desktop-session"),
        ("thread", "desktop-thread"),
    ] {
        directory.push(label);
        let encoded: String = value
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        for chunk in encoded.as_bytes().chunks(120) {
            directory.push(std::str::from_utf8(chunk).unwrap());
        }
    }
    std::fs::create_dir_all(&directory).unwrap();
    let base = fixture.host_state.join("runtime");
    let mut node = directory.as_path();
    while node.starts_with(&base) {
        std::fs::set_permissions(node, std::fs::Permissions::from_mode(0o700)).unwrap();
        node = node.parent().unwrap();
    }
    let current = directory.join("current.json");
    std::fs::write(
        &current,
        json!({"schema":"collab-desktop-capability/v1","desktop_bridge":{
            "endpoint":bridge.endpoint,"namespace":"codex_app","session_id":"desktop-session",
            "thread_id":"desktop-thread","cwd":cwd
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o600)).unwrap();
    current
}

#[test]
fn desktop_connect_failure_resumes_after_bridge_and_daemon_restart_without_agent_action() {
    let mut fixture = Fixture::without_tmux("desktop-connect-resume");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &bridge);
    let enrolled = desktop_command(&fixture, &["context"]);
    assert!(
        enrolled.status.success(),
        "{}",
        String::from_utf8_lossy(&enrolled.stderr)
    );
    let enrolled: Value = serde_json::from_slice(&enrolled.stdout).unwrap();
    let worker = enrolled["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let socket = PathBuf::from(bridge.endpoint.strip_prefix("unix://").unwrap());
    let offline = socket.with_extension("offline");
    std::fs::rename(&socket, &offline).unwrap();
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            worker,
            "--subject",
            "connect-recovery",
            "retain this message across bridge downtime",
        ],
    );
    assert!(!sent.status.success());
    assert!(String::from_utf8_lossy(&sent.stderr).contains("\"durable\":true"));
    assert!(bridge.sends.lock().unwrap().is_empty());
    let failed = fixture.current_state();
    assert!(failed["notification_delivery_failures"]
        .as_object()
        .unwrap()
        .values()
        .any(|failure| failure["retryable"] == true
            && failure["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("DESKTOP_BRIDGE_NOT_SUBMITTED:"))));
    fixture.restart_daemon();
    std::fs::rename(&offline, &socket).unwrap();
    assert!(
        wait_until(Duration::from_secs(6), || bridge
            .sends
            .lock()
            .unwrap()
            .len()
            == 1),
        "a never-submitted Desktop message was stranded after recovery"
    );
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        bridge.sends.lock().unwrap().len(),
        1,
        "accepted recovery is never replayed"
    );
}

#[test]
fn consecutive_desktop_sends_do_not_wait_for_recipient_consumption() {
    let fixture = Fixture::without_tmux("desktop-immediate-burst");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &bridge);
    let enrolled = desktop_command(&fixture, &["context"]);
    assert!(enrolled.status.success());
    let enrolled: Value = serde_json::from_slice(&enrolled.stdout).unwrap();
    let worker = enrolled["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    for index in 0..5 {
        let subject = format!("immediate-burst-{index}");
        let sent = desktop_command(
            &fixture,
            &[
                "sendmessage",
                "--to",
                worker,
                "--subject",
                &subject,
                "deliver without waiting for prior consumption",
            ],
        );
        assert!(
            sent.status.success(),
            "{}",
            String::from_utf8_lossy(&sent.stderr)
        );
        assert!(
            bridge
                .sends
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains(&subject)),
            "native send must happen before the public command returns"
        );
    }
    let received = desktop_command(
        &fixture,
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "immediate-burst-consumption",
        ],
    );
    assert!(
        received.status.success(),
        "{}",
        String::from_utf8_lossy(&received.stderr)
    );
    let received = String::from_utf8_lossy(&received.stdout);
    for index in 0..5 {
        assert!(received.contains(&format!("immediate-burst-{index}")));
    }
}

#[test]
fn missing_desktop_publication_preserves_live_capability_and_identity() {
    let fixture = Fixture::without_tmux("desktop-missing-live");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    let declaration = publish_capability(&fixture, &bridge);
    let initial = desktop_command(&fixture, &["context"]);
    assert!(initial.status.success());
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();
    std::fs::remove_file(&declaration).unwrap();
    let restored = desktop_command(&fixture, &["context"]);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(
        restored["result"]["snapshot"]["binding"],
        initial["result"]["snapshot"]["binding"]
    );
    std::thread::sleep(Duration::from_millis(2200));
    let log = std::fs::read_to_string(fixture.host_state.join("log.txt")).unwrap_or_default();
    assert!(!log.contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"), "{log}");
    assert!(!log.contains("DESKTOP_BRIDGE_CAPABILITY_LOST"), "{log}");
    // An unsafe declaration is a real control error, not publication absence.
    let declaration = publish_capability(&fixture, &bridge);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(declaration, std::fs::Permissions::from_mode(0o666)).unwrap();
    let unsafe_control = desktop_command(&fixture, &["context"]);
    assert!(!unsafe_control.status.success());
    assert!(String::from_utf8_lossy(&unsafe_control.stderr)
        .contains("DESKTOP_BRIDGE_DECLARATION_UNSAFE"));
}

#[test]
fn missing_desktop_publication_with_lost_known_capability_requests_only_its_peer_update() {
    let fixture = Fixture::without_tmux("desktop-missing-dead");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    let declaration = publish_capability(&fixture, &bridge);
    let initial = desktop_command(&fixture, &["context"]);
    assert!(initial.status.success());
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();
    std::fs::remove_file(&declaration).unwrap();
    drop(bridge);
    assert!(
        wait_until(Duration::from_secs(8), || std::fs::read_to_string(
            fixture.host_state.join("log.txt")
        )
        .unwrap_or_default()
        .contains("DESKTOP_BRIDGE_CAPABILITY_LOST")),
        "known lost capability was not classified"
    );
    let restored = desktop_command(&fixture, &["context"]);
    assert!(
        restored.status.success(),
        "identity must survive: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(
        restored["result"]["snapshot"]["binding"],
        initial["result"]["snapshot"]["binding"]
    );
}

#[test]
fn desktop_endpoint_replacement_converges_without_agent_context() {
    let fixture = Fixture::without_tmux("desktop-background");
    let runtime = fixture.host_state.join("runtime/desktop");
    let first = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &first);
    let enrolled = desktop_command(&fixture, &["context"]);
    assert!(
        enrolled.status.success(),
        "{}",
        String::from_utf8_lossy(&enrolled.stderr)
    );
    let enrolled: Value = serde_json::from_slice(&enrolled.stdout).unwrap();
    let worker = enrolled["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let original_worker = fixture.current_state()["workers"][worker].clone();
    drop(first);
    let second = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &second);
    assert!(
        wait_until(Duration::from_secs(8), || {
            fixture.current_state()["workers"][worker]["transport"]["endpoint"] == second.endpoint
        }),
        "replacement endpoint was not synchronized by the daemon"
    );
    // A normal business command, with no context/re-register after replacement.
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            worker,
            "--subject",
            "background-replacement",
            "same identity, current bridge",
        ],
    );
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(second
        .sends
        .lock()
        .unwrap()
        .iter()
        .any(|body| body.contains("background-replacement")));
    let current = fixture.current_state();
    assert_eq!(current["workers"].as_object().unwrap().len(), 1);
    assert_eq!(current["workers"][worker]["id"], worker);
    assert_eq!(
        current["workers"][worker]["token"],
        original_worker["token"]
    );
    assert_eq!(
        current["workers"][worker]["registered_ms"],
        original_worker["registered_ms"]
    );
}

#[test]
fn bare_desktop_context_discovers_current_scope_refreshes_and_preserves_identity_on_failure() {
    let fixture = Fixture::without_tmux("desktop-discovery");
    let runtime = fixture.host_state.join("runtime/desktop");
    let first = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    let current = publish_capability(&fixture, &first);
    let restored = desktop_command(&fixture, &["context"]);
    assert!(
        restored.status.success(),
        "{} {}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    let a: Value = serde_json::from_slice(&restored.stdout).unwrap();
    let identity = a["result"]["snapshot"]["identity"]["worker_id"].clone();
    let second = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &second);
    drop(first);
    let refreshed = desktop_command(&fixture, &["context"]);
    assert!(
        refreshed.status.success(),
        "{}",
        String::from_utf8_lossy(&refreshed.stderr)
    );
    let b: Value = serde_json::from_slice(&refreshed.stdout).unwrap();
    assert_eq!(b["result"]["snapshot"]["identity"]["worker_id"], identity);
    for field in ["display_name", "role"] {
        assert_eq!(
            b["result"]["snapshot"]["identity"][field],
            a["result"]["snapshot"]["identity"][field]
        );
    }
    assert_eq!(
        b["result"]["snapshot"]["binding"]["session_id"],
        "desktop-session"
    );
    assert_eq!(
        b["result"]["snapshot"]["identity"]["transport"]["endpoint"],
        second.endpoint
    );
    let repeated = desktop_command(&fixture, &["context"]);
    assert!(repeated.status.success());
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(
        repeated["result"]["snapshot"]["binding"],
        b["result"]["snapshot"]["binding"]
    );
    let sent = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            identity.as_str().unwrap(),
            "--subject",
            "discovered-replacement",
            "current bridge owns delivery",
        ],
    );
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(second
        .sends
        .lock()
        .unwrap()
        .iter()
        .any(|text| text.contains("discovered-replacement")));
    let received = desktop_command(
        &fixture,
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "discovered-replacement-read",
        ],
    );
    assert!(received.status.success());
    assert!(String::from_utf8_lossy(&received.stdout).contains("current bridge owns delivery"));
    let operation = b["operation_id"].as_str().unwrap();
    std::fs::write(&current, "not JSON").unwrap();
    let queried = desktop_command(&fixture, &["context", "--query", "--op", operation]);
    assert!(
        queried.status.success(),
        "query must not read declaration: {}",
        String::from_utf8_lossy(&queried.stderr)
    );
    let invalid = desktop_command(&fixture, &["context"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("DESKTOP_BRIDGE_"));
    publish_capability(&fixture, &second);
    drop(second);
    let dead = desktop_command(&fixture, &["context"]);
    assert!(!dead.status.success());
    let who = desktop_command(&fixture, &["who"]);
    assert!(
        who.status.success(),
        "identity survives failed capability: {}",
        String::from_utf8_lossy(&who.stderr)
    );
    let who: Value = serde_json::from_slice(&who.stdout).unwrap();
    assert!(who.to_string().contains(identity.as_str().unwrap()));
}

#[test]
fn unenrolled_desktop_context_reports_bridge_capability_instead_of_appserver_endpoint() {
    let fixture = Fixture::without_tmux("desktop-unenrolled");
    let result = desktop_command(&fixture, &["context"]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn inherited_desktop_marker_does_not_block_a_verified_current_tmux_pane() {
    let (fixture, pane) = Fixture::isolated("desktop-marker-pane");
    let exit = fixture.root.join("desktop-marker.exit");
    let consumer = fixture.run_script_in_pane_on_socket(
        &fixture.tmux_socket,
        &pane,
        "desktop-marker",
        r#"export CODEX_INTERNAL_ORIGINATOR_OVERRIDE='Codex Desktop'
unset CODEX_THREAD_ID
"$COLLAB_BINARY" context >"$PANE_STDOUT" 2>"$PANE_STDERR"
printf '%s' "$?" >"$PROJECT_ROOT/desktop-marker.exit"
"#,
    );
    assert!(
        wait_until(Duration::from_secs(15), || {
            std::fs::read_to_string(&exit).is_ok_and(|code| !code.is_empty())
        }),
        "{}",
        consumer.output()
    );
    assert_eq!(
        std::fs::read_to_string(exit).unwrap(),
        "0",
        "{}",
        consumer.output()
    );
    let result: Value = serde_json::from_slice(&std::fs::read(consumer.stdout).unwrap()).unwrap();
    assert_eq!(result["result"]["outcome"], "completed");
    assert_eq!(
        result["result"]["snapshot"]["identity"]["transport"]["kind"],
        "tmux"
    );
}

#[test]
fn desktop_discovery_rejects_unsafe_or_foreign_control_without_overriding_explicit_facts() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::without_tmux("desktop-discovery-control");
    let bridge =
        DesktopBridgeFixture::start_at(&fixture.root, &fixture.host_state.join("runtime/desktop"));
    let current = publish_capability(&fixture, &bridge);
    let valid = std::fs::read(&current).unwrap();
    let baseline = desktop_command(
        &fixture,
        &[
            "context",
            "--provide",
            r#"{"session_id":"desktop-session"}"#,
        ],
    );
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    for field in ["session_id", "thread_id", "cwd", "namespace"] {
        let mut record: Value = serde_json::from_slice(&valid).unwrap();
        record["desktop_bridge"][field] = json!("foreign");
        std::fs::write(&current, record.to_string()).unwrap();
        let rejected = desktop_command(&fixture, &["context"]);
        assert!(!rejected.status.success(), "must reject foreign {field}");
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("DESKTOP_BRIDGE_SCOPE_MISMATCH"));
    }
    std::fs::write(&current, vec![b' '; 65537]).unwrap();
    let oversized = desktop_command(&fixture, &["context"]);
    assert!(String::from_utf8_lossy(&oversized.stderr).contains("exceeds 64 KiB"));
    std::fs::write(&current, &valid).unwrap();
    std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o666)).unwrap();
    let unsafe_mode = desktop_command(&fixture, &["context"]);
    assert!(
        String::from_utf8_lossy(&unsafe_mode.stderr).contains("DESKTOP_BRIDGE_DECLARATION_UNSAFE")
    );
    std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o600)).unwrap();
    let target = fixture.root.join("foreign-control.json");
    std::fs::write(&target, &valid).unwrap();
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&target, &current).unwrap();
    let symlink = desktop_command(&fixture, &["context"]);
    assert!(!symlink.status.success());
    assert!(
        String::from_utf8_lossy(&symlink.stderr).contains("DESKTOP_BRIDGE_DECLARATION_UNAVAILABLE")
    );
    let explicit = json!({"session_id":"desktop-session","thread_id":"desktop-thread","desktop_bridge":{
        "endpoint":bridge.endpoint,"namespace":"codex_app","session_id":"desktop-session","thread_id":"desktop-thread","cwd":fixture.root
    }}).to_string();
    let supplied = desktop_command(&fixture, &["context", "--provide", &explicit]);
    assert!(
        supplied.status.success(),
        "explicit transport must skip bad discovery: {}",
        String::from_utf8_lossy(&supplied.stderr)
    );
    std::fs::remove_file(&current).unwrap();
    publish_capability(&fixture, &bridge);
    let foreign_thread = fixture
        .configured_command(&["context"], None)
        .env("CODEX_SESSION_ID", "desktop-session")
        .env("CODEX_THREAD_ID", "foreign-thread")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    assert!(!foreign_thread.status.success());
    assert!(String::from_utf8_lossy(&foreign_thread.stderr)
        .contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"));
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
    assert_eq!(again["result"]["snapshot"]["binding"], snapshot["binding"]);
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
    assert_eq!(
        renewed["result"]["snapshot"]["identity"]["transport"]["endpoint"],
        replacement.endpoint
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
    let current = fixture.current_state();
    let failure = current["notification_delivery_failures"]
        .as_object()
        .unwrap()
        .values()
        .find(|failure| failure["retryable"] == false)
        .expect("explicit failure receipt");
    assert_eq!(failure["retryable"], false, "{failure}");
    // A later safe connection recovery must not batch-replay the unknown sibling.
    let socket = PathBuf::from(bridge.endpoint.strip_prefix("unix://").unwrap());
    let offline = socket.with_extension("offline");
    std::fs::rename(&socket, &offline).unwrap();
    let later = desktop_command(
        &fixture,
        &[
            "sendmessage",
            "--to",
            id,
            "--subject",
            "safe-after-unknown",
            "only this never-submitted message may resume",
        ],
    );
    assert!(!later.status.success());
    bridge.disconnect_send.store(false, Ordering::SeqCst);
    std::fs::rename(&offline, &socket).unwrap();
    assert!(wait_until(Duration::from_secs(6), || bridge
        .sends
        .lock()
        .unwrap()
        .len()
        >= 2));
    std::thread::sleep(Duration::from_millis(1200));
    let texts = bridge.sends.lock().unwrap();
    assert_eq!(
        texts.len(),
        2,
        "only the new unsubmitted message may resume"
    );
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.contains("unknown-bridge-result"))
            .count(),
        1,
        "a safe sibling retry must not resend a post-write unknown result"
    );
    drop(texts);
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

/// An external caller with no agent facts: no session/thread, no product
/// originator override. It still resolves project context from cwd like any
/// real out-of-process CLI user, but it carries no Desktop bridge identity.
fn anonymous_command(fixture: &Fixture, args: &[&str]) -> Output {
    fixture
        .configured_command(args, None)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
        .output()
        .unwrap()
}

#[test]
fn anonymous_cli_send_to_directory_peer_delivers_precise_thread_to_native_bridge() {
    let fixture = Fixture::without_tmux("desktop-anon-cli");
    let runtime = fixture.host_state.join("runtime/desktop");
    let bridge = DesktopBridgeFixture::start_at(&fixture.root, &runtime);
    publish_capability(&fixture, &bridge);

    // 1. Existing Desktop receiver registers; its worker identity is returned.
    let enrolled = desktop_command(&fixture, &["context"]);
    assert!(
        enrolled.status.success(),
        "{}",
        String::from_utf8_lossy(&enrolled.stderr)
    );
    let enrolled: Value = serde_json::from_slice(&enrolled.stdout).unwrap();
    let worker = enrolled["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let desktop_session = enrolled["result"]["snapshot"]["binding"]["session_id"]
        .as_str()
        .unwrap();
    let registered_workers = fixture.current_state()["workers"].clone();

    // 2. External process with no agent facts queries the public directory.
    let dir = anonymous_command(&fixture, &["directory"]);
    assert!(
        dir.status.success(),
        "{}",
        String::from_utf8_lossy(&dir.stderr)
    );
    let dir: Value = serde_json::from_slice(&dir.stdout).unwrap();
    let target = dir["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["worker_id"].as_str() == Some(worker))
        .expect("directory exposes the registered Desktop peer");
    let peer_session = target["session_id"].as_str().unwrap();
    assert_eq!(peer_session, desktop_session);
    assert_eq!(
        target["transport"]["kind"].as_str().unwrap(),
        "desktop_bridge"
    );

    // 3. Anonymous CLI send from that external process, addressed to the
    // directory-discovered peer and its current session. No reply obligation
    // is allowed for an anonymous sender, and none is requested here.
    let sent = anonymous_command(
        &fixture,
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            worker,
            "--target-session",
            peer_session,
            "--subject",
            "anon-bridge-subject",
            "anonymous bridge body must reach desktop-thread verbatim",
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

    // 4. The actual native bridge socket receives the wake for the precise
    // target thread. Full message content follows through the public recv API.
    assert!(
        wait_until(Duration::from_secs(5), || !bridge
            .sends
            .lock()
            .unwrap()
            .is_empty()),
        "native bridge consumer never received the anonymous send"
    );
    let texts = bridge.sends.lock().unwrap();
    let wake_matched = texts
        .iter()
        .any(|text| text.contains("anon-bridge-subject"));
    assert!(
        wake_matched,
        "native bridge did not receive the message wake: {texts:?}"
    );
    drop(texts);
    let threads = bridge.received_threads.lock().unwrap();
    assert!(
        threads.iter().any(|thread| thread == "desktop-thread"),
        "native bridge was not addressed to the registered Desktop thread: {threads:?}"
    );
    drop(threads);

    let received = desktop_command(
        &fixture,
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "desktop-anonymous-consume",
        ],
    );
    assert!(
        received.status.success(),
        "{} {}",
        String::from_utf8_lossy(&received.stdout),
        String::from_utf8_lossy(&received.stderr)
    );
    let received: Value = serde_json::from_slice(&received.stdout).unwrap();
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "anon-bridge-subject")
        .expect("Desktop recipient consumes the anonymous message through recv");
    assert_eq!(message["from"], "unknown");
    assert_eq!(
        message["body"],
        "anonymous bridge body must reach desktop-thread verbatim"
    );

    // 5. Durable message persisted with no new sender identity.
    let current = fixture.current_state();
    let stored_message = current["msgs"]
        .as_object()
        .unwrap()
        .values()
        .find(|message| message["subject"] == "anon-bridge-subject")
        .expect("current anonymous message persists after consumption");
    assert_eq!(stored_message["from"], "unknown");
    assert_eq!(stored_message["state"], "read");
    assert_eq!(
        current["workers"], registered_workers,
        "anonymous send must not register a new sender identity"
    );
}
