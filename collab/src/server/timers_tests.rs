use super::*;
use crate::proto::SelectedTransport;
use crate::server::keepalive::Record;
use crate::server::state::{
    Event, MigrationRecord, NotificationSubscription, State, TaskRec, WaitSpec,
};
use std::sync::Mutex;

fn test_server() -> (Arc<Server>, std::path::PathBuf) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-notification-timer-{}-{sequence}",
        std::process::id()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server_dir.join("journal.jsonl"))
        .unwrap();
    (
        Arc::new(Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(State::default()),
            journal: Mutex::new(journal),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(SelectedTransport {
                    kind: crate::proto::TransportKind::AppServer,
                    endpoint: Some(candidate.endpoint.clone()),
                    namespace: Some(candidate.namespace.clone()),
                    session_id: Some(candidate.session_id.clone()),
                    thread_id: Some(candidate.thread_id.clone()),
                    tmux_endpoint: None,
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
            mailbox_notify: tokio::sync::Notify::new(),
        }),
        root,
    )
}

fn register(server: &Server, worker_id: &str) {
    let response =
        crate::server::peer_tests::register(server, worker_id, &format!("thread-{worker_id}"));
    assert!(response.ok, "worker registration failed: {response:?}");
}

fn subscription_messages<'a>(
    state: &'a State,
    subscription_id: &str,
) -> Vec<&'a crate::server::state::Message> {
    state
        .wake_bindings
        .iter()
        .filter(|(_, bound)| bound.as_str() == subscription_id)
        .map(|(message_id, _)| &state.msgs[message_id])
        .collect()
}

fn wait_timeout_messages(state: &State) -> Vec<&crate::server::state::Message> {
    state
        .msgs
        .values()
        .filter(|message| {
            message.mtype == "notify" && message.subject.as_deref() == Some("wait-timeout:task")
        })
        .collect()
}

fn register_tmux(server: &Server, worker_id: &str, endpoint: crate::proto::TmuxEndpoint) {
    let response = crate::server::peer_tests::register_tmux(server, worker_id, endpoint);
    assert!(response.ok, "worker registration failed: {response:?}");
}

fn seed_ledger_owner(server: &Server, worker_id: &str) {
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: server
            .root
            .join("prior-ledger-owner.sock")
            .display()
            .to_string(),
        server_pid: 42,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 99,
        codex_session_id: Some(format!("session-{worker_id}")),
        codex_thread_id: Some(format!("thread-{worker_id}")),
    };
    let transport = SelectedTransport {
        kind: crate::proto::TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint.codex_session_id.clone(),
        thread_id: endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "prior committed ledger owner".into(),
    };
    let response = crate::server::peer_tests::seed_committed_owner(
        server,
        worker_id,
        &format!("token-{worker_id}"),
        &transport,
        &server.root.display().to_string(),
        Some(crate::identity::AppServerId::new("tui-default").unwrap()),
    );
    assert!(response.ok, "committed ledger fixture failed: {response:?}");
    let binding = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding(&crate::identity::BindingId::new(format!("binding-{worker_id}")).unwrap())
        .unwrap()
        .clone();
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet { binding }])
        .unwrap();
}

fn register_master(server: &Server) {
    register(server, "master");
    let now = now_ms();
    let promoted = crate::server::handle_master_promote(
        server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record: Record {
            observed: "idle".into(),
            idle_since_ms: now - 900_001,
            ..Record::default()
        },
    }]);
}

fn master_idle_subscription(server: &Server, interval_ms: i64) -> String {
    let now = now_ms();
    let id = format!("sub-master-idle-{interval_ms}");
    let (target, method) = subscription_target(server, "master");
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.clone(),
            worker_id: "master".into(),
            event: "master-idle".into(),
            subject: Some("master-idle".into()),
            target,
            method,
            trigger_ms: Some(now - 1),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(interval_ms),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: now + 86_400_000,
            status: "armed".into(),
            created_ms: now - interval_ms,
            updated_ms: now,
            status_reason: None,
        },
    }]);
    id
}

fn subscribe(
    server: &Server,
    worker_id: &str,
    event: &str,
    subject: Option<&str>,
    trigger_ms: Option<i64>,
) -> String {
    let id = format!("sub-{worker_id}-{event}");
    let (target, method) = subscription_target(server, worker_id);
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.clone(),
            worker_id: worker_id.into(),
            event: event.into(),
            subject: subject.map(str::to_owned),
            target,
            method,
            trigger_ms,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now_ms() + 60_000,
            status: "armed".into(),
            created_ms: now_ms(),
            updated_ms: now_ms(),
            status_reason: None,
        },
    }]);
    id
}

fn periodic_deadline_subscription(server: &Server, worker_id: &str, subject: &str) -> String {
    let now = now_ms();
    let id = format!("sub-{worker_id}-periodic-deadline");
    let (target, method) = subscription_target(server, worker_id);
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.clone(),
            worker_id: worker_id.into(),
            event: "deadline".into(),
            subject: Some(subject.into()),
            target,
            method,
            trigger_ms: Some(now - 1),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(600_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: now + 86_400_000,
            status: "armed".into(),
            created_ms: now - 600_000,
            updated_ms: now,
            status_reason: None,
        },
    }]);
    id
}

fn subscription_target(server: &Server, worker_id: &str) -> (String, String) {
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get(worker_id)
        .and_then(|worker| worker.transport.as_ref())
        .map(|transport| {
            (
                transport.thread_id.clone().unwrap_or_default(),
                transport.kind.as_str().to_owned(),
            )
        })
        .unwrap_or_else(|| (format!("thread-{worker_id}"), "appserver".into()))
}

