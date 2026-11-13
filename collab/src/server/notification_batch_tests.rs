use super::*;
use crate::proto::RexEndpoint;
use crate::server::state::{Event, NotificationSubscription, State, WorkerRec};
use serde_json::Value;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

fn rex_transport(
    agent_session: &str,
    server: &str,
    rex_session: &str,
    block: &str,
) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some(agent_session.into()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(RexEndpoint {
            server: server.into(),
            session_id: rex_session.into(),
            block_id: block.into(),
            host_id: Some("host".into()),
            instance_id: "instance".into(),
            cwd: "/tmp".into(),
        }),
        capabilities: vec!["send_message_to_block".into()],
        self_check: "test rex transport".into(),
    }
}

fn seed_rex_route(server: &Server) -> crate::scope::RouteScope {
    let project_scope_id =
        crate::server::GlobalState::canonical_project_scope(&server.root).unwrap();
    let app_scope_id = AppServerId::new("appserver-cli").unwrap();
    server.commit(&[Event::GlobalProjectRegistered {
        registration: ProjectRegistration::new(project_scope_id.clone(), app_scope_id.clone())
            .unwrap(),
    }]);
    crate::scope::RouteScope {
        project_scope_id,
        app_scope_id,
    }
}

fn rex_binding_value(
    route: &crate::scope::RouteScope,
    name: &str,
    worker: &str,
    transport: SelectedTransport,
) -> crate::server::binding_slot::BindingValue {
    let binding = RuntimeBinding::new_with_session(
        route.project_scope_id.clone(),
        route.app_scope_id.clone(),
        crate::identity::AgentId::new(worker).unwrap(),
        crate::identity::RuntimeId::new(format!("runtime-{name}")).unwrap(),
        crate::identity::BindingId::new(format!("binding-{name}")).unwrap(),
        Some(crate::identity::SessionId::new(transport.session_id.clone().unwrap()).unwrap()),
        None,
    )
    .unwrap();
    crate::server::binding_slot::BindingValue::new(binding, transport)
}

fn seed_current_rex_bindings(
    server: &Server,
    route: &crate::scope::RouteScope,
    bindings: &[(&str, &str, &SelectedTransport)],
) {
    let mut control = crate::server::binding_slot::BindingControlSet::new();
    for (name, worker, transport) in bindings {
        control
            .commit(
                &crate::server::binding_slot::IdentityBindingKey::new(
                    route.project_scope_id.clone(),
                    route.app_scope_id.clone(),
                    name.to_string(),
                ),
                rex_binding_value(route, name, worker, (*transport).clone()),
                None,
            )
            .unwrap();
    }
    crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
        .save(&control)
        .unwrap();
}

fn seed_duplicate_current_rex_bindings(
    server: &Server,
    route: &crate::scope::RouteScope,
    bindings: &[(&str, &str, &SelectedTransport)],
) {
    let mut slots = Vec::new();
    for (name, worker, transport) in bindings {
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            route.project_scope_id.clone(),
            route.app_scope_id.clone(),
            name.to_string(),
        );
        let mut slot = crate::server::binding_slot::IdentityBindingSlot::new(key.clone());
        slot.commit(
            &key,
            rex_binding_value(route, name, worker, (*transport).clone()),
            None,
        )
        .unwrap();
        slots.push(slot);
    }
    let control = crate::server::binding_slot::BindingControlSet::from_parts(slots, Vec::new());
    crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
        .save(&control)
        .unwrap();
}

fn register_rex_worker(server: &Server, worker: &str, transport: &SelectedTransport) -> String {
    let subscription_id = register_and_subscribe(server, worker);
    {
        let mut state = server.state.lock().unwrap();
        state.workers.get_mut(worker).unwrap().transport = Some(transport.clone());
        let subscription = state
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap();
        subscription.method = "rex".into();
        subscription.target = notification_target(transport).unwrap().into_owned();
    }
    subscription_id
}

fn send_notify(server: &Server, from: &str, to: &str) -> Resp {
    handle_send(
        server,
        from.into(),
        to.into(),
        "notify".into(),
        Some(format!("rex-current-owner-{to}")),
        format!("DETAIL-{to}"),
        None,
        "immediate".into(),
    )
}

fn assert_rex_refusal_with_state(response: &Resp, error_code: &str, server: &Server) {
    assert!(!response.ok, "{response:?}");
    let error = response.error.as_deref().unwrap_or_default();
    assert!(
        error.contains(error_code),
        "expected {error_code}, got {error}"
    );
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    let message = &state.msgs[message_id];
    assert_eq!(message.state, "pending");
    assert_eq!(message.wake_attempt_count, 0);
    assert!(!state
        .notification_delivery_accepted
        .contains_key(message_id));
    assert!(!state
        .notification_delivery_bindings
        .contains_key(message_id));
}

fn test_server() -> (Arc<Server>, std::path::PathBuf) {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-notification-batch-{}-{sequence}",
        std::process::id()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let state_path = server_dir.join("state.json");
    current_state::save(&state_path, &State::default()).unwrap();
    (
        Arc::new(Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            state_path,
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            register_gate: Arc::new(Mutex::new(())),
            state: Mutex::new(State::default()),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some(candidate.endpoint.clone()),
                    namespace: Some(candidate.namespace.clone()),
                    session_id: Some(candidate.session_id.clone()),
                    thread_id: Some(candidate.thread_id.clone()),
                    tmux_endpoint: None,
                    rex_endpoint: None,
                    capabilities: vec!["send_message_to_thread".into()],
                    self_check: "test appserver".into(),
                })
            }),
            appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
                Ok(serde_json::json!({"accepted": true}))
            }),
            appserver_thread_status: Arc::new(|_, thread_id| {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                }))
            }),
            appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({"archived": true}))),
            rex_monitor: Default::default(),
            mailbox_notify: tokio::sync::Notify::new(),
        }),
        root,
    )
}

fn public_collab_binary() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("COLLAB_TEST_BINARY") {
        return std::path::PathBuf::from(path);
    }
    let executable = std::env::current_exe().expect("current test executable");
    let debug_dir = executable
        .parent()
        .and_then(std::path::Path::parent)
        .expect("test executable is below target/debug");
    let binary = debug_dir.join("collab");
    assert!(
        binary.is_file(),
        "public Collab binary is missing at {}; build it before this test",
        binary.display()
    );
    binary
}

