use super::*;
use crate::proto::RexEndpoint;
use crate::server::state::{Event, NotificationSubscription, State, WorkerRec};
use serde_json::Value;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn rex_transport(
    agent_session: &str,
    server: &str,
    rex_session: &str,
    block: &str,
) -> SelectedTransport {
    let canonical_session = if rex_session.starts_with("session:") {
        rex_session.to_owned()
    } else {
        "session:11111111-1111-1111-1111-111111111111".to_owned()
    };
    let canonical_block = match block {
        "block-a" => "block:11111111-1111-1111-1111-111111111111",
        "old-block" => "block:33333333-3333-3333-3333-333333333333",
        "new-block" => "block:44444444-4444-4444-4444-444444444444",
        "test-block" => "block:55555555-5555-5555-5555-555555555555",
        value if value.starts_with("block:") => value,
        _ => "block:22222222-2222-2222-2222-222222222222",
    };
    SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some(agent_session.into()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(RexEndpoint {
            server: server.into(),
            session_id: canonical_session,
            block_id: canonical_block.to_owned(),
            host_id: Some("host-fixture".into()),
            instance_id: "instance-fixture".into(),
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
    mark_agent_online(
        server,
        worker,
        transport.session_id.as_deref().unwrap_or(""),
    );
    subscription_id
}

fn mark_agent_online(server: &Server, worker: &str, session_id: &str) {
    let project_scope = crate::server::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();
    server.commit(&[Event::AgentPresenceObserved {
        presence: crate::server::name_state::AgentPresenceRecord {
            project_scope,
            worker_id: worker.into(),
            session_id: session_id.into(),
            presence: crate::server::name_state::AgentPresence::Online,
            reason: "notification test recipient is online".into(),
            observed_ms: now_ms(),
        },
    }]);
}

fn mark_agent_offline(server: &Server, worker: &str, session_id: &str) {
    let project_scope = crate::server::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();
    server.commit(&[Event::AgentPresenceObserved {
        presence: crate::server::name_state::AgentPresenceRecord {
            project_scope,
            worker_id: worker.into(),
            session_id: session_id.into(),
            presence: crate::server::name_state::AgentPresence::Offline,
            reason: "notification test recipient is offline".into(),
            observed_ms: now_ms(),
        },
    }]);
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

struct RexCliFixture {
    dir: std::path::PathBuf,
    old_blocks_file: Option<std::ffi::OsString>,
    old_log_dir: Option<std::ffi::OsString>,
    old_scenario: Option<std::ffi::OsString>,
    foreground_env: Option<(Option<std::ffi::OsString>, Option<std::ffi::OsString>)>,
}

impl RexCliFixture {
    fn new(root: &std::path::Path, scenario: &str) -> Self {
        let dir = root.join("rex-cli-fixture");
        std::fs::create_dir_all(&dir).unwrap();
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
        // The full serial suite can delay Python fixture startup on a busy host.
        // Keep the fixture bounded without treating scheduler latency as a Rex timeout.
        crate::client::adapters::rex::set_test_timeout(Some(std::time::Duration::from_secs(15)));
        let blocks_file = dir.join("blocks.json");
        std::fs::write(
            &blocks_file,
            serde_json::to_vec(&serde_json::json!([
                "block:11111111-1111-1111-1111-111111111111",
                "block:22222222-2222-2222-2222-222222222222",
                "block:33333333-3333-3333-3333-333333333333",
                "block:44444444-4444-4444-4444-444444444444",
                "block:55555555-5555-5555-5555-555555555555"
            ]))
            .unwrap(),
        )
        .unwrap();
        let old_blocks_file = std::env::var_os("REXPRIMITIVE_BLOCKS_FILE");
        let old_log_dir = std::env::var_os("REXPRIMITIVE_LOG_DIR");
        let old_scenario = std::env::var_os("REXPRIMITIVE_SCENARIO");
        std::env::set_var("REXPRIMITIVE_BLOCKS_FILE", &blocks_file);
        std::env::set_var("REXPRIMITIVE_LOG_DIR", &dir);
        std::env::set_var("REXPRIMITIVE_SCENARIO", scenario);
        Self {
            dir,
            old_blocks_file,
            old_log_dir,
            old_scenario,
            foreground_env: None,
        }
    }

    fn with_codex_foreground(mut self) -> Self {
        self.foreground_env = Some((
            std::env::var_os("COLLAB_TEST_REX_OWNER_PID"),
            std::env::var_os("COLLAB_TEST_REX_OWNER_NAME"),
        ));
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", "4242");
        std::env::set_var("COLLAB_TEST_REX_OWNER_NAME", "codex");
        self
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
}

impl Drop for RexCliFixture {
    fn drop(&mut self) {
        crate::client::adapters::rex::set_test_cli(None);
        crate::client::adapters::rex::set_test_timeout(None);
        match self.old_blocks_file.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_BLOCKS_FILE", value),
            None => std::env::remove_var("REXPRIMITIVE_BLOCKS_FILE"),
        }
        match self.old_log_dir.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_LOG_DIR", value),
            None => std::env::remove_var("REXPRIMITIVE_LOG_DIR"),
        }
        match self.old_scenario.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_SCENARIO", value),
            None => std::env::remove_var("REXPRIMITIVE_SCENARIO"),
        }
        if let Some((pid, name)) = self.foreground_env.take() {
            for (key, previous) in [
                ("COLLAB_TEST_REX_OWNER_PID", pid),
                ("COLLAB_TEST_REX_OWNER_NAME", name),
            ] {
                match previous {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
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
        "com.superlogical.terminal.process",
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
    let project_scope = crate::server::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();
    let session_id = format!("session-{worker_id}");
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
        Event::AgentPresenceObserved {
            presence: crate::server::name_state::AgentPresenceRecord {
                project_scope,
                worker_id: worker_id.into(),
                session_id,
                presence: crate::server::name_state::AgentPresence::Online,
                reason: "notification test recipient is online".into(),
                observed_ms: now,
            },
        },
    ]);
    subscription_id
}

fn run_late_old_acceptance_interleaving(new_wake_fails: bool) {
    let (mut server, root) = test_server();
    let old_subscription = register_and_subscribe(&server, "old");
    let new_subscription = register_and_subscribe(&server, "new");
    let now = now_ms();
    queue_message(&server, "old", &old_subscription, "notice", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "notice".into(),
        mode: "immediate".into(),
        source_thread_id: None,
    }]);

    let (started_tx, started_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = Arc::new(Mutex::new(0usize));
    let calls_for_sink = calls.clone();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
        let call = {
            let mut calls = calls_for_sink.lock().unwrap();
            *calls += 1;
            *calls
        };
        if call == 1 {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .expect("old delivery must be released by the test");
            return Ok(json!({"accepted": "old"}));
        }
        if new_wake_fails {
            Err("new holder wake failed".into())
        } else {
            Ok(json!({"accepted": "new"}))
        }
    });

    let server = Arc::new(server);
    let old_server = server.clone();
    let old_subscription_for_thread = old_subscription.clone();
    let old_delivery = thread::spawn(move || {
        attempt_notification_detailed_with_at(
            &old_server,
            "notice",
            &old_subscription_for_thread,
            now,
        )
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("old delivery callback must start");

    let handoff = {
        let state = server.state.lock().unwrap();
        let message = &state.msgs["notice"];
        super::name_mailbox::MailboxHandoff {
            project_scope: "/project".into(),
            name: "Alice".into(),
            occupancy_revision: 2,
            session_id: "session-new".into(),
            message_id: message.id.clone(),
            previous_recipient: message.to.clone(),
            recipient: "new".into(),
            wake_attempt_count: message.wake_attempt_count,
            previous_subscription: Some(old_subscription.clone()),
            accepted_ms: state.notification_delivery_accepted.get("notice").copied(),
            evidence: state.notification_delivery_evidence.get("notice").cloned(),
            failure: state.notification_delivery_failures.get("notice").cloned(),
        }
    };
    server.commit(&[Event::MailboxReassigned { handoff }]);

    let new_delivery =
        attempt_notification_detailed_with_at(&server, "notice", &new_subscription, now + 1);
    if new_wake_fails {
        assert!(matches!(new_delivery, NotificationAttempt::Rejected(_)));
    } else {
        assert!(new_delivery.accepted(), "{new_delivery:?}");
    }

    release_tx.send(()).unwrap();
    let old_delivery = old_delivery.join().unwrap();
    assert!(!old_delivery.accepted(), "{old_delivery:?}");
    let old_response = notification_send_response(
        json!({"msg_id": "notice"}),
        true,
        "appserver",
        &old_delivery,
    );
    assert!(!old_response.ok, "{old_response:?}");
    assert_eq!(old_response.data["durable"], true);
    assert_eq!(old_response.data["notification"], "stale-attempt-accepted");
    assert_eq!(old_response.data["consumed"], false);

    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["notice"].to, "new");
    assert_eq!(
        state.wake_bindings["notice"],
        super::mailbox::default_direct_message_id("new")
    );
    if new_wake_fails {
        let failure = state
            .notification_delivery_failures
            .get("notice")
            .expect("new holder failure must remain visible");
        assert!(
            state
                .notification_delivery_accepted
                .get("notice")
                .is_none_or(|accepted| *accepted < failure.failed_ms),
            "late old acceptance must not hide the new holder failure"
        );
    } else {
        assert_eq!(
            state.notification_delivery_evidence.get("notice"),
            Some(&json!({"accepted": "new"})),
            "late old acceptance must not overwrite the new delivery evidence"
        );
    }
}

#[test]
fn late_acceptance_cannot_replace_evidence_from_a_newer_attempt() {
    let (server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(&server, "recipient", &subscription, "notice", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "notice".into(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);
    let transport = server.state.lock().unwrap().workers["recipient"]
        .transport
        .clone()
        .unwrap();
    let attempt = attempt_tmux_notification_with_at(
        &server,
        "notice",
        &subscription,
        "recipient",
        &transport,
        None,
        0,
        true,
        now,
        false,
        &|_, _, _, _, _, _| {
            server.commit(&[
                Event::WakeAttempted {
                    ids: vec!["notice".into()],
                    attempted_ms: now + 1,
                    retry: false,
                },
                Event::NotificationDeliveryAccepted {
                    message_id: "notice".into(),
                    accepted_ms: now + 2,
                    evidence: Some(json!({"receipt": "new"})),
                },
            ]);
            Ok(json!({"receipt": "old"}))
        },
    );
    assert!(
        matches!(attempt, NotificationAttempt::StaleAccepted),
        "{attempt:?}"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_delivery_evidence["notice"],
        json!({"receipt": "new"})
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_reserved_attempt_cannot_reuse_the_previous_attempts_retry_proof() {
    let (server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(&server, "recipient", &subscription, "notice", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "notice".into(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);
    let transport = server.state.lock().unwrap().workers["recipient"]
        .transport
        .clone()
        .unwrap();
    let refused = attempt_tmux_notification_with_at(
        &server,
        "notice",
        &subscription,
        "recipient",
        &transport,
        None,
        0,
        true,
        now,
        false,
        &|_, _, _, _, _, _| Err("ADAPTER_ROUTE_UNAVAILABLE: disconnected".into()),
    );
    assert!(
        matches!(refused, NotificationAttempt::Rejected(_)),
        "{refused:?}"
    );
    assert!(explicit_retry_refusal(&server.state.lock().unwrap(), "notice").is_none());

    // This is the persisted state while a subsequent native attempt is in
    // flight, or after restart if that attempt never published its outcome.
    server.commit(&[Event::WakeAttempted {
        ids: vec!["notice".into()],
        attempted_ms: now + 1,
        retry: false,
    }]);
    let reloaded = current_state::load(&server.state_path, false).unwrap();
    assert!(explicit_retry_refusal(&reloaded, "notice").is_some());
    assert!(!notification_reconnect_pending(
        &reloaded,
        &reloaded.msgs["notice"]
    ));
    let stale_retry = attempt_tmux_notification_with_retry(
        &server,
        "notice",
        &subscription,
        "recipient",
        &transport,
        None,
        0,
        true,
        now + 2,
        true,
        true,
        false,
        &|_, _, _, _, _, _| Ok(json!({"replayed": true})),
    );
    assert!(
        matches!(stale_retry, NotificationAttempt::NotAttempted(_)),
        "{stale_retry:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepted_native_delivery_reports_an_unconfirmed_receipt_if_save_fails() {
    let (server, root) = test_server();
    let subscription = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(&server, "recipient", &subscription, "notice", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "notice".into(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);
    let transport = server.state.lock().unwrap().workers["recipient"]
        .transport
        .clone()
        .unwrap();
    let attempt = attempt_tmux_notification_with_at(
        &server,
        "notice",
        &subscription,
        "recipient",
        &transport,
        None,
        0,
        true,
        now,
        false,
        &|_, _, _, _, _, _| {
            let reserved_state = server.state_path.with_extension("reserved");
            std::fs::rename(&server.state_path, &reserved_state).unwrap();
            std::fs::create_dir(&server.state_path).unwrap();
            Ok(json!({"accepted": true}))
        },
    );
    let response =
        notification_send_response(json!({"msg_id": "notice"}), true, "appserver", &attempt);
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.data["notification"],
        "submitted-receipt-unconfirmed"
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["consumed"], false);
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["notice"].wake_attempt_count, 1);
    assert!(!state.notification_delivery_accepted.contains_key("notice"));
    assert!(explicit_retry_refusal(&state, "notice").is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_handoff_late_old_acceptance_does_not_overwrite_new_delivery() {
    run_late_old_acceptance_interleaving(false);
}

#[test]
fn name_handoff_late_old_acceptance_does_not_hide_new_failure() {
    run_late_old_acceptance_interleaving(true);
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
            assert!(notification_reconnect_pending(&state, &state.msgs[id]));
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
        assert!(!notification_reconnect_pending(&state, &state.msgs[id]));
        assert!(state.notification_delivery_accepted.contains_key(id));
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn periodic_deadline_advances_after_prior_read_is_pruned() {
    let (mut server, root) = test_server();
    let direct_message = register_and_subscribe(&server, "recipient");
    let subscription = format!("storm-{direct_message}");
    let now = now_ms();
    let interval = 60_000;
    let second_trigger = now + interval;
    let third_trigger = second_trigger + interval;
    let expires_ms = now + 86_400_000;
    {
        let mut state = server.state.lock().unwrap();
        let direct = state
            .notification_subscriptions
            .get(&direct_message)
            .cloned()
            .unwrap();
        let sub = NotificationSubscription {
            id: subscription.clone(),
            event: "deadline".into(),
            subject: Some("goal:task:storm".into()),
            trigger_ms: Some(now),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(interval),
            repeat_count: 100,
            expires_ms,
            periodic_declared: true,
            ..direct
        };
        state.notification_subscriptions.insert(sub.id.clone(), sub);
    }
    timers::tick_with_idle_at(&server, now, &|_| false);
    let first_id = {
        let state = server.state.lock().unwrap();
        state
            .wake_bindings
            .iter()
            .find_map(|(id, bound)| {
                (bound == &subscription
                    && state
                        .notification_occurrences
                        .get(id)
                        .is_some_and(|occurrence| occurrence.trigger_ms == now))
                .then(|| id.clone())
            })
            .expect("the first occurrence is issued with an occurrence marker")
    };
    server.commit(&[Event::Acked {
        ids: vec![first_id.clone()],
        consumed_ms: Some(now + 1),
    }]);
    server.commit(&[Event::Acked {
        ids: vec![first_id.clone()],
        consumed_ms: Some(now + 1),
    }]);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&subscription].fired_count,
            1
        );
        assert_eq!(
            state.notification_subscriptions[&subscription].trigger_ms,
            Some(second_trigger)
        );
    }

    let config = &mut Arc::get_mut(&mut server).unwrap().config;
    config.retention.ttl_days = 0;
    timers::tick_with_idle_at(&server, second_trigger, &|_| false);
    let second_id = {
        let state = server.state.lock().unwrap();
        let id = state
            .wake_bindings
            .iter()
            .find_map(|(id, bound)| {
                (bound == &subscription
                    && state
                        .notification_occurrences
                        .get(id)
                        .is_some_and(|occurrence| occurrence.trigger_ms == second_trigger))
                .then(|| id.clone())
            })
            .expect("the next periodic occurrence remains valid");
        assert_eq!(
            state.notification_subscriptions[&subscription].fired_count,
            1
        );
        assert!(
            !state.msgs.contains_key(&first_id),
            "the old read occurrence must be removed by the real retention owner"
        );
        id
    };
    Arc::get_mut(&mut server).unwrap().config.retention.ttl_days = 7;
    {
        let reloaded = current_state::load(&server.state_path, false).unwrap();
        assert_eq!(
            reloaded.notification_occurrences[&second_id].trigger_ms,
            second_trigger
        );
        *server.state.lock().unwrap() = reloaded;
    }
    server.commit(&[Event::Acked {
        ids: vec![second_id.clone()],
        consumed_ms: Some(second_trigger + 1),
    }]);
    server.commit(&[Event::Acked {
        ids: vec![second_id.clone()],
        consumed_ms: Some(second_trigger + 2),
    }]);
    timers::tick_with_idle_at(&server, third_trigger - 1, &|_| false);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&subscription].fired_count,
            2
        );
        assert_eq!(
            state.notification_subscriptions[&subscription].trigger_ms,
            Some(third_trigger)
        );
        assert_eq!(
            state.notification_subscriptions[&subscription].expires_ms,
            expires_ms
        );
        assert_eq!(
            state
                .notification_occurrences
                .values()
                .filter(|occurrence| occurrence.subscription_id == subscription)
                .count(),
            1,
            "a consumed occurrence stays for periodic retention cleanup: {:?}",
            state.notification_subscriptions[&subscription]
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn periodic_deadline_consumption_marks_occurrence_for_retention() {
    let (mut server, root) = test_server();
    let direct_message = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    let interval = 86_400_000;
    let second_trigger = now + interval;
    let third_trigger = second_trigger + interval;
    Arc::get_mut(&mut server).unwrap().config.retention.ttl_days = 1;
    let assert_consumed_marker = |server: &Arc<Server>, trigger: i64, expected: bool| {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_occurrences.values().any(|occurrence| {
                occurrence.subscription_id == "storm-retention-condition"
                    && occurrence.trigger_ms == trigger
            }),
            expected
        );
    };
    let subscription = {
        let state = server.state.lock().unwrap();
        let direct = state
            .notification_subscriptions
            .get(&direct_message)
            .cloned()
            .unwrap();
        NotificationSubscription {
            id: "storm-retention-condition".into(),
            event: "deadline".into(),
            subject: Some("goal:retention:condition".into()),
            trigger_ms: Some(now),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(interval),
            repeat_count: 100,
            expires_ms: now + 2 * 86_400_000,
            periodic_declared: true,
            ..direct
        }
    };
    server.commit(&[Event::NotificationSubscribed { subscription }]);

    for (trigger, received_ms) in [(now, now + 1), (second_trigger, second_trigger + 1)] {
        timers::tick_with_idle_at(&server, trigger, &|_| false);
        let message_id = {
            let state = server.state.lock().unwrap();
            state.wake_bindings.iter().find_map(|(id, bound)| {
                (bound == "storm-retention-condition"
                    && state.notification_occurrences[id].trigger_ms == trigger)
                    .then(|| id.clone())
            })
        }
        .expect("the due timer issues one marked occurrence");
        assert_eq!(
            server.state.lock().unwrap().notification_subscriptions["storm-retention-condition"]
                .trigger_ms,
            Some(trigger)
        );
        server.commit(&[Event::Acked {
            ids: vec![message_id],
            consumed_ms: Some(received_ms),
        }]);
        assert_consumed_marker(&server, trigger, true);
    }
    let next_after_late_receive = server.state.lock().unwrap().notification_subscriptions
        ["storm-retention-condition"]
        .trigger_ms;
    assert_eq!(next_after_late_receive, Some(third_trigger));
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["storm-retention-condition"].fired_count,
        2
    );
    assert_eq!(
        state
            .notification_occurrences
            .values()
            .filter(|occurrence| occurrence.subscription_id == "storm-retention-condition")
            .count(),
        1,
        "retention keeps only the current read occurrence until its cleanup window"
    );
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
fn queued_appserver_wake_drains_through_the_existing_timer_once() {
    let (mut server, root) = test_server();
    let subscription_id = register_and_subscribe(&server, "recipient");
    let now = now_ms();
    queue_message(
        &server,
        "recipient",
        &subscription_id,
        "queued-appserver",
        now,
    );
    server.commit(&[Event::DeliveryMode {
        msg_id: "queued-appserver".into(),
        mode: "queued".into(),
        source_thread_id: None,
    }]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, _, message_id, _, mode| {
        observed
            .lock()
            .unwrap()
            .push((message_id.to_owned(), mode.to_owned()));
        Ok(json!({"accepted": true}))
    });

    timers::tick_with_idle_at(&server, now + 1, &|_| false);

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [("queued-appserver".to_owned(), "queued".to_owned())]
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["queued-appserver"].id, "queued-appserver");
    assert_eq!(state.msgs["queued-appserver"].state, "pending");
    assert_eq!(state.msgs["queued-appserver"].wake_attempt_count, 1);
    assert!(state
        .notification_delivery_accepted
        .contains_key("queued-appserver"));
    drop(state);

    timers::tick_with_idle_at(&server, now + 2, &|_| false);
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "accepted queued AppServer wake must not be submitted again"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dsh_known_non_submission_resumes_once_and_keeps_the_original_message() {
    let (mut server, root) = test_server();
    let gateway = crate::server::dsh_channel_tests::registration_gateway(
        "notification-retry",
        &root,
        None,
        crate::server::dsh_channel_tests::wake_reply,
    );
    let subscription_id = register_and_subscribe(&server, "recipient");
    {
        let mut state = server.state.lock().unwrap();
        let transport = state
            .workers
            .get_mut("recipient")
            .unwrap()
            .transport
            .as_mut()
            .unwrap();
        transport.kind = TransportKind::Dsh;
        transport.endpoint = Some(gateway.endpoint());
        transport.namespace = Some("rt-1".into());
        transport.session_id = Some("ses-1".into());
        transport.thread_id = Some("ses-1".into());
        state
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap()
            .method = "dsh".into();
    }
    let now = now_ms();
    queue_message(&server, "recipient", &subscription_id, "dsh-retry", now);
    server.commit(&[Event::DeliveryMode {
        msg_id: "dsh-retry".into(),
        mode: "immediate".into(),
        source_thread_id: None,
    }]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, _, message_id, _, _| {
        let mut calls = observed.lock().unwrap();
        calls.push(message_id.to_owned());
        if calls.len() == 1 {
            Err("DSH_ENDPOINT_BLOCKED: gateway control socket is unreachable".into())
        } else {
            Ok(json!({"accepted": true}))
        }
    });

    let first = attempt_notification_detailed_with_at(&server, "dsh-retry", &subscription_id, now);
    assert!(
        matches!(first, NotificationAttempt::Rejected(_)),
        "{first:?}"
    );
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["dsh-retry"].id, "dsh-retry");
        assert_eq!(state.msgs["dsh-retry"].wake_attempt_count, 1);
        let failure = state
            .notification_delivery_failures
            .get("dsh-retry")
            .expect("typed DSH non-submission is durable");
        assert!(failure.retryable);
        assert_eq!(
            failure.failed_ms,
            state.msgs["dsh-retry"].last_wake_attempt_ms
        );
        assert!(notification_reconnect_pending(
            &state,
            &state.msgs["dsh-retry"]
        ));
    }

    timers::tick_with_idle_at(&server, now + 1, &|_| false);
    timers::tick_with_idle_at(&server, now + 2, &|_| false);

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["dsh-retry".to_owned(), "dsh-retry".to_owned()]
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["dsh-retry"].id, "dsh-retry");
    assert_eq!(state.msgs["dsh-retry"].wake_attempt_count, 2);
    assert!(state
        .notification_delivery_accepted
        .contains_key("dsh-retry"));
    assert!(!state
        .notification_delivery_failures
        .contains_key("dsh-retry"));
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
    let _presence = RexCliFixture::new(&root, "normal");
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
    let _presence = RexCliFixture::new(&root, "normal");
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
    let _presence = RexCliFixture::new(&root, "normal");
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
    let _presence = RexCliFixture::new(&root, "normal");
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
    let _presence = RexCliFixture::new(&root, "normal");
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
fn retained_identity_without_transport_never_delivers_to_cached_holder() {
    for kind in [TransportKind::Rex, TransportKind::AppServer] {
        let (mut server, root) = test_server();
        let route = seed_rex_route(&server);
        let mut old = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
        old.kind = kind;
        if old.kind == TransportKind::AppServer {
            old.endpoint = Some("/tmp/unused-forward-appserver.sock".into());
            old.namespace = Some("test".into());
            old.thread_id = Some("thread-a".into());
            old.rex_endpoint = None;
        }
        let subscription = register_rex_worker(&server, "a", &old);
        {
            let mut state = server.state.lock().unwrap();
            state
                .notification_subscriptions
                .get_mut(&subscription)
                .unwrap()
                .method = notification_transport_method(&old).into();
        }
        seed_current_rex_bindings(&server, &route, &[("Alice", "a", &old)]);
        let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
        let control = store.load().unwrap();
        let slot = control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, "a")
            .unwrap()
            .unwrap();
        let mut retained = slot.active_binding().unwrap().clone();
        retained.transport = None;
        store
            .commit(binding_store::ControlChange::Commit {
                key: slot.identity_key.clone(),
                value: retained,
                expected_generation: Some(slot.generation),
            })
            .unwrap();
        let calls = install_capture_sink(&mut server);
        let response = send_notify(&server, "sender", "a");
        assert!(calls.lock().unwrap().is_empty(), "{response:?}");
        let state = server.state.lock().unwrap();
        let message_id = response.data["msg_id"].as_str().unwrap();
        assert_eq!(state.msgs[message_id].state, "pending");
        assert!(!state
            .notification_delivery_accepted
            .contains_key(message_id));
        drop(state);
        let control = store.load().unwrap();
        let slot = control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, "a")
            .unwrap()
            .unwrap();
        assert!(slot.active_binding().unwrap().transport.is_none());
        assert_eq!(
            slot.reference_previous().unwrap().transport.as_ref(),
            Some(&old)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn rex_current_owner_same_block_on_separate_servers_remains_independent() {
    let (mut server, root) = test_server();
    let _presence = RexCliFixture::new(&root, "normal");
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
fn rex_current_owner_busy_publication_queues_the_same_message() {
    let (mut server, root) = test_server();
    let _presence = RexCliFixture::new(&root, "normal");
    let route = seed_rex_route(&server);
    let current = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
    register_rex_worker(&server, "a", &current);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    let sink_calls = install_capture_sink(&mut server);

    let wire_gate = wire_route_mutation_gate(&server);
    let _wire_guard = wire_gate.lock().unwrap();
    let _register_guard = server.register_gate.lock().unwrap();
    let response = send_notify(&server, "sender", "a");

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "queued");
    assert!(response.data["notification_error"]
        .as_str()
        .unwrap()
        .contains("REX_BINDING_PUBLICATION_BUSY"));
    assert_eq!(response.data["consumed"], false);
    let id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.delivery_modes[id], "immediate");
        assert_eq!(state.msgs[id].state, "pending");
        assert_eq!(state.msgs[id].wake_attempt_count, 0);
        assert!(!state.notification_delivery_accepted.contains_key(id));
        assert!(!state.notification_delivery_failures.contains_key(id));
        assert!(!state.notification_delivery_bindings.contains_key(id));
    }
    let before = dispatch(&server, Req::MsgStatus { msg_id: id.into() });
    assert!(before.ok, "{before:?}");
    assert_eq!(before.data["state"], "pending");
    assert!(sink_calls.lock().unwrap().is_empty());
    drop(_register_guard);
    drop(_wire_guard);
    timers::tick_with_idle_at(&server, now_ms() + 1, &|_| true);
    {
        let state = server.state.lock().unwrap();
        assert!(state.notification_delivery_accepted.contains_key(id));
        assert_eq!(state.msgs[id].state, "pending");
        assert_eq!(state.msgs[id].wake_attempt_count, 1);
        assert_eq!(state.delivery_modes[id], "immediate");
    }
    assert_eq!(sink_calls.lock().unwrap().len(), 1);
    let accepted = dispatch(&server, Req::MsgStatus { msg_id: id.into() });
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(accepted.data["state"], "pending");

    timers::tick_with_idle_at(&server, now_ms() + 2, &|_| true);
    assert_eq!(
        sink_calls.lock().unwrap().len(),
        1,
        "accepted original attempt must not be submitted again"
    );

    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.delivery_modes[id], "immediate");
        assert_eq!(state.msgs[id].wake_attempt_count, 1);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn timer_resumes_only_the_current_board_invitation_after_publication_busy() {
    let (mut server, root) = test_server();
    let _presence = RexCliFixture::new(&root, "normal");
    let route = seed_rex_route(&server);
    let transport = rex_transport("agent-a", "rex-server", "rex-session", "block-a");
    let subscription = register_rex_worker(&server, "a", &transport);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &transport)]);
    let sink_calls = install_capture_sink(&mut server);
    let now = now_ms();

    let invitation = "current-invitation";
    let stale = "old-binding-invitation";
    let ordinary = "unrelated-explicit";
    let unconfirmed = "unconfirmed-invitation";
    for id in [invitation, stale, ordinary, unconfirmed] {
        queue_message(&server, "a", &subscription, id, now);
        server.commit(&[Event::DeliveryMode {
            msg_id: id.into(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        }]);
    }
    for (task_id, message_id, binding_id) in [
        ("current-task", invitation, "binding-Alice"),
        ("stale-task", stale, "previous-binding"),
        ("unconfirmed-task", unconfirmed, "binding-Alice"),
    ] {
        let mut details = crate::board::BoardTaskDetails::legacy(task_id, true);
        details.invitation = Some(crate::board::BoardInvitation {
            peer_id: "a".into(),
            binding_id: binding_id.into(),
            message_id: message_id.into(),
            created_ms: now,
            original_status: Some("pending".into()),
            original_owner: Some("sender".into()),
            inviter_name: None,
            automatic_source: None,
        });
        server.commit(&[
            Event::TaskCreated {
                task: serde_json::from_value(serde_json::json!({
                    "id": task_id, "owner": "sender", "created_by": "sender",
                    "status": "invited", "created_ms": now, "updated_ms": now,
                }))
                .unwrap(),
            },
            Event::BoardDetailsChanged {
                task_id: task_id.into(),
                details,
            },
        ]);
    }
    server.commit(&[Event::WakeAttempted {
        ids: vec![unconfirmed.into()],
        attempted_ms: now,
        retry: false,
    }]);
    let gate = server.register_gate.lock().unwrap();
    let deferred = attempt_notification_detailed_with_at(&server, invitation, &subscription, now);
    assert!(matches!(deferred, NotificationAttempt::Deferred(_)));
    assert!(sink_calls.lock().unwrap().is_empty());
    drop(gate);

    timers::tick_with_idle_at(&server, now + 1, &|_| true);
    timers::tick_with_idle_at(&server, now + 2, &|_| true);
    assert_eq!(sink_calls.lock().unwrap().len(), 1);
    let state = server.state.lock().unwrap();
    assert!(state
        .notification_delivery_accepted
        .contains_key(invitation));
    assert_eq!(state.msgs[invitation].wake_attempt_count, 1);
    assert_eq!(state.msgs[ordinary].wake_attempt_count, 0);
    assert_eq!(state.msgs[stale].wake_attempt_count, 0);
    assert_eq!(state.msgs[unconfirmed].wake_attempt_count, 1);
    assert!(!state
        .notification_delivery_accepted
        .contains_key(unconfirmed));
    assert_eq!(state.tasks["current-task"].status, "invited");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_current_owner_slow_delivery_serializes_replacement() {
    let (mut server, root) = test_server();
    let _presence = RexCliFixture::new(&root, "normal");
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
        Some(old_transport.clone())
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
        let _presence = RexCliFixture::new(&root, "normal");
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
    let fixture = RexCliFixture::new(&root, "service_changed").with_codex_foreground();
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
    mark_agent_offline(&server, "a", stale.session_id.as_deref().unwrap());

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
    let refreshed_transport = refreshed.transport.unwrap();
    let refreshed_endpoint = refreshed_transport.rex_endpoint.as_ref().unwrap();
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
async fn rex_routed_send_refuses_non_agent_foreground_with_online_monitor() {
    let _env_guard = crate::scope::TEST_ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server).unwrap().appserver_notification_sink =
        default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "normal");
    let route = seed_rex_route(&server);
    let transport = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    register_rex_worker(&server, "a", &transport);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &transport)]);
    persist_rex_worker_binding(&server, &route, "a", &transport);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let send_root = root.clone();
    let response = tokio::task::spawn_blocking(move || manager_send_to_rex(&manager, &send_root, "a"))
        .await.unwrap();
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "queued-recipient-offline");
    let calls = fixture.calls();
    assert!(calls.iter().any(|call| rex_method(call) == "com.superlogical.terminal.process"));
    assert!(!calls.iter().any(|call| matches!(
        rex_method(call).as_str(), "com.superlogical.terminal.write" | "send-key"
    )));
    std::fs::remove_dir_all(root).unwrap();
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
        assert_eq!(message.wake_attempt_count, 1);
        assert!(!message.retry_attempted);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_status_error_with_service_marker_but_no_typed_change_is_not_recovered() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (mut server, root) = test_server();
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_notification_sink = default_appserver_notification_sink();
    let fixture = RexCliFixture::new(&root, "status_marker_only");
    let route = seed_rex_route(&server);
    let current = rex_transport(
        "agent-a",
        "unix:///tmp/rex-fixture.sock",
        "session:11111111-1111-1111-1111-111111111111",
        "block:22222222-2222-2222-2222-222222222222",
    );
    register_rex_worker(&server, "a", &current);
    seed_current_rex_bindings(&server, &route, &[("Alice", "a", &current)]);
    persist_rex_worker_binding(&server, &route, "a", &current);

    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let send_root = root.clone();
    let response =
        tokio::task::spawn_blocking(move || manager_send_to_rex(&manager, &send_root, "a"))
            .await
            .unwrap();

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["notification"], "queued-recipient-offline");
    assert!(response.data["notification_error"]
        .as_str()
        .unwrap()
        .starts_with("RECIPIENT_NOT_ONLINE:"));
    assert!(
        response.data.get("rex_binding_refresh").is_none(),
        "a non-generation status error must not enter service refresh: {response:?}"
    );
    assert!(
        response.data.get("rex_binding_retry").is_none(),
        "{response:?}"
    );
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[message_id].wake_attempt_count, 0);
        assert!(!state
            .notification_delivery_failures
            .contains_key(message_id));
    }
    let calls = fixture.calls();
    assert!(calls.iter().any(|call| rex_method(call) == "server.status"));
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
        let gateway = (kind == TransportKind::Dsh).then(|| {
            crate::server::dsh_channel_tests::registration_gateway(
                "native-notification",
                &root,
                None,
                crate::server::dsh_channel_tests::wake_reply,
            )
        });
        let subscription = register_and_subscribe(&server, "recipient");
        {
            let mut state = server.state.lock().unwrap();
            let transport = state
                .workers
                .get_mut("recipient")
                .unwrap()
                .transport
                .as_mut()
                .unwrap();
            transport.kind = kind.clone();
            if let Some(gateway) = &gateway {
                transport.endpoint = Some(gateway.endpoint());
                transport.namespace = Some("rt-1".into());
                transport.session_id = Some("ses-1".into());
                transport.thread_id = Some("ses-1".into());
            }
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
        let gateway = (kind == TransportKind::Dsh).then(|| {
            crate::server::dsh_channel_tests::registration_gateway(
                "native-notification-unconsumed",
                &root,
                None,
                crate::server::dsh_channel_tests::wake_reply,
            )
        });
        let subscription = register_and_subscribe(&server, "recipient");
        {
            let mut state = server.state.lock().unwrap();
            let transport = state
                .workers
                .get_mut("recipient")
                .unwrap()
                .transport
                .as_mut()
                .unwrap();
            transport.kind = kind.clone();
            if let Some(gateway) = &gateway {
                transport.endpoint = Some(gateway.endpoint());
                transport.namespace = Some("rt-1".into());
                transport.session_id = Some("ses-1".into());
                transport.thread_id = Some("ses-1".into());
            }
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
fn queued_tmux_wake_stays_silent_while_recipient_offline() {
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
        matches!(
            &attempt,
            NotificationAttempt::NotAttempted(error) if error.starts_with("RECIPIENT_NOT_ONLINE:")
        ),
        "an absent pane is offline and must not receive queued input: {attempt:?}"
    );
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
    assert!(
        calls.lock().unwrap().is_empty(),
        "the timer must not submit to an offline pane"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.delivery_modes["deferred-tmux"], "queued");
    assert_eq!(state.msgs["deferred-tmux"].state, "pending");
    assert_eq!(state.msgs["deferred-tmux"].wake_attempt_count, 0);
    assert!(!state
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
        0,
        "offline queued wake must stay silent across timer ticks"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_recipient_never_receives_transport_input() {
    let (mut server, root) = test_server();
    let submitted = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let submitted = Arc::clone(&submitted);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink = Arc::new(move |_transport, _, _, message_id, _, _| {
            submitted.lock().unwrap().push(message_id.to_string());
            Ok(serde_json::json!({"accepted": true}))
        });
    }
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": { "type": "systemError" },
                "canAcceptDirectInput": false
            }
        }))
    });
    let server = Arc::new(server);
    let subscription = register_and_subscribe(&server, "offline-peer");
    {
        let mut state = server.state.lock().unwrap();
        let worker = state.workers.get_mut("offline-peer").unwrap();
        worker.transport = Some(rex_transport(
            "session-offline-peer",
            "test-server",
            "test-session",
            "test-block",
        ));
        state
            .notification_subscriptions
            .get_mut(&subscription)
            .unwrap()
            .method = "rex".into();
        state
            .notification_subscriptions
            .get_mut(&subscription)
            .unwrap()
            .target = "test-session/test-block".into();
    }
    mark_agent_offline(&server, "offline-peer", "session-offline-peer");
    queue_message(
        &server,
        "offline-peer",
        &subscription,
        "offline-message",
        now_ms(),
    );

    let attempt =
        attempt_notification_detailed_with_at(&server, "offline-message", &subscription, now_ms());
    assert!(
        matches!(
            attempt,
            NotificationAttempt::NotAttempted(ref error)
                if error.starts_with("RECIPIENT_NOT_ONLINE:")
        ),
        "offline recipient must be refused before transport input: {attempt:?}"
    );
    assert!(
        submitted.lock().unwrap().is_empty(),
        "an offline agent must never receive shell or thread input"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["offline-message"].state, "pending");
    assert_eq!(state.msgs["offline-message"].wake_attempt_count, 0);
    assert!(!state
        .notification_delivery_accepted
        .contains_key("offline-message"));
    drop(state);
    std::fs::remove_dir_all(root).ok();
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
    assert!(matches!(
        attempt_notification_detailed_with_at(
            &server,
            "backlog-0",
            &subscription_id,
            window_start + 120_000,
        ),
        NotificationAttempt::Deferred(None)
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