fn freeze_admission(server: &Server) {
    let now = now_ms();
    server.commit(&[Event::MigrationUpdated {
        migration: MigrationRecord {
            id: "migration".into(),
            from_version: "v1".into(),
            to_version: "v1".into(),
            phase: "applied".into(),
            admission_frozen: true,
            snapshot_hash: None,
            worker_count: 1,
            task_count: 0,
            message_count: 0,
            operator: "test".into(),
            issues: Vec::new(),
            created_ms: now,
            updated_ms: now,
        },
    }]);
}

fn bind_message(server: &Server, worker_id: &str, subscription_id: &str) -> String {
    bind_message_with_id(
        server,
        worker_id,
        subscription_id,
        &format!("message-{worker_id}"),
    )
}

fn bind_message_with_type(
    server: &Server,
    worker_id: &str,
    subscription_id: &str,
    message_id: &str,
    message_type: &str,
) -> String {
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: message_id.to_string(),
                from: "peer".into(),
                to: worker_id.into(),
                mtype: message_type.into(),
                subject: Some("released:held".into()),
                body: "RESOURCE_RELEASED task=held".into(),
                in_reply_to: None,
                created_ms: now_ms() - 120_001,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: message_id.to_string(),
            subscription_id: subscription_id.into(),
        },
    ]);
    message_id.to_string()
}

fn bind_message_with_id(
    server: &Server,
    worker_id: &str,
    subscription_id: &str,
    message_id: &str,
) -> String {
    bind_message_with_type(server, worker_id, subscription_id, message_id, "notify")
}

fn working_task(server: &Server, worker_id: &str) {
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task".into(),
            owner: worker_id.into(),
            created_by: worker_id.into(),
            feature_id: Some("feature".into()),
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "working".into(),
            next_step: Some("keep working".into()),
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
}