struct PublicDaemon {
    socket: std::path::PathBuf,
    root: Option<std::path::PathBuf>,
    running: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl PublicDaemon {
    async fn start(root: std::path::PathBuf, host_paths: HostPaths) -> Self {
        let socket = host_paths.socket_path();
        let mut daemon = Self {
            socket,
            root: Some(root.clone()),
            running: Some(tokio::spawn(run_with_host_paths(
                Scope { root },
                host_paths,
            ))),
        };
        daemon.wait_ready().await;
        daemon
    }

    async fn wait_ready(&mut self) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::os::unix::net::UnixStream::connect(&self.socket).is_ok() {
                return;
            }
            if self
                .running
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                let result = self.running.take().expect("running daemon task").await;
                panic!(
                    "isolated public daemon exited before publishing {}: {result:?}",
                    self.socket.display()
                );
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "isolated public daemon did not publish {}",
                self.socket.display()
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    async fn shutdown(mut self) {
        if let Some(running) = self.running.take() {
            running.abort();
            let _ = running.await;
        }
        if let Some(root) = self.root.take() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl Drop for PublicDaemon {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.abort();
        }
        if let Some(root) = self.root.take() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

const PUBLIC_REX_SERVER: &str = "unix:///tmp/rex-fixture.sock";
const PUBLIC_REX_SESSION: &str = "session:11111111-1111-1111-1111-111111111111";
const PUBLIC_SENDER_BLOCK: &str = "block:55555555-5555-5555-5555-555555555555";
const PUBLIC_RECIPIENT_BLOCK: &str = "block:22222222-2222-2222-2222-222222222222";
const PUBLIC_MOVED_BLOCK: &str = "block:66666666-6666-6666-6666-666666666666";

#[derive(Debug)]
struct PublicCliResult {
    success: bool,
    value: Value,
    stdout: String,
    stderr: String,
}

fn public_cli_command(
    root: &std::path::Path,
    host_paths: &HostPaths,
    agent_session: &str,
) -> Command {
    let mut command = Command::new(public_collab_binary());
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "en_US.UTF-8")
        .env("HOME", root.join("home"))
        .env("CODEX_HOME", root.join("home"))
        .env("COLLAB_STATE_DIR", host_paths.state_root())
        .env("CODEX_SESSION_ID", agent_session)
        .env("CODEX_THREAD_ID", format!("thread-{agent_session}"));
    command
}

fn decode_public_cli_output(output: Output) -> PublicCliResult {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let value = serde_json::from_str(stdout.trim())
        .or_else(|stdout_error| {
            stderr
                .lines()
                .find_map(|line| line.strip_prefix("collab response: "))
                .ok_or(stdout_error)
                .and_then(serde_json::from_str)
        })
        .unwrap_or_else(|error| {
            panic!(
                "public Collab CLI did not return JSON: {error}; stdout={stdout} stderr={stderr}"
            )
        });
    PublicCliResult {
        success: output.status.success(),
        value,
        stdout,
        stderr,
    }
}

fn run_public_cli(
    root: &std::path::Path,
    host_paths: &HostPaths,
    agent_session: &str,
    args: &[&str],
) -> PublicCliResult {
    let output = public_cli_command(root, host_paths, agent_session)
        .args(args)
        .output()
        .expect("run public Collab CLI");
    decode_public_cli_output(output)
}

fn spawn_public_cli(
    root: &std::path::Path,
    host_paths: &HostPaths,
    agent_session: &str,
    args: &[&str],
) -> Child {
    public_cli_command(root, host_paths, agent_session)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn public Collab CLI")
}

async fn wait_public_cli(child: Child) -> PublicCliResult {
    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .expect("join public Collab CLI")
        .expect("wait for public Collab CLI");
    decode_public_cli_output(output)
}

fn public_context_args(operation: &str, session_id: &str, block_id: &str) -> Vec<String> {
    let facts = serde_json::json!({
        "session_id": session_id,
        "rex": {
            "server": PUBLIC_REX_SERVER,
            "session_id": PUBLIC_REX_SESSION,
            "block_id": block_id,
            "cwd": "/tmp",
        }
    });
    vec![
        "context".into(),
        "--op".into(),
        operation.into(),
        "--provide".into(),
        facts.to_string(),
    ]
}

fn public_snapshot(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .unwrap_or(value)
}

fn public_name(snapshot: &Value) -> String {
    snapshot["name"]
        .as_str()
        .or_else(|| snapshot["identity"]["name"].as_str())
        .unwrap_or_else(|| panic!("public Context did not return a stable name: {snapshot}"))
        .to_owned()
}

fn public_worker_id(snapshot: &Value) -> String {
    snapshot["identity"]["worker_id"]
        .as_str()
        .unwrap_or_else(|| panic!("public Context did not return a worker id: {snapshot}"))
        .to_owned()
}

fn register_public_rex(
    root: &std::path::Path,
    host_paths: &HostPaths,
    agent_session: &str,
    block_id: &str,
    label: &str,
) -> Value {
    let args = public_context_args(
        &format!("{label}-{}-{}", std::process::id(), now_ms()),
        agent_session,
        block_id,
    );
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let result = run_public_cli(root, host_paths, agent_session, &args);
    assert!(
        result.success && result.value["result"]["outcome"] == "completed",
        "public Context registration failed: success={} value={} stdout={} stderr={}",
        result.success,
        result.value,
        result.stdout,
        result.stderr
    );
    result.value
}

fn public_send_args<'a>(name: &'a str, subject: &'a str, body: &'a str) -> Vec<String> {
    vec![
        "send".into(),
        "--to".into(),
        name.into(),
        "--subject".into(),
        subject.into(),
        "--content".into(),
        body.into(),
    ]
}

fn public_method_count(calls: &[Value], method: &str) -> usize {
    calls
        .iter()
        .filter(|call| rex_method(call) == method)
        .count()
}

fn public_write_targets(calls: &[Value]) -> Vec<String> {
    calls
        .iter()
        .filter(|call| rex_method(call) == "com.superlogical.terminal.write")
        .filter_map(|call| {
            let stdin = call["stdin"].as_str()?;
            let request: Value = serde_json::from_str(stdin).ok()?;
            Some(format!(
                "{}#{}",
                request["session_id"].as_str()?,
                request["block_id"].as_str()?
            ))
        })
        .collect()
}

fn public_message_id(result: &PublicCliResult) -> String {
    result.value["msg_id"]
        .as_str()
        .or_else(|| result.value["data"]["msg_id"].as_str())
        .unwrap_or_default()
        .to_owned()
}

fn public_message_id_for_subject(state: &State, recipient: &str, subject: &str) -> String {
    state
        .msgs
        .values()
        .find(|message| message.to == recipient && message.subject.as_deref() == Some(subject))
        .map(|message| message.id.clone())
        .unwrap_or_default()
}

fn public_state_summary(state: &State, message_id: &str, recipient_worker: &str) -> Value {
    let message = state.msgs.get(message_id);
    let current_endpoint = state
        .workers
        .get(recipient_worker)
        .and_then(selected_transport_for_worker)
        .and_then(|transport| transport.rex_endpoint.clone());
    serde_json::json!({
        "message_id": message_id,
        "message": message,
        "current_worker_endpoint": current_endpoint,
        "selected_binding": state.notification_delivery_bindings.get(message_id),
        "failure": state.notification_delivery_failures.get(message_id),
        "accepted": state.notification_delivery_accepted.contains_key(message_id),
        "accepted_ms": state.notification_delivery_accepted.get(message_id),
        "wake_attempt_count": message.map(|message| message.wake_attempt_count),
        "retry_attempted": message.map(|message| message.retry_attempted),
        "subscription_id": state.wake_bindings.get(message_id),
    })
}

fn public_terminal_calls(calls: &[Value], method: &str) -> Vec<Value> {
    calls
        .iter()
        .filter(|call| rex_method(call) == method)
        .map(|call| {
            let request = call["stdin"]
                .as_str()
                .and_then(|stdin| serde_json::from_str::<Value>(stdin).ok());
            serde_json::json!({
                "serial": call["serial"],
                "argv": call["argv"],
                "request": request,
            })
        })
        .collect()
}

fn public_cli_summary(result: &PublicCliResult) -> Value {
    serde_json::json!({
        "success": result.success,
        "response": result.value,
        "stdout": result.stdout,
        "stderr": result.stderr,
    })
}

fn emit_public_observation(label: &str, observation: Value) {
    eprintln!(
        "PUBLIC_REX_OBSERVATION {}",
        serde_json::json!({
            "label": label,
            "observation": observation,
        })
    );
}

fn public_state(root: &std::path::Path, host_paths: &HostPaths) -> State {
    let canonical = std::fs::canonicalize(root).unwrap();
    let storage = host_paths.app_storage_root(
        canonical.to_str().expect("temporary root is UTF-8"),
        crate::identity::CLI_APP_SERVER_ID,
    );
    current_state::load(&storage.join(".agent-collab/server/state.json"), false).unwrap()
}

struct RexCliFixture {
    dir: std::path::PathBuf,
    barriers: std::path::PathBuf,
    scenario_file: std::path::PathBuf,
    blocks_file: std::path::PathBuf,
    old_log_dir: Option<std::ffi::OsString>,
    old_scenario: Option<std::ffi::OsString>,
    old_scenario_file: Option<std::ffi::OsString>,
    old_barrier_dir: Option<std::ffi::OsString>,
    old_blocks_file: Option<std::ffi::OsString>,
}

impl RexCliFixture {
    fn new(root: &std::path::Path, scenario: &str) -> Self {
        let dir = root.join("rex-cli-fixture");
        std::fs::create_dir_all(&dir).unwrap();
        let barriers = dir.join("barriers");
        std::fs::create_dir_all(&barriers).unwrap();
        let scenario_file = dir.join("scenario");
        let blocks_file = dir.join("blocks.json");
        std::fs::write(&scenario_file, scenario).unwrap();
        std::fs::write(
            &blocks_file,
            serde_json::to_vec(&serde_json::json!([
                "block:22222222-2222-2222-2222-222222222222"
            ]))
            .unwrap(),
        )
        .unwrap();
        let fixture = dir.join("rex-cli");
        std::fs::write(
            fixture.as_path(),
            include_str!("../adapters/rex/fixture_rex.py"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fixture.as_path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        crate::client::adapters::rex::set_test_cli(Some(fixture));
        crate::client::adapters::rex::set_test_timeout(Some(std::time::Duration::from_secs(5)));
        let old_log_dir = std::env::var_os("REXPRIMITIVE_LOG_DIR");
        let old_scenario = std::env::var_os("REXPRIMITIVE_SCENARIO");
        let old_scenario_file = std::env::var_os("REXPRIMITIVE_SCENARIO_FILE");
        let old_barrier_dir = std::env::var_os("REXPRIMITIVE_BARRIER_DIR");
        let old_blocks_file = std::env::var_os("REXPRIMITIVE_BLOCKS_FILE");
        std::env::set_var("REXPRIMITIVE_LOG_DIR", &dir);
        std::env::set_var("REXPRIMITIVE_SCENARIO", scenario);
        std::env::set_var("REXPRIMITIVE_SCENARIO_FILE", &scenario_file);
        std::env::set_var("REXPRIMITIVE_BARRIER_DIR", &barriers);
        std::env::set_var("REXPRIMITIVE_BLOCKS_FILE", &blocks_file);
        Self {
            dir,
            barriers,
            scenario_file,
            blocks_file,
            old_log_dir,
            old_scenario,
            old_scenario_file,
            old_barrier_dir,
            old_blocks_file,
        }
    }

    fn calls(&self) -> Vec<Value> {
        let path = self.dir.join("invocations.jsonl");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).expect("fixture invocation JSON"))
            .collect()
    }

    fn set_scenario(&self, scenario: &str) {
        std::fs::write(&self.scenario_file, scenario).unwrap();
    }

    fn set_blocks(&self, blocks: &[&str]) {
        std::fs::write(&self.blocks_file, serde_json::to_vec(&blocks).unwrap()).unwrap();
    }

    fn arm_method_pause(&self, method: &str) {
        for name in ["pause-method", "method-entered", "method-release"] {
            let _ = std::fs::remove_file(self.barriers.join(name));
        }
        std::fs::write(self.barriers.join("pause-method"), method).unwrap();
    }

    fn wait_method_pause(&self) {
        let marker = self.barriers.join("method-entered");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.is_file() {
            assert!(
                std::time::Instant::now() < deadline,
                "Rex fixture did not enter the armed method barrier"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn release_method_pause(&self) {
        std::fs::write(self.barriers.join("method-release"), "release").unwrap();
    }
}

impl Drop for RexCliFixture {
    fn drop(&mut self) {
        crate::client::adapters::rex::set_test_cli(None);
        crate::client::adapters::rex::set_test_timeout(None);
        match self.old_log_dir.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_LOG_DIR", value),
            None => std::env::remove_var("REXPRIMITIVE_LOG_DIR"),
        }
        match self.old_scenario.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_SCENARIO", value),
            None => std::env::remove_var("REXPRIMITIVE_SCENARIO"),
        }
        match self.old_scenario_file.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_SCENARIO_FILE", value),
            None => std::env::remove_var("REXPRIMITIVE_SCENARIO_FILE"),
        }
        match self.old_barrier_dir.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_BARRIER_DIR", value),
            None => std::env::remove_var("REXPRIMITIVE_BARRIER_DIR"),
        }
        match self.old_blocks_file.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_BLOCKS_FILE", value),
            None => std::env::remove_var("REXPRIMITIVE_BLOCKS_FILE"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rex_method(call: &Value) -> String {
    let argv = call["argv"].as_array().expect("fixture argv array");
    for candidate in [
        "server.status",
        "session.list",
        "session.list_blocks",
        "com.superlogical.terminal.write",
        "send-key",
    ] {
        if argv.iter().any(|value| value.as_str() == Some(candidate)) {
            return candidate.into();
        }
    }
    String::new()
}

fn current_rex_binding(
    server: &Server,
    route: &crate::scope::RouteScope,
    worker: &str,
) -> RuntimeBinding {
    let control =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, worker)
        .unwrap()
        .unwrap()
        .clone();
    control
        .slot(&key)
        .and_then(|slot| slot.active_binding())
        .expect("current Rex binding")
        .binding
        .clone()
}

fn persist_rex_worker_binding(
    server: &Server,
    route: &crate::scope::RouteScope,
    worker: &str,
    transport: &SelectedTransport,
) {
    let binding = current_rex_binding(server, route, worker);
    server.commit(&[
        Event::GlobalRuntimeBound {
            binding: binding.clone(),
        },
        Event::GlobalCurrentThreadRouteSet {
            binding: binding.clone(),
        },
    ]);
    let token = server.state.lock().unwrap().workers[worker].token.clone();
    let runtime = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    let mut ident = crate::identity::Identity {
        worker_id: worker.into(),
        token,
        project_scope: None,
        runtime: None,
        transport: None,
    };
    crate::identity::persist_current_binding_at(
        &server.host_paths,
        &crate::scope::Scope {
            root: server.root.clone(),
        },
        &mut ident,
        runtime,
        transport.clone(),
    )
    .unwrap();
}

fn manager_send_to_rex(
    manager: &ProjectRuntimeManager,
    root: &std::path::Path,
    recipient: &str,
) -> Resp {
    let context = ProjectContext::for_registered_root_with_app(
        root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    manager
        .dispatch_sync(
            Some(context),
            Req::Send {
                from: "unknown".into(),
                worker_id: None,
                token: None,
                command: None,
                to: recipient.into(),
                mtype: "notify".into(),
                subject: Some("rex-service-refresh".into()),
                body: "DETAIL-rex-service-refresh".into(),
                in_reply_to: None,
                delivery: "immediate".into(),
                reply_policy: None,
                target_session: None,
            },
        )
        .1
}

fn install_capture_sink(server: &mut Arc<Server>) -> Arc<Mutex<Vec<SelectedTransport>>> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    Arc::get_mut(server)
        .expect("unique test server")
        .appserver_notification_sink = Arc::new(move |transport, _, _, message_id, _, _| {
        observed.lock().unwrap().push(transport.clone());
        Ok(serde_json::json!({
            "accepted": true,
            "messageId": message_id,
        }))
    });
    calls
}

fn register_and_subscribe(server: &Server, worker_id: &str) -> String {
    let now = now_ms();
    let subscription_id = super::mailbox::default_direct_message_id(worker_id);
    server.commit(&[
        Event::Registered {
            worker: WorkerRec {
                id: worker_id.into(),
                token: format!("token-{worker_id}"),
                cwd: "/tmp".into(),
                registered_ms: now,
                transport: Some(SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
                    namespace: Some("codex_tui".into()),
                    session_id: Some(format!("session-{worker_id}")),
                    thread_id: Some(format!("thread-{worker_id}")),
                    tmux_endpoint: None,
                    rex_endpoint: None,
                    capabilities: vec!["send_message_to_thread".into()],
                    self_check: "test appserver".into(),
                }),
            },
        },
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: worker_id.into(),
                event: "direct-message".into(),
                subject: None,
                target: format!("thread-{worker_id}"),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 300_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
                goal_content: None,
            },
        },
    ]);
    subscription_id
}