#[test]
fn ordinary_work_never_generates_periodic_continuation() {
    let (server, root) = test_server();
    register(&server, "owner");
    working_task(&server, "owner");
    tick_with_idle(&server, &|_| true);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn admission_freeze_blocks_deadline_wakeup_without_appserver_probe() {
    let (mut server, root) = test_server();
    let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
    register_tmux(&server, "master", tmux.endpoints().remove(0));
    let now = now_ms();
    let promoted = crate::server::handle_master_promote(
        &server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record: Record {
            observed: "idle".into(),
            idle_since_ms: now - 900_001,
            ..Record::default()
        },
    }]);
    periodic_deadline_subscription(&server, "master", "goal:frozen");
    freeze_admission(&server);

    tick_at(&server, now_ms());

    assert!(server.state.lock().unwrap().msgs.is_empty());
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deadline_wakeup_does_not_probe_appserver_readiness() {
    let (mut server, root) = test_server();
    let probes = Arc::new(Mutex::new(Vec::new()));
    {
        let probes = Arc::clone(&probes);
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_thread_status = Arc::new(move |_, thread_id| {
            probes.lock().unwrap().push(thread_id.to_string());
            Ok(serde_json::json!({
                "thread": {
                    "id": thread_id,
                    "status": {"type": "idle"},
                    "canAcceptDirectInput": true
                }
            }))
        });
    }
    let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
    let endpoints = tmux.endpoints();
    register_tmux(&server, "master", endpoints[0].clone());
    register_tmux(&server, "worker", endpoints[1].clone());
    let now = now_ms();
    let promoted = crate::server::handle_master_promote(
        &server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record: Record {
            observed: "idle".into(),
            idle_since_ms: now - 900_001,
            ..Record::default()
        },
    }]);
    periodic_deadline_subscription(&server, "master", "periodic:due-master");
    subscribe(
        &server,
        "worker",
        "deadline",
        Some("periodic:not-due-worker"),
        Some(now_ms() + 600_000),
    );
    probes.lock().unwrap().clear();

    probes.lock().unwrap().clear();
    tick_at(&server, now_ms());

    assert!(probes.lock().unwrap().is_empty());
    assert!(server.state.lock().unwrap().msgs.values().any(|message| {
        message.to == "master" && message.subject.as_deref() == Some("deadline:periodic:due-master")
    }));
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deadline_wakeup_does_not_depend_on_appserver_status_callback() {
    let (mut server, root) = test_server();
    let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
    register_tmux(&server, "worker", tmux.endpoints().remove(0));
    Arc::get_mut(&mut server)
        .expect("unique test server")
        .appserver_thread_status = Arc::new(|_, _| panic!("AppServer probe must not run"));
    let subscription_id = subscribe(
        &server,
        "worker",
        "deadline",
        Some("worker:one-shot"),
        Some(now_ms() - 1),
    );

    tick_at(&server, now_ms());

    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs.len(), 1);
    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.status, "armed");
    drop(state);
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn configured_immediate_and_disabled_delivery_are_respected() {
    for (mode, enabled, expected) in [
        ("immediate", true, true),
        ("batch", true, false),
        ("immediate", false, false),
    ] {
        let (mut server, root) = test_server();
        let config = &mut Arc::get_mut(&mut server).unwrap().config;
        config.notifications.mode = mode.into();
        config.notifications.enabled = enabled;
        register(&server, "owner");
        let sub = subscribe(&server, "owner", "direct-message", None, None);
        let id = bind_message(&server, "owner", &sub);
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .get_mut(&id)
            .unwrap()
            .created_ms = now_ms();
        assert_eq!(
            super::super::attempt_notification_with_default(
                &server,
                &id,
                &sub,
                &|_| true,
                &|_, _| { true }
            ),
            expected
        );
        assert_eq!(
            server.state.lock().unwrap().msgs[&id].wake_attempt_count,
            if expected { 1 } else { 0 }
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn no_subscription_means_zero_wake_attempts() {
    let (server, root) = test_server();
    register(&server, "owner");
    server.commit(&[Event::Sent {
        msg: Message {
            id: "message".into(),
            from: "peer".into(),
            to: "owner".into(),
            mtype: "notify".into(),
            subject: Some("released:held".into()),
            body: "RESOURCE_RELEASED task=held".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    tick_with_idle(&server, &|_| true);
    assert_eq!(
        server.state.lock().unwrap().msgs["message"].wake_attempt_count,
        0
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn failed_one_shot_notification_has_one_attempt_lifetime_cap() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "resource-released", Some("held"), None);
    let message_id = bind_message(&server, "owner", &subscription_id);
    for _ in 0..4 {
        super::super::attempt_notification_with_default(
            &server,
            &message_id,
            &subscription_id,
            &|_| true,
            &|_, _| false,
        );
    }
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.msgs[&message_id].wake_attempt_count,
        MAX_WAKE_ATTEMPTS
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "armed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn failed_direct_message_exhausts_only_the_message() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
    let message_id = bind_message(&server, "owner", &subscription_id);
    for _ in 0..MAX_WAKE_ATTEMPTS {
        super::super::attempt_notification_with_default(
            &server,
            &message_id,
            &subscription_id,
            &|_| true,
            &|_, _| false,
        );
    }
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.msgs[&message_id].wake_attempt_count,
        MAX_WAKE_ATTEMPTS
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "armed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restart_does_not_reset_exhausted_direct_message_attempts() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
    let message_id = bind_message(&server, "owner", &subscription_id);
    for _ in 0..MAX_WAKE_ATTEMPTS {
        super::super::attempt_notification_with_default(
            &server,
            &message_id,
            &subscription_id,
            &|_| true,
            &|_, _| false,
        );
    }
    drop(server);

    let replayed = super::super::replay(&root).unwrap();
    let journal = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".agent-collab/server/journal.jsonl"))
        .unwrap();
    let restarted = Server {
        config: crate::config::Config::default(),
        root: root.clone(),
        storage_root: root.clone(),
        journal_path: root.join(".agent-collab/server/journal.jsonl"),
        host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(replayed),
        journal: Mutex::new(journal),
        appserver_candidate_check: super::super::default_appserver_candidate_check(),
        appserver_notification_sink: super::super::default_appserver_notification_sink(),
        appserver_thread_status: super::super::default_appserver_thread_status(),
        appserver_thread_archive: super::super::default_appserver_thread_archive(),
        mailbox_notify: tokio::sync::Notify::new(),
    };
    let sent = std::sync::atomic::AtomicBool::new(false);
    assert!(!super::super::attempt_notification_with_default(
        &restarted,
        &message_id,
        &subscription_id,
        &|_| true,
        &|_, _| {
            sent.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        },
    ));
    let state = restarted.state.lock().unwrap();
    assert!(!sent.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(
        state.msgs[&message_id].wake_attempt_count,
        MAX_WAKE_ATTEMPTS
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "armed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

/// After a daemon restart the timer replays only undelivered pending mail. A
/// pending message inside its retry budget is attempted again; a message that
/// already reached `delivered` is never re-sent. Their durable states stay
/// distinguishable so a restart cannot replay consumed work.
#[test]
fn restart_replays_only_undelivered_pending() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
    let undelivered =
        bind_message_with_type(&server, "owner", &subscription_id, "undelivered", "notify");
    let delivered =
        bind_message_with_type(&server, "owner", &subscription_id, "delivered", "notify");
    server.commit(&[Event::Delivered {
        ids: vec![delivered.clone()],
    }]);
    drop(server);

    let replayed = super::super::replay(&root).unwrap();
    let journal = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".agent-collab/server/journal.jsonl"))
        .unwrap();
    let restarted = Arc::new(Server {
        config: crate::config::Config::default(),
        root: root.clone(),
        storage_root: root.clone(),
        journal_path: root.join(".agent-collab/server/journal.jsonl"),
        host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(replayed),
        journal: Mutex::new(journal),
        appserver_candidate_check: super::super::default_appserver_candidate_check(),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
            Ok(serde_json::json!({"accepted": true}))
        }),
        appserver_thread_status: super::super::default_appserver_thread_status(),
        appserver_thread_archive: super::super::default_appserver_thread_archive(),
        mailbox_notify: tokio::sync::Notify::new(),
    });

    tick_with_idle_at(&restarted, now_ms(), &|_| true);

    let state = restarted.state.lock().unwrap();
    assert_eq!(
        state.msgs[&undelivered].state, "pending",
        "a replayed message stays pending until a receive receipt exists"
    );
    assert_eq!(
        state.msgs[&undelivered].wake_attempt_count, 1,
        "restart must replay a pending message inside its retry budget"
    );
    assert!(
        state
            .notification_delivery_accepted
            .contains_key(&undelivered),
        "the replayed pending message must record its new delivery"
    );
    assert_eq!(
        state.msgs[&delivered].state, "delivered",
        "the delivered message keeps its durable state across restart"
    );
    assert_eq!(
        state.msgs[&delivered].wake_attempt_count, 0,
        "restart must not replay a delivered message"
    );
    assert!(
        !state
            .notification_delivery_accepted
            .contains_key(&delivered),
        "a delivered message must not gain a second delivery"
    );
    drop(state);

    // A later tick inside the batch window replays neither message.
    tick_with_idle_at(&restarted, now_ms(), &|_| true);
    let state = restarted.state.lock().unwrap();
    assert_eq!(state.msgs[&undelivered].wake_attempt_count, 1);
    assert_eq!(state.msgs[&delivered].wake_attempt_count, 0);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn successful_event_notification_consumes_one_shot_subscription() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "resource-released", Some("held"), None);
    let message_id = bind_message(&server, "owner", &subscription_id);
    assert!(super::super::attempt_notification_with_default(
        &server,
        &message_id,
        &subscription_id,
        &|_| true,
        &|_, _| true,
    ));
    server.commit(&[Event::NotificationConsumed {
        subscription_id: subscription_id.clone(),
        message_id: message_id.clone(),
        consumed_ms: now_ms(),
    }]);
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].state, "pending");
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "consumed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn successful_direct_messages_reuse_subscription_without_a_burst() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
    let first_id = bind_message(&server, "owner", &subscription_id);
    assert!(super::super::attempt_notification_with_default(
        &server,
        &first_id,
        &subscription_id,
        &|_| true,
        &|_, _| true,
    ));

    let second_id = "message-owner-second".to_string();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: second_id.clone(),
                from: "peer".into(),
                to: "owner".into(),
                mtype: "notify".into(),
                subject: Some("second".into()),
                body: "SECOND_NOTICE".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: second_id.clone(),
            subscription_id: subscription_id.clone(),
        },
    ]);
    assert!(!super::super::attempt_notification_with_default(
        &server,
        &second_id,
        &subscription_id,
        &|_| true,
        &|_, _| true,
    ));
    assert_eq!(
        server.state.lock().unwrap().msgs[&second_id].wake_attempt_count,
        0
    );

    server.commit(&[Event::NotificationStatus {
        subscription_id: subscription_id.clone(),
        status: "armed".into(),
        updated_ms: now_ms() - super::super::DIRECT_MESSAGE_WAKE_COOLDOWN_MS - 1,
    }]);
    {
        let mut state = server.state.lock().unwrap();
        state.msgs.get_mut(&second_id).unwrap().created_ms = now_ms() - 120_001;
        state.msgs.get_mut(&first_id).unwrap().last_wake_attempt_ms = now_ms() - 120_001;
    }
    assert!(super::super::attempt_notification_with_default(
        &server,
        &second_id,
        &subscription_id,
        &|_| true,
        &|_, _| true,
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&first_id].state, "pending");
    assert_eq!(state.msgs[&first_id].wake_attempt_count, 1);
    assert_eq!(state.msgs[&second_id].state, "pending");
    assert_eq!(state.msgs[&second_id].wake_attempt_count, 1);
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "armed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn batch_excludes_messages_after_first_window_and_never_replays() {
    let (server, root) = test_server();
    register(&server, "owner");
    let subscription = subscribe(&server, "owner", "direct-message", None, None);
    let first = bind_message(&server, "owner", &subscription);
    let mut second = server.state.lock().unwrap().msgs[&first].clone();
    second.id = "second".into();
    second.subject = Some("new topic".into());
    second.created_ms = now_ms();
    server.commit(&[
        Event::Sent { msg: second },
        Event::WakeBound {
            message_id: "second".into(),
            subscription_id: subscription.clone(),
        },
    ]);
    let calls = std::cell::RefCell::new(Vec::new());
    assert!(super::super::attempt_notification_with_default(
        &server,
        &first,
        &subscription,
        &|_| true,
        &|_, text| {
            calls.borrow_mut().push(text.to_string());
            true
        }
    ));
    assert_eq!(calls.borrow().len(), 1);
    assert!(calls.borrow()[0].contains(&first));
    assert!(calls.borrow()[0].contains("message_ids=message-owner"));
    assert!(!calls.borrow()[0].contains("second"));
    assert!(calls.borrow()[0].contains("action_categories="));
    assert_eq!(server.state.lock().unwrap().msgs["second"].state, "pending");
    assert!(!super::super::attempt_notification_with_default(
        &server,
        &first,
        &subscription,
        &|_| true,
        &|_, _| panic!("duplicate delivery")
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_delivery_mode_bypasses_batch_window() {
    let (server, root) = test_server();
    register(&server, "owner");
    let sub = subscribe(&server, "owner", "direct-message", None, None);
    let id = bind_message(&server, "owner", &sub);
    server.commit(&[Event::DeliveryMode {
        msg_id: id.clone(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    }]);
    let calls = std::cell::Cell::new(0);
    assert!(super::super::attempt_notification_with_default(
        &server,
        &id,
        &sub,
        &|_| true,
        &|_, _| {
            calls.set(calls.get() + 1);
            true
        }
    ));
    assert_eq!(calls.get(), 1);
    assert_eq!(server.state.lock().unwrap().msgs[&id].state, "pending");
    assert_eq!(server.state.lock().unwrap().msgs[&id].wake_attempt_count, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ledger_maintenance_skips_routes_this_daemon_does_not_own() {
    // The ledger is resident control state. A host-wide route whose project
    // scope this daemon never registered cannot be classified here: the
    // global reducer rejects the record, so the tick would commit an event
    // that replay can never reduce.
    use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
    use crate::scope::ProjectScopeId;
    use crate::server::global_state::RuntimeBinding;

    let (server, root) = test_server();
    seed_ledger_owner(&server, "codex-owner");
    let foreign_scope = ProjectScopeId::new("/tmp/collab-unregistered-project".to_owned()).unwrap();
    let foreign_app = AppServerId::new("appserver-cli".to_owned()).unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let foreign = RuntimeBinding::new_with_session(
            foreign_scope.clone(),
            foreign_app.clone(),
            AgentId::new("codex-owner".to_owned()).unwrap(),
            RuntimeId::new("runtime-foreign".to_owned()).unwrap(),
            BindingId::new("binding-foreign".to_owned()).unwrap(),
            1,
            Some(SessionId::new("session-foreign".to_owned()).unwrap()),
            Some(NativeThreadId::new("thread-foreign".to_owned()).unwrap()),
        )
        .expect("foreign binding");
        state
            .global
            .set_current_thread_route(foreign)
            .expect("host-wide route");
    }

    super::tick_ledger_maintenance_at(&server, 1_000);

    let state = server.state.lock().unwrap();
    assert!(
        state.journal_poison.is_none(),
        "ledger maintenance must not commit a record the reducer rejects: {:?}",
        state.journal_poison
    );
    assert!(
        state.global.lookup_project(&foreign_scope).is_none(),
        "fixture: the foreign project scope must stay unregistered"
    );
    let classified: usize = state
        .global
        .projects
        .values()
        .map(|project| project.runtime_binding_ledger.len())
        .sum();
    assert!(
        classified > 0,
        "a route this daemon owns must still be classified"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

/// A route can be retired between the snapshot a tick reads and the commit
/// that writes it. The reducer rejects a classification this project no
/// longer holds, and a rejected commit fails the journal closed, so the
/// commit boundary asks the reducer's own predicate and drops what it
/// rejects.
#[test]
fn ledger_commit_drops_classifications_the_reducer_would_reject() {
    use crate::identity::{AgentId, AppServerId, BindingId, OperationId, RuntimeId};
    use crate::scope::ProjectScopeId;
    use crate::server::global_state::{
        LedgerScanReceipt, RuntimeBinding, RuntimeBindingLedgerRecord, RuntimeBindingLedgerState,
    };
    use crate::server::state::Event;

    let (server, root) = test_server();
    seed_ledger_owner(&server, "codex-owner");
    let owned = {
        let state = server.state.lock().unwrap();
        let project_scope =
            crate::server::GlobalState::canonical_project_scope(&server.root).unwrap();
        state
            .global
            .lookup_project(&project_scope)
            .unwrap()
            .runtime_bindings
            .values()
            .next()
            .unwrap()
            .clone()
    };
    let record = |binding: &RuntimeBinding, suffix: &str| RuntimeBindingLedgerRecord {
        project_scope: binding.project_scope.clone(),
        app_scope_id: binding.app_scope_id.clone(),
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        binding_id: binding.binding_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        state: RuntimeBindingLedgerState::Live,
        probe_state: Some(RuntimeBindingLedgerState::Live),
        reason: None,
        classified_ms: 1_000,
        operation_id: OperationId::new(format!("ledger-{suffix}")).unwrap(),
        receipt_id: format!("ledger-receipt-{suffix}"),
    };
    // A route whose scope this index never registered. The snapshot cannot
    // see it go away because it was never there.
    let retired = RuntimeBinding::new_with_session(
        ProjectScopeId::new("/tmp/collab-retired-after-snapshot".to_owned()).unwrap(),
        AppServerId::new("appserver-cli".to_owned()).unwrap(),
        AgentId::new("codex-owner".to_owned()).unwrap(),
        RuntimeId::new("runtime-retired".to_owned()).unwrap(),
        BindingId::new("binding-retired".to_owned()).unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-retired".to_owned()).unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-retired".to_owned()).unwrap()),
    )
    .unwrap();
    // Another project holds a binding with the id the record names, while
    // the record's own project does not. A host-wide binding lookup accepts
    // this record; the reducer, which looks inside the record's own
    // project, does not.
    let other_app = AppServerId::new("appserver-cli".to_owned()).unwrap();
    let cross = RuntimeBinding::new_with_session(
        ProjectScopeId::new("/tmp/collab-other-project".to_owned()).unwrap(),
        other_app.clone(),
        AgentId::new("agent-other".to_owned()).unwrap(),
        RuntimeId::new("runtime-other".to_owned()).unwrap(),
        BindingId::new("binding-cross-project".to_owned()).unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-other".to_owned()).unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-other".to_owned()).unwrap()),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::GlobalProjectRegistered {
                registration: crate::server::global_state::ProjectRegistration::new(
                    cross.project_scope.clone(),
                    other_app.clone(),
                )
                .unwrap(),
            },
            Event::GlobalRuntimeBound {
                binding: cross.clone(),
            },
        ])
        .expect("registering the other project must commit");
    let mut cross_record = record(&cross, "cross-project");
    cross_record.project_scope = owned.project_scope.clone();
    cross_record.app_scope_id = owned.app_scope_id.clone();
    // A binding id held by both projects. The host-wide lookup that the
    // reducer runs before its project check finds two candidates and fails
    // closed, so the record is unreducible even though its own project
    // holds a binding with that id.
    let duplicate_id = BindingId::new("binding-duplicate".to_owned()).unwrap();
    let duplicate_in_root = RuntimeBinding::new_with_session(
        owned.project_scope.clone(),
        owned.app_scope_id.clone(),
        AgentId::new("agent-duplicate-root".to_owned()).unwrap(),
        RuntimeId::new("runtime-duplicate-root".to_owned()).unwrap(),
        duplicate_id.clone(),
        1,
        Some(crate::identity::SessionId::new("session-duplicate-root".to_owned()).unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-duplicate-root".to_owned()).unwrap()),
    )
    .unwrap();
    let duplicate_in_other = RuntimeBinding::new_with_session(
        cross.project_scope.clone(),
        other_app,
        AgentId::new("agent-duplicate-other".to_owned()).unwrap(),
        RuntimeId::new("runtime-duplicate-other".to_owned()).unwrap(),
        duplicate_id,
        1,
        Some(crate::identity::SessionId::new("session-duplicate-other".to_owned()).unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-duplicate-other".to_owned()).unwrap()),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::GlobalRuntimeBound {
                binding: duplicate_in_root.clone(),
            },
            Event::GlobalRuntimeBound {
                binding: duplicate_in_other,
            },
        ])
        .expect("binding one id in two projects must commit");
    let duplicate_record = record(&duplicate_in_root, "duplicate");
    {
        let state = server.state.lock().unwrap();
        let global = &state.global;
        assert!(
            global
                .lookup_registration(&cross_record.project_scope, &cross_record.app_scope_id)
                .is_some()
                && global.lookup_binding(&cross_record.binding_id).is_some(),
            "fixture: a host-wide binding lookup accepts the cross-project record"
        );
        assert!(
            global
                .runtime_binding_ledger_rejection(&cross_record)
                .is_some(),
            "the reducer looks inside the record's own project and rejects it"
        );
        assert!(
            global
                .lookup_binding(&duplicate_record.binding_id)
                .is_none()
                && global
                    .lookup_project(&duplicate_record.project_scope)
                    .is_some_and(|project| project
                        .lookup_binding(&duplicate_record.binding_id)
                        .is_some()),
            "fixture: the duplicate id is ambiguous host-wide but present in its own project"
        );
        assert!(
            global
                .runtime_binding_ledger_rejection(&duplicate_record)
                .is_some(),
            "the reducer's host-wide pre-check rejects an ambiguous binding id"
        );
    }

    let mut events = vec![
        Event::GlobalRuntimeBindingLedgerClassified {
            record: record(&owned, "owned"),
        },
        Event::GlobalRuntimeBindingLedgerClassified {
            record: record(&retired, "retired"),
        },
        Event::GlobalRuntimeBindingLedgerClassified {
            record: cross_record,
        },
        Event::GlobalRuntimeBindingLedgerClassified {
            record: duplicate_record,
        },
        Event::GlobalLedgerScanReceiptRecorded {
            receipt: LedgerScanReceipt {
                scan_id: "ledger-scan-1000".to_owned(),
                scanned_ms: 1_000,
                classified: 4,
                transitioned: 4,
                unchanged: 0,
                blocked: 0,
                mailbox_messages_unchanged: true,
            },
        },
    ];

    let dropped = {
        let state = server.state.lock().unwrap();
        super::drop_unowned_ledger_classifications(&state, &mut events)
    };
    assert_eq!(
        dropped, 3,
        "every classification the reducer rejects must be dropped"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::GlobalRuntimeBindingLedgerClassified { .. }))
            .count(),
        1,
        "only the classification this project still holds may survive"
    );
    let receipt = events
        .iter()
        .find_map(|event| match event {
            Event::GlobalLedgerScanReceiptRecorded { receipt } => Some(receipt),
            _ => None,
        })
        .expect("the scan receipt must survive");
    assert_eq!(
        (receipt.classified, receipt.transitioned, receipt.blocked),
        (1, 1, 3),
        "the receipt must describe what actually commits"
    );

    // The surviving batch is exactly what the reducer accepts, so the
    // commit does not fail the journal closed.
    server
        .commit_checked(&events)
        .expect("the surviving batch must commit");
    let state = server.state.lock().unwrap();
    assert!(state.journal_poison.is_none(), "{:?}", state.journal_poison);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