fn queue_message(
    server: &Server,
    worker_id: &str,
    subscription_id: &str,
    message_id: &str,
    created_ms: i64,
) {
    queue_message_with_body(
        server,
        worker_id,
        subscription_id,
        message_id,
        created_ms,
        format!("DETAIL-{message_id}"),
    );
}

fn queue_message_with_body(
    server: &Server,
    worker_id: &str,
    subscription_id: &str,
    message_id: &str,
    created_ms: i64,
    body: String,
) {
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: message_id.into(),
                from: "peer".into(),
                to: worker_id.into(),
                mtype: "notify".into(),
                subject: Some(format!("topic-{message_id}")),
                body,
                in_reply_to: None,
                created_ms,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: message_id.into(),
            subscription_id: subscription_id.into(),
        },
    ]);
}

#[test]
fn desktop_connect_failure_preserves_retry_proof_for_the_entire_batch() {
    let (mut server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("recipient")
        .unwrap()
        .transport
        .as_mut()
        .unwrap()
        .kind = TransportKind::DesktopBridge;
    server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .get_mut(&subscription)
        .unwrap()
        .method = "desktop_bridge".into();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink =
        Arc::new(|_, _, _, _, _, _| Err("DESKTOP_BRIDGE_NOT_SUBMITTED: connection refused".into()));
    let now = now_ms();
    for id in ["batch-a", "batch-b"] {
        queue_message(&server, "recipient", &subscription, id, now);
    }
    assert!(
        !attempt_notification_detailed_with_at(&server, "batch-a", &subscription, now).accepted()
    );
    {
        let state = server.state.lock().unwrap();
        for id in ["batch-a", "batch-b"] {
            assert_eq!(state.msgs[id].wake_attempt_count, 1);
            assert!(desktop_connect_retry(&state, &state.msgs[id]));
        }
    }
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink =
        Arc::new(|_, _, _, _, _, _| Ok(serde_json::json!({"accepted": true})));
    for id in ["batch-a", "batch-b"] {
        assert!(
            attempt_notification_detailed_with_at(&server, id, &subscription, now + 1).accepted()
        );
    }
    let state = server.state.lock().unwrap();
    for id in ["batch-a", "batch-b"] {
        assert_eq!(state.msgs[id].wake_attempt_count, 2);
        assert!(!desktop_connect_retry(&state, &state.msgs[id]));
        assert!(state.notification_delivery_accepted.contains_key(id));
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_batch_failure_records_every_submitted_member_without_resend() {
    let (mut server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    let sink_calls = Arc::new(Mutex::new(0usize));
    let sink_calls_for_sink = sink_calls.clone();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
        *sink_calls_for_sink.lock().unwrap() += 1;
        Err("test sink unknown outcome".into())
    });
    let now = now_ms();
    for id in ["batch-a", "batch-b"] {
        queue_message(&server, "recipient", &subscription, id, now);
        server.commit(&[Event::DeliveryMode {
            msg_id: id.into(),
            mode: "immediate".into(),
            source_thread_id: None,
        }]);
    }
    let attempt = attempt_notification_detailed_with_at(&server, "batch-a", &subscription, now);
    assert!(!attempt.accepted(), "{attempt:?}");
    {
        let state = server.state.lock().unwrap();
        for id in ["batch-a", "batch-b"] {
            let failure = state
                .notification_delivery_failures
                .get(id)
                .unwrap_or_else(|| panic!("unknown batch failure must record {id}"));
            assert_eq!(failure.error, "test sink unknown outcome", "{id}");
            assert!(!failure.retryable, "{id}: unknown outcome must not retry");
            assert_eq!(state.msgs[id].wake_attempt_count, 1, "{id}");
            assert!(
                !state.notification_delivery_accepted.contains_key(id),
                "{id}"
            );
        }
    }
    assert_eq!(*sink_calls.lock().unwrap(), 1);
    let next = attempt_notification_detailed_with_at(&server, "batch-b", &subscription, now + 1);
    assert!(
        matches!(next, NotificationAttempt::NotAttempted(_)),
        "unknown batch must not be resent: {next:?}"
    );
    assert_eq!(*sink_calls.lock().unwrap(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_notification_target_is_the_full_service_scoped_terminal_address() {
    let rex = SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some("agent-session".into()),
        thread_id: Some("agent-thread".into()),
        tmux_endpoint: None,
        rex_endpoint: Some(RexEndpoint {
            server: "macstudio".into(),
            session_id: "rex-session".into(),
            block_id: "rex-block".into(),
            host_id: None,
            instance_id: "instance-7".into(),
            cwd: "/tmp".into(),
        }),
        capabilities: Vec::new(),
        self_check: "test rex transport".into(),
    };
    assert_eq!(
        notification_target(&rex).unwrap().as_ref(),
        r#"["macstudio","rex-session","rex-block"]"#
    );
    let mut subscription = owned_subscription("worker", "rex-lease", "armed");
    subscription.method = "rex".into();
    subscription.target = notification_target(&rex).unwrap().into_owned();
    assert!(subscription_matches_transport(&subscription, &rex));
    subscription.target = r#"["other-server","rex-session","rex-block"]"#.into();
    assert!(
        !subscription_matches_transport(&subscription, &rex),
        "the same Rex session/block on another server must not match"
    );
    subscription.target = notification_target(&rex).unwrap().into_owned();
    subscription.method = "appserver".into();
    assert!(
        !subscription_matches_transport(&subscription, &rex),
        "a Rex target must not match an old transport method"
    );
    let appserver = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/test.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session".into()),
        thread_id: Some("thread-id".into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: Vec::new(),
        self_check: "test appserver".into(),
    };
    assert_eq!(
        notification_target(&appserver).unwrap().as_ref(),
        "thread-id",
        "old transports must keep borrowing their thread id"
    );
}

#[test]
fn rex_current_owner_stale_worker_after_replacement_is_not_attempted() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let stale_transport = rex_transport("agent-old", "rex-server", "rex-session", "shared-block");
    register_rex_worker(&server, "old", &stale_transport);
    let replacement = rex_transport("agent-new", "rex-server", "rex-session", "shared-block");
    seed_current_rex_bindings(&server, &route, &[("Bob", "new", &replacement)]);
    let sink_calls = install_capture_sink(&mut server);

    let response = send_notify(&server, "sender", "old");

    assert_rex_refusal_with_state(&response, "REX_CURRENT_BINDING_MISSING", &server);
    assert!(sink_calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_duplicate_block_current_is_not_attempted() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let first = rex_transport("agent-a", "rex-server", "rex-session", "shared-block");
    let second = rex_transport("agent-b", "rex-server", "rex-session", "shared-block");
    register_rex_worker(&server, "a", &first);
    register_rex_worker(&server, "b", &second);
    seed_duplicate_current_rex_bindings(
        &server,
        &route,
        &[("Alice", "a", &first), ("Bob", "b", &second)],
    );
    let sink_calls = install_capture_sink(&mut server);

    let response = send_notify(&server, "sender", "a");

    assert_rex_refusal_with_state(&response, "REX_BLOCK_OWNER_AMBIGUOUS", &server);
    assert!(sink_calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_missing_current_is_not_attempted() {
    let (mut server, root) = test_server();
    let _route = seed_rex_route(&server);
    let cached = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
    register_rex_worker(&server, "a", &cached);
    let sink_calls = install_capture_sink(&mut server);

    let response = send_notify(&server, "sender", "a");

    assert_rex_refusal_with_state(&response, "REX_CURRENT_BINDING_MISSING", &server);
    assert!(sink_calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_stale_endpoint_is_not_attempted() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let cached = rex_transport("agent-a", "rex-server", "rex-session", "old-block");
    register_rex_worker(&server, "a", &cached);
    let current = rex_transport("agent-a", "rex-server", "rex-session", "new-block");
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    let sink_calls = install_capture_sink(&mut server);

    let response = send_notify(&server, "sender", "a");

    assert_rex_refusal_with_state(&response, "REX_CURRENT_BINDING_CHANGED", &server);
    assert!(sink_calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_unique_binding_delivers_and_records_selection() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let current = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
    register_rex_worker(&server, "a", &current);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    let sink_calls = install_capture_sink(&mut server);

    let response = send_notify(&server, "sender", "a");

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["notification"], "rex-input-submitted");
    assert_eq!(response.data["consumed"], false);
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[message_id].state, "pending");
        assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
        assert!(state
            .notification_delivery_accepted
            .contains_key(message_id));
        let selected = state
            .notification_delivery_bindings
            .get(message_id)
            .unwrap();
        assert_eq!(selected.transport, current);
        assert_eq!(selected.binding.agent_id.as_str(), "a");
    }
    let calls = sink_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], current);
    drop(calls);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_same_block_on_separate_servers_remains_independent() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let first = rex_transport("agent-a", "rex-server-a", "rex-session", "shared-block");
    let second = rex_transport("agent-b", "rex-server-b", "rex-session", "shared-block");
    register_rex_worker(&server, "a", &first);
    register_rex_worker(&server, "b", &second);
    seed_current_rex_bindings(
        &server,
        &route,
        &[("Alice", "a", &first), ("Bob", "b", &second)],
    );
    let sink_calls = install_capture_sink(&mut server);

    let first_response = send_notify(&server, "sender", "a");
    let second_response = send_notify(&server, "sender", "b");

    assert!(first_response.ok, "{first_response:?}");
    assert!(second_response.ok, "{second_response:?}");
    let calls = sink_calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], first);
    assert_eq!(calls[1], second);
    drop(calls);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_refusal_preserves_entire_durable_batch() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let stale = rex_transport("agent-old", "rex-server", "rex-session", "shared-block");
    let subscription = register_rex_worker(&server, "old", &stale);
    let replacement = rex_transport("agent-new", "rex-server", "rex-session", "shared-block");
    seed_current_rex_bindings(&server, &route, &[("Bob", "new", &replacement)]);
    let sink_calls = install_capture_sink(&mut server);
    let now = now_ms();
    for id in ["owner-batch-a", "owner-batch-b"] {
        queue_message(&server, "old", &subscription, id, now);
        server.commit(&[Event::DeliveryMode {
            msg_id: id.into(),
            mode: "immediate".into(),
            source_thread_id: None,
        }]);
    }

    let attempt =
        attempt_notification_detailed_with_at(&server, "owner-batch-a", &subscription, now);

    assert!(
        matches!(attempt, NotificationAttempt::NotAttempted(_)),
        "{attempt:?}"
    );
    assert!(sink_calls.lock().unwrap().is_empty());
    let state = server.state.lock().unwrap();
    for id in ["owner-batch-a", "owner-batch-b"] {
        assert_eq!(state.msgs[id].state, "pending");
        assert_eq!(state.msgs[id].wake_attempt_count, 0);
        assert!(!state.notification_delivery_accepted.contains_key(id));
        assert!(!state.notification_delivery_bindings.contains_key(id));
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_busy_publication_is_not_attempted() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let current = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
    register_rex_worker(&server, "a", &current);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    let sink_calls = install_capture_sink(&mut server);

    let wire_gate = wire_route_mutation_gate(&server);
    let _wire_guard = wire_gate.lock().unwrap();
    let _register_guard = server.register_gate.lock().unwrap();
    let response = send_notify(&server, "sender", "a");

    assert_rex_refusal_with_state(&response, "REX_BINDING_PUBLICATION_BUSY", &server);
    assert!(sink_calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_slow_delivery_serializes_replacement() {
    let (mut server, root) = test_server();
    let route = seed_rex_route(&server);
    let old_transport = rex_transport("agent-old", "rex-server", "rex-session", "shared-block");
    let old_subscription = register_rex_worker(&server, "old", &old_transport);
    let new_transport = rex_transport("agent-new", "rex-server", "rex-session", "shared-block");
    register_rex_worker(&server, "new", &new_transport);
    seed_current_rex_bindings(&server, &route, &[("Alice", "old", &old_transport)]);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_sink = calls.clone();
    let (sink_entered_tx, sink_entered_rx) = std::sync::mpsc::channel();
    let (release_sink_tx, release_sink_rx) = std::sync::mpsc::channel();
    let release_sink_rx = Mutex::new(release_sink_rx);
    let first_call = std::sync::atomic::AtomicBool::new(true);
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = Arc::new(move |transport, _, _, message_id, _, _| {
        calls_for_sink.lock().unwrap().push(transport.clone());
        if first_call.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = sink_entered_tx.send(());
            release_sink_rx.lock().unwrap().recv().unwrap();
        }
        Ok(serde_json::json!({
            "accepted": true,
            "messageId": message_id,
        }))
    });

    let now = now_ms();
    queue_message(&server, "old", &old_subscription, "slow-delivery", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "slow-delivery".into(),
        mode: "immediate".into(),
        source_thread_id: None,
    }]);
    let manager = Arc::new(ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap());
    let attempt_server = server.clone();
    let attempt_subscription = old_subscription.clone();
    let attempt = std::thread::spawn(move || {
        attempt_notification_detailed_with_at(
            &attempt_server,
            "slow-delivery",
            &attempt_subscription,
            now,
        )
    });
    sink_entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("delivery sink did not start");

    let key = crate::server::binding_slot::IdentityBindingKey::new(
        route.project_scope_id.clone(),
        route.app_scope_id.clone(),
        "Bob",
    );
    let replacement = rex_binding_value(&route, "Bob", "new", new_transport.clone());
    let (replacement_started_tx, replacement_started_rx) = std::sync::mpsc::channel();
    let (replacement_done_tx, replacement_done_rx) = std::sync::mpsc::channel();
    let replacement_manager = manager.clone();
    let replacement_thread = std::thread::spawn(move || {
        let _ = replacement_started_tx.send(());
        let result = replacement_manager.commit_control_change(
            crate::server::binding_store::ControlChange::Commit {
                key,
                value: replacement,
                expected_generation: None,
            },
        );
        let _ = replacement_done_tx.send(result);
    });
    replacement_started_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("replacement did not start");

    assert!(
        manager.register_gate.try_lock().is_err(),
        "slow sink must retain the shared register gate"
    );
    assert!(
        replacement_done_rx.try_recv().is_err(),
        "replacement must not publish while the old delivery is in flight"
    );
    let control =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
    assert_eq!(
        control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, "old",)
            .unwrap()
            .unwrap()
            .active_binding()
            .unwrap()
            .transport,
        old_transport
    );
    assert!(control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "new",)
        .unwrap()
        .is_none());

    release_sink_tx.send(()).unwrap();
    assert!(attempt.join().unwrap().accepted());
    replacement_done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("replacement did not finish")
        .unwrap();
    replacement_thread.join().unwrap();

    let old_response = send_notify(&server, "sender", "old");
    assert_rex_refusal_with_state(&old_response, "REX_CURRENT_BINDING_MISSING", &server);
    let new_response = send_notify(&server, "sender", "new");
    assert!(new_response.ok, "{new_response:?}");

    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["slow-delivery"].wake_attempt_count, 1);
    assert!(state
        .notification_delivery_accepted
        .contains_key("slow-delivery"));
    assert_eq!(
        state
            .notification_delivery_bindings
            .get("slow-delivery")
            .unwrap()
            .transport,
        old_transport
    );
    drop(state);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], old_transport);
    assert_eq!(calls[1], new_transport);
    drop(calls);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_outcomes_survive_replacement_and_retry() {
    for (label, sink_error) in [
        ("accepted", None),
        ("unknown", Some("test sink unknown outcome")),
    ] {
        let (mut server, root) = test_server();
        let route = seed_rex_route(&server);
        let old_transport = rex_transport("agent-old", "rex-server", "rex-session", "shared-block");
        let new_transport = rex_transport("agent-new", "rex-server", "rex-session", "shared-block");
        let subscription = register_rex_worker(&server, "old", &old_transport);
        register_rex_worker(&server, "new", &new_transport);
        seed_current_rex_bindings(&server, &route, &[("Alice", "old", &old_transport)]);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_sink = calls.clone();
        let first_call = std::sync::atomic::AtomicBool::new(true);
        let expected_error = sink_error.map(str::to_owned);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |transport, _, _, message_id, _, _| {
            calls_for_sink.lock().unwrap().push(transport.clone());
            if first_call.swap(false, std::sync::atomic::Ordering::SeqCst) {
                if let Some(error) = expected_error.clone() {
                    return Err(error);
                }
            }
            Ok(json!({
                "accepted": true,
                "messageId": message_id,
            }))
        });

        let first_response = send_notify(&server, "sender", "old");
        assert_eq!(
            first_response.ok,
            sink_error.is_none(),
            "{label}: {first_response:?}"
        );
        let message_id = first_response.data["msg_id"].as_str().unwrap().to_string();
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "{label}");
            assert_eq!(calls[0], old_transport, "{label}");
        }

        let original = {
            let state = server.state.lock().unwrap();
            let message = &state.msgs[&message_id];
            assert_eq!(message.to, "old", "{label}");
            assert_eq!(message.body, "DETAIL-old", "{label}");
            assert_eq!(message.state, "pending", "{label}");
            assert_eq!(message.wake_attempt_count, 1, "{label}");
            assert!(state.receive_receipts.is_empty(), "{label}");
            (
                state
                    .notification_delivery_accepted
                    .get(&message_id)
                    .copied(),
                state
                    .notification_delivery_failures
                    .get(&message_id)
                    .cloned(),
                state
                    .notification_delivery_bindings
                    .get(&message_id)
                    .cloned(),
            )
        };
        let (accepted, failure, selected) = &original;
        assert_eq!(accepted.is_some(), sink_error.is_none(), "{label}");
        assert_eq!(failure.is_some(), sink_error.is_some(), "{label}");
        if let Some(error) = sink_error {
            assert_eq!(failure.as_ref().unwrap().error, error, "{label}");
            assert!(!failure.as_ref().unwrap().retryable, "{label}");
        }
        assert_eq!(
            selected.as_ref().unwrap().transport,
            old_transport,
            "{label}"
        );
        assert_eq!(selected.as_ref().unwrap().binding.agent_id.as_str(), "old");

        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        manager
            .commit_control_change(crate::server::binding_store::ControlChange::Commit {
                key: crate::server::binding_slot::IdentityBindingKey::new(
                    route.project_scope_id.clone(),
                    route.app_scope_id.clone(),
                    "Bob",
                ),
                value: rex_binding_value(&route, "Bob", "new", new_transport.clone()),
                expected_generation: None,
            })
            .unwrap();

        let retry = attempt_notification_detailed_with_mode_at(
            &server,
            &message_id,
            &subscription,
            now_ms() + 1,
            true,
        );
        assert!(
            matches!(&retry, NotificationAttempt::NotAttempted(error)
                if error.contains("REX_CURRENT_BINDING_MISSING")),
            "{label}: {retry:?}"
        );
        assert_eq!(calls.lock().unwrap().len(), 1, "{label}");
        {
            let state = server.state.lock().unwrap();
            let message = &state.msgs[&message_id];
            assert_eq!(message.to, "old", "{label}");
            assert_eq!(message.body, "DETAIL-old", "{label}");
            assert_eq!(message.state, "pending", "{label}");
            assert_eq!(message.wake_attempt_count, 1, "{label}");
            assert!(!message.retry_attempted, "{label}");
            assert!(!state.handoff_wake_pending(message), "{label}");
            assert!(
                !state.mailbox_name_addresses.contains_key(&message_id),
                "{label}"
            );
            assert!(state.receive_receipts.is_empty(), "{label}");
            assert_eq!(
                state
                    .notification_delivery_accepted
                    .get(&message_id)
                    .copied(),
                *accepted,
                "{label}"
            );
            assert_eq!(
                state.notification_delivery_failures.get(&message_id),
                failure.as_ref(),
                "{label}"
            );
            assert_eq!(
                state.notification_delivery_bindings.get(&message_id),
                selected.as_ref(),
                "{label}"
            );
        }

        let new_response = send_notify(&server, "sender", "new");
        assert!(new_response.ok, "{label}: {new_response:?}");
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "{label}");
            assert_eq!(calls[0], old_transport, "{label}");
            assert_eq!(calls[1], new_transport, "{label}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_service_generation_change_refreshes_same_block_and_retries_once() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "service_changed");
    let route = seed_rex_route(&server);
    let stale = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    register_rex_worker(&server, "a", &stale);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &stale)]);
    persist_rex_worker_binding(&server, &route, "a", &stale);

    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let send_root = root.clone();
    let response =
        tokio::task::spawn_blocking(move || manager_send_to_rex(&manager, &send_root, "a"))
            .await
            .unwrap();

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["notification"], "rex-input-submitted");
    assert_eq!(
        response.data["rex_binding_refresh"],
        "same-server-session-block-service-generation"
    );
    assert_eq!(
        response.data["rex_binding_retry"],
        "original-durable-message-once"
    );
    for failure_field in [
        "notification_error",
        "failure",
        "repair_required",
        "escalation",
    ] {
        assert!(
            response.data.get(failure_field).is_none(),
            "successful recovery retained {failure_field}: {response:?}"
        );
    }
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[message_id].wake_attempt_count, 2);
        assert!(state
            .notification_delivery_accepted
            .contains_key(message_id));
    }

    let control =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
    let refreshed = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "a")
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let refreshed_endpoint = refreshed.transport.rex_endpoint.as_ref().unwrap();
    let stale_endpoint = stale.rex_endpoint.as_ref().unwrap();
    assert_eq!(refreshed_endpoint.server, stale_endpoint.server);
    assert_eq!(refreshed_endpoint.session_id, stale_endpoint.session_id);
    assert_eq!(refreshed_endpoint.block_id, stale_endpoint.block_id);
    assert_eq!(refreshed_endpoint.instance_id, "instance-fixture-refreshed");

    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "com.superlogical.terminal.write")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "send-key")
            .count(),
        1
    );
    assert!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "server.status")
            .count()
            >= 3
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_public_cli_can_operate_an_isolated_test_daemon() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "collab-rex-public-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let fixture = RexCliFixture::new(&root, "normal");
    let daemon = PublicDaemon::start(root.clone(), host_paths.clone()).await;

    let operation = format!("public-rex-context-{}-0", std::process::id());
    let rex = serde_json::json!({
        "session_id": "agent-a",
        "rex": {
            "server": "unix:///tmp/rex-fixture.sock",
            "session_id": "session:11111111-1111-1111-1111-111111111111",
            "block_id": "block:22222222-2222-2222-2222-222222222222",
            "cwd": root.to_string_lossy(),
        }
    });
    let output = Command::new(public_collab_binary())
        .arg("context")
        .arg("--op")
        .arg(&operation)
        .arg("--provide")
        .arg(rex.to_string())
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", root.join("home"))
        .env("CODEX_HOME", root.join("home"))
        .env("COLLAB_STATE_DIR", &host_paths.state_root())
        .env("CODEX_SESSION_ID", "agent-a")
        .output()
        .expect("run public Collab context");
    assert!(
        output.status.success(),
        "public context failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("public context returns JSON");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    assert_eq!(
        response["result"]["snapshot"]["identity"]["transport"]["rex_endpoint"]["session_id"],
        "session:11111111-1111-1111-1111-111111111111"
    );
    let calls = fixture.calls();
    assert!(
        calls.iter().any(|call| rex_method(call) == "server.status"),
        "the isolated daemon must use the injected Rex CLI: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| rex_method(call) == "session.list_blocks"),
        "public context must verify the selected block: {calls:?}"
    );

    drop(fixture);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_public_recovery_refuses_after_context_moves_the_block() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "collab-rex-h1-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let fixture = RexCliFixture::new(&root, "normal");
    fixture.set_blocks(&[
        PUBLIC_SENDER_BLOCK,
        PUBLIC_RECIPIENT_BLOCK,
        PUBLIC_MOVED_BLOCK,
    ]);
    let daemon = PublicDaemon::start(root.clone(), host_paths.clone()).await;

    let _sender = register_public_rex(
        &root,
        &host_paths,
        "agent-sender",
        PUBLIC_SENDER_BLOCK,
        "public-h1-sender",
    );
    let recipient = register_public_rex(
        &root,
        &host_paths,
        "agent-a",
        PUBLIC_RECIPIENT_BLOCK,
        "public-h1-recipient",
    );
    let recipient_snapshot = public_snapshot(&recipient);
    let recipient_name = public_name(recipient_snapshot);
    let recipient_worker = public_worker_id(recipient_snapshot);

    fixture.set_scenario("service_changed");
    fixture.arm_method_pause("session.list");
    let subject = "public-original-binding-move";
    let body = "DETAIL-public-original-binding-move";
    let send_args = public_send_args(&recipient_name, subject, body);
    let send_request = serde_json::json!(send_args);
    let send_args = send_args.iter().map(String::as_str).collect::<Vec<_>>();
    let send_child = spawn_public_cli(&root, &host_paths, "agent-sender", &send_args);
    fixture.wait_method_pause();
    let refresh_state = public_state(&root, &host_paths);
    let refresh_message_id =
        public_message_id_for_subject(&refresh_state, &recipient_worker, subject);
    let refresh_observation =
        public_state_summary(&refresh_state, &refresh_message_id, &recipient_worker);

    let context_args = public_context_args(
        &format!("public-h1-move-{}-{}", std::process::id(), now_ms()),
        "agent-a",
        PUBLIC_MOVED_BLOCK,
    );
    let context_request = serde_json::json!(context_args);
    let context_args = context_args.iter().map(String::as_str).collect::<Vec<_>>();
    let moved = run_public_cli(&root, &host_paths, "agent-a", &context_args);
    let after_move_state = public_state(&root, &host_paths);
    let after_move_observation =
        public_state_summary(&after_move_state, &refresh_message_id, &recipient_worker);
    fixture.release_method_pause();
    let send = wait_public_cli(send_child).await;
    let calls = fixture.calls();
    let write_targets = public_write_targets(&calls);
    let enter_count = public_method_count(&calls, "send-key");
    let state = public_state(&root, &host_paths);
    let response_message_id = public_message_id(&send);
    let message_id = if response_message_id.is_empty() {
        refresh_message_id.clone()
    } else {
        response_message_id
    };
    let accepted = state
        .notification_delivery_accepted
        .contains_key(&message_id);
    let selected_block = state
        .notification_delivery_bindings
        .get(&message_id)
        .and_then(|binding| binding.transport.rex_endpoint.as_ref())
        .map(|endpoint| endpoint.block_id.clone());
    let final_snapshot = public_snapshot(&moved.value).clone();
    let failures = state
        .notification_delivery_failures
        .get(&message_id)
        .cloned();
    let message = state.msgs.get(&message_id).cloned();
    let final_observation = public_state_summary(&state, &message_id, &recipient_worker);

    emit_public_observation(
        "h1_original_binding_move",
        serde_json::json!({
            "request_order": [
                {"actor": "agent-sender", "public_request": send_request},
                {"actor": "agent-a", "public_request": context_request},
            ],
            "barrier": "paused at the recovery server.status call while the public Context moved the binding",
            "recipient_worker": recipient_worker,
            "binding_at_refresh": refresh_observation,
            "binding_after_move": after_move_observation,
            "binding_at_final": final_observation,
            "write_calls": public_terminal_calls(&calls, "com.superlogical.terminal.write"),
            "enter_calls": public_terminal_calls(&calls, "send-key"),
            "send_response": public_cli_summary(&send),
            "context_response": public_cli_summary(&moved),
            "selected_block": selected_block,
            "accepted": accepted,
            "failure": failures.clone(),
            "final_message": message.clone(),
        }),
    );

    drop(fixture);
    daemon.shutdown().await;

    assert!(
        moved.success
            && final_snapshot["identity"]["transport"]["rex_endpoint"]["block_id"]
                == PUBLIC_MOVED_BLOCK,
        "the public Context move must complete before recovery resumes: {moved:?}"
    );
    assert!(
        !write_targets
            .iter()
            .any(|target| target.ends_with(PUBLIC_MOVED_BLOCK)),
        "the moved binding must receive no body input; moved={moved:?} send={send:?} recipient={recipient_worker} message={message_id} selected={selected_block:?} message={message:?} failure={failures:?}"
    );
    assert!(
        message.is_some(),
        "the original durable message must remain"
    );
    assert!(
        !send.success,
        "recovery must refuse the replaced original binding: {send:?}"
    );
    assert_eq!(
        write_targets,
        Vec::<String>::new(),
        "a replaced original binding must write nowhere: {write_targets:?}"
    );
    assert_eq!(enter_count, 0, "a refused recovery must send no Enter");
    assert!(!accepted, "a refused recovery must not record acceptance");
    assert!(
        failures.as_ref().is_some_and(|failure| {
            send.value["error"]
                .as_str()
                .is_some_and(|error| error.contains(&failure.error))
        }),
        "the original delivery failure must remain: failure={failures:?} response={send:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_public_recovery_revalidates_acceptance_before_retry() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "collab-rex-h2-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let fixture = RexCliFixture::new(&root, "normal");
    fixture.set_blocks(&[PUBLIC_SENDER_BLOCK, PUBLIC_RECIPIENT_BLOCK]);
    let daemon = PublicDaemon::start(root.clone(), host_paths.clone()).await;

    let _sender = register_public_rex(
        &root,
        &host_paths,
        "agent-sender",
        PUBLIC_SENDER_BLOCK,
        "public-h2-sender",
    );
    let recipient = register_public_rex(
        &root,
        &host_paths,
        "agent-a",
        PUBLIC_RECIPIENT_BLOCK,
        "public-h2-recipient",
    );
    let recipient_snapshot = public_snapshot(&recipient);
    let recipient_name = public_name(recipient_snapshot);
    let recipient_worker = public_worker_id(recipient_snapshot);

    fixture.set_scenario("service_changed");
    fixture.arm_method_pause("session.list");
    let subject = "public-final-admission-race";
    let body = "DETAIL-public-final-admission-race";
    let send_args = public_send_args(&recipient_name, subject, body);
    let send_request = serde_json::json!(send_args);
    let send_args = send_args.iter().map(String::as_str).collect::<Vec<_>>();
    let first_child = spawn_public_cli(&root, &host_paths, "agent-sender", &send_args);
    fixture.wait_method_pause();
    let refresh_state = public_state(&root, &host_paths);
    let refresh_message_id =
        public_message_id_for_subject(&refresh_state, &recipient_worker, subject);
    let refresh_observation =
        public_state_summary(&refresh_state, &refresh_message_id, &recipient_worker);

    fixture.set_scenario("normal");
    let duplicate = run_public_cli(&root, &host_paths, "agent-sender", &send_args);
    let pre_admission_state = public_state(&root, &host_paths);
    let pre_admission_observation =
        public_state_summary(&pre_admission_state, &refresh_message_id, &recipient_worker);
    fixture.set_scenario("service_changed");
    fixture.release_method_pause();
    let first = wait_public_cli(first_child).await;

    let calls = fixture.calls();
    let write_targets = public_write_targets(&calls);
    let enter_count = public_method_count(&calls, "send-key");
    let state = public_state(&root, &host_paths);
    let duplicate_message_id = public_message_id(&duplicate);
    let first_message_id = {
        let response_id = public_message_id(&first);
        if response_id.is_empty() {
            refresh_message_id.clone()
        } else {
            response_id
        }
    };
    let accepted = state
        .notification_delivery_accepted
        .contains_key(&duplicate_message_id);
    let message = state.msgs.get(&duplicate_message_id).cloned();
    let failures = state
        .notification_delivery_failures
        .get(&duplicate_message_id)
        .cloned();
    let final_observation = public_state_summary(&state, &duplicate_message_id, &recipient_worker);

    emit_public_observation(
        "h2_concurrent_accepted",
        serde_json::json!({
            "public_request": send_request,
            "barrier": "paused at the recovery server.status call while a duplicate public Send accepted",
            "recipient_worker": recipient_worker,
            "binding_at_refresh": refresh_observation,
            "binding_at_final_admission": pre_admission_observation,
            "binding_after_recovery": final_observation,
            "write_calls": public_terminal_calls(&calls, "com.superlogical.terminal.write"),
            "enter_calls": public_terminal_calls(&calls, "send-key"),
            "first_response": public_cli_summary(&first),
            "duplicate_response": public_cli_summary(&duplicate),
            "accepted": accepted,
            "failure": failures.clone(),
            "final_message": message.clone(),
        }),
    );

    drop(fixture);
    daemon.shutdown().await;

    assert!(
        duplicate.success,
        "the duplicate public send must complete during the paused refresh: {duplicate:?}"
    );
    assert_eq!(
        duplicate_message_id, first_message_id,
        "the duplicate must retain the original durable message"
    );
    assert!(
        accepted,
        "the duplicate accepted attempt was not durably recorded: message={message:?} failure={failures:?}"
    );
    assert_eq!(
        write_targets.len(),
        1,
        "a message already accepted during refresh must not be written again: {write_targets:?} first={first:?} duplicate={duplicate:?} message={message:?}"
    );
    assert_eq!(enter_count, 1, "the accepted message must have one Enter");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_public_recovery_revalidates_unknown_before_retry() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "collab-rex-h2u-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let fixture = RexCliFixture::new(&root, "normal");
    fixture.set_blocks(&[PUBLIC_SENDER_BLOCK, PUBLIC_RECIPIENT_BLOCK]);
    let daemon = PublicDaemon::start(root.clone(), host_paths.clone()).await;

    let _sender = register_public_rex(
        &root,
        &host_paths,
        "agent-sender",
        PUBLIC_SENDER_BLOCK,
        "public-h2u-sender",
    );
    let recipient = register_public_rex(
        &root,
        &host_paths,
        "agent-a",
        PUBLIC_RECIPIENT_BLOCK,
        "public-h2u-recipient",
    );
    let recipient_snapshot = public_snapshot(&recipient);
    let recipient_name = public_name(recipient_snapshot);
    let recipient_worker = public_worker_id(recipient_snapshot);

    fixture.set_scenario("service_changed");
    fixture.arm_method_pause("session.list");
    let subject = "public-final-admission-unknown";
    let body = "DETAIL-public-final-admission-unknown";
    let send_args = public_send_args(&recipient_name, subject, body);
    let send_request = serde_json::json!(send_args);
    let send_args = send_args.iter().map(String::as_str).collect::<Vec<_>>();
    let first_child = spawn_public_cli(&root, &host_paths, "agent-sender", &send_args);
    fixture.wait_method_pause();
    let refresh_state = public_state(&root, &host_paths);
    let refresh_message_id =
        public_message_id_for_subject(&refresh_state, &recipient_worker, subject);
    let refresh_observation =
        public_state_summary(&refresh_state, &refresh_message_id, &recipient_worker);

    fixture.set_scenario("enter_unknown");
    let duplicate = run_public_cli(&root, &host_paths, "agent-sender", &send_args);
    let pre_admission_state = public_state(&root, &host_paths);
    let pre_admission_observation =
        public_state_summary(&pre_admission_state, &refresh_message_id, &recipient_worker);
    let pre_admission_failure = pre_admission_state
        .notification_delivery_failures
        .get(&refresh_message_id)
        .cloned();
    fixture.set_scenario("service_changed");
    fixture.release_method_pause();
    let first = wait_public_cli(first_child).await;

    let calls = fixture.calls();
    let write_targets = public_write_targets(&calls);
    let enter_count = public_method_count(&calls, "send-key");
    let state = public_state(&root, &host_paths);
    let duplicate_message_id = public_message_id(&duplicate);
    let first_message_id = {
        let response_id = public_message_id(&first);
        if response_id.is_empty() {
            refresh_message_id.clone()
        } else {
            response_id
        }
    };
    let accepted = state
        .notification_delivery_accepted
        .contains_key(&duplicate_message_id);
    let message = state.msgs.get(&duplicate_message_id).cloned();
    let failure = state
        .notification_delivery_failures
        .get(&duplicate_message_id)
        .cloned();
    let final_observation = public_state_summary(&state, &duplicate_message_id, &recipient_worker);

    emit_public_observation(
        "h2_concurrent_unknown",
        serde_json::json!({
            "public_request": send_request,
            "barrier": "paused at the recovery server.status call while a duplicate public Send became unknown",
            "recipient_worker": recipient_worker,
            "binding_at_refresh": refresh_observation,
            "binding_at_final_admission": pre_admission_observation,
            "binding_after_recovery": final_observation,
            "write_calls": public_terminal_calls(&calls, "com.superlogical.terminal.write"),
            "enter_calls": public_terminal_calls(&calls, "send-key"),
            "first_response": public_cli_summary(&first),
            "duplicate_response": public_cli_summary(&duplicate),
            "accepted": accepted,
            "failure": failure.clone(),
            "final_message": message.clone(),
        }),
    );

    drop(fixture);
    daemon.shutdown().await;

    assert!(
        !duplicate.success,
        "the concurrent unknown public Send must remain failed: {duplicate:?}"
    );
    assert_eq!(
        duplicate_message_id, first_message_id,
        "the concurrent unknown attempt must retain the original durable message"
    );
    assert!(
        !first.success,
        "recovery must not replace the concurrent unknown outcome: {first:?}"
    );
    assert!(
        !accepted,
        "a concurrent unknown outcome must not become accepted: message={message:?} failure={failure:?}"
    );
    assert_eq!(
        failure, pre_admission_failure,
        "recovery must preserve the concurrent unknown failure record"
    );
    assert!(
        failure.as_ref().is_some_and(|failure| !failure.retryable),
        "the concurrent unknown outcome must remain non-retryable: {failure:?}"
    );
    assert_eq!(
        write_targets.len(),
        1,
        "a concurrent unknown outcome must not be written again: {write_targets:?} first={first:?} duplicate={duplicate:?}"
    );
    assert_eq!(
        enter_count, 1,
        "a concurrent unknown outcome must not receive another Enter"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_public_queued_body_enter_rejection_is_unknown_and_not_resent() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "crq-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let fixture = RexCliFixture::new(&root, "normal");
    fixture.set_blocks(&[PUBLIC_SENDER_BLOCK, PUBLIC_RECIPIENT_BLOCK]);
    let daemon = PublicDaemon::start(root.clone(), host_paths.clone()).await;

    let _sender = register_public_rex(
        &root,
        &host_paths,
        "agent-sender",
        PUBLIC_SENDER_BLOCK,
        "public-queued-sender",
    );
    let recipient = register_public_rex(
        &root,
        &host_paths,
        "agent-a",
        PUBLIC_RECIPIENT_BLOCK,
        "public-queued-recipient",
    );
    let recipient_snapshot = public_snapshot(&recipient);
    let recipient_name = public_name(recipient_snapshot);
    let recipient_worker = public_worker_id(recipient_snapshot);

    fixture.set_scenario("enter_typed_reject");
    let subject = "public-queued-body-reject";
    let body = "DETAIL-public-queued-body-reject";
    let send_args = public_send_args(&recipient_name, subject, body);
    let send_request = serde_json::json!(send_args);
    let send_args = send_args.iter().map(String::as_str).collect::<Vec<_>>();
    let first = run_public_cli(&root, &host_paths, "agent-sender", &send_args);
    let duplicate = run_public_cli(&root, &host_paths, "agent-sender", &send_args);

    let calls = fixture.calls();
    let write_targets = public_write_targets(&calls);
    let enter_count = public_method_count(&calls, "send-key");
    let state = public_state(&root, &host_paths);
    let first_message_id = public_message_id(&first);
    let duplicate_message_id = public_message_id(&duplicate);
    let failure = state
        .notification_delivery_failures
        .get(&first_message_id)
        .cloned();
    let accepted = state
        .notification_delivery_accepted
        .contains_key(&first_message_id);
    let final_observation = public_state_summary(&state, &first_message_id, &recipient_worker);

    emit_public_observation(
        "queued_body_enter_rejection",
        serde_json::json!({
            "public_request": send_request,
            "request_order": "first public Send then the identical public Send",
            "recipient_worker": recipient_worker,
            "binding_after_rejection": final_observation,
            "write_calls": public_terminal_calls(&calls, "com.superlogical.terminal.write"),
            "enter_calls": public_terminal_calls(&calls, "send-key"),
            "first_response": public_cli_summary(&first),
            "duplicate_response": public_cli_summary(&duplicate),
            "accepted": accepted,
            "failure": failure.clone(),
        }),
    );

    drop(fixture);
    daemon.shutdown().await;

    assert!(
        !first.success,
        "the typed Enter rejection must fail: {first:?}"
    );
    assert!(
        !duplicate.success,
        "the duplicate must retain the original rejection: {duplicate:?}"
    );
    assert_eq!(
        duplicate_message_id, first_message_id,
        "the duplicate must retain the original durable message"
    );
    assert_eq!(
        write_targets,
        vec![format!("{PUBLIC_REX_SESSION}#{PUBLIC_RECIPIENT_BLOCK}")],
        "one body write only: {write_targets:?}"
    );
    assert_eq!(enter_count, 1, "one Enter only");
    assert!(
        failure.as_ref().is_some_and(|failure| {
            failure.error.contains("REX_UNKNOWN: operation=notify")
                && failure.error.contains("stage=WriteQueued")
        }),
        "the current public path must retain its actual Unknown plus queued-body classification: {failure:?}"
    );
    assert!(
        failure.as_ref().is_some_and(|failure| !failure.retryable),
        "the queued Enter rejection must remain non-retryable: {failure:?}"
    );
    assert!(
        !accepted,
        "a rejected Enter must not be recorded as accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_service_generation_change_is_recovered_from_timer_owner() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "service_changed");
    let route = seed_rex_route(&server);
    let stale = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    let subscription = register_rex_worker(&server, "a", &stale);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &stale)]);
    persist_rex_worker_binding(&server, &route, "a", &stale);
    let now = now_ms();
    queue_message(&server, "a", &subscription, "timer-service-change", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "timer-service-change".into(),
        mode: "immediate".into(),
        source_thread_id: None,
    }]);

    let tick_server = server.clone();
    tokio::task::spawn_blocking(move || {
        timers::tick_with_idle_at(&tick_server, now + 1, &|_| true);
    })
    .await
    .unwrap();

    {
        let state = server.state.lock().unwrap();
        let message = &state.msgs["timer-service-change"];
        assert_eq!(message.state, "pending");
        assert_eq!(message.wake_attempt_count, 2);
        assert!(state
            .notification_delivery_accepted
            .contains_key("timer-service-change"));
    }
    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "com.superlogical.terminal.write")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "send-key")
            .count(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_service_generation_change_is_recovered_for_qqbot_ceo_inbound() {
    use crate::ceo::CeoGrant;
    use crate::server::channel_envelope::ChannelEnvelope;

    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "service_changed");
    let route = seed_rex_route(&server);
    let stale = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    register_rex_worker(&server, "a", &stale);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &stale)]);
    persist_rex_worker_binding(&server, &route, "a", &stale);
    server.commit(&[Event::CeoGrantSet {
        grant: Some(CeoGrant {
            workspace: route.project_scope_id.as_str().into(),
            app_scope_id: route.app_scope_id.as_str().into(),
            name: "Alice".into(),
            granted_by: "user".into(),
            approval: "approved".into(),
            granted_ms: 1,
        }),
    }]);

    let channel_server = server.clone();
    let response = tokio::task::spawn_blocking(move || {
        handle_channel_inbound_with_rex_recovery(
            &channel_server,
            "external:qqbot".into(),
            "ceo".into(),
            "QQBot".into(),
            "qqbot ceo service-change".into(),
            ChannelEnvelope {
                source_channel: "qqbot".into(),
                channel_instance: "qqbot".into(),
                external_user: "qq-user".into(),
                external_conversation: "conversation".into(),
                platform_message_id: "platform-service-change".into(),
                reply_route: "qqbot://c2c/conversation".into(),
                recipient_address: "ceo".into(),
                attachments: vec![],
            },
        )
    })
    .await
    .unwrap();

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["notification"], "rex-input-submitted");
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        let message = &state.msgs[message_id];
        assert_eq!(message.to, "a");
        assert_eq!(message.wake_attempt_count, 2);
        assert!(state
            .notification_delivery_accepted
            .contains_key(message_id));
    }
    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "com.superlogical.terminal.write")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "send-key")
            .count(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_unknown_write_outcome_is_not_retried_by_timer_owner() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "write_unknown");
    let route = seed_rex_route(&server);
    let current = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    let subscription = register_rex_worker(&server, "a", &current);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    persist_rex_worker_binding(&server, &route, "a", &current);
    let now = now_ms();
    queue_message(&server, "a", &subscription, "timer-write-unknown", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "timer-write-unknown".into(),
        mode: "immediate".into(),
        source_thread_id: None,
    }]);

    let tick_server = server.clone();
    tokio::task::spawn_blocking(move || {
        timers::tick_with_idle_at(&tick_server, now + 1, &|_| true);
    })
    .await
    .unwrap();

    {
        let state = server.state.lock().unwrap();
        let message = &state.msgs["timer-write-unknown"];
        assert_eq!(message.state, "pending");
        assert_eq!(message.wake_attempt_count, 2);
        assert!(message.retry_attempted);
        assert!(!state
            .notification_delivery_accepted
            .contains_key("timer-write-unknown"));
        let failure = &state.notification_delivery_failures["timer-write-unknown"];
        assert!(!failure.retryable, "{failure:?}");
        assert!(failure.error.contains("REX_UNKNOWN"), "{failure:?}");
    }
    let calls = fixture.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| rex_method(call) == "com.superlogical.terminal.write")
            .count(),
        1
    );
    assert!(calls.iter().all(|call| rex_method(call) != "send-key"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_service_generation_change_cannot_move_the_block_anchor() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "service_changed_wrong_block");
    let route = seed_rex_route(&server);
    let stale = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    register_rex_worker(&server, "a", &stale);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &stale)]);
    persist_rex_worker_binding(&server, &route, "a", &stale);

    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let send_root = root.clone();
    let response =
        tokio::task::spawn_blocking(move || manager_send_to_rex(&manager, &send_root, "a"))
            .await
            .unwrap();

    assert!(!response.ok, "{response:?}");
    assert_eq!(response.data["rex_binding_refresh"], "failed");
    assert!(response.data["rex_binding_refresh_error"]
        .as_str()
        .unwrap()
        .contains("exact terminal block was not present"));
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[message_id].state, "pending");
        assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
        assert!(!state
            .notification_delivery_accepted
            .contains_key(message_id));
    }
    let calls = fixture.calls();
    assert!(calls
        .iter()
        .all(|call| rex_method(call) != "com.superlogical.terminal.write"));
    assert!(calls.iter().all(|call| rex_method(call) != "send-key"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_input_receipt_names_the_actual_transport_without_consuming_mail() {
    let response = notification_send_response(
        json!({"msg_id": "rex-message"}),
        true,
        "rex",
        &NotificationAttempt::Accepted,
    );
    assert!(response.ok);
    assert_eq!(response.data["notification"], "rex-input-submitted");
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["consumed"], false);
}

#[test]
fn rex_sink_uses_only_the_selected_rex_arm() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let transport = SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some("agent-session".into()),
        thread_id: Some("agent-thread".into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: Vec::new(),
        self_check: "test rex transport".into(),
    };
    let error = default_appserver_notification_sink()(
        &transport,
        None,
        "body",
        "message-id",
        false,
        "immediate",
    )
    .unwrap_err();
    assert!(error.starts_with("REX_NOT_DELIVERED:"), "{error}");
}