include!("timers_tests_part2.rs");

const PROJECT_PATROL_SUBJECT: &str = "project-patrol";
const PROJECT_PATROL_INTERVAL_MS: i64 = 900_000;

fn project_patrol_subscription(
    server: &Server,
    trigger_ms: i64,
    expires_ms: i64,
    repeat_count: u32,
    fired_count: u32,
) -> String {
    let now = now_ms();
    let id = format!("sub-project-patrol-{now}");
    let (target, method) = subscription_target(server, "master");
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.clone(),
            worker_id: "master".into(),
            event: "master-idle".into(),
            subject: Some(PROJECT_PATROL_SUBJECT.into()),
            target,
            method,
            trigger_ms: Some(trigger_ms),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(PROJECT_PATROL_INTERVAL_MS),
            repeat_count,
            fired_count,
            expires_ms,
            status: "armed".into(),
            created_ms: now - PROJECT_PATROL_INTERVAL_MS,
            updated_ms: now,
            status_reason: None,
        },
    }]);
    id
}

fn set_master_activity(server: &Server, observed: &str, idle_since_ms: i64) {
    let mut record = server
        .state
        .lock()
        .unwrap()
        .keepalives
        .get("master")
        .cloned()
        .unwrap_or_default();
    record.observed = observed.into();
    record.idle_since_ms = idle_since_ms;
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record,
    }]);
}

fn project_patrol_bodies(server: &Server, subscription_id: &str) -> Vec<String> {
    let state = server.state.lock().unwrap();
    subscription_messages(&state, subscription_id)
        .into_iter()
        .map(|message| message.body.clone())
        .collect()
}

fn register_project_patrol_master(server: &Server) {
    let transport = crate::server::peer_tests::test_appserver_transport("thread-master");
    let app = crate::identity::AppServerId::new("tui-default").unwrap();
    let cwd = server.root.display().to_string();
    let registered = crate::server::peer_tests::seed_committed_owner(
        server,
        "master",
        "token-master",
        &transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(registered.ok, "master registration failed: {registered:?}");
    crate::server::commit_current_thread_route_for_runtime(
        server,
        server,
        "master",
        &cwd,
        Some(&app),
    )
    .expect("master native thread route must commit");
    crate::server::restore_registered_peer_default_leases(server);

    let promoted = crate::server::handle_master_promote(
        server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record: Record {
            observed: "idle".into(),
            idle_since_ms: now_ms() - 900_001,
            ..Record::default()
        },
    }]);
}

fn install_project_patrol_thread_status(server: &mut Arc<Server>, status: Arc<Mutex<String>>) {
    Arc::get_mut(server)
        .expect("unique project patrol test server")
        .appserver_thread_status = Arc::new(move |_, thread_id| {
        let observed = status.lock().unwrap().clone();
        let status = match observed.as_str() {
            "working" => serde_json::json!({"type": "active", "activeFlags": []}),
            "unknown" => return Err("fixture native thread status is unknown".into()),
            _ => serde_json::json!({"type": "idle"}),
        };
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": status,
                "canAcceptDirectInput": observed == "idle"
            }
        }))
    });
}