#[test]
fn native_notifications_do_not_wait_for_batch_window() {
    for kind in [TransportKind::Dsh, TransportKind::DesktopBridge] {
        let (server, root) = test_server();
        let subscription = register_and_subscribe(&server, "recipient");
        {
            let mut state = server.state.lock().unwrap();
            state
                .workers
                .get_mut("recipient")
                .unwrap()
                .transport
                .as_mut()
                .unwrap()
                .kind = kind.clone();
            state
                .notification_subscriptions
                .get_mut(&subscription)
                .unwrap()
                .method = kind.as_str().into();
        }
        let now = now_ms();
        queue_message(&server, "recipient", &subscription, "new-native", now);
        let attempt =
            attempt_notification_detailed_with_at(&server, "new-native", &subscription, now);
        assert!(attempt.accepted(), "{kind:?}: {attempt:?}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn native_notifications_do_not_wait_for_previous_consumption() {
    for kind in [TransportKind::Dsh, TransportKind::DesktopBridge] {
        let (server, root) = test_server();
        let subscription = register_and_subscribe(&server, "recipient");
        {
            let mut state = server.state.lock().unwrap();
            state
                .workers
                .get_mut("recipient")
                .unwrap()
                .transport
                .as_mut()
                .unwrap()
                .kind = kind.clone();
            state
                .notification_subscriptions
                .get_mut(&subscription)
                .unwrap()
                .method = kind.as_str().into();
        }
        let now = now_ms();
        for index in 0..server.config.notifications.max_unacked {
            let id = format!("unconsumed-{index}");
            queue_message(&server, "recipient", &subscription, &id, now - 1);
            server.commit(&[Event::Delivered { ids: vec![id] }]);
        }
        queue_message(&server, "recipient", &subscription, "next-native", now);
        server.commit(&[Event::DeliveryMode {
            msg_id: "next-native".into(),
            mode: "immediate".into(),
            source_thread_id: None,
        }]);
        let attempt =
            attempt_notification_detailed_with_at(&server, "next-native", &subscription, now);
        assert!(attempt.accepted(), "{kind:?}: {attempt:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["unconsumed-0"].state, "delivered");
        assert!(state
            .notification_delivery_accepted
            .contains_key("next-native"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn accepted_batch_is_not_overwritten_by_a_later_empty_batch_skip() {
    let (server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    for id in ["notice-a", "notice-b"] {
        queue_message(&server, "recipient", &subscription, id, now);
        server.commit(&[Event::DeliveryMode {
            msg_id: id.into(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        }]);
    }
    let first = attempt_notification_detailed_with_at(&server, "notice-a", &subscription, now);
    assert!(matches!(first, NotificationAttempt::Accepted), "{first:?}");
    {
        let state = server.state.lock().unwrap();
        for id in ["notice-a", "notice-b"] {
            assert!(state.notification_delivery_accepted.contains_key(id));
            assert!(!state.notification_delivery_failures.contains_key(id));
        }
    }
    let before = std::fs::read(&server.state_path).unwrap();
    let second = attempt_notification_detailed_with_at(&server, "notice-b", &subscription, now);
    assert!(
        matches!(second, NotificationAttempt::NotAttempted(_)),
        "{second:?}"
    );
    assert_eq!(
        std::fs::read(&server.state_path).unwrap(),
        before,
        "a skipped second attempt must preserve both actual batch acceptance receipts"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_carrier_acceptance_does_not_hide_a_new_handoff_wake_failure() {
    let (server, root) = test_server();
    let old_subscription = register_and_subscribe(&server, "old");
    let new_subscription = register_and_subscribe(&server, "new");
    let now = now_ms();
    queue_message(&server, "old", &old_subscription, "notice", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "notice".into(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);
    assert!(
        attempt_notification_detailed_with_at(&server, "notice", &old_subscription, now).accepted()
    );
    let (handoff, mut subscription) = {
        let state = server.state.lock().unwrap();
        (
            super::name_mailbox::MailboxHandoff {
                project_scope: "/project".into(),
                name: "Jenny".into(),
                occupancy_revision: 2,
                session_id: "session-new".into(),
                message_id: "notice".into(),
                previous_recipient: "old".into(),
                recipient: "new".into(),
                wake_attempt_count: state.msgs["notice"].wake_attempt_count,
                previous_subscription: Some(old_subscription),
                accepted_ms: state.notification_delivery_accepted.get("notice").copied(),
                evidence: state.notification_delivery_evidence.get("notice").cloned(),
                failure: None,
            },
            state.notification_subscriptions[&new_subscription].clone(),
        )
    };
    subscription.expires_ms = now - 1;
    server.commit(&[
        Event::MailboxReassigned { handoff },
        Event::NotificationSubscribed { subscription },
    ]);
    let attempt = attempt_notification_detailed_with_at(&server, "notice", &new_subscription, now);
    assert!(
        matches!(attempt, NotificationAttempt::NotAttempted(_)),
        "{attempt:?}"
    );
    let state = server.state.lock().unwrap();
    assert!(state.handoff_wake_pending(&state.msgs["notice"]));
    assert!(
        state.notification_delivery_failures.contains_key("notice"),
        "old carrier acceptance must not suppress the new carrier's failed wake"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_notification_uses_sender_native_thread_for_delegation() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    register_and_subscribe(&server, "sender");
    let now = now_ms();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: "sender-attribution".into(),
                from: "sender".into(),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("topic".into()),
                body: "DETAIL".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: "sender-attribution".into(),
            subscription_id: subscription_id.clone(),
        },
        Event::DeliveryMode {
            msg_id: "sender-attribution".into(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
    ]);

    let observed = Arc::new(Mutex::new(None));
    {
        let observed = Arc::clone(&observed);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, source, _, _, explicit, _mode| {
            *observed.lock().unwrap() = Some((source.map(str::to_owned), explicit));
            Ok(json!({"accepted": true}))
        });
    }

    assert!(attempt_notification_with_at(
        &server,
        "sender-attribution",
        &subscription_id,
        now,
    ));
    assert_eq!(
        observed.lock().unwrap().as_ref(),
        Some(&(Some("thread-sender".into()), true))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_notification_uses_immediate_without_sender_thread() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "automatic-notice",
        now - 120_001,
    );
    let observed = Arc::new(Mutex::new(None));
    {
        let observed = Arc::clone(&observed);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, source, _, _, explicit, _mode| {
            *observed.lock().unwrap() = Some((source.map(str::to_owned), explicit));
            Ok(json!({"accepted": true}))
        });
    }

    assert!(attempt_notification_with_at(
        &server,
        "automatic-notice",
        &subscription_id,
        now,
    ));
    assert_eq!(observed.lock().unwrap().as_ref(), Some(&(None, false)));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_tmux_wake_drains_through_the_existing_timer_without_consumption() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    let (worker, subscription) = {
        let state = server.state.lock().unwrap();
        (
            state.workers["recipient"].clone(),
            state.notification_subscriptions[&subscription_id].clone(),
        )
    };
    let mut worker = worker;
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: root.join("absent.sock").display().to_string(),
        server_pid: 1,
        pane_pid: 2,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        codex_session_id: None,
        codex_thread_id: None,
    };
    worker.transport = Some(SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some("$1".into()),
        session_id: None,
        thread_id: None,
        tmux_endpoint: Some(endpoint),
        rex_endpoint: None,
        capabilities: vec!["send_message_to_pane".into()],
        self_check: "fixture".into(),
    });
    let mut subscription = subscription;
    subscription.method = "tmux".into();
    subscription.target = "%1".into();
    server.commit(&[
        Event::Registered { worker },
        Event::NotificationSubscribed { subscription },
    ]);
    queue_message(&server, "recipient", &subscription_id, "deferred-tmux", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "deferred-tmux".into(),
        mode: "queued".into(),
        source_thread_id: None,
    }]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, _, _, _, mode| {
        observed.lock().unwrap().push(mode.to_string());
        Ok(json!({"submitted": true, "consumed": false}))
    });
    let attempt =
        attempt_notification_detailed_with_at(&server, "deferred-tmux", &subscription_id, now);
    assert!(
        matches!(attempt, NotificationAttempt::Deferred),
        "{attempt:?}"
    );
    let response =
        notification_send_response(json!({"msg_id":"deferred-tmux"}), true, "tmux", &attempt);
    assert!(response.ok);
    assert_eq!(response.data["notification"], "queued");
    assert_eq!(response.data["consumed"], false);
    assert!(calls.lock().unwrap().is_empty());
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["deferred-tmux"].wake_attempt_count, 0);
        assert!(!state
            .notification_delivery_accepted
            .contains_key("deferred-tmux"));
        assert!(!state
            .notification_delivery_failures
            .contains_key("deferred-tmux"));
    }
    timers::tick_with_idle_at(
        &server,
        now + server.config.notifications.delay_ms("direct-message"),
        &|_| false,
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![DAEMON_LIVE_CLOSURE_MODE.to_string()]
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.delivery_modes["deferred-tmux"], "queued");
    assert_eq!(state.msgs["deferred-tmux"].state, "pending");
    assert_eq!(state.msgs["deferred-tmux"].wake_attempt_count, 1);
    assert!(state
        .notification_delivery_accepted
        .contains_key("deferred-tmux"));
    drop(state);
    timers::tick_with_idle_at(
        &server,
        now + 2 * server.config.notifications.delay_ms("direct-message"),
        &|_| false,
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "accepted queued wake must not be submitted again"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_delivery_mode_reaches_notification_sink() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "queued-notice",
        now - 120_001,
    );
    server.commit(&[Event::DeliveryMode {
        msg_id: "queued-notice".into(),
        mode: "queued".into(),
        source_thread_id: None,
    }]);
    let observed = Arc::new(Mutex::new(None));
    {
        let observed = Arc::clone(&observed);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, _, _, _, mode| {
            *observed.lock().unwrap() = Some(mode.to_string());
            Ok(json!({"accepted": true}))
        });
    }

    assert!(attempt_notification_with_at(
        &server,
        "queued-notice",
        &subscription_id,
        now,
    ));
    assert_eq!(observed.lock().unwrap().as_deref(), Some("queued"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn immediate_and_queued_messages_are_not_batched_into_one_mode() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "immediate-notice",
        now - 120_001,
    );
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "queued-notice",
        now - 120_001,
    );
    server.commit(&[
        Event::DeliveryMode {
            msg_id: "immediate-notice".into(),
            mode: "immediate".into(),
            source_thread_id: None,
        },
        Event::DeliveryMode {
            msg_id: "queued-notice".into(),
            mode: "queued".into(),
            source_thread_id: None,
        },
    ]);

    let observed = Arc::new(Mutex::new(None));
    {
        let observed = Arc::clone(&observed);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, _, _, _, mode| {
            *observed.lock().unwrap() = Some(mode.to_string());
            Ok(json!({"accepted": true}))
        });
    }

    assert!(attempt_notification_with_at(
        &server,
        "immediate-notice",
        &subscription_id,
        now,
    ));
    assert_eq!(observed.lock().unwrap().as_deref(), Some("immediate"));
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["immediate-notice"].wake_attempt_count, 1);
        assert_eq!(state.msgs["queued-notice"].wake_attempt_count, 0);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_appserver_sink_does_not_fallback_to_tmux() {
    let transport = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-thread-recipient".into()),
        thread_id: Some("thread-recipient".into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let sink = default_appserver_notification_sink();
    let error = sink(
        &transport,
        None,
        "explicit",
        "message-explicit",
        true,
        "immediate",
    )
    .unwrap_err();
    assert!(
        error.contains("ADAPTER_ROUTE_UNAVAILABLE:")
            || error.contains("APPSERVER_ENDPOINT_REJECTED:")
            || error.contains("RPC")
    );
}

#[test]
fn automatic_batch_does_not_cross_the_first_notice_window() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    let long_body = format!("DETAIL-old-notice-{}", "x".repeat(200));
    // A body long enough that the compact wake preview cannot carry it in full.
    queue_message_with_body(
        &server,
        "recipient",
        &subscription_id,
        "old-notice",
        now - 120_001,
        long_body.clone(),
    );
    queue_message(&server, "recipient", &subscription_id, "late-notice", now);

    let delivered = Arc::new(Mutex::new(Vec::new()));
    {
        let delivered = Arc::clone(&delivered);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, text, _, _, _mode| {
            delivered.lock().unwrap().push(text.to_string());
            Ok(serde_json::json!({"accepted": true}))
        });
    }
    assert!(attempt_notification_with_at(
        &server,
        "old-notice",
        &subscription_id,
        now,
    ));

    let text = delivered.lock().unwrap().join("\n");
    assert!(text.contains("old-notice"));
    assert!(
        !text.contains("late-notice"),
        "a notice arriving after the first 120-second window must remain pending"
    );
    assert!(
        !text.contains(&long_body),
        "the batch wake carries a compact preview while current state retains full details: {text}"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["old-notice"].state, "pending");
    assert_eq!(state.msgs["old-notice"].wake_attempt_count, 1);
    assert_eq!(state.msgs["late-notice"].state, "pending");
    assert_eq!(state.msgs["old-notice"].body, long_body);
    assert_eq!(state.msgs["late-notice"].body, "DETAIL-late-notice");
    drop(state);
    let loaded = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(loaded.msgs["old-notice"].body, long_body);
    assert_eq!(loaded.msgs["late-notice"].body, "DETAIL-late-notice");
    assert!(
        text.contains("old-notice:topic-old-notice=DETAIL-old-notice-"),
        "the batch wake must still carry the task summary preview: {text}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn four_notice_batch_uses_the_original_window_start_after_capping() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let window_start = 10_000_000;
    for (index, offset) in [(0, 0), (1, 60_000), (2, 70_000), (3, 80_000)] {
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            &format!("backlog-{index}"),
            window_start + offset,
        );
    }

    let delivered = Arc::new(Mutex::new(Vec::new()));
    {
        let delivered = Arc::clone(&delivered);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, text, _, _, _mode| {
            delivered.lock().unwrap().push(text.to_string());
            Ok(serde_json::json!({"accepted": true}))
        });
    }
    assert!(attempt_notification_with_at(
        &server,
        "backlog-0",
        &subscription_id,
        window_start + 120_000,
    ));
    let text = delivered.lock().unwrap().join("\n");
    assert!(!text.contains("backlog-0"));
    assert!(text.contains("backlog-1"));
    assert!(text.contains("backlog-2"));
    assert!(text.contains("backlog-3"));
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["backlog-0"].state, "pending");
    assert_eq!(state.msgs["backlog-1"].state, "pending");
    assert_eq!(state.msgs["backlog-1"].wake_attempt_count, 1);
    assert_eq!(state.msgs["backlog-2"].state, "pending");
    assert_eq!(state.msgs["backlog-2"].wake_attempt_count, 1);
    assert_eq!(state.msgs["backlog-3"].state, "pending");
    assert_eq!(state.msgs["backlog-3"].wake_attempt_count, 1);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_notice_is_immediate_and_isolated_from_automatic_batching() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "automatic-notice",
        now - 120_001,
    );
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "explicit-notice",
        now,
    );
    server.commit(&[Event::DeliveryMode {
        msg_id: "explicit-notice".into(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);

    let delivered = Arc::new(Mutex::new(Vec::new()));
    {
        let delivered = Arc::clone(&delivered);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, text, _, explicit, _mode| {
            delivered.lock().unwrap().push((explicit, text.to_string()));
            Ok(serde_json::json!({"accepted": true}))
        });
    }
    assert!(attempt_notification_with_at(
        &server,
        "explicit-notice",
        &subscription_id,
        now,
    ));
    let delivered_snapshot = delivered.lock().unwrap();
    assert_eq!(delivered_snapshot.len(), 1);
    assert!(
        delivered_snapshot[0].0,
        "explicit sendmessage must use immediate notify"
    );
    let text = &delivered_snapshot[0].1;
    assert!(text.contains("explicit-notice"));
    assert!(!text.contains("automatic-notice"));
    drop(delivered_snapshot);
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["explicit-notice"].state, "pending");
    assert_eq!(state.msgs["explicit-notice"].wake_attempt_count, 1);
    assert_eq!(state.msgs["automatic-notice"].state, "pending");
    drop(state);
    assert!(attempt_notification_with_at(
        &server,
        "automatic-notice",
        &subscription_id,
        now,
    ));
    let delivered = delivered.lock().unwrap();
    assert!(delivered
        .iter()
        .any(|(explicit, text)| { !explicit && text.contains("automatic-notice") }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_reservation_is_durable_and_is_never_replayed() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "reserved-once",
        now - 120_001,
    );

    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink =
        Arc::new(|_, _, _, _, _, _| Err("test sink rejected".into()));
    assert!(!attempt_notification_with_at(
        &server,
        "reserved-once",
        &subscription_id,
        now,
    ));
    assert_eq!(
        server.state.lock().unwrap().msgs["reserved-once"].state,
        "pending"
    );
    assert_eq!(
        server.state.lock().unwrap().msgs["reserved-once"].wake_attempt_count,
        1
    );
    assert!(!attempt_notification_with_at(
        &server,
        "reserved-once",
        &subscription_id,
        now + 300_000,
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wake_rebinds_a_stale_default_direct_message_binding() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    {
        let mut state = server.state.lock().unwrap();
        state
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap()
            .target = "thread-stale".into();
    }
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "stale-default",
        now - 120_001,
    );
    let observed = Arc::new(Mutex::new(false));
    {
        let observed = Arc::clone(&observed);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_, _, _, _, _, _mode| {
            *observed.lock().unwrap() = true;
            Ok(json!({"accepted": true}))
        });
    }

    assert!(attempt_notification_with_at(
        &server,
        "stale-default",
        &subscription_id,
        now,
    ));
    assert!(*observed.lock().unwrap());
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions[&subscription_id].target,
        "thread-recipient"
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "armed"
    );
    assert_eq!(state.wake_bindings["stale-default"], subscription_id);
    assert_eq!(state.msgs["stale-default"].state, "pending");
    std::fs::remove_dir_all(root).unwrap();
}

fn owned_subscription(worker_id: &str, id: &str, status: &str) -> NotificationSubscription {
    NotificationSubscription {
        id: id.into(),
        worker_id: worker_id.into(),
        event: "deadline".into(),
        subject: Some("goal:test".into()),
        target: "thread-a".into(),
        method: "appserver".into(),
        trigger_ms: Some(2_000),
        trigger_times_ms: vec![2_000, 4_000],
        interval_ms: Some(2_000),
        repeat_count: 4,
        fired_count: 1,
        expires_ms: 20_000,
        status: status.into(),
        created_ms: 100,
        updated_ms: 1_500,
        status_reason: (status == "cancelled").then(|| EXPLICIT_UNSUBSCRIBE_REASON.into()),
        patrol_idle_since_ms: None,
        patrol_last_push_ms: 0,
        periodic_declared: false,
        goal_content: None,
    }
}

#[test]
fn retarget_same_target_transport_lost_preserves_schedule_and_terminal_states() {
    let worker_id = "worker-a";
    let target = "thread-a";
    let now = 10_000;
    let mut state = State::default();
    for (id, status) in [
        ("sub-lost", "transport-lost"),
        ("sub-cancelled", "cancelled"),
        ("sub-expired", "expired"),
        ("sub-consumed", "consumed"),
    ] {
        state
            .notification_subscriptions
            .insert(id.into(), owned_subscription(worker_id, id, status));
    }
    let transport = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-a".into()),
        thread_id: Some(target.into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };

    let events = retarget_owned_notification_events(&state, worker_id, &transport, now);
    assert_eq!(events.len(), 1, "{events:?}");
    let Event::NotificationSubscribed { subscription } = &events[0] else {
        panic!("expected subscription rearm: {:?}", events[0]);
    };
    assert_eq!(subscription.id, "sub-lost");
    assert_eq!(subscription.status, "armed");
    assert_eq!(subscription.status_reason, None);
    assert_eq!(subscription.method, "appserver");
    assert_eq!(subscription.target, target);
    assert_eq!(subscription.trigger_ms, Some(2_000));
    assert_eq!(subscription.trigger_times_ms, vec![2_000, 4_000]);
    assert_eq!(subscription.interval_ms, Some(2_000));
    assert_eq!(subscription.repeat_count, 4);
    assert_eq!(subscription.fired_count, 1);
    assert_eq!(subscription.expires_ms, 20_000);
    assert_eq!(subscription.updated_ms, now);
}