#[test]
fn project_patrol_wakes_with_master_tasks_old_notice_and_idle_caps() {
    let (server, root) = test_server();
    register_project_patrol_master(&server);
    let now = now_ms();
    let subscription_id = project_patrol_subscription(&server, now - 1, now + 86_400_000, 100, 0);
    working_task(&server, "master");
    let created_ms =
        server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
    let mut record = server.state.lock().unwrap().keepalives["master"].clone();
    record.idle_episode_notices = 3;
    record.idle_episode_stopped = true;
    record.idle_since_ms = created_ms - 1;
    record.idle_episode_tasks = vec![("task".into(), "working".into())];
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record,
    }]);

    let old_message_id = "old-project-patrol-notice".to_string();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: old_message_id.clone(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("master-idle:project-patrol".into()),
                body: "old accepted project patrol".into(),
                in_reply_to: None,
                created_ms: now - 60_000,
                state: "pending".into(),
                wake_attempt_count: 1,
                last_wake_attempt_ms: now - 60_000,
                retry_attempted: true,
            },
        },
        Event::WakeBound {
            message_id: old_message_id.clone(),
            subscription_id: subscription_id.clone(),
        },
        Event::NotificationDeliveryAccepted {
            message_id: old_message_id.clone(),
            accepted_ms: now - 60_000,
            evidence: Some(serde_json::json!({"accepted": true})),
        },
    ]);

    tick_with_idle_at(&server, now, &|_| false);

    let state = server.state.lock().unwrap();
    let messages = subscription_messages(&state, &subscription_id);
    assert_eq!(
        messages.len(),
        2,
        "a due standing patrol must create a new occurrence despite an old accepted notice"
    );
    let body = messages
        .iter()
        .find(|message| message.id != old_message_id)
        .expect("new project patrol notice")
        .body
        .clone();
    for phrase in [
        "current activity",
        "unclosed board ownership",
        "per-task board patrol cancellations",
        "real blockers",
        "eligible uncancelled tasks",
        "peer-owned",
        "collab notify status",
        "project_board_patrol",
        "collab board show",
        "collab ceo status",
        "CEO_NOT_CONFIGURED",
        "collab ceo reply",
        "Direct-message reception is daemon-owned",
    ] {
        assert!(body.contains(phrase), "missing {phrase}: {body}");
    }
    assert_eq!(
        state.keepalives["master"].idle_episode_notices, 3,
        "the standing schedule must not consume the finite idle-episode budget"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn project_patrol_skips_non_idle_and_advances_without_catch_up() {
    for observed in ["working", "unknown"] {
        let (mut server, root) = test_server();
        let native_status = Arc::new(Mutex::new(observed.to_string()));
        install_project_patrol_thread_status(&mut server, Arc::clone(&native_status));
        register_project_patrol_master(&server);
        let base = now_ms();
        let subscription_id =
            project_patrol_subscription(&server, base - 1, base + 86_400_000, 100, 0);
        set_master_activity(&server, observed, 0);

        tick_with_idle_at(&server, base, &|_| false);

        {
            let state = server.state.lock().unwrap();
            assert!(
                subscription_messages(&state, &subscription_id).is_empty(),
                "{observed} master must not be woken"
            );
            let subscription = &state.notification_subscriptions[&subscription_id];
            assert_eq!(
                subscription.trigger_ms,
                Some(base + PROJECT_PATROL_INTERVAL_MS),
                "{observed} skip must advance from the current instant"
            );
            assert_eq!(subscription.status, "armed", "{observed}");
            assert_eq!(
                subscription.fired_count, 0,
                "{observed} skip must not increment the successful notice count"
            );
        }

        *native_status.lock().unwrap() = "idle".into();
        set_master_activity(&server, "idle", base + 1);
        tick_with_idle_at(&server, base + 1, &|_| false);
        {
            let state = server.state.lock().unwrap();
            assert!(
                subscription_messages(&state, &subscription_id).is_empty(),
                "{observed}: becoming idle must not backfill the skipped occurrence"
            );
            assert_eq!(
                state.notification_subscriptions[&subscription_id].fired_count, 0,
                "{observed}: no skipped occurrence may count as delivered"
            );
        }

        tick_with_idle_at(&server, base + PROJECT_PATROL_INTERVAL_MS, &|_| false);
        {
            let state = server.state.lock().unwrap();
            let messages = subscription_messages(&state, &subscription_id);
            assert_eq!(messages.len(), 1, "{observed}: next interval must wake");
            assert!(messages[0].body.contains("MASTER_IDLE_WAKE"));
            assert_eq!(
                state.notification_subscriptions[&subscription_id].fired_count, 1,
                "{observed}: only the idle occurrence counts as delivered"
            );
        }
        std::fs::remove_dir_all(root).ok();
    }
}

#[test]
fn project_patrol_renews_finite_budget_and_ttl_on_same_subscription() {
    let (server, root) = test_server();
    register_project_patrol_master(&server);
    let base = now_ms();
    let subscription_id = project_patrol_subscription(&server, base - 1, base + 1_000, 2, 1);
    set_master_activity(&server, "idle", 0);

    tick_with_idle_at(&server, base, &|_| false);

    let mut at = base;
    {
        let state = server.state.lock().unwrap();
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "armed");
        assert_eq!(subscription.id, subscription_id);
        assert_eq!(subscription.fired_count, 2);
        assert!(
            subscription.repeat_count > subscription.fired_count,
            "renewed budget must leave headroom: {subscription:?}"
        );
        assert!(
            subscription.expires_ms >= base + 604_800_000,
            "renewed TTL must extend the same subscription: {subscription:?}"
        );
        assert_eq!(
            subscription.trigger_ms,
            Some(base + PROJECT_PATROL_INTERVAL_MS)
        );
    }

    for _ in 0..120 {
        at = at.saturating_add(PROJECT_PATROL_INTERVAL_MS);
        tick_with_idle_at(&server, at, &|_| false);
    }

    {
        let state = server.state.lock().unwrap();
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "armed");
        assert_eq!(subscription.id, subscription_id);
        assert!(
            subscription.fired_count >= 100,
            "the renewal must survive the old 100-occurrence boundary: {subscription:?}"
        );
        assert!(
            subscription.repeat_count > subscription.fired_count,
            "the standing patrol must retain a live occurrence budget: {subscription:?}"
        );
        assert!(
            subscription.expires_ms > at,
            "the standing patrol must retain a live TTL: {subscription:?}"
        );
    }

    server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .get_mut(&subscription_id)
        .unwrap()
        .expires_ms = at + 1_000;
    at = at.saturating_add(PROJECT_PATROL_INTERVAL_MS);
    tick_with_idle_at(&server, at, &|_| false);
    let state = server.state.lock().unwrap();
    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.status, "armed");
    assert!(
        subscription.expires_ms >= at + 604_800_000,
        "near-expiry renewal must stay on the same subscription: {subscription:?}"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn project_patrol_prompt_uses_ceo_reply_when_configured() {
    let (server, root) = test_server();
    register_project_patrol_master(&server);
    let now = now_ms();
    let subscription_id = project_patrol_subscription(&server, now - 1, now + 86_400_000, 100, 0);

    tick_with_idle_at(&server, now, &|_| false);

    let bodies = project_patrol_bodies(&server, &subscription_id);
    assert_eq!(bodies.len(), 1);
    for phrase in [
        "collab ceo status",
        "CEO_NOT_CONFIGURED",
        "If configured",
        "collab ceo reply",
        "substantive progress",
    ] {
        assert!(
            bodies[0].contains(phrase),
            "missing {phrase} without a project CEO grant: {}",
            bodies[0]
        );
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_wake_hold_suppresses_standing_project_patrol_until_expiry_or_release() {
    let (server, root) = test_server();
    register_project_patrol_master(&server);
    let base = now_ms();
    let subscription_id = project_patrol_subscription(&server, base - 1, base + 86_400_000, 100, 0);

    let held = crate::server::handle_master_wake_hold(
        &server,
        "master".into(),
        "token-master".into(),
        "external gate: release when upstream CI finishes".into(),
        60,
    );
    assert!(held.ok, "{held:?}");

    tick_with_idle_at(&server, base, &|_| false);
    {
        let state = server.state.lock().unwrap();
        assert!(
            subscription_messages(&state, &subscription_id).is_empty(),
            "an active hold must suppress the standing patrol notice"
        );
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "armed");
        assert_eq!(
            subscription.trigger_ms,
            Some(base + PROJECT_PATROL_INTERVAL_MS),
            "the held occurrence must still advance the schedule"
        );
        assert_eq!(
            subscription.fired_count, 0,
            "a suppressed notice must not count as delivered"
        );
    }

    let after_expiry = base + PROJECT_PATROL_INTERVAL_MS + 1;
    tick_with_idle_at(&server, after_expiry, &|_| false);
    {
        let state = server.state.lock().unwrap();
        let messages = subscription_messages(&state, &subscription_id);
        assert_eq!(messages.len(), 1, "an expired hold must resume the patrol");
        assert!(messages[0].body.contains("MASTER_IDLE_WAKE"));
    }

    let held = crate::server::handle_master_wake_hold(
        &server,
        "master".into(),
        "token-master".into(),
        "second external gate".into(),
        604_800,
    );
    assert!(held.ok, "{held:?}");
    let suppressed = after_expiry + PROJECT_PATROL_INTERVAL_MS + 1;
    assert!(
        suppressed
            < server
                .state
                .lock()
                .unwrap()
                .master_wake
                .wake_hold
                .as_ref()
                .unwrap()
                .until_ms,
        "fixture must tick while the repeated hold is active"
    );
    tick_with_idle_at(&server, suppressed, &|_| false);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            subscription_messages(&state, &subscription_id).len(),
            1,
            "a repeated hold must suppress the next due notice"
        );
    }

    let released =
        crate::server::handle_master_wake_release(&server, "master".into(), "token-master".into());
    assert!(released.ok, "{released:?}");
    let resumed = suppressed + PROJECT_PATROL_INTERVAL_MS + 1;
    tick_with_idle_at(&server, resumed, &|_| false);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            subscription_messages(&state, &subscription_id).len(),
            2,
            "an explicit release must resume the standing patrol"
        );
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_wake_hold_suppresses_legacy_finite_master_idle_until_release() {
    let (mut server, root) = test_server();
    // Keep the seeded idle observation stable; this test is about the hold
    // gate, not the observation harness.
    Arc::get_mut(&mut server).unwrap().config.keepalive.enabled = false;
    register_master(&server);
    let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
    let created_ms =
        server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
    let mut record = server.state.lock().unwrap().keepalives["master"].clone();
    record.idle_since_ms = created_ms + 1;
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "master".into(),
        record,
    }]);

    let held = crate::server::handle_master_wake_hold(
        &server,
        "master".into(),
        "token-master".into(),
        "external gate".into(),
        300,
    );
    assert!(held.ok, "{held:?}");

    tick_with_idle(&server, &|_| false);
    {
        let state = server.state.lock().unwrap();
        assert!(
            subscription_messages(&state, &subscription_id).is_empty(),
            "an active hold must suppress the finite master-idle notice"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            0
        );
    }

    let released =
        crate::server::handle_master_wake_release(&server, "master".into(), "token-master".into());
    assert!(released.ok, "{released:?}");
    tick_with_idle(&server, &|_| false);
    {
        let state = server.state.lock().unwrap();
        let messages = subscription_messages(&state, &subscription_id);
        assert_eq!(
            messages.len(),
            1,
            "releasing the hold must resume the finite master-idle notice"
        );
        assert!(messages[0]
            .subject
            .as_deref()
            .is_some_and(|subject| subject.starts_with("master-idle")));
    }
    std::fs::remove_dir_all(root).ok();
}
