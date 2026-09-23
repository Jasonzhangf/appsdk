use super::*;
use crate::identity::{runtime_from_registration_receipt, BindingId, RuntimeId, SessionId};
use crate::server::notification_contract::JournalError;
use crate::server::state::{default_priority, is_goal_deadline, TaskRec};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

#[test]
fn handoff_to_unregistered_recipient_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "operator".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "candidate awaits routing".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("operator"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["error_code"], "HANDOFF_TARGET_UNRESOLVED");
    assert_eq!(response.data["unresolved"]["kind"], "recipient");
    assert_eq!(response.data["unresolved"]["value"], "operator");
    assert_eq!(response.data["reason"], "recipient_not_registered");
    assert_eq!(response.data["recorded"], false);
    assert!(
        server.state.lock().unwrap().msgs.is_empty(),
        "an unresolved handoff target must not create a durable message"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_missing_worktree_path_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let missing = root.join("playground/does-not-exist").display().to_string();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {missing}"),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("does-not-exist"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["unresolved"]["kind"], "worktree");
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_registered_recipient_and_existing_path_succeeds() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let worktree = root.join("playground/handoff-live");
    std::fs::create_dir_all(&worktree).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {}", worktree.display()),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    assert!(response.data["msg_id"].is_string());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_relative_missing_worktree_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "preflight implementation lives at playground/gone-relative".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("gone-relative"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_absolute_missing_worktree_outside_playground_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let missing = "/tmp/m1-tailscale-replay-20260908T121151Z/worktree";
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation at ({missing}) is gone"),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("m1-tailscale-replay"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_unrelated_playground_path_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("unrelated path mention".into()),
        "see /Volumes/extension/code/other-project/playground/other-worktree for reference".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_dedup_resend_survives_a_path_that_disappeared_after_delivery() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let worktree = root.join("playground/handoff-dedup");
    std::fs::create_dir_all(&worktree).unwrap();
    let body = format!("preflight implementation lives at {}", worktree.display());
    let first = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body.clone(),
        None,
        "immediate".into(),
    );
    assert!(first.ok, "{first:?}");
    let msg_id = first.data["msg_id"].as_str().unwrap().to_owned();
    std::fs::remove_dir_all(&worktree).unwrap();
    let resend = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body,
        None,
        "immediate".into(),
    );
    assert_eq!(resend.data["deduplicated"], true);
    assert_eq!(resend.data["msg_id"], msg_id.as_str());
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn test_server() -> (Server, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("collab-peer-{}-{n}", std::process::id()));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server_dir.join("journal.jsonl"))
        .unwrap();
    (
        Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(State::default()),
            journal: Mutex::new(journal),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(test_appserver_transport(&candidate.thread_id))
            }),
            appserver_notification_sink: Arc::new(|_, _, _, _, _| {
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
        },
        root,
    )
}

pub(crate) fn test_appserver_transport(thread_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{thread_id}")),
        thread_id: Some(thread_id.into()),
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    }
}

pub(crate) fn test_appserver_candidate(thread_id: &str) -> crate::proto::AppServerCandidate {
    crate::proto::AppServerCandidate {
        endpoint: "unix:///tmp/collab-test-appserver.sock".into(),
        namespace: "codex_tui".into(),
        session_id: format!("session-{thread_id}"),
        thread_id: thread_id.into(),
        cwd: env!("CARGO_MANIFEST_DIR").into(),
    }
}

pub(super) fn register(server: &Server, id: &str, thread_id: &str) -> Resp {
    let thread_id = thread_id
        .strip_prefix('%')
        .map(|legacy_fixture| format!("thread-{legacy_fixture}"))
        .unwrap_or_else(|| thread_id.to_string());
    handle_register_with_app_scope(
        server,
        id.into(),
        format!("token-{id}"),
        server.root.display().to_string(),
        Some(AppServerId::new("tui-default").unwrap()),
        Some(TransportCandidates {
            appserver: Some(test_appserver_candidate(&thread_id)),
        }),
    )
}

fn promote_master(server: &Server, worker_id: &str, approval: &str) {
    let response = handle_master_promote(
        server,
        worker_id.into(),
        format!("token-{worker_id}"),
        approval.into(),
    );
    assert!(
        response.ok,
        "master promotion for {worker_id} failed: {:?}",
        response.error
    );
}

fn send_command(root: &Path, id: &str) -> crate::proto::CommandEnvelope {
    use crate::identity::{AppServerId, BindingId, CommandId, OperationId};
    use crate::proto::CommandEnvelope;
    let app = AppServerId::new("tui-default").unwrap();
    let scope = crate::scope::RouteScope::for_registered_project(app, root).unwrap();
    CommandEnvelope::new(
        CommandId::new(format!("command-{id}")).unwrap(),
        OperationId::new(format!("operation-{id}")).unwrap(),
        BindingId::new(format!("binding-{id}")).unwrap(),
        1,
        scope,
        None,
        None,
        None,
        None,
    )
}

fn authenticated_send(root: &Path, from: &str, to: &str, subject: &str) -> Req {
    Req::Send {
        from: from.into(),
        worker_id: Some(from.into()),
        token: Some(format!("token-{from}")),
        command: Some(send_command(root, from)),
        to: to.into(),
        mtype: "notify".into(),
        subject: Some(subject.into()),
        body: "body".into(),
        in_reply_to: None,
        delivery: "immediate".into(),
    }
}

#[test]
fn master_idle_subscription_is_restricted_to_the_live_master_and_supported_intervals() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "worker", "%worker");
    promote_master(&server, "master", "user-approved");

    let worker = handle_notification_subscribe(
        &server,
        "worker".into(),
        "token-worker".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(900_000),
        3,
        86_400,
    );
    assert!(!worker.ok);
    assert_eq!(
        worker.error.as_deref(),
        Some("master-idle subscription requires the live registered master")
    );

    let accepted = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(3_600_000),
        3,
        86_400,
    );
    assert!(accepted.ok, "{}", accepted.error.unwrap_or_default());

    let invalid = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(60_000),
        3,
        86_400,
    );
    assert!(!invalid.ok);
    assert_eq!(
        invalid.error.as_deref(),
        Some("master-idle interval must be exactly 900000 or 3600000 ms")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_missing_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    {
        let mut state = server.state.lock().unwrap();
        let scope = send_command(&root, "sender").scope;
        state
            .global
            .projects
            .get_mut(scope.project_scope_id.as_str())
            .unwrap()
            .runtime_bindings
            .remove("binding-sender");
    }
    let server_arc = Arc::new(server);
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "sender")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED")
            && err.contains("authoritative runtime binding is missing"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_ambiguous_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let extra_binding = {
        let state = server.state.lock().unwrap();
        let mut binding = state
            .global
            .lookup_binding_for(
                &send_command(&root, "sender").scope,
                &BindingId::new("binding-sender").unwrap(),
            )
            .unwrap()
            .clone();
        binding.binding_id = BindingId::new("binding-sender-extra").unwrap();
        binding.runtime_id = RuntimeId::new("runtime-sender-extra").unwrap();
        binding
    };
    server.commit(&[Event::GlobalRuntimeBound {
        binding: extra_binding,
    }]);
    let resp = dispatch(
        &Arc::new(server),
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "sender")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED")
            && err.contains("authoritative runtime binding is ambiguous"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_another_workers_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let resp = dispatch(
        &Arc::new(server),
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "receiver")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED") && err.contains("actor binding mismatch"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_stale_generation() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let mut command = send_command(&root, "sender");
    command.endpoint_generation = 0;
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED") && err.contains("stale endpoint generation"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_enforces_request_cooldown() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let command = send_command(&root, "sender");
    let first = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("cooldown".into()),
            body: "first".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let second = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("cooldown".into()),
            body: "second".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!second.ok);
    let err = second.error.unwrap_or_default();
    assert!(
        err.contains("request cooldown active"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_supersedes_earlier_reply() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let command = send_command(&root, "sender");
    let command_receiver = send_command(&root, "receiver");
    let req_resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("supersede".into()),
            body: "ask".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(req_resp.ok, "{}", req_resp.error.unwrap_or_default());
    let request_id = req_resp.data["msg_id"].as_str().unwrap().to_owned();
    let first = dispatch(
        &server_arc,
        Req::Send {
            from: "receiver".into(),
            worker_id: Some("receiver".into()),
            token: Some("token-receiver".into()),
            command: Some(command_receiver.clone()),
            to: "sender".into(),
            mtype: "reply".into(),
            subject: Some("supersede".into()),
            body: "first".into(),
            in_reply_to: Some(request_id.clone()),
            delivery: "immediate".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let first_id = first.data["msg_id"].as_str().unwrap().to_owned();
    let second = dispatch(
        &server_arc,
        Req::Send {
            from: "receiver".into(),
            worker_id: Some("receiver".into()),
            token: Some("token-receiver".into()),
            command: Some(command_receiver.clone()),
            to: "sender".into(),
            mtype: "reply".into(),
            subject: Some("supersede".into()),
            body: "second".into(),
            in_reply_to: Some(request_id.clone()),
            delivery: "immediate".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let state = server_arc.state.lock().unwrap();
    let first_msg = state.msgs.get(&first_id).unwrap();
    assert_eq!(first_msg.state, "superseded");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
#[test]
fn cancelling_master_idle_subscription_supersedes_pending_wake() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    server.commit(&[
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-master-idle".into(),
                worker_id: "master".into(),
                event: "master-idle".into(),
                subject: Some("master-idle".into()),
                target: "thread-master".into(),
                method: "appserver".into(),
                trigger_ms: Some(now_ms() - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(900_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: now_ms() + 86_400_000,
                status: "armed".into(),
                created_ms: now_ms() - 900_000,
                updated_ms: now_ms(),
                status_reason: None,
            },
        },
        Event::Sent {
            msg: Message {
                id: "pending-idle".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("master-idle:master-idle".into()),
                body: "MASTER_IDLE_WAKE scheduling continues".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: "pending-idle".into(),
            subscription_id: "sub-master-idle".into(),
        },
    ]);
    let cancelled = handle_notification_unsubscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "sub-master-idle".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-master-idle"].status,
        "cancelled"
    );
    assert_eq!(state.msgs["pending-idle"].state, "superseded");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deadline_subscription_requires_live_master_authority() {
    let (server, root) = test_server();
    register(&server, "worker", "%worker");
    register(&server, "master", "%master");

    let denied = handle_notification_subscribe(
        &server,
        "worker".into(),
        "token-worker".into(),
        "deadline".into(),
        Some("goal:test".into()),
        None,
        vec![now_ms() + 10_000],
        None,
        1,
        60,
    );
    assert!(!denied.ok);
    assert!(denied.error.unwrap().contains("deadline subscriptions"));

    promote_master(&server, "master", "user approved test master");
    let accepted = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:test".into()),
        None,
        vec![now_ms() + 10_000],
        None,
        1,
        60,
    );
    assert!(accepted.ok, "master should be allowed: {accepted:?}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn goal_deadline_rejects_periodic_rearm_options() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let response = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:inactive".into()),
        None,
        Vec::new(),
        Some(600_000),
        100,
        86_400,
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("goal deadline subscriptions are one-shot and require one at-ms trigger")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn goal_deadline_registration_deduplicates_same_deadline() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let trigger_ms = now_ms() + 10_000;
    let first = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-7".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let second = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-7".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    assert_eq!(second.data["deduplicated"], true);
    assert_eq!(
        second.data["subscription"]["id"],
        first.data["subscription"]["id"]
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .filter(|subscription| is_goal_deadline(subscription))
            .count(),
        1
    );

    let next_revision = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-8".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(
        next_revision.ok,
        "{}",
        next_revision.error.unwrap_or_default()
    );
    assert!(next_revision.data.get("deduplicated").is_none());
    assert_ne!(
        next_revision.data["subscription"]["id"],
        first.data["subscription"]["id"]
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .filter(|subscription| is_goal_deadline(subscription))
            .count(),
        2
    );
    std::fs::remove_dir_all(root).ok();
}

fn create_task(server: &Server, owner: &str, id: &str, feature: &str) -> Resp {
    handle_task_register(
        server,
        owner.into(),
        format!("token-{owner}"),
        id.into(),
        None,
        Some(feature.into()),
        None,
        None,
        None,
        default_priority(),
    )
}

fn initialize_main(root: &Path) {
    for args in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Collab Test"].as_slice(),
        ["commit", "--allow-empty", "-q", "-m", "main"].as_slice(),
        ["branch", "-M", "main"].as_slice(),
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

fn current_head(root: &Path) -> String {
    rev_parse(root, "HEAD")
}

/// Drive one task to the accepted state that `task integrated` requires.
fn accept_task(server: &Server, owner: &str, id: &str) {
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                server,
                owner.into(),
                format!("token-{owner}"),
                id.into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            true,
            false,
            "review passed".into(),
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().tasks[id].status, "accepted");
}

fn rev_parse(root: &Path, rev: &str) -> String {
    String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", rev])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
}

fn git_ok(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failed_journal_cannot_apply_a_keepalive_reservation() {
    let (server, root) = test_server();
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let mut state = State::default();
    let result = server.commit_locked_checked(
        &mut state,
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
    );
    assert!(result.is_err());
    assert!(state.keepalives.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_retry_returns_original_outcome_without_reapplying_events() {
    let (server, root) = test_server();
    let event = Event::KeepaliveUpdated {
        worker_id: "worker".into(),
        record: crate::server::keepalive::Record::default(),
    };
    let first = server
        .commit_command(
            "command-1",
            "operation-1",
            std::slice::from_ref(&event),
            json!({"accepted": true}),
        )
        .unwrap();
    assert!(!first.replayed);
    let second = server
        .commit_command(
            "command-1",
            "operation-1",
            &[event],
            json!({"accepted": false}),
        )
        .unwrap();
    assert!(second.replayed);
    assert_eq!(first.receipt, second.receipt);
    assert_eq!(first.operation_id, second.operation_id);
    assert_eq!(second.outcome, json!({"accepted": true}));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .contains_key("command-1"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_registration_uses_global_binding_and_host_idempotency() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let typed = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    let first = server.typed_dispatch(typed.clone()).unwrap();
    assert!(!first.replayed);
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(Path::new(cwd)).unwrap();
    let state = server.state.lock().unwrap();
    assert!(state
        .global
        .lookup_registration(
            &project_scope,
            &crate::identity::AppServerId::new("tui-default").unwrap(),
        )
        .is_some());
    assert_eq!(
        state.global.projects[project_scope.as_str()]
            .runtime_bindings
            .len(),
        1
    );
    assert!(state
        .global
        .command_receipts
        .contains_key(first.receipt.command_id.as_str()));
    drop(state);

    let replay = server.typed_dispatch(typed).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_receipt_revision_is_the_next_cas_and_stale_after_another_mutation() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();

    let first = server
        .typed_dispatch(
            server
                .typed_register_envelope("worker", "token-worker", "%worker", cwd)
                .unwrap(),
        )
        .unwrap();
    let mut second = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    second.envelope.expected_revision = Some(first.receipt.revision);
    let second = server
        .typed_dispatch(second)
        .expect("a receipt revision must be reusable as the next CAS revision");

    let mut stale = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    stale.envelope.expected_revision = Some(second.receipt.revision);
    server
        .commit_checked(&[Event::KeepaliveUpdated {
            worker_id: "other-reducer".into(),
            record: crate::server::keepalive::Record::default(),
        }])
        .unwrap();
    let journal_before_stale =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let error = server
        .typed_dispatch(stale)
        .expect_err("an intervening reducer mutation must reject the old CAS revision");
    assert!(error
        .to_string()
        .contains("compare-and-swap revision mismatch"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        journal_before_stale,
        "a stale CAS must not append a journal event"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_rebinds_survive_journal_rewrite_and_replay_with_one_version_axis() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let mut previous_revision = 0;
    for _ in 0..3 {
        let mut typed = server
            .typed_register_envelope("worker", "token-worker", "%worker", cwd)
            .unwrap();
        typed.envelope.expected_revision = Some(previous_revision);
        let outcome = server.typed_dispatch(typed).unwrap();
        previous_revision = outcome.receipt.revision;
    }

    let (version, binding_generation, receipts) = {
        let state = server.state.lock().unwrap();
        (
            (state.sequence, state.revision, state.global.version()),
            state
                .global
                .projects
                .values()
                .next()
                .unwrap()
                .runtime_bindings["binding-worker"]
                .endpoint_generation,
            state.global.command_receipts.clone(),
        )
    };
    let state = server.state.lock().unwrap();
    state.global.validate().unwrap();
    server.rewrite_journal_locked(&state).unwrap();
    drop(state);

    let replayed = super::replay(&root).unwrap();
    replayed.global.validate().unwrap();
    assert_eq!(
        (replayed.sequence, replayed.revision),
        (version.0, version.1)
    );
    assert_eq!(replayed.global.version(), version.2);
    assert_eq!(
        replayed
            .global
            .projects
            .values()
            .next()
            .unwrap()
            .runtime_bindings["binding-worker"]
            .endpoint_generation,
        binding_generation
    );
    assert_eq!(replayed.global.command_receipts, receipts);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_command_record_rewrite_and_replay_preserve_checkpoint_version() {
    let (server, root) = test_server();
    let receipt = crate::server::state::CommandReceipt {
        operation_id: "legacy-operation".into(),
        outcome: json!({"accepted": true}),
        sequence: 1,
        revision: 1,
    };
    server
        .commit_checked(&[Event::CommandRecorded {
            command_id: "legacy-command".into(),
            receipt: receipt.clone(),
        }])
        .unwrap();

    let state = server.state.lock().unwrap();
    assert_eq!((state.sequence, state.revision), (1, 1));
    server.rewrite_journal_locked(&state).unwrap();
    drop(state);

    let compacted =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(compacted.contains("\"ev\":\"CommandRecorded\""));
    assert!(!compacted.contains("\"ev\":\"CommandStarted\""));
    assert!(!compacted.contains("\"ev\":\"CommandCompleted\""));
    let replayed =
        super::replay(&root).expect("a compacted legacy CommandRecorded must replay successfully");
    assert_eq!((replayed.sequence, replayed.revision), (1, 1));
    assert_eq!(replayed.command_receipts["legacy-command"], receipt);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_a_checkpoint_that_regresses_real_history() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::Registered {
            worker: crate::server::state::WorkerRec {
                id: "checkpoint-worker".into(),
                token: "checkpoint-token".into(),
                cwd: "/tmp".into(),
                registered_ms: 1,
                transport: Some(test_appserver_transport("thread-checkpoint-worker")),
            },
        },
        Event::ReducerCheckpoint {
            sequence: 0,
            revision: 0,
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("a real checkpoint rollback must fail closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("reducer checkpoint regresses version"),
        "unexpected error: {error}"
    );
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_registration_generation_reaches_max_then_fails_before_journaling_overflow() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let mut first = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    let crate::server::state::TypedCommand::RegisterWorker { binding, .. } = &mut first.command;
    binding.endpoint_generation = u64::MAX - 1;
    first.envelope.endpoint_generation = u64::MAX - 1;
    first.envelope.command_id = crate::identity::CommandId::new("register-max-minus-one").unwrap();
    first.envelope.operation_id =
        crate::identity::OperationId::new("register-op-max-minus-one").unwrap();
    let first = server
        .typed_dispatch(first)
        .expect("MAX-1 binding generation must be accepted");
    assert!(!first.replayed);

    let max = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    assert_eq!(max.envelope.endpoint_generation, u64::MAX);
    let max = server
        .typed_dispatch(max)
        .expect("MAX binding generation must be accepted");
    assert!(!max.replayed);
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .values()
            .next()
            .unwrap()
            .runtime_bindings["binding-worker"]
            .endpoint_generation,
        u64::MAX
    );

    let journal_before_overflow =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let receipt_count_before_overflow = server.state.lock().unwrap().global.command_receipts.len();
    let error = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .expect_err("MAX binding generation must fail explicitly on increment overflow");
    assert!(
        error.contains("endpoint generation overflow"),
        "unexpected error: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        journal_before_overflow,
        "generation overflow must happen before any journal append"
    );
    assert_eq!(
        server.state.lock().unwrap().global.command_receipts.len(),
        receipt_count_before_overflow,
        "generation overflow must not masquerade as a replayed receipt"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_invalid_command_receipt_without_rewriting_the_journal() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let event = Event::CommandRecorded {
        command_id: "invalid-receipt".into(),
        receipt: crate::server::state::CommandReceipt {
            operation_id: "invalid-receipt-operation".into(),
            outcome: json!({"accepted": true}),
            sequence: 0,
            revision: 0,
        },
    };
    let body = format!("{}\n", serde_json::to_string(&event).unwrap());
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("replay must apply command receipt validation before exposing state"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("command receipt sequence"));
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_invalid_command_completion_receipt_without_rewriting_the_journal() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::CommandStarted {
            command_id: "invalid-completion".into(),
            operation_id: "invalid-completion-operation".into(),
        },
        Event::CommandCompleted {
            command_id: "invalid-completion".into(),
            operation_id: "invalid-completion-operation".into(),
            receipt: crate::server::state::CommandReceipt {
                operation_id: "invalid-completion-operation".into(),
                outcome: json!({"accepted": true}),
                sequence: 1,
                revision: 0,
            },
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("replay must validate a command completion receipt"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("command receipt revision"));
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_dispatch_rejects_stale_revision_without_appending() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let first = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    server.typed_dispatch(first).unwrap();
    let mut stale = server
        .typed_register_envelope("worker-2", "token-worker-2", "%worker-2", cwd)
        .unwrap();
    stale.envelope.command_id = crate::identity::CommandId::new("register-stale").unwrap();
    stale.envelope.operation_id = crate::identity::OperationId::new("register-stale-op").unwrap();
    stale.envelope.expected_revision = Some(0);
    let error = server.typed_dispatch(stale).unwrap_err();
    assert!(error
        .to_string()
        .contains("compare-and-swap revision mismatch"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_dispatch_rejects_wrong_principal_and_scope() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let mut wrong_principal = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    if let crate::server::state::TypedCommand::RegisterWorker { binding, .. } =
        &mut wrong_principal.command
    {
        binding.agent_id = crate::identity::AgentId::new("other-worker").unwrap();
    }
    let principal_error = server.typed_dispatch(wrong_principal).unwrap_err();
    assert!(principal_error
        .to_string()
        .contains("runtime binding agent does not match worker identity"));

    let mut wrong_scope = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    wrong_scope.envelope.scope.project_scope_id =
        crate::scope::ProjectScopeId::new("/other-project").unwrap();
    let scope_error = server.typed_dispatch(wrong_scope).unwrap_err();
    assert!(scope_error
        .to_string()
        .contains("envelope scope does not match binding route scope"));
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .trim()
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_token_rotation_rejects_dual_key_session_mismatch() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let project_scope = crate::server::global_state::GlobalState::canonical_project_scope(
        std::path::Path::new(cwd),
    )
    .unwrap();
    let app_scope = crate::identity::AppServerId::new("tui-default").unwrap();
    let shared_thread = "thread-dual-key-worker";
    let transport_first = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{shared_thread}")),
        thread_id: Some(shared_thread.into()),
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let first = server
        .typed_register_envelope_for_scope(
            "dual-key-worker",
            "old-token-dual-key-worker",
            &transport_first,
            project_scope.clone(),
            cwd,
            app_scope.clone(),
            false,
        )
        .unwrap();
    server
        .typed_dispatch(first)
        .expect("initial typed register must succeed");
    let before_journal =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let before_global = server.state.lock().unwrap().global.clone();

    let other_session = "session-dual-key-worker-other";
    let transport_second = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(other_session.into()),
        thread_id: Some(shared_thread.into()),
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let mut second = server
        .typed_register_envelope_for_scope(
            "dual-key-worker",
            "new-token-dual-key-worker",
            &transport_second,
            project_scope,
            cwd,
            app_scope,
            false,
        )
        .unwrap();
    if let crate::server::state::TypedCommand::RegisterWorker {
        binding, worker, ..
    } = &mut second.command
    {
        binding.session_id = SessionId::new(other_session).ok();
        if let Some(transport) = worker.transport.as_mut() {
            transport.session_id = Some(other_session.into());
        }
    }

    let error = server.typed_dispatch(second).expect_err(
        "a same-thread register with a different session must be rejected as a dual-key mismatch",
    );
    let error_message = error.to_string();
    assert!(
        error_message.contains("worker token does not belong to the registered runtime identity"),
        "unexpected error: {error_message}"
    );
    let after_journal =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert_eq!(
        after_journal, before_journal,
        "a rejected dual-key mismatch must not append a journal event"
    );
    assert_eq!(
        server.state.lock().unwrap().global,
        before_global,
        "a rejected dual-key mismatch must not mutate the global reducer"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn global_reducer_failure_is_explicit_and_poisoned_after_journal_append() {
    let (server, root) = test_server();
    let binding = crate::server::global_state::RuntimeBinding::new(
        crate::scope::ProjectScopeId::new("/unregistered-project").unwrap(),
        crate::identity::AppServerId::new("tui-default").unwrap(),
        crate::identity::AgentId::new("worker").unwrap(),
        crate::identity::RuntimeId::new("runtime-worker").unwrap(),
        crate::identity::BindingId::new("binding-worker").unwrap(),
        1,
        None,
    )
    .unwrap();
    let error = server
        .commit_checked(&[Event::GlobalRuntimeBound { binding }])
        .unwrap_err();
    assert!(matches!(error, JournalError::Reducer(_)));
    assert!(server.state.lock().unwrap().journal_poison.is_some());
    assert_eq!(server.state.lock().unwrap().global.projects.len(), 0);
    let journal = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(journal.contains("GlobalRuntimeBound"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_start_append_and_sync_failures_fail_closed_before_business_apply() {
    for fault in [
        CommandJournalFault::StartAppend,
        CommandJournalFault::StartSync,
    ] {
        let (server, root) = test_server();
        inject_command_journal_fault(fault);
        let result = server.commit_command(
            "command-start-fault",
            "operation-start-fault",
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": true}),
        );
        assert!(result.is_err());
        assert!(server.state.lock().unwrap().keepalives.is_empty());
        let replay = super::replay(&root);
        match fault {
            CommandJournalFault::StartAppend => {
                assert!(replay.is_ok(), "failed first append must leave no command");
            }
            CommandJournalFault::StartSync => {
                assert!(
                    replay.is_err(),
                    "sync failure after start append must poison replay"
                );
            }
            _ => unreachable!(),
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn command_completion_append_failure_leaves_incomplete_replay() {
    let (server, root) = test_server();
    inject_command_journal_fault(CommandJournalFault::CompletionAppend);
    let result = server.commit_command(
        "command-completion-append",
        "operation-completion-append",
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
        json!({"accepted": true}),
    );
    assert!(result.is_err());
    assert!(server.state.lock().unwrap().keepalives.is_empty());
    assert!(super::replay(&root).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_completion_sync_failure_is_explicit_and_replayable_if_marker_was_written() {
    let (server, root) = test_server();
    inject_command_journal_fault(CommandJournalFault::CompletionSync);
    let result = server.commit_command(
        "command-completion-sync",
        "operation-completion-sync",
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
        json!({"accepted": true}),
    );
    assert!(result.is_err());
    assert!(server.state.lock().unwrap().keepalives.is_empty());
    let replayed = super::replay(&root).expect("written completion marker must replay");
    assert!(replayed.keepalives.contains_key("worker"));
    assert_eq!(
        replayed.command_receipts["command-completion-sync"].operation_id,
        "operation-completion-sync"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn subagent_record(id: &str, status: &str, peer: &str) -> crate::subagent::Record {
    crate::subagent::Record {
        id: id.into(),
        parent: "parent".into(),
        peer: peer.into(),
        status: status.into(),
        thread_id: Some(format!("thread-{peer}")),
        profile: None,
        created_ms: now_ms(),
        ready_deadline_ms: now_ms() + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
    }
}

fn subagent_req(command: crate::subagent::Action) -> Req {
    Req::Subagent {
        worker_id: "parent".into(),
        token: "token-parent".into(),
        command,
        launch_env: Default::default(),
    }
}

#[test]
fn missing_subagent_retires_without_snapshot_when_responsibilities_are_resolved() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("missing-retire", "idle", "child"),
    }]);
    let archive_calls = Arc::new(AtomicU32::new(0));
    let archive_calls_for_stub = Arc::clone(&archive_calls);
    server.appserver_candidate_check = Arc::new(|_| {
        Err(
            "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
                .into(),
        )
    });
    server.appserver_thread_archive = Arc::new(move |_, _| {
        archive_calls_for_stub.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"archived": true}))
    });
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Missing,
        "route-unavailable identity must classify as definitive Missing"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "missing-retire".into(),
        }),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["subagent"]["status"], "closed");
    assert!(response.data["snapshot_captured_ms"].is_null());
    assert_eq!(archive_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        server.state.lock().unwrap().subagents["missing-retire"].status,
        "closed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cold_subagent_still_requires_a_snapshot_before_close() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("cold-retire", "idle", "child"),
    }]);
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "notLoaded"},
                "canAcceptDirectInput": false
            }
        }))
    });
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Cold,
        "notLoaded thread must stay Cold, not Missing"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "cold-retire".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    assert_eq!(
        server.state.lock().unwrap().subagents["cold-retire"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_subagent_with_unresolved_responsibility_still_requires_a_snapshot() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("missing-busy", "idle", "child"),
        },
        Event::TaskCreated {
            task: TaskRec {
                id: "task-missing-busy".into(),
                owner: "child".into(),
                created_by: "parent".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                status: "working".into(),
                next_step: None,
                wait: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        },
    ]);
    server.appserver_candidate_check = Arc::new(|_| {
        Err(
            "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
                .into(),
        )
    });
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(worker_presence(&server, &child), IdentityPresence::Missing);
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "missing-busy".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    let state = server.state.lock().unwrap();
    assert_eq!(state.subagents["missing-busy"].status, "idle");
    assert_eq!(state.tasks["task-missing-busy"].status, "working");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_subagent_still_requires_a_snapshot_before_close() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("unknown-retire", "idle", "child"),
    }]);
    server.appserver_candidate_check =
        Arc::new(|_| Err("ADAPTER_TIMEOUT: candidate self-check timed out".into()));
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Unknown,
        "inconclusive probe must stay Unknown"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "unknown-retire".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    assert_eq!(
        server.state.lock().unwrap().subagents["unknown-retire"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_start_journal_failure_does_not_launch_or_write_success() {
    use crate::server::SubagentJournalFault::{StartAppend, StartSync};

    for fault in [StartAppend, StartSync] {
        let (mut server, root) = test_server();
        register(&server, "parent", "%parent");
        promote_master(&server, "parent", "start journal regression");
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            subagent_req(crate::subagent::Action::Start {
                id: Some("start-journal-fault".into()),
                runtime: Some("codex".into()),
            }),
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        let state = server.state.lock().unwrap();
        assert!(state.subagents.is_empty());
        assert!(state.journal_poison.is_some());
        drop(state);
        assert!(!root
            .join(".agent-collab/server/launch-start-journal-fault.json")
            .exists());
        let replayed = replay(&root).unwrap();
        match fault {
            StartAppend => assert!(replayed.subagents.is_empty()),
            StartSync => {
                assert!(replayed
                    .subagents
                    .values()
                    .all(|record| record.status != "starting" && record.status != "failed"));
            }
            _ => unreachable!(),
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_working_journal_failure_preserves_task_and_subagent_and_poison_rejects_mutation() {
    use crate::server::SubagentJournalFault::{WorkingAppend, WorkingSync};

    for fault in [WorkingAppend, WorkingSync] {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        let mut record = subagent_record("working-journal-fault", "assigned", "child");
        record.last_message = Some("working-journal-fault".into());
        server.commit(&[
            Event::SubagentUpdated { subagent: record },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-working-journal-fault".into(),
                    owner: "child".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "assigned".into(),
                    next_step: None,
                    wait: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            Req::Subagent {
                worker_id: "child".into(),
                token: "token-child".into(),
                command: crate::subagent::Action::Working {
                    id: "working-journal-fault".into(),
                },
                launch_env: Default::default(),
            },
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["working-journal-fault"].status, "assigned");
        assert_eq!(state.tasks["task-working-journal-fault"].status, "assigned");
        assert!(state.journal_poison.is_some());
        drop(state);
        let rejected = dispatch(
            &server,
            Req::Subagent {
                worker_id: "child".into(),
                token: "token-child".into(),
                command: crate::subagent::Action::Working {
                    id: "working-journal-fault".into(),
                },
                launch_env: Default::default(),
            },
        );
        assert!(!rejected.ok);
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["working-journal-fault"].status, "assigned");
        assert_eq!(state.tasks["task-working-journal-fault"].status, "assigned");
        drop(state);
        assert!(replay(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_close_first_journal_failure_does_not_remove_external_manifest() {
    use crate::server::SubagentJournalFault::{CloseFirstAppend, CloseFirstSync};

    for fault in [CloseFirstAppend, CloseFirstSync] {
        let (mut server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        server.commit(&[Event::SubagentUpdated {
            subagent: subagent_record("close-first-fault", "idle", "child"),
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "close-first-fault".into(),
            thread_id: "thread-child".into(),
            captured_ms: now_ms(),
        }]);
        let manifest = root.join(".agent-collab/server/launch-close-first-fault.json");
        std::fs::write(&manifest, b"test-only manifest").unwrap();
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            subagent_req(crate::subagent::Action::Close {
                id: "close-first-fault".into(),
            }),
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["close-first-fault"].status, "idle");
        assert!(state.journal_poison.is_some());
        drop(state);
        assert!(manifest.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_close_final_journal_failure_reports_unknown_and_stays_open() {
    use crate::server::SubagentJournalFault::{CloseFinalAppend, CloseFinalSync};

    for fault in [CloseFinalAppend, CloseFinalSync] {
        let (mut server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        server.commit(&[Event::SubagentUpdated {
            subagent: subagent_record("close-final-fault", "idle", "child"),
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "close-final-fault".into(),
            thread_id: "thread-child".into(),
            captured_ms: now_ms(),
        }]);
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            subagent_req(crate::subagent::Action::Close {
                id: "close-final-fault".into(),
            }),
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        assert!(result.error.unwrap().contains("outcome unknown"));
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["close-final-fault"].status, "closing");
        assert!(state.journal_poison.is_some());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn replayed_command_is_idempotent_and_operation_conflict_fails_closed() {
    let (server, root) = test_server();
    server
        .commit_command(
            "command-replay",
            "operation-replay",
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": true}),
        )
        .unwrap();
    let restarted = {
        let journal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap();
        Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(super::replay(&root).unwrap()),
            journal: Mutex::new(journal),
            appserver_candidate_check: crate::server::default_appserver_candidate_check(),
            appserver_notification_sink: crate::server::default_appserver_notification_sink(),
            appserver_thread_status: crate::server::default_appserver_thread_status(),
            appserver_thread_archive: crate::server::default_appserver_thread_archive(),
            mailbox_notify: tokio::sync::Notify::new(),
        }
    };
    let retry = restarted
        .commit_command(
            "command-replay",
            "operation-replay",
            &[Event::KeepaliveUpdated {
                worker_id: "duplicate".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": false}),
        )
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.outcome, json!({"accepted": true}));
    let conflict = restarted.commit_command(
        "command-other",
        "operation-replay",
        &[],
        json!({"accepted": true}),
    );
    assert!(matches!(conflict, Err(JournalError::InvalidCommand(_))));
    assert!(!restarted
        .state
        .lock()
        .unwrap()
        .keepalives
        .contains_key("duplicate"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_business_event_persisted_without_command_completion() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::CommandStarted {
            command_id: "incomplete".into(),
            operation_id: "operation-incomplete".into(),
        },
        Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&journal, format!("{body}\n")).unwrap();
    let error = match super::replay(&root) {
        Ok(_) => panic!("incomplete command replay must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("incomplete"), "unexpected error: {error}");
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_command_record_without_outcome_is_rejected() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(
        &journal,
        r#"{"ev":"CommandRecorded","command_id":"legacy","receipt":{"operation_id":"op"}}
"#,
    )
    .unwrap();
    let error = match super::replay(&root) {
        Ok(_) => panic!("legacy command record without outcome must be rejected"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("outcome"), "unexpected error: {error}");
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn try_commit_reports_journal_failure_without_applying_state() {
    let (server, root) = test_server();
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let mut state = State::default();
    let error = server
        .try_commit_locked(
            &mut state,
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
        )
        .expect_err("read-only journal must be reported to the caller");
    assert!(error.contains("journal append"));
    assert!(state.keepalives.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_close_does_not_report_success_when_transition_cannot_persist() {
    use crate::subagent::{Action, Record};

    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed-close".into(),
            parent: "parent".into(),
            peer: "child".into(),
            status: "idle".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now_ms(),
            ready_deadline_ms: 0,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
        },
    }]);
    server.commit(&[Event::SubagentSnapshotCaptured {
        subagent_id: "managed-close".into(),
        thread_id: "thread-child".into(),
        captured_ms: now_ms(),
    }]);
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();

    let response = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Close {
            id: "managed-close".into(),
        },
    );
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("journal append"));
    assert_eq!(
        server.state.lock().unwrap().subagents["managed-close"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_log_never_copies_launch_credentials() {
    let request = Req::Subagent {
        worker_id: "parent".into(),
        token: "token".into(),
        command: crate::subagent::Action::Start {
            id: None,
            runtime: None,
        },
        launch_env: std::collections::BTreeMap::from([("SECRET".into(), "do-not-log".into())]),
    };
    let log = request_activity(&request, &Resp::data(json!({})));
    assert!(log["request"].get("launch_env").is_none());
    assert!(!log.to_string().contains("do-not-log"));
}

#[test]
fn managed_subagent_is_authenticated_persistent_and_replayable() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    register(&server, "other", "%other");
    let record = Record {
        id: "managed".into(),
        parent: "parent".into(),
        peer: "child".into(),
        status: "starting".into(),
        thread_id: Some("thread-child".into()),
        profile: None,
        created_ms: now_ms(),
        ready_deadline_ms: now_ms() + 90000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: None,
    };
    let event = Event::SubagentUpdated { subagent: record };
    let encoded = serde_json::to_string(&event).unwrap();
    let mut replay = State::default();
    replay.apply(&serde_json::from_str(&encoded).unwrap());
    assert_eq!(replay.subagents["managed"].status, "starting");
    server.commit(&[event]);
    let child_ctx = handle_context(&server, "child".into(), "token-child".into());
    assert_eq!(child_ctx.data["authority"]["must_obey_master"], true);
    assert_eq!(
        child_ctx.data["authority"]["may_decline_master_invite"],
        false
    );
    let parent_ctx = handle_context(&server, "parent".into(), "token-parent".into());
    assert_eq!(
        parent_ctx.data["authority"]["may_decline_master_invite"],
        true
    );
    assert!(
        !crate::subagent::handle(
            &server,
            "other",
            "token-other",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(!crate::subagent::handle(&server, "parent", "wrong", Action::List).ok);
    assert!(
        !crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    let count = server.state.lock().unwrap().msgs.len();
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().msgs.len(), count);
    let no_assigned = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(!no_assigned.ok);
    assert_eq!(
        no_assigned.error.as_deref(),
        Some("no assigned task to accept")
    );
    let unknown = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "missing-managed".into(),
        },
    );
    assert!(!unknown.ok);
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Send {
                id: "managed".into(),
                subject: "test".into(),
                body: "task".into()
            }
        )
        .ok
    );
    let message_id = server.state.lock().unwrap().subagents["managed"]
        .last_message
        .clone()
        .unwrap();
    assert_eq!(
        server.state.lock().unwrap().tasks[&format!("task-{message_id}")].status,
        "assigned"
    );
    let server = Arc::new(server);
    let message = dispatch(&server, Req::MsgStatus { msg_id: message_id });
    assert_eq!(message.data["body"], "task");
    assert!(
        !crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Send {
                id: "managed".into(),
                subject: "test".into(),
                body: "task".into()
            }
        )
        .ok
    );
    let still_working = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "must-wait".into(),
            body: "active task still owns the child".into(),
        },
    );
    assert!(!still_working.ok);
    assert_eq!(
        still_working.error.as_deref(),
        Some("subagent is not idle; query status instead of resending")
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "managed".into()
            }
        )
        .ok
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .tasks
            .values()
            .next()
            .unwrap()
            .status,
        "working"
    );
    let observed = dispatch(
        &server,
        Req::SubagentObserve {
            id: Some("managed".into()),
            snapshot_lines: None,
        },
    );
    assert!(observed.ok);
    assert_eq!(observed.data["notification_channel"], "none");
    assert!(observed.data.get("screen_tail").is_none());
    assert!(observed.data["tasks"].as_array().unwrap().len() == 1);
    let ready = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Ready {
            id: "managed".into(),
        },
    );
    assert!(!ready.ok);
    assert!(ready
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with("APPSERVER_NOTIFICATION_REJECTED:"));
    assert_eq!(
        server.state.lock().unwrap().subagents["managed"].status,
        "idle"
    );
    server.commit(&[Event::SubagentSnapshotCaptured {
        subagent_id: "managed".into(),
        thread_id: "thread-child".into(),
        captured_ms: now_ms(),
    }]);
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_send_binds_the_selected_child_when_multiple_children_are_assigned() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child-a", "%child-a");
    register(&server, "child-b", "%child-b");
    let now = now_ms();
    server.commit(&[
        Event::SubagentUpdated {
            subagent: Record {
                id: "managed-a".into(),
                parent: "parent".into(),
                peer: "child-a".into(),
                status: "idle".into(),
                thread_id: Some("thread-child-a".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
            },
        },
        Event::SubagentUpdated {
            subagent: Record {
                id: "managed-b".into(),
                parent: "parent".into(),
                peer: "child-b".into(),
                status: "idle".into(),
                thread_id: Some("thread-child-b".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
            },
        },
    ]);

    let wrong_binding = handle_send_with_task(
        &server,
        "parent".into(),
        "child-a".into(),
        "notify".into(),
        Some("wrong-binding".into()),
        "must fail".into(),
        None,
        "immediate".into(),
        true,
        Some("managed-b"),
    );
    assert!(!wrong_binding.ok);
    assert_eq!(
        wrong_binding.error.as_deref(),
        Some("managed subagent owner mismatch")
    );
    let unknown_binding = handle_send_with_task(
        &server,
        "parent".into(),
        "child-a".into(),
        "notify".into(),
        Some("unknown-binding".into()),
        "must fail".into(),
        None,
        "immediate".into(),
        true,
        Some("missing"),
    );
    assert!(!unknown_binding.ok);
    assert_eq!(
        unknown_binding.error.as_deref(),
        Some("unknown managed subagent missing")
    );

    let first = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed-a".into(),
            subject: "same-task".into(),
            body: "same body".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let first_message = server.state.lock().unwrap().subagents["managed-a"]
        .last_message
        .clone()
        .unwrap();
    let second = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed-b".into(),
            subject: "same-task".into(),
            body: "same body".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let second_message = server.state.lock().unwrap().subagents["managed-b"]
        .last_message
        .clone()
        .unwrap();

    let journal: Vec<serde_json::Value> =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let first_sent_index = journal
        .iter()
        .position(|event| event["ev"] == "Sent" && event["msg"]["id"] == first_message)
        .unwrap();
    let second_sent_index = journal
        .iter()
        .position(|event| event["ev"] == "Sent" && event["msg"]["id"] == second_message)
        .unwrap();
    assert!(!journal[first_sent_index..second_sent_index]
        .iter()
        .any(|event| event["ev"] == "SubagentUpdated" && event["subagent"]["id"] == "managed-b"),
        "the selected child must be bound only in the message commit");

    let state = server.state.lock().unwrap();
    let first_message = state.subagents["managed-a"].last_message.clone().unwrap();
    let second_message = state.subagents["managed-b"].last_message.clone().unwrap();
    assert_ne!(first_message, second_message);
    assert_eq!(state.tasks.len(), 2);
    assert_eq!(
        state.tasks[&format!("task-{first_message}")].owner,
        "child-a"
    );
    assert_eq!(
        state.tasks[&format!("task-{second_message}")].owner,
        "child-b"
    );
    drop(state);

    assert!(
        crate::subagent::handle(
            &server,
            "child-a",
            "token-child-a",
            Action::Working {
                id: "managed-a".into(),
            },
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child-b",
            "token-child-b",
            Action::Working {
                id: "managed-b".into(),
            },
        )
        .ok
    );
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.subagents["managed-a"].status, "working");
    assert_eq!(replayed.subagents["managed-b"].status, "working");
    assert_eq!(
        replayed.tasks[&format!("task-{first_message}")].status,
        "working"
    );
    assert_eq!(
        replayed.tasks[&format!("task-{second_message}")].status,
        "working"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_send_reclaims_working_child_without_an_owned_task() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    let now = now_ms();
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed".into(),
            parent: "parent".into(),
            peer: "child".into(),
            // A keepalive runtime observation can leave this stale after the
            // child has reported ready and consumed an empty recv cycle.
            status: "working".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now,
            ready_deadline_ms: now + 90_000,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
        },
    }]);

    let assigned = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "next-task".into(),
            body: "dispatch after ready and recv".into(),
        },
    );
    assert!(assigned.ok, "{}", assigned.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.subagents["managed"].status, "assigned");
    assert_eq!(state.tasks.len(), 1);
    assert_eq!(state.tasks.values().next().unwrap().owner, "child");
    drop(state);
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "managed".into(),
            },
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into(),
            },
        )
        .ok
    );
    let before = {
        let state = server.state.lock().unwrap();
        (
            state.tasks.len(),
            state.subagents["managed"].last_message.clone(),
            state.subagents["managed"].status.clone(),
        )
    };
    let idle_with_active_task = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "must-wait-idle".into(),
            body: "idle status still has an active task".into(),
        },
    );
    assert!(!idle_with_active_task.ok);
    assert_eq!(
        idle_with_active_task.error.as_deref(),
        Some("managed subagent already has an active task")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks.len(), before.0);
    assert_eq!(state.subagents["managed"].last_message, before.1);
    assert_eq!(state.subagents["managed"].status, before.2);
    drop(state);
    let direct_idle_with_active_task = handle_send_with_task(
        &server,
        "parent".into(),
        "child".into(),
        "notify".into(),
        Some("direct-must-wait".into()),
        "direct active task still owns the child".into(),
        None,
        "immediate".into(),
        true,
        Some("managed"),
    );
    assert!(!direct_idle_with_active_task.ok);
    assert_eq!(
        direct_idle_with_active_task.error.as_deref(),
        Some("managed subagent already has an active task")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_working_requires_existing_owned_assigned_task() {
    use crate::subagent::{Action, Record};

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[Event::SubagentUpdated {
            subagent: Record {
                id: "missing-task".into(),
                parent: "parent".into(),
                peer: "child".into(),
                status: "assigned".into(),
                thread_id: Some("thread-child".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: Some("missing".into()),
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
            },
        }]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "missing-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(
            result.error.as_deref(),
            Some("assigned task task-missing not found")
        );
        assert_eq!(
            server.state.lock().unwrap().subagents["missing-task"].status,
            "assigned"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: Record {
                    id: "terminal-task".into(),
                    parent: "parent".into(),
                    peer: "child".into(),
                    status: "assigned".into(),
                    thread_id: Some("thread-child".into()),
                    profile: None,
                    created_ms: now,
                    ready_deadline_ms: now + 90_000,
                    last_message: Some("terminal".into()),
                    error: None,
                    probe_failures: Vec::new(),
                    runtime: None,
                },
            },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-terminal".into(),
                    owner: "child".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "closed".into(),
                    next_step: None,
                    wait: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "terminal-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(
            result.error.as_deref(),
            Some("assigned task task-terminal is not in assigned state (status=closed)")
        );
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["terminal-task"].status, "assigned");
        assert_eq!(state.tasks["task-terminal"].status, "closed");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: Record {
                    id: "owner-task".into(),
                    parent: "parent".into(),
                    peer: "child".into(),
                    status: "assigned".into(),
                    thread_id: Some("thread-child".into()),
                    profile: None,
                    created_ms: now,
                    ready_deadline_ms: now + 90_000,
                    last_message: Some("owner".into()),
                    error: None,
                    probe_failures: Vec::new(),
                    runtime: None,
                },
            },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-owner".into(),
                    owner: "other".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "assigned".into(),
                    next_step: None,
                    wait: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "owner-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("task owner mismatch"));
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["owner-task"].status, "assigned");
        assert_eq!(state.tasks["task-owner"].owner, "other");
        assert_eq!(state.tasks["task-owner"].status, "assigned");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn managed_subagent_working_accepts_assignment_after_probe_race_and_is_idempotent() {
    use crate::subagent::{Action, Record};

    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    let now = now_ms();
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed".into(),
            parent: "parent".into(),
            peer: "child".into(),
            status: "idle".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now,
            ready_deadline_ms: now + 90_000,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
        },
    }]);

    let sent = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "race-task".into(),
            body: "claim the task after the probe observes working".into(),
        },
    );
    assert!(sent.ok, "{}", sent.error.unwrap_or_default());
    let message_id = server.state.lock().unwrap().subagents["managed"]
        .last_message
        .clone()
        .unwrap();
    let task_id = format!("task-{message_id}");
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["managed"].status, "assigned");
        assert_eq!(state.tasks[&task_id].status, "assigned");
        assert_eq!(state.tasks.len(), 1);
    }

    // Model the keepalive runtime probe winning the race: it observes the child
    // as working and persists that managed status while the task is assigned.
    let mut probed = server.state.lock().unwrap().subagents["managed"].clone();
    probed.status = "working".into();
    server.commit(&[Event::SubagentUpdated { subagent: probed }]);

    let first = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["managed"].status, "working");
        assert_eq!(state.tasks[&task_id].status, "working");
        assert_eq!(state.tasks[&task_id].owner, "child");
        assert_eq!(state.tasks.len(), 1);
    }

    // Once both durable records are working, a repeated claim is a harmless
    // replay of the same transition and must not append or create anything.
    let journal_after_first =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count();
    let second = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let journal_after_second =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count();
    assert_eq!(journal_after_second, journal_after_first);
    assert_eq!(server.state.lock().unwrap().tasks.len(), 1);

    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.subagents["managed"].status, "working");
    assert_eq!(replayed.tasks[&task_id].status, "working");
    assert_eq!(replayed.tasks[&task_id].owner, "child");
    assert_eq!(replayed.tasks.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_is_master_only_audited_and_refuses_to_strand_tasks() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%a");
    register(&server, "peer-b", "%b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );

    // A non-master peer cannot retire another peer.
    let outsider = super::handle_worker_close(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "peer-a".into(),
        "trying to close the master".into(),
    );
    assert!(!outsider.ok);

    // The audit reason is mandatory.
    let no_reason = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "   ".into(),
    );
    assert!(!no_reason.ok);
    assert!(no_reason.error.unwrap().contains("--reason"));

    // Master may not close itself into a headless project.
    let self_close = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-a".into(),
        "self".into(),
    );
    assert!(!self_close.ok);
    assert!(self_close.error.unwrap().contains("cannot close itself"));

    let missing_snapshot = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!missing_snapshot.ok);
    assert!(missing_snapshot
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));
    server.commit(&[Event::SubagentUpdated {
        subagent: crate::subagent::Record {
            thread_id: Some("thread-b".into()),
            ..subagent_record("managed-peer-b", "idle", "peer-b")
        },
    }]);
    let mismatched_thread = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!mismatched_thread.ok);
    assert!(mismatched_thread
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));

    // A worker holding live work keeps its registration; the task lifecycle
    // has to be resolved first or the worktree is stranded.
    server.commit(&[Event::TaskCreated {
        task: crate::server::state::TaskRec {
            id: "task-b".into(),
            owner: "peer-b".into(),
            created_by: "peer-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let owns_work = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!owns_work.ok);
    assert!(owns_work.error.unwrap().contains("task-b"));

    server.commit(&[Event::TaskUpdated {
        task: crate::server::state::TaskRec {
            id: "task-b".into(),
            owner: "peer-b".into(),
            created_by: "peer-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "closed".into(),
            next_step: None,
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "peer-b".into(),
        thread_id: "thread-b".into(),
        captured_ms: now_ms(),
    }]);
    let closed = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport dead after snapshot".into(),
    );
    assert!(closed.ok, "{}", closed.error.clone().unwrap_or_default());
    assert_eq!(closed.data["closed"], "peer-b");
    assert_eq!(closed.data["reason"], "transport dead after snapshot");
    assert!(closed.data["snapshot_captured_ms"].is_i64());
    assert!(closed.data.get("archived_thread").is_none());

    let repeated = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "a different reason must not rewrite the receipt".into(),
    );
    assert!(
        repeated.ok,
        "{}",
        repeated.error.clone().unwrap_or_default()
    );
    assert_eq!(repeated.data["closed"], "peer-b");
    assert_eq!(repeated.data["closed_by"], "peer-a");
    assert_eq!(repeated.data["reason"], "transport dead after snapshot");
    assert_eq!(repeated.data["reused"], true);

    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key("peer-b"));
    assert!(!state.keepalives.contains_key("peer-b"));
    assert_eq!(
        state.worker_closures["peer-b"].snapshot_captured_ms,
        Some(closed.data["snapshot_captured_ms"].as_i64().unwrap())
    );
    drop(state);

    let replayed = super::replay(&root).unwrap();
    assert_eq!(replayed.worker_snapshots["peer-b"].thread_id, "thread-b");
    assert_eq!(
        replayed.worker_closures["peer-b"].reason,
        "transport dead after snapshot"
    );
    assert!(replayed.worker_closures["peer-b"]
        .snapshot_captured_ms
        .is_some());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_refuses_every_unfinished_task_status() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "peer-b", "%peer-b");
    assert!(
        super::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user approved master".into(),
        )
        .ok
    );
    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "peer-b".into(),
        thread_id: "thread-peer-b".into(),
        captured_ms: now_ms(),
    }]);

    // A task that raised no keepalive nudge still owns a worktree, branch, and
    // delivery obligation, so worker retirement must refuse it.
    for status in ["blocked", "waiting", "delivered", "accepted", "merged"] {
        server.commit(&[Event::TaskCreated {
            task: TaskRec {
                id: format!("task-{status}"),
                owner: "peer-b".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: status.into(),
                next_step: None,
                wait: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        let refused = super::handle_worker_close(
            &server,
            "master".into(),
            "token-master".into(),
            "peer-b".into(),
            format!("retire while {status}"),
        );
        assert!(
            !refused.ok,
            "worker close must refuse an unfinished {status} task: {:?}",
            refused.data
        );
        assert!(
            refused
                .error
                .unwrap_or_default()
                .contains(&format!("task-{status}")),
            "the refusal must name the blocking task for {status}"
        );
        server.commit(&[Event::TaskUpdated {
            task: TaskRec {
                id: format!("task-{status}"),
                owner: "peer-b".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
    }

    let closed = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "peer-b".into(),
        "no unfinished task remains".into(),
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn last_task_close_cancels_default_lease_and_preserves_pending_unread_payload() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    let sent = handle_send(
        &server,
        "sender".into(),
        "owner".into(),
        "notify".into(),
        Some("unread while busy".into()),
        "payload that must survive the close".into(),
        None,
        "immediate".into(),
    );
    assert!(sent.ok, "{}", sent.error.clone().unwrap_or_default());
    let msg_id = sent.data["msg_id"].as_str().unwrap().to_string();

    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate verified".into()),
            Some("/tmp/lifecycle-r1-worktree".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(
        closed.data["notification"],
        "subscribed resource waiters only"
    );

    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions["sub-default-direct-message-owner"].status,
            "cancelled",
            "the last task close must end the default direct-message lease"
        );
        assert_eq!(
            state.msgs[&msg_id].state, "pending",
            "automatic lease cancellation must not supersede unread mailbox bytes"
        );
        assert!(state.inbox_of("owner").iter().any(|m| m.id == msg_id));
    }

    // Durable replay must keep both facts: lease cancelled, payload still owed.
    let replayed = super::replay(&root).unwrap();
    assert_eq!(
        replayed.notification_subscriptions["sub-default-direct-message-owner"].status,
        "cancelled"
    );
    assert_eq!(replayed.msgs[&msg_id].state, "pending");

    // An explicit recv after the close still returns the unread payload.
    let recv = poll_messages_with_context(
        &server,
        "owner",
        None,
        None,
        Some("recv-last-close-unread-preserved"),
    )
    .expect("the preserved unread payload must still be delivered");
    assert!(recv.ok, "{:?}", recv.error);
    assert_eq!(recv.data["count"], 1);
    assert_eq!(recv.data["messages"][0]["id"], msg_id.as_str());
    assert_eq!(
        recv.data["messages"][0]["body"],
        "payload that must survive the close"
    );

    // Explicit rearm after the automatic cancellation still works.
    let rearmed = handle_notification_subscribe(
        &server,
        "owner".into(),
        "token-owner".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3_600,
    );
    assert!(rearmed.ok, "{}", rearmed.error.unwrap_or_default());
    assert_eq!(rearmed.data["subscription"]["status"], "armed");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ordinary_worker_requires_its_own_snapshot_and_closes_idempotently() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "ordinary", "%ordinary");
    assert!(
        super::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user approved master".into(),
        )
        .ok
    );

    let missing = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "confirmed offline".into(),
    );
    assert!(!missing.ok);
    assert!(missing
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));

    let outsider_snapshot = super::handle_worker_snapshot(
        &server,
        "ordinary".into(),
        "token-ordinary".into(),
        "master".into(),
        40,
    );
    assert!(!outsider_snapshot.ok);
    assert!(outsider_snapshot
        .error
        .unwrap()
        .contains("master authority required"));

    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "ordinary".into(),
        thread_id: "thread-ordinary".into(),
        captured_ms: now_ms(),
    }]);
    let closed = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "confirmed offline after snapshot".into(),
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(closed.data["snapshot_captured_ms"].is_i64());

    let repeated = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "ignored duplicate".into(),
    );
    assert!(repeated.ok, "{}", repeated.error.unwrap_or_default());
    assert_eq!(repeated.data["reused"], true);
    assert_eq!(repeated.data["reason"], "confirmed offline after snapshot");

    let replayed = replay(&root).unwrap();
    assert!(!replayed.workers.contains_key("ordinary"));
    assert_eq!(
        replayed.worker_closures["ordinary"].reason,
        "confirmed offline after snapshot"
    );
    assert_eq!(
        replayed.worker_snapshots["ordinary"].thread_id,
        "thread-ordinary"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_promotion_requires_user_approval_and_existing_master_delegates() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    let worker_registration = register(&server, "peer-a", "%a");
    let target_registration = register(&server, "peer-b", "%b");
    assert_eq!(worker_registration.data["role_brief"]["role"], "worker");
    assert_eq!(target_registration.data["role_brief"]["role"], "worker");
    assert!(worker_registration.data["role_brief"]["role_task"]
        .as_str()
        .unwrap()
        .contains("independent task"));
    assert_eq!(
        worker_registration.data["role_brief"]["communication_recovery"]["close_only_when"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let missing =
        super::handle_master_promote(&server, "peer-a".into(), "token-peer-a".into(), "".into());
    assert!(!missing.ok);
    assert!(missing.error.unwrap().contains("approval"));

    let promoted = super::handle_master_promote(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "user approved peer-a as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(promoted.data["mode"], "user_approved_self_promotion");
    assert_eq!(promoted.data["role_brief"]["role"], "master");
    assert!(promoted.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line.as_str().unwrap().contains("assign tasks")));
    assert!(
        promoted.data["role_brief"]["communication_recovery"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("collab worker recover"))
    );
    let context = handle_context(&server, "peer-a".into(), "token-peer-a".into());
    assert_eq!(context.data["master"]["worker_id"], "peer-a");
    assert_eq!(context.data["role_brief"]["role"], "master");
    let status = super::handle_master_status(&server);
    assert_eq!(status.data["master"]["worker_id"], "peer-a");
    assert_eq!(
        status.data["master"]["approval"],
        "user approved peer-a as collab master"
    );

    let rejected = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b as collab master".into(),
    );
    assert!(!rejected.ok);
    assert!(rejected
        .error
        .unwrap()
        .contains("only the registered master"));

    let outsider = super::handle_master_delegate(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "peer-a".into(),
    );
    assert!(!outsider.ok);
    assert!(outsider.error.unwrap().contains("master authority"));

    let delegated = super::handle_master_delegate(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
    );
    assert!(delegated.ok, "{}", delegated.error.unwrap_or_default());
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    assert_eq!(delegated.data["role_brief"]["role"], "master");
    let target_context = handle_context(&server, "peer-b".into(), "token-peer-b".into());
    assert_eq!(target_context.data["identity"]["role"], "master");
    assert_eq!(target_context.data["role_brief"]["role"], "master");
    let workers = dispatch_with_route_context(&server, Req::Workers, None, None);
    let target_worker = workers.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "peer-b")
        .unwrap();
    assert_eq!(target_worker["role_brief"]["role"], "master");
    let status = dispatch_with_route_context(&server, Req::StatusAll, None, None);
    let target_status = status.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "peer-b")
        .unwrap();
    assert_eq!(target_status["role_brief"], target_worker["role_brief"]);
    assert_eq!(target_registration.data["role_brief"]["role"], "worker");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn role_contract_is_identical_across_context_workers_and_status() {
    use crate::subagent::Record;

    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "worker", "%worker");
    register(&server, "child", "%child");
    promote_master(&server, "master", "user approved master");
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed-role".into(),
            parent: "master".into(),
            peer: "child".into(),
            status: "idle".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now_ms(),
            ready_deadline_ms: 0,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
        },
    }]);
    let server = Arc::new(server);

    let cases = [
        ("master", "master"),
        ("worker", "worker"),
        ("child", "managed-subagent"),
    ];
    for (worker_id, expected_role) in cases {
        let context = handle_context(&server, worker_id.into(), format!("token-{worker_id}"));
        assert!(context.ok, "{context:?}");
        assert_eq!(context.data["identity"]["role"], expected_role);
        assert_eq!(context.data["role_brief"]["role"], expected_role);
        assert_eq!(
            context.data["authority"],
            context.data["role_brief"]["authority"]
        );
        assert_eq!(
            context.data["role_brief"]["derivation"]["parent"].as_str(),
            (expected_role == "managed-subagent").then_some("master")
        );
    }

    let workers = dispatch_with_route_context(&server, Req::Workers, None, None);
    assert!(workers.ok, "{workers:?}");
    let status = dispatch_with_route_context(&server, Req::StatusAll, None, None);
    assert!(status.ok, "{status:?}");
    for worker in workers.data["workers"].as_array().unwrap() {
        let id = worker["id"].as_str().unwrap();
        let status_worker = status.data["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == id)
            .unwrap();
        assert_eq!(worker["role"], worker["role_brief"]["role"]);
        assert_eq!(status_worker["role_brief"], worker["role_brief"]);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dead_master_transport_is_not_claimable_and_allows_approved_self_promote() {
    let (mut server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    register(&server, "peer-b", "thread-b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );
    server.appserver_candidate_check = Arc::new(|candidate| {
        if candidate.thread_id == "thread-b" {
            Ok(test_appserver_transport("thread-b"))
        } else {
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "thread is not live".into(),
            }
            .to_string())
        }
    });
    let status = super::handle_master_status(&server);
    assert!(status.data["master"].is_null(), "{status:?}");
    assert_eq!(status.data["recorded_unusable"]["worker_id"], "peer-a");
    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master runtime died".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    let live = super::handle_master_status(&server);
    assert_eq!(live.data["master"]["worker_id"], "peer-b");
    assert!(live.data["recorded_unusable"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn not_loaded_master_thread_is_recorded_unusable_and_allows_approved_self_promote() {
    let (mut server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    register(&server, "peer-b", "thread-b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": if thread_id == "thread-a" {"notLoaded"} else {"idle"}},
                "canAcceptDirectInput": thread_id != "thread-a"
            }
        }))
    });

    let status = super::handle_master_status(&server);
    assert!(status.data["master"].is_null(), "{status:?}");
    assert_eq!(status.data["recorded_unusable"]["worker_id"], "peer-a");
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        None
    );
    let context = handle_context(&server, "peer-b".into(), "token-peer-b".into());
    assert!(context.data["master"].is_null(), "{context:?}");
    assert_eq!(context.data["recorded_unusable"]["worker_id"], "peer-a");

    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master thread became unusable".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_send_requires_master_endpoints_on_both_sides() {
    let (server, root) = test_server();
    register(&server, "target-master", "%target-master");
    register(&server, "target-peer", "%target-peer");
    let promoted = super::handle_master_promote(
        &server,
        "target-master".into(),
        "token-target-master".into(),
        "user approved target-master as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let denied_peer = super::handle_cross_project_send(
        &server,
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-operator".into(),
        None,
        1,
        "target-peer".into(),
        "feature".into(),
        "must reject non-master target".into(),
        None,
    );
    assert!(!denied_peer.ok);
    assert!(denied_peer
        .error
        .unwrap()
        .contains("target to be a live master"));

    let delivered = super::handle_cross_project_send(
        &server,
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-operator".into(),
        None,
        1,
        "target-master".into(),
        "feature".into(),
        "master-to-master message".into(),
        None,
    );
    assert!(delivered.ok, "{}", delivered.error.unwrap_or_default());
    let msg_id = delivered.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[msg_id].to, "target-master");
    assert_eq!(state.msgs[msg_id].from, "appsdk-master@/tmp/appsdk");
    drop(state);

    let missing_approval = super::handle_cross_project_send(
        &server,
        "self-promoted".into(),
        "/tmp/appsdk".into(),
        "thread-self-promoted".into(),
        "self-promoted".into(),
        None,
        1,
        "target-master".into(),
        "feature".into(),
        "must reject missing approval".into(),
        None,
    );
    assert!(!missing_approval.ok);
    assert!(missing_approval.error.unwrap().contains("user approval"));
    std::fs::remove_dir_all(root).unwrap();
}

fn register_appserver(server: &mut Server, id: &str, thread_id: &str) -> Resp {
    let app_scope = AppServerId::new("appserver-test").unwrap();
    let candidate = crate::proto::AppServerCandidate {
        endpoint: format!("unix:///tmp/collab-appserver-{id}.sock"),
        namespace: "codex_tui".into(),
        session_id: format!("session-{thread_id}"),
        thread_id: thread_id.into(),
        cwd: server.root.display().to_string(),
    };
    let candidate_for_closure = candidate.clone();
    let checked = {
        let expected = thread_id.to_owned();
        move |candidate: &crate::proto::AppServerCandidate| {
            assert_eq!(candidate.thread_id, expected);
            Ok(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some(candidate.endpoint.clone()),
                namespace: Some(candidate.namespace.clone()),
                session_id: Some(candidate.session_id.clone()),
                thread_id: Some(candidate.thread_id.clone()),
                capabilities: vec!["send_message_to_thread".into()],
                self_check: "test App Server candidate".into(),
            })
        }
    };
    server.appserver_candidate_check = Arc::new(checked);
    handle_register_with_app_scope(
        server,
        id.into(),
        format!("token-{id}"),
        server.root.display().to_string(),
        Some(app_scope),
        Some(TransportCandidates {
            appserver: Some(candidate_for_closure),
        }),
    )
}

#[test]
fn init_registration_result_exposes_persisted_runtime_identity() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "peer-init", "thread-init");
    assert!(registration.ok, "{registration:?}");
    let runtime =
        runtime_from_registration_receipt(&registration.data, "peer-init", &root).unwrap();
    assert_eq!(
        runtime.runtime_id.as_str(),
        "runtime-appserver-session-thread-init-thread-init"
    );
    assert_eq!(
        runtime.native_thread_id.as_ref().unwrap().as_str(),
        "thread-init"
    );
    assert_eq!(registration.data["transport_selected"]["kind"], "appserver");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_promotion_requires_live_transport() {
    let (mut server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    server.appserver_candidate_check = Arc::new(|_| {
        Err(crate::client::adapters::AdapterError::RouteUnavailable {
            detail: "thread is not live".into(),
        }
        .to_string())
    });
    let denied = super::handle_master_promote(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "user approved peer-a as collab master".into(),
    );
    assert!(!denied.ok);
    assert!(denied.error.unwrap().contains("live registered transport"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_promotion_allows_verified_appserver() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    assert_eq!(registered.data["transport_selected"]["kind"], "appserver");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as appserver master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-appserver".into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_assigned_replays_approval_and_live_identity() {
    let event = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("user approved peer-a as collab master".into()),
        assigned_ms: 1,
    };
    let encoded = serde_json::to_string(&event).unwrap();
    let mut replay = State::default();
    replay.apply(&serde_json::from_str(&encoded).unwrap());
    assert_eq!(replay.master_worker_id.as_deref(), Some("peer-a"));
    assert_eq!(replay.master_assigned_by.as_deref(), Some("peer-a"));
    assert_eq!(
        replay.master_approval.as_deref(),
        Some("user approved peer-a as collab master")
    );
    assert_eq!(replay.master_assigned_ms, Some(1));
    let legacy = r#"{"ev":"RootAssigned","worker_id":"peer-b","assigned_by":"peer-a","approval":null,"assigned_ms":2}"#;
    let mut legacy_replay = State::default();
    legacy_replay.apply(&serde_json::from_str(legacy).unwrap());
    assert_eq!(legacy_replay.master_worker_id.as_deref(), Some("peer-b"));
}

#[test]
fn master_authority_is_generation_bound_and_replays_from_typed_grant() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");

    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let (scope, generation) = {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer-appserver")
            .unwrap()
            .clone();
        let grant = state
            .global
            .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
            .expect("promotion must commit a generation-bound typed master grant");
        assert_eq!(grant.endpoint_generation, binding.endpoint_generation);
        (binding.route_scope(), binding.endpoint_generation)
    };

    let reconnected = register_appserver(&mut server, "peer-appserver", "thread-appserver-next");
    assert!(reconnected.ok, "{reconnected:?}");
    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
            .unwrap();
        assert_eq!(binding.endpoint_generation, generation + 1);
        // A same-principal generation replacement is the recovery path, so
        // the grant is reissued for the new generation in the same
        // transaction instead of being dropped.
        let reissued = state
            .global
            .lookup_master_grant_for(&scope, &binding.binding_id)
            .expect("same-principal reconnect must reissue the master grant");
        assert_eq!(reissued.endpoint_generation, binding.endpoint_generation);
        assert_eq!(
            state
                .global
                .role_for_binding(&scope.project_scope_id, &binding.binding_id),
            crate::server::global_state::PeerRole::Master
        );
    }
    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "peer-appserver");
    assert!(status.data["recorded_unusable"].is_null(), "{status:?}");

    let replayed = super::replay(&root).unwrap();
    let binding = replayed
        .global
        .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
        .unwrap();
    let replayed_grant = replayed
        .global
        .lookup_master_grant_for(&scope, &binding.binding_id)
        .expect("replay must keep the reissued grant");
    assert_eq!(
        replayed_grant.endpoint_generation,
        binding.endpoint_generation
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_runtime_registration_recovery_preserves_master_authority() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as appserver master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let (scope, generation) = {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer-appserver")
            .unwrap()
            .clone();
        (binding.route_scope(), binding.endpoint_generation)
    };

    let recovered = super::handle_register_with_app_scope_inner(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        root.display().to_string(),
        Some(AppServerId::new("appserver-test").unwrap()),
        Some(TransportCandidates {
            appserver: Some(crate::proto::AppServerCandidate {
                endpoint: "unix:///tmp/collab-appserver-peer-appserver.sock".into(),
                namespace: "codex_tui".into(),
                session_id: "session-thread-appserver".into(),
                thread_id: "thread-appserver".into(),
                cwd: root.display().to_string(),
            }),
        }),
        true,
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.data["recovered"], true);
    assert_eq!(recovered.data["role_brief"]["role"], "master");

    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
            .unwrap();
        assert_eq!(binding.endpoint_generation, generation + 1);
        let grant = state
            .global
            .lookup_master_grant_for(&scope, &binding.binding_id)
            .expect("same-principal recovery must reissue the master grant");
        assert_eq!(grant.endpoint_generation, binding.endpoint_generation);
    }
    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "peer-appserver");

    let replayed = super::replay(&root).unwrap();
    let binding = replayed
        .global
        .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
        .unwrap();
    assert_eq!(binding.endpoint_generation, generation + 1);
    let replayed_grant = replayed
        .global
        .lookup_master_grant_for(&scope, &binding.binding_id)
        .expect("replay must keep the reissued master grant");
    assert_eq!(
        replayed_grant.endpoint_generation,
        binding.endpoint_generation
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Rebind one registered worker onto a new App Server thread, which is the
/// generation replacement a real registration recovery performs.
fn recover_worker_on_new_thread(server: &mut Server, id: &str, thread_id: &str) -> Resp {
    let app_scope = AppServerId::new("appserver-test").unwrap();
    let candidate = crate::proto::AppServerCandidate {
        endpoint: format!("unix:///tmp/collab-appserver-{id}.sock"),
        namespace: "codex_tui".into(),
        session_id: format!("session-{thread_id}"),
        thread_id: thread_id.into(),
        cwd: server.root.display().to_string(),
    };
    server.appserver_candidate_check = Arc::new(|candidate: &crate::proto::AppServerCandidate| {
        Ok(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(candidate.endpoint.clone()),
            namespace: Some(candidate.namespace.clone()),
            session_id: Some(candidate.session_id.clone()),
            thread_id: Some(candidate.thread_id.clone()),
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test App Server candidate".into(),
        })
    });
    handle_register_with_app_scope(
        server,
        id.into(),
        format!("token-{id}"),
        server.root.display().to_string(),
        Some(app_scope),
        Some(TransportCandidates {
            appserver: Some(candidate),
        }),
    )
}

fn registered_binding(server: &Server, id: &str) -> crate::server::global_state::RuntimeBinding {
    server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} has no runtime binding"))
}

#[test]
fn wire_master_recover_reissues_master_grant_for_new_generation() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "recover-master", "thread-recover-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "recover-master", "user approved recover-master");

    let previous = registered_binding(&server, "recover-master");
    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-master", "thread-recover-master-new");
    assert!(recovered.ok, "{recovered:?}");

    let state = server.state.lock().unwrap();
    let route_scope = previous.route_scope();
    let current = state
        .global
        .lookup_binding_for(&route_scope, &previous.binding_id)
        .cloned()
        .expect("recovery keeps the binding id");
    assert_eq!(
        current.endpoint_generation,
        previous.endpoint_generation + 1
    );
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &previous.binding_id)
        .expect("same-principal recovery must reissue the master grant");
    assert_eq!(grant.endpoint_generation, current.endpoint_generation);
    assert_eq!(
        state
            .global
            .role_for_route(&route_scope, &current.binding_id),
        crate::server::global_state::PeerRole::Master
    );
    drop(state);

    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "recover-master");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wire_peer_recover_does_not_revoke_unrelated_master() {
    let (mut server, root) = test_server();
    let master = register_appserver(&mut server, "recover-keep-master", "thread-keep-master");
    assert!(master.ok, "{master:?}");
    let peer = recover_worker_on_new_thread(&mut server, "recover-plain-peer", "thread-plain-peer");
    assert!(peer.ok, "{peer:?}");
    promote_master(
        &server,
        "recover-keep-master",
        "user approved recover-keep-master",
    );
    let master_binding = registered_binding(&server, "recover-keep-master");

    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-plain-peer", "thread-plain-peer-new");
    assert!(recovered.ok, "{recovered:?}");

    let state = server.state.lock().unwrap();
    let route_scope = master_binding.route_scope();
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &master_binding.binding_id)
        .expect("an unrelated peer recovery must not remove the master grant");
    assert_eq!(
        grant.endpoint_generation,
        master_binding.endpoint_generation
    );
    assert_eq!(
        state
            .global
            .role_for_route(&route_scope, &master_binding.binding_id),
        crate::server::global_state::PeerRole::Master
    );
    let grants: Vec<_> = state
        .global
        .projects
        .values()
        .flat_map(|project| project.master_grants.values())
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "recovery must not create a duplicate grant"
    );
    assert_eq!(grants[0].binding_id, master_binding.binding_id);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovered_master_keeps_goal_deadline_scheduling() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "recover-deadline", "thread-deadline-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(
        &server,
        "recover-deadline",
        "user approved recover-deadline",
    );
    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-deadline", "thread-deadline-new");
    assert!(recovered.ok, "{recovered:?}");

    let scheduled = handle_notification_subscribe(
        &server,
        "recover-deadline".into(),
        "token-recover-deadline".into(),
        "deadline".into(),
        Some("goal:recover-deadline".into()),
        Some(now_ms() + 60_000),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(
        scheduled.ok,
        "recovered master must still schedule goal deadlines: {scheduled:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_cannot_override_typed_grant_on_replay() {
    use std::io::Write;

    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let legacy = Event::MasterAssigned {
        worker_id: "peer-appserver".into(),
        assigned_by: "legacy-operator".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let replayed = super::replay(&root).expect("mixed legacy and typed authority must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-appserver")
        .unwrap();
    let grant = replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .expect("typed grant must remain authoritative");
    assert_eq!(grant.granted_by, "peer-appserver");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_before_runtime_rebind_does_not_regrant_authority() {
    use std::io::Write;

    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");

    let legacy = Event::MasterAssigned {
        worker_id: "peer-appserver".into(),
        assigned_by: "legacy-operator".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let reconnected = register_appserver(&mut server, "peer-appserver", "thread-appserver-next");
    assert!(reconnected.ok, "{reconnected:?}");

    let replayed = super::replay(&root).expect("legacy authority before reconnect must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-appserver")
        .unwrap();
    assert_eq!(binding.endpoint_generation, 2);
    assert!(replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_after_runtime_binding_is_migrated() {
    use std::io::Write;

    let (server, root) = test_server();
    register(&server, "peer-a", "thread-peer-a");

    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let replayed = super::replay(&root).expect("legacy authority after binding must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-a")
        .unwrap();
    let grant = replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .expect("current legacy authority must migrate to a typed grant");
    assert_eq!(grant.granted_by, "peer-a");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_without_runtime_binding_fails_replay_closed() {
    use crate::identity::AppServerId;
    use crate::server::global_state::ProjectRegistration;
    use std::io::Write;

    let root = std::env::temp_dir().join(format!(
        "collab-legacy-master-missing-binding-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let registration = ProjectRegistration::new(project_scope, app_scope).unwrap();
    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let unrelated_binding = crate::server::global_state::RuntimeBinding::new(
        registration.project_scope.clone(),
        registration.app_scope_id.clone(),
        crate::identity::AgentId::new("peer-b").unwrap(),
        crate::identity::RuntimeId::new("runtime-b").unwrap(),
        crate::identity::BindingId::new("binding-b").unwrap(),
        1,
        None,
    )
    .unwrap();
    let journal = server_dir.join("journal.jsonl");
    let mut file = std::fs::File::create(&journal).unwrap();
    for event in [
        Event::GlobalProjectRegistered { registration },
        Event::GlobalRuntimeBound {
            binding: unrelated_binding,
        },
        legacy,
    ] {
        writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
    drop(file);

    let error = match replay(&root) {
        Ok(_) => panic!("missing binding must fail replay closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("MASTER_AUTHORITY_REQUIRES_RUNTIME_BINDING"),
        "{error:#}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_with_ambiguous_binding_fails_replay_closed() {
    use crate::identity::{AppServerId, BindingId, RuntimeId};
    use crate::server::global_state::{ProjectRegistration, RuntimeBinding};
    use std::io::Write;

    let root = std::env::temp_dir().join(format!(
        "collab-legacy-master-ambiguous-binding-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let registration = ProjectRegistration::new(project_scope.clone(), app_scope.clone()).unwrap();
    let first = RuntimeBinding::new(
        project_scope.clone(),
        app_scope.clone(),
        crate::identity::AgentId::new("peer-a").unwrap(),
        RuntimeId::new("runtime-a").unwrap(),
        BindingId::new("binding-a").unwrap(),
        1,
        None,
    )
    .unwrap();
    let second = RuntimeBinding::new(
        project_scope,
        app_scope,
        crate::identity::AgentId::new("peer-a").unwrap(),
        RuntimeId::new("runtime-b").unwrap(),
        BindingId::new("binding-b").unwrap(),
        1,
        None,
    )
    .unwrap();
    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = server_dir.join("journal.jsonl");
    let mut file = std::fs::File::create(&journal).unwrap();
    for event in [
        Event::GlobalProjectRegistered { registration },
        Event::GlobalRuntimeBound { binding: first },
        Event::GlobalRuntimeBound { binding: second },
        legacy,
    ] {
        writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
    drop(file);

    let error = match replay(&root) {
        Ok(_) => panic!("ambiguous binding must fail replay closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("MASTER_AUTHORITY_AMBIGUOUS_BINDING"),
        "{error:#}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn journal_root_assigned_rewrites_to_master_on_replay() {
    let root = std::env::temp_dir().join(format!(
        "collab-root-to-master-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = server_dir.join("journal.jsonl");
    std::fs::write(
        &journal,
        r#"{"ev":"RootAssigned","worker_id":"peer-a","assigned_by":"peer-a","approval":"user approved","assigned_ms":1}
"#,
    )
    .unwrap();
    let state = replay(&root).unwrap();
    assert_eq!(state.master_worker_id.as_deref(), Some("peer-a"));
    let rewritten = std::fs::read_to_string(&journal).unwrap();
    assert!(rewritten.contains("MasterAssigned"), "{rewritten}");
    assert!(!rewritten.contains("RootAssigned"), "{rewritten}");
    let again = replay(&root).unwrap();
    assert_eq!(again.master_worker_id.as_deref(), Some("peer-a"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_and_later_registration_are_equal_peers() {
    let (server, root) = test_server();
    assert_eq!(
        register(&server, "peer-a", "%peer-a").data["identity_kind"],
        "peer"
    );
    assert_eq!(
        register(&server, "peer-b", "%peer-b").data["identity_kind"],
        "peer"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.workers.len(), 2);
    assert!(state.master_worker_id.is_none());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registration_creates_one_finite_default_direct_message_subscription() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    assert!(register(&server, "peer", "%peer").ok);
    let state = server.state.lock().unwrap();
    let subscriptions: Vec<_> = state
        .notification_subscriptions
        .values()
        .filter(|subscription| subscription.worker_id == "peer")
        .collect();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].event, "direct-message");
    assert_eq!(subscriptions[0].method, "appserver");
    assert_eq!(subscriptions[0].target, "thread-peer");
    assert_eq!(subscriptions[0].status, "armed");
    assert!(subscriptions[0].expires_ms > now_ms());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn cancelled_default_lease_stays_suppressed_until_explicit_subscribe() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    let cancelled = handle_notification_unsubscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "sub-default-direct-message-peer".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
    assert!(register(&server, "peer", "%peer").ok);

    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"].status,
        "cancelled"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-peer"),
        now_ms()
    )
    .is_empty());
    drop(state);

    let explicit = handle_notification_subscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3_600,
    );
    assert!(explicit.ok, "{}", explicit.error.unwrap_or_default());
    let explicit_id = explicit.data["subscription"]["id"].as_str().unwrap();
    assert_ne!(explicit_id, "sub-default-direct-message-peer");
    assert_eq!(
        explicit.data["subscription"]["status"],
        serde_json::Value::String("armed".into())
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registration_adds_default_lease_when_only_short_direct_message_lease_exists() {
    let mut state = State::default();
    let now = 10_000;
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-short".into(),
            worker_id: "peer".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-peer".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now + 600_000,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
        },
    });

    let events = default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-peer"),
        now,
    );
    let default = events
        .iter()
        .find_map(|event| match event {
            Event::NotificationSubscribed { subscription } => Some(subscription),
            _ => None,
        })
        .expect("a short explicit lease must not suppress the default peer lease");
    assert_eq!(default.id, "sub-default-direct-message-peer");
    assert_eq!(
        default.expires_ms,
        now + DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000
    );

    for event in &events {
        state.apply(event);
    }
    assert_eq!(
        state.notification_subscriptions["sub-short"].status,
        "rebound"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-peer"),
        now + 1
    )
    .is_empty());
}

#[test]
fn daemon_replay_restores_default_lease_for_registered_peer() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_appserver_transport("thread-peer")),
        },
    });

    let events = registered_peer_default_events(&state, 10_000);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.id == "sub-default-direct-message-peer"
    )));
}

#[test]
fn daemon_restart_restores_default_lease_from_registered_appserver_transport() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_appserver_transport("thread-current")),
        },
    });

    let lease_events = registered_peer_default_events(&state, 10_000);
    assert!(lease_events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.worker_id == "peer" && subscription.target == "thread-current"
    )));
}

#[test]
fn daemon_restart_reuses_existing_deadline_without_recreating_it() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_appserver_transport("thread-current")),
        },
    });
    let original = NotificationSubscription {
        id: "sub-goal".into(),
        worker_id: "peer".into(),
        event: "deadline".into(),
        subject: Some("goal:sha256:test".into()),
        target: "thread-current".into(),
        method: "appserver".into(),
        trigger_ms: Some(20_000),
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        expires_ms: 60_000,
        status: "armed".into(),
        created_ms: 1,
        updated_ms: 1,
        status_reason: None,
    };
    state.apply(&Event::NotificationSubscribed {
        subscription: original.clone(),
    });

    let events = registered_peer_default_events(&state, 10_000);
    assert!(!events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.id == "sub-goal"
    )));
    let rebound = &state.notification_subscriptions["sub-goal"];
    assert_eq!(rebound.id, original.id);
    assert_eq!(rebound.target, "thread-current");
    assert_eq!(rebound.trigger_ms, original.trigger_ms);
    assert_eq!(rebound.expires_ms, original.expires_ms);
    assert_eq!(rebound.status, "armed");
}

#[test]
fn daemon_restart_does_not_restore_without_registered_transport() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: None,
        },
    });

    assert!(registered_peer_default_events(&state, 10_000).is_empty());
}

#[test]
fn expired_mailbox_and_journal_are_removed_and_do_not_replay() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let now = now_ms();
    let old_id = "m-old".to_string();
    let fresh_id = "m-fresh".to_string();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: old_id.clone(),
                from: "peer".into(),
                to: "peer".into(),
                mtype: "notify".into(),
                subject: Some("old".into()),
                body: "expired body".into(),
                in_reply_to: None,
                created_ms: now - 8 * 86_400_000,
                state: "read".into(),
                wake_attempt_count: 1,
                last_wake_attempt_ms: now - 8 * 86_400_000,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: fresh_id.clone(),
                from: "peer".into(),
                to: "peer".into(),
                mtype: "notify".into(),
                subject: Some("new".into()),
                body: "fresh body".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::DeliveryMode {
            msg_id: old_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: Some("thread-source".into()),
        },
    ]);
    let mailbox = root.join(".agent-collab/mailbox");
    assert!(mailbox.join("m-old.json").exists());
    assert!(mailbox.join("m-fresh.json").exists());
    let journal_before = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
        .unwrap()
        .lines()
        .count();

    assert_eq!(purge_expired_storage(&server, now), 1);
    let state = server.state.lock().unwrap();
    assert!(!state.msgs.contains_key(&old_id));
    assert!(!state.delivery_source_threads.contains_key(&old_id));
    assert!(state.msgs.contains_key(&fresh_id));
    assert!(state.workers.contains_key("peer"));
    drop(state);
    assert!(!mailbox.join("m-old.json").exists());
    assert!(mailbox.join("m-fresh.json").exists());
    let journal = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(!journal.contains("m-old"));
    assert!(journal.contains("m-fresh"));
    assert!(journal.lines().count() < journal_before);

    let replayed = replay(&root).unwrap();
    assert!(!replayed.msgs.contains_key(&old_id));
    assert!(!replayed.delivery_source_threads.contains_key(&old_id));
    assert_eq!(replayed.msgs[&fresh_id].body, "fresh body");
    assert_eq!(
        replayed.workers["peer"]
            .transport
            .as_ref()
            .and_then(|transport| transport.thread_id.as_deref()),
        Some("thread-peer")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retention_skips_fresh_messages_and_frozen_admission() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let now = now_ms();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "m-keep".into(),
            from: "peer".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("keep".into()),
            body: "keep".into(),
            in_reply_to: None,
            created_ms: now - 86_400_000,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    assert_eq!(purge_expired_storage(&server, now), 0);
    assert!(server.state.lock().unwrap().msgs.contains_key("m-keep"));

    server.commit(&[Event::Sent {
        msg: Message {
            id: "m-old-frozen".into(),
            from: "peer".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("old".into()),
            body: "old".into(),
            in_reply_to: None,
            created_ms: now - 8 * 86_400_000,
            state: "read".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
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
            message_count: 2,
            operator: "peer".into(),
            issues: Vec::new(),
            created_ms: now,
            updated_ms: now,
        },
    }]);
    assert_eq!(purge_expired_storage(&server, now), 0);
    assert!(server
        .state
        .lock()
        .unwrap()
        .msgs
        .contains_key("m-old-frozen"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn fresh_default_does_not_skip_legacy_duplicate_cleanup() {
    let mut state = State::default();
    for event in default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-one"),
        1000,
    ) {
        state.apply(&event);
    }
    let mut old = state
        .notification_subscriptions
        .values()
        .next()
        .unwrap()
        .clone();
    old.id = "sub-legacy".into();
    state.apply(&Event::NotificationSubscribed { subscription: old });
    for event in default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-one"),
        2000,
    ) {
        state.apply(&event);
    }
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|sub| sub.status == "armed")
            .count(),
        1
    );
    assert_eq!(
        state.notification_subscriptions["sub-legacy"].status,
        "rebound"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-one"),
        2000
    )
    .is_empty());
}

#[test]
fn default_subscription_renews_matching_target_and_keeps_stale_target_visible() {
    let mut state = State::default();
    for event in default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-one"),
        1000,
    ) {
        state.apply(&event);
    }
    let id = state
        .notification_subscriptions
        .keys()
        .next()
        .unwrap()
        .clone();
    let ttl = DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000;
    for (thread_id, time) in [("thread-one", ttl), ("thread-one", ttl * 3)] {
        for event in default_direct_message_events(
            &state,
            "peer",
            &test_appserver_transport(thread_id),
            time,
        ) {
            state.apply(&event);
        }
        assert_eq!(state.notification_subscriptions.len(), 1);
        let sub = &state.notification_subscriptions[&id];
        assert_eq!(sub.target, thread_id);
        assert_eq!(sub.status, "armed");
        assert_eq!(sub.expires_ms, time + ttl);
    }

    // A changed App Server thread must not silently rewrite the armed lease
    // target from a re-registration path; the stale target stays visible until
    // an explicit rebind/recovery decides the new address.
    let events = default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-two"),
        ttl * 4,
    );
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(state.notification_subscriptions[&id].target, "thread-one");
    assert_eq!(state.notification_subscriptions.len(), 1);
}

#[test]
fn replay_removes_legacy_declared_roles() {
    let mut state = State::default();
    for (id, role) in [("legacy-master", "master"), ("legacy-worker", "worker")] {
        let json = format!(
            r#"{{"ev":"Registered","worker":{{"id":"{id}","token":"token-{id}","pane":"%{id}","cwd":"/tmp","registered_ms":1,"role":"{role}"}}}}"#
        );
        let event: Event = serde_json::from_str(&json).unwrap();
        state.apply(&event);
    }
    assert!(state
        .workers
        .values()
        .all(|worker| { serde_json::to_value(worker).unwrap().get("role").is_none() }));
}

#[test]
fn only_owner_mutates_and_closes_task() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%peer-a");
    register(&server, "peer-b", "%peer-b");
    assert!(create_task(&server, "peer-a", "task-a", "feature-a").ok);

    let update = handle_task_update(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "task-a".into(),
        Some("verifying".into()),
        None,
    );
    assert!(!update.ok);
    let close = handle_task_close(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "task-a".into(),
        false,
        None,
    );
    assert!(!close.ok);
    assert_eq!(
        server.state.lock().unwrap().tasks["task-a"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_completes_local_lifecycle_without_peer_reports() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            Some("verifying".into()),
            Some("continue verifying".into()),
        )
        .ok
    );
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("tests and candidate commit verified".into()),
        Some("/tmp/task-worktree".into()),
    );
    assert!(delivered.ok);
    assert_eq!(delivered.data["notification"], "none");
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            true,
            false,
            "reviewed candidate".into(),
        )
        .ok
    );
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "closed");
    assert_eq!(state.cleanup_receipts["task"].task_id, "task");
    assert!(
        state.msgs.is_empty(),
        "normal lifecycle must not report to peers"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_uses_integrated_main_not_daemon_head_for_cleanup() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/cleanup-live"]);
    std::fs::write(root.join("cleanup-live.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "cleanup-live.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "cleanup task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/cleanup-live"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    git_ok(&root, &["checkout", "-q", "-b", "root-snapshot", &base]);
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/cleanup-live");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/cleanup-live",
        ],
    );

    let registered = handle_task_register(
        &server,
        "peer".into(),
        "token-peer".into(),
        "cleanup-live".into(),
        None,
        Some("feature".into()),
        Some("playground/cleanup-live".into()),
        Some("codex/cleanup-live".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["cleanup-live"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "peer".into(),
                "token-peer".into(),
                "cleanup-live".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "cleanup-live".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(!worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/cleanup-live"])
        .output()
        .unwrap();
    assert!(!branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["cleanup-live"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["cleanup-live"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn live_master_closes_merged_task_with_verified_cleanup() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master close test");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/master-cleanup"]);
    std::fs::write(root.join("master-cleanup.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "master-cleanup.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "master cleanup task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/master-cleanup"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/master-cleanup");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/master-cleanup",
        ],
    );

    let registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "master-cleanup".into(),
        None,
        Some("feature".into()),
        Some("playground/master-cleanup".into()),
        Some("codex/master-cleanup".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["master-cleanup"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "master-cleanup".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "master-cleanup".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "master-cleanup".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "master-cleanup".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );

    let closed = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "master-cleanup".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(!worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/master-cleanup"])
        .output()
        .unwrap();
    assert!(!branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["master-cleanup"].owner, "owner");
    assert_eq!(state.tasks["master-cleanup"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["master-cleanup"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_records_verified_receipt_after_prior_safe_cleanup() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/already-clean"]);
    std::fs::write(root.join("already-clean.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "already-clean.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "already clean task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/already-clean"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/already-clean");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/already-clean",
        ],
    );

    let registered = handle_task_register(
        &server,
        "peer".into(),
        "token-peer".into(),
        "already-clean".into(),
        None,
        Some("feature".into()),
        Some("playground/already-clean".into()),
        Some("codex/already-clean".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["already-clean"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "peer".into(),
                "token-peer".into(),
                "already-clean".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );
    git_ok(&root, &["worktree", "remove", &worktree_string]);
    git_ok(&root, &["branch", "-D", "codex/already-clean"]);

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "already-clean".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["already-clean"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["already-clean"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_force_close_skips_owner_and_cleanup_requirements() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved force-close test");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "stuck".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: Some("playground/stuck".into()),
            branch: Some("codex/stuck".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "blocked".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "stuck".into(),
        true,
        Some("worktree dirty and merge blocked; force closing per master".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    assert_eq!(resp.data["status"], "closed");
    assert_eq!(resp.data["cleanup"]["result"], "unverified");
    assert_eq!(
        resp.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["stuck"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["stuck"].manual_reason.as_deref(),
        Some("worktree dirty and merge blocked; force closing per master"),
    );
    assert_eq!(
        state.cleanup_receipts["stuck"].verification,
        crate::server::state::CleanupVerification::Unverified
    );
    let projected = task_view(&state, &state.tasks["stuck"]);
    assert_eq!(projected["cleanup"]["status"], "unverified");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

/// Force-close a holder that declares no worktree/branch, leaving one waiter
/// blocked on it with a resource-released subscription. This is the fixture the
/// finalize regressions build on.
fn force_closed_unverified_holder_with_waiter(server: &Server) -> Vec<String> {
    register(server, "holder", "%holder");
    register(server, "waiter", "%waiter");
    register(server, "master", "%master");
    promote_master(server, "master", "user approved finalize test");
    assert!(create_task(server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    assert!(
        handle_notification_subscribe(
            server,
            "waiter".into(),
            "token-waiter".into(),
            "resource-released".into(),
            Some("held".into()),
            None,
            Vec::new(),
            None,
            1,
            60,
        )
        .ok
    );
    let forced = handle_task_close(
        server,
        "master".into(),
        "token-master".into(),
        "held".into(),
        true,
        Some("holder abandoned mid-flight; force closing".into()),
    );
    assert!(forced.ok, "{}", forced.error.unwrap_or_default());
    assert_eq!(forced.data["cleanup"]["result"], "unverified");
    // A force close records the obligation; it must not release the waiter.
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert_eq!(
        state.cleanup_receipts["held"].verification,
        crate::server::state::CleanupVerification::Unverified
    );
    drop(state);
    Vec::new()
}

#[test]
fn force_close_without_finalize_reports_unverified_and_does_not_release() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["held"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["held"].verification,
        crate::server::state::CleanupVerification::Unverified,
        "an unfinalized force close keeps the unverified receipt, not a success"
    );
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert!(
        state
            .msgs
            .values()
            .all(|message| !message.body.starts_with("RESOURCE_RELEASED ")),
        "an unverified force close must not release dependents"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_releases_a_waiter_exactly_once() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert_eq!(finalized.data["finalized"], true);
    assert_eq!(finalized.data["released_dependents"], json!(["waiting"]));
    assert_eq!(
        finalized.data["cleanup"]["manual_reason"], "holder abandoned mid-flight; force closing",
        "the manual reason stays auditable after finalization"
    );

    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["waiting"].status, "blocked");
        assert!(state.tasks["waiting"].wait.is_none());
        assert_eq!(
            state.cleanup_receipts["held"].verification,
            crate::server::state::CleanupVerification::Verified
        );
        assert_eq!(
            state.cleanup_receipts["held"].manual_reason.as_deref(),
            Some("holder abandoned mid-flight; force closing")
        );
        let releases = state
            .msgs
            .values()
            .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
            .count();
        assert_eq!(releases, 1, "finalize must release the waiter once");
    }

    // A second finalize must not double-release or duplicate the notification.
    let again = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(again.ok, "{}", again.error.unwrap_or_default());
    assert_eq!(again.data["idempotent"], true);
    assert_eq!(again.data["released_dependents"], json!([]));
    let state = server.state.lock().unwrap();
    let releases = state
        .msgs
        .values()
        .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
        .count();
    assert_eq!(releases, 1, "a repeated finalize must not double-release");
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|message| message.subject.as_deref() == Some("released:held"))
            .count(),
        1
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn interrupted_finalize_is_resumed_by_a_retry_not_silently_completed() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let first = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    assert_eq!(first.data["idempotent"], false);

    // Model interruption after the verified receipt committed but before the
    // release ran by rewinding only the waiter back to its waiting state.
    {
        let mut state = server.state.lock().unwrap();
        let mut waiter = state.tasks["waiting"].clone();
        waiter.status = "waiting".into();
        waiter.wait = Some(crate::server::state::WaitSpec {
            deadline_ms: now_ms() + 60_000,
            escalation: String::new(),
            reason: "recheck after finalize".into(),
            responsible_actor: "holder".into(),
            resume_on: vec![],
            waiter: "waiter".into(),
            waiting_for: "held".into(),
        });
        state.tasks.insert("waiting".into(), waiter);
    }

    let retried = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(retried.ok, "{}", retried.error.unwrap_or_default());
    assert_eq!(
        retried.data["idempotent"], true,
        "a verified receipt with an unfinished release must resume, not re-verify"
    );
    assert_eq!(
        retried.data["released_dependents"],
        json!(["waiting"]),
        "the resumed attempt must finish the remaining release explicitly"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "blocked");
    assert!(state.tasks["waiting"].wait.is_none());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_preserves_unread_direct_message_payloads() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    server.commit(&[Event::Sent {
        msg: Message {
            id: "unread-payload".into(),
            from: "holder".into(),
            to: "waiter".into(),
            mtype: "request".into(),
            subject: Some("must survive".into()),
            body: "unread direct message body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let unread = state
        .msgs
        .get("unread-payload")
        .expect("finalize must not delete unread direct-message payloads");
    assert_eq!(unread.body, "unread direct message body");
    assert_eq!(unread.state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_requires_a_closed_task_and_authorized_caller() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "owner", "open-task", "feature").ok);
    let not_closed = handle_task_finalize_cleanup(
        &server,
        "owner".into(),
        "token-owner".into(),
        "open-task".into(),
    );
    assert!(!not_closed.ok);
    assert!(
        not_closed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("must be closed")),
        "{not_closed:?}"
    );
    drop(server);
    std::fs::remove_dir_all(root).ok();
}

/// Build a real project with a clean, merged feature worktree/branch, a
/// force-closed holder task that declared them, and an armed default lease on
/// the holder. This is the fixture the ownership and lease regressions use.
fn force_closed_real_worktree_holder(server: &Server, root: &Path) -> (String, String) {
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).status.success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "collab test"])
        .status
        .success());
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    assert!(git(&["add", "README.md"]).status.success());
    assert!(git(&["commit", "-q", "-m", "base"]).status.success());
    assert!(git(&["branch", "-M", "main"]).status.success());
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        "playground/held-wt"
    ])
    .status
    .success());
    std::fs::write(root.join("playground/held-wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/held-wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/held-wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );
    // Merged into main so cleanup is otherwise allowed to remove it.
    assert!(git(&["merge", "-q", "feature"]).status.success());

    register(server, "holder", "%holder");
    register(server, "master", "%master");
    promote_master(server, "master", "user approved finalize ownership test");
    let now = now_ms();
    let canonical_wt = root
        .join("playground/held-wt")
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "held".into(),
            owner: "holder".into(),
            created_by: "holder".into(),
            feature_id: None,
            worktree_path: Some(canonical_wt.clone()),
            branch: Some("feature".into()),
            base_commit: None,
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let forced = handle_task_close(
        server,
        "master".into(),
        "token-master".into(),
        "held".into(),
        true,
        Some("holder abandoned the worktree; force closing".into()),
    );
    assert!(forced.ok, "{}", forced.error.unwrap_or_default());
    assert_eq!(forced.data["cleanup"]["result"], "unverified");
    let lease_armed = server.state.lock().unwrap().notification_subscriptions
        [&crate::server::mailbox::default_direct_message_id("holder")]
        .status
        .clone();
    (canonical_wt, lease_armed)
}

#[test]
fn finalize_refuses_a_worktree_taken_over_by_another_open_task() {
    let (server, root) = test_server();
    let (worktree, _) = force_closed_real_worktree_holder(&server, &root);
    // A peer legitimately claims the same worktree after the force close,
    // because the closed task no longer counts as an active resource holder.
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "taken-over", "peer-feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut taken = state.tasks["taken-over"].clone();
        taken.worktree_path = Some(worktree.clone());
        taken.branch = Some("feature".into());
        state.tasks.insert("taken-over".into(), taken);
    }

    let refused = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("CLEANUP_FINALIZE_REFUSED")),
        "{refused:?}"
    );
    assert_eq!(refused.data["competing_task"], "taken-over");
    assert_eq!(refused.data["finalized"], false);
    // The competing task's resource must still exist untouched.
    assert!(
        root.join("playground/held-wt").is_dir(),
        "finalize must not destroy a resource another open task owns"
    );
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", "--verify", "refs/heads/feature"])
            .output()
            .unwrap()
            .status
            .success(),
        "finalize must not delete a branch another open task owns"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_removes_the_worktree_when_the_closed_task_still_owns_it() {
    let (server, root) = test_server();
    let (worktree, _) = force_closed_real_worktree_holder(&server, &root);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert_eq!(finalized.data["cleanup"]["worktree"], worktree);
    assert!(!root.join("playground/held-wt").exists());
    assert!(!Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/feature"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_stops_the_owner_default_lease_on_last_responsibility() {
    let (server, root) = test_server();
    let (_, lease_armed) = force_closed_real_worktree_holder(&server, &root);
    assert_eq!(
        lease_armed, "armed",
        "registration must arm the owner's default lease for this test to mean anything"
    );
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions
        [&crate::server::mailbox::default_direct_message_id("holder")];
    assert_eq!(
        lease.status, "cancelled",
        "finalize must stop the owner's automatic lease once its last responsibility is verified"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn non_master_force_close_is_rejected() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "peer", "%peer");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "stuck".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: Some("playground/stuck".into()),
            branch: Some("codex/stuck".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "stuck".into(),
        true,
        Some("not authorized".into()),
    );
    assert!(!resp.ok);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["stuck"].status, "working");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn orphan_force_close_defers_when_owner_appserver_probe_is_unknown() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "working", "feature").ok);
    server.appserver_candidate_check = Arc::new(|candidate| {
        if candidate.thread_id == "thread-owner" {
            Err("owner route probe unknown".into())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working".into(),
        true,
        Some("owner route probe is unknown; defer orphan close".into()),
    );
    assert!(!resp.ok);
    assert!(resp.error.as_deref().is_some_and(|error| {
        error.contains("not authorized")
            || error.contains("owner route probe is unknown")
            || error.contains("live master")
    }));
    assert_eq!(
        server.state.lock().unwrap().tasks["working"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn orphan_force_close_refuses_a_cold_owner_because_cold_is_not_dead() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": if thread_id == "thread-owner" {"notLoaded"} else {"idle"}},
                "canAcceptDirectInput": thread_id != "thread-owner"
            }
        }))
    });

    // A cold thread is not evidence that its owner is dead: the owner may
    // simply be idle on the endpoint, so force close stays refused.
    let refused = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some("owner native thread is not loaded".into()),
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not authorized")),
        "{refused:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["orphan"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_force_close_when_no_live_master_is_allowed() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "orphan".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: default_priority(),
            status: "blocked".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "orphan".into(),
        true,
        Some("master unreachable; owner closes".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["orphan"].status, "closed");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registered_peer_force_closes_orphaned_owner_with_no_live_master() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    server.appserver_candidate_check = Arc::new(|candidate| {
        if candidate.thread_id == "thread-owner" {
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "owner route lost".into(),
            }
            .to_string())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some("owner route lost; no live master; peer closes orphan".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["orphan"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["orphan"].manual_reason.as_deref(),
        Some("owner route lost; no live master; peer closes orphan"),
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn repeated_orphan_force_close_is_idempotent_after_journal_replay() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    server.appserver_candidate_check = Arc::new(|candidate| {
        if candidate.thread_id == "thread-owner" {
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "owner route lost".into(),
            }
            .to_string())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let reason = "owner route lost; replay closes the same orphan";
    let first = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some(reason.into()),
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let receipt_id = first.data["receipt_id"].as_str().unwrap().to_owned();
    assert_eq!(first.data["cleanup"]["result"], "unverified");
    assert_eq!(
        first.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    let task_updated_ms = server.state.lock().unwrap().tasks["orphan"].updated_ms;
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let journal_after_first = std::fs::read_to_string(&journal_path).unwrap();
    *server.state.lock().unwrap() = replay(&root).unwrap();

    let second = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some(reason.into()),
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    assert_eq!(second.data["idempotent"], true);
    assert_eq!(second.data["receipt_id"], receipt_id);
    assert_eq!(second.data["cleanup"]["result"], "unverified");
    assert_eq!(
        second.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["orphan"].updated_ms,
        task_updated_ms
    );
    assert_eq!(
        std::fs::read_to_string(&journal_path).unwrap(),
        journal_after_first
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_manual_cleanup_receipt_replays_as_unverified() {
    let receipt: CleanupReceipt = serde_json::from_value(json!({
        "id": "cleanup-manual-legacy-1",
        "task_id": "legacy-task",
        "worktree_path": "playground/legacy-task",
        "branch": "codex/legacy-task",
        "verified_ms": 1,
        "manual_reason": "legacy force close without verified cleanup",
    }))
    .unwrap();
    assert_eq!(receipt.verification, CleanupVerification::Unverified);
}

#[test]
fn worktree_claim_requires_cleanup_and_cannot_cancel() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: Some("feature".into()),
            worktree_path: Some("playground/task-wt".into()),
            branch: Some("codex/task-wt".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let cancelled = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("cancelled".into()),
        None,
    );
    assert!(!cancelled.ok);
    assert_eq!(
        cancelled.error.as_deref(),
        Some(
            "CLEANUP_REQUIRED_BEFORE_CANCEL: task owns a worktree; close only after merged cleanup"
        )
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn merged_worktree_without_cleanup_receipt_fails_audit() {
    let (server, root) = test_server();
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "merged-task".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: None,
            worktree_path: Some("playground/merged-wt".into()),
            branch: Some("codex/merged-wt".into()),
            base_commit: None,
            priority: default_priority(),
            status: "merged".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let issues = migration_issues(&server, &server.state.lock().unwrap());
    assert!(issues
        .iter()
        .any(|issue| issue.starts_with("TASK_CLEANUP_INCOMPLETE:merged-task:")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn conflict_is_durable_and_wait_targets_resource_holder() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    let conflict = create_task(&server, "waiter", "waiting", "shared-feature");
    assert!(!conflict.ok);
    assert_eq!(conflict.error.as_deref(), Some("TASK_RESOURCE_CONFLICT"));
    assert_eq!(conflict.data["responsible_actor"], "holder");
    assert_eq!(server.state.lock().unwrap().msgs.len(), 0);
    assert_eq!(
        conflict.data["notification"],
        "none; use explicit sendmessage when coordination is needed"
    );

    let waiting = handle_task_wait(
        &server,
        "waiter".into(),
        "token-waiter".into(),
        "waiting".into(),
        "held".into(),
    );
    assert!(waiting.ok);
    let state = server.state.lock().unwrap();
    let wait = state.tasks["waiting"].wait.as_ref().unwrap();
    assert_eq!(wait.waiter, "waiter");
    assert_eq!(wait.responsible_actor, "holder");
    assert!(wait.deadline_ms > now_ms());
    assert!(wait.resume_on.contains(&"resource_released".into()));
    assert_eq!(wait.escalation, "resource_owner_and_waiter_recheck");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn holder_close_persists_release_only_for_waiter() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(&server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    assert!(
        handle_notification_subscribe(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "resource-released".into(),
            Some("held".into()),
            None,
            Vec::new(),
            None,
            1,
            60,
        )
        .ok
    );
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "holder".into(),
                "token-holder".into(),
                "held".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            Some("candidate verified".into()),
            Some("/tmp/holder-worktree".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            true,
            false,
            "reviewed candidate".into(),
        )
        .ok
    );
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );
    assert!(
        handle_task_close(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            false,
            None,
        )
        .ok
    );

    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "blocked");
    assert!(state.tasks["waiting"].wait.is_none());
    assert!(state.tasks["waiting"]
        .next_step
        .as_deref()
        .unwrap()
        .starts_with("RESOURCE_RELEASED=held"));
    let releases: Vec<&Message> = state
        .msgs
        .values()
        .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
        .collect();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].to, "waiter");
    assert!(state
        .msgs
        .values()
        .all(|message| !message.body.starts_with("TASK_CLOSED ")));
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn direct_two_peer_and_three_peer_wait_cycles_fail_closed() {
    let (server, root) = test_server();
    for (id, thread) in [("a", "%a"), ("b", "%b"), ("c", "%c")] {
        register(&server, id, thread);
    }
    assert!(create_task(&server, "a", "a-task", "a-feature").ok);
    let direct = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "a-task".into(),
    );
    assert!(!direct.ok);
    assert_eq!(direct.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));

    let now = now_ms();
    let make = |id: &str, owner: &str, waiting_for: Option<&str>| TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: Some("shared".into()),
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: default_priority(),
        status: if waiting_for.is_some() {
            "waiting"
        } else {
            "blocked"
        }
        .into(),
        next_step: None,
        wait: waiting_for.map(|blocking| WaitSpec {
            waiter: owner.into(),
            waiting_for: blocking.into(),
            responsible_actor: "a".into(),
            reason: "resource_conflict".into(),
            deadline_ms: now + 60_000,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        }),
        created_ms: now,
        updated_ms: now,
    };
    server.commit(&[
        Event::TaskUpdated {
            task: make("a-task", "a", None),
        },
        Event::TaskCreated {
            task: make("b-task", "b", Some("a-task")),
        },
    ]);
    let two = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert_eq!(two.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));

    server.commit(&[
        Event::TaskUpdated {
            task: make("b-task", "b", Some("c-task")),
        },
        Event::TaskCreated {
            task: make("c-task", "c", Some("a-task")),
        },
    ]);
    let three = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert_eq!(three.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn terminal_or_delivered_task_cannot_wait() {
    let (server, root) = test_server();
    register(&server, "a", "%a");
    register(&server, "b", "%b");
    assert!(create_task(&server, "a", "a-task", "a-feature").ok);
    assert!(create_task(&server, "b", "b-task", "b-feature").ok);
    server
        .state
        .lock()
        .unwrap()
        .tasks
        .get_mut("a-task")
        .unwrap()
        .status = "delivered".into();
    let response = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert!(!response.ok);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn peer_migration_freezes_snapshot_and_resumes_after_verify() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(handle_migration_inspect(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    let applied = handle_migration_apply(&server, "peer".into(), "token-peer".into());
    assert!(applied.ok);
    assert!(applied.data["admission_frozen"].as_bool().unwrap());
    let verified = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(verified.ok);
    assert!(verified.data["verified"].as_bool().unwrap());
    assert!(!server.state.lock().unwrap().admission_frozen());
    let repeated = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(repeated.ok);
    assert_eq!(repeated.data["verified"], true);
    assert_eq!(repeated.data["idempotent"], true);
    assert_eq!(repeated.data["resumed"], false);
    assert!(repeated.data["next"]
        .as_str()
        .unwrap()
        .contains("do not rerun"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_transaction_lease_rejects_second_peer() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%peer-a");
    register(&server, "peer-b", "%peer-b");
    assert!(handle_migration_plan(&server, "peer-a".into(), "token-peer-a".into()).ok);
    let second = handle_migration_plan(&server, "peer-b".into(), "token-peer-b".into());
    assert!(!second.ok);
    assert_eq!(
        second.error.as_deref(),
        Some("MIGRATION_TRANSACTION_HELD_BY_ANOTHER_PEER")
    );
    assert_eq!(second.data["holder"], "peer-a");
    assert_eq!(second.data["requester"], "peer-b");
    assert_eq!(second.data["retry_allowed"], false);
    assert!(second.data["next"]
        .as_str()
        .unwrap()
        .contains("do not retry"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_verify_rejection_exposes_current_state_and_stops_retry() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let response = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("no migration record to verify")
    );
    assert_eq!(response.data["retry_allowed"], false);
    assert!(response.data["next"].as_str().unwrap().contains("inspect"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_rejects_wait_without_matching_active_resource_holder() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(&server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    server
        .state
        .lock()
        .unwrap()
        .tasks
        .get_mut("held")
        .unwrap()
        .status = "closed".into();

    let inspected = handle_migration_inspect(&server, "waiter".into(), "token-waiter".into());
    assert!(inspected.ok);
    assert!(inspected.data["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue
            .as_str()
            .unwrap()
            .contains("inactive blocking task held")));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn changed_migration_snapshot_remains_frozen() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_apply(&server, "peer".into(), "token-peer".into()).ok);

    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "tampered".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: Some("tampered".into()),
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let verified = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(verified.ok);
    assert!(!verified.data["verified"].as_bool().unwrap());
    assert!(verified.data["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue.as_str().unwrap().contains("snapshot hash mismatch")));
    assert!(server.state.lock().unwrap().admission_frozen());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_freeze_rejects_mutations_but_allows_rebind_and_reads() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_apply(&server, "peer".into(), "token-peer".into()).ok);
    let server = Arc::new(server);

    let mutations = vec![
        Req::Send {
            from: "peer".into(),
            worker_id: Some("peer".into()),
            token: Some("token-peer".into()),
            command: Some(send_command(&root, "peer-freeze")),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("release".into()),
            body: "RESOURCE_RELEASED feature".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
        Req::Poll {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            timeout_ms: 1,
            receive_id: None,
        },
        Req::Ack {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            ids: vec!["message".into()],
        },
        Req::TaskUpdate {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            task_id: "task".into(),
            status: Some("verifying".into()),
            next_step: None,
        },
        Req::MigrationPlan {
            worker_id: "peer".into(),
            token: "token-peer".into(),
        },
        Req::MigrationApply {
            worker_id: "peer".into(),
            token: "token-peer".into(),
        },
    ];
    for request in mutations {
        let response = dispatch(&server, request);
        assert_eq!(
            response.error.as_deref(),
            Some(
                "MIGRATION_ADMISSION_FROZEN: only identity rebind, read queries, daemon restart, and migration verify are allowed"
            )
        );
    }

    let read = dispatch(
        &server,
        Req::TaskStatus {
            task_id: Some("task".into()),
        },
    );
    assert!(read.ok);
    let rebound = dispatch(
        &server,
        Req::Register {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            cwd: root.display().to_string(),
            candidates: Some(crate::proto::TransportCandidates {
                appserver: Some(test_appserver_candidate("thread-peer")),
            }),
        },
    );
    assert!(rebound.ok);
    let new_identity = dispatch(
        &server,
        Req::Register {
            worker_id: "new-peer".into(),
            token: "token-new-peer".into(),
            cwd: root.display().to_string(),
            candidates: Some(crate::proto::TransportCandidates {
                appserver: Some(test_appserver_candidate("thread-new-peer")),
            }),
        },
    );
    assert_eq!(
        new_identity.error.as_deref(),
        Some("MIGRATION_ADMISSION_FROZEN: only an existing App Server identity may rebind")
    );
    assert_eq!(server.state.lock().unwrap().workers.len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_uses_registered_cwd_for_authoritative_route_scope() {
    let (server, root) = test_server();
    let registered_cwd = root.clone();
    assert!(
        handle_register_with_app_scope(
            &server,
            "sender".into(),
            "token-sender".into(),
            registered_cwd.display().to_string(),
            Some(AppServerId::new("tui-default").unwrap()),
            Some(TransportCandidates {
                appserver: Some(test_appserver_candidate("thread-sender")),
            }),
        )
        .ok
    );
    assert!(register(&server, "recipient", "%recipient").ok);

    let mut request = authenticated_send(&root, "sender", "recipient", "scope");
    if let Req::Send {
        command: Some(command),
        ..
    } = &mut request
    {
        command.scope = crate::scope::RouteScope::for_registered_project(
            crate::identity::AppServerId::new("appserver-cli").unwrap(),
            &root,
        )
        .unwrap();
    }
    let response = dispatch(&Arc::new(server), request);
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("SEND_BINDING_REJECTED:")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_returns_typed_durability_failure_before_wake() {
    let (server, root) = test_server();
    assert!(register(&server, "sender", "%sender").ok);
    assert!(register(&server, "recipient", "%recipient").ok);
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();

    let response = dispatch(
        &Arc::new(server),
        authenticated_send(&root, "sender", "recipient", "durability"),
    );
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("SEND_DURABILITY_FAILED:")));
    assert!(response.error.as_deref().unwrap().contains("journal"));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn duplicate_daemon_rejection_preserves_authoritative_pid() {
    let _startup_test_lock = startup_test_lock();
    let root = PathBuf::from(format!(
        "/tmp/collab-sd-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let scope = Scope { root: root.clone() };
    let first = tokio::spawn(run(Scope { root: root.clone() }));
    for _ in 0..100 {
        if scope.sock_path().exists() && scope.server_dir().join("server.pid").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(scope.sock_path().exists());
    let pid_path = scope.server_dir().join("server.pid");
    let authoritative_pid = std::fs::read_to_string(&pid_path).unwrap();

    let error = run(Scope { root: root.clone() })
        .await
        .err()
        .expect("second daemon must be rejected");
    assert!(error.to_string().contains("server already running"));
    assert_eq!(
        std::fs::read_to_string(&pid_path).unwrap(),
        authoritative_pid
    );

    first.abort();
    let _ = first.await;
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn long_polls_do_not_starve_ping_on_the_blocking_pool() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;
        use tokio::time::{timeout, Duration};

        let (server, root) = test_server();
        register(&server, "peer", "%peer");
        let server = Arc::new(server);
        let mut poll_clients = Vec::new();
        let mut poll_tasks = Vec::new();

        for _ in 0..8 {
            let (mut client, server_stream) = UnixStream::pair().unwrap();
            poll_tasks.push(tokio::spawn(conn_task(server.clone(), server_stream)));
            let request = serde_json::to_string(&Req::Poll {
                worker_id: "peer".into(),
                token: "token-peer".into(),
                timeout_ms: 10_000,
                receive_id: None,
            })
            .unwrap();
            client.write_all(request.as_bytes()).await.unwrap();
            client.write_all(b"\n").await.unwrap();
            poll_clients.push(client);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut ping_client, server_stream) = UnixStream::pair().unwrap();
        let ping_task = tokio::spawn(conn_task(server.clone(), server_stream));
        let request = serde_json::to_string(&Req::Ping).unwrap();
        ping_client.write_all(request.as_bytes()).await.unwrap();
        ping_client.write_all(b"\n").await.unwrap();
        let mut response = String::new();
        timeout(
            Duration::from_secs(1),
            BufReader::new(&mut ping_client).read_line(&mut response),
        )
        .await
        .expect("Ping must not wait behind long Poll requests")
        .unwrap();
        let response: Resp = serde_json::from_str(response.trim()).unwrap();
        assert!(response.ok);

        ping_task.abort();
        for task in poll_tasks {
            task.abort();
        }
        drop(poll_clients);
        std::fs::remove_dir_all(root).ok();
    });
}

#[tokio::test]
async fn poll_wakes_when_a_message_is_committed() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let server = Arc::new(server);
    let poll = tokio::spawn(handle_poll_async(server.clone(), "peer".into(), 5_000));

    tokio::time::sleep(Duration::from_millis(10)).await;
    server.commit(&[Event::Sent {
        msg: Message {
            id: "wake-message".into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("wake".into()),
            body: "message".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let response = tokio::time::timeout(Duration::from_secs(1), poll)
        .await
        .expect("Poll must wake after a durable message commit")
        .unwrap();
    assert!(response.ok);
    assert_eq!(response.data["count"], 1);
    assert_eq!(response.data["messages"][0]["id"], "wake-message");
    assert_eq!(
        server.state.lock().unwrap().msgs["wake-message"].state,
        "read"
    );
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn recv_consumes_messages_without_a_follow_up_ack() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    server.commit(&[Event::Sent {
        msg: Message {
            id: "recv-message".into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("recv".into()),
            body: "message".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let server = Arc::new(server);
    let response = handle_poll_async(server.clone(), "peer".into(), 100).await;
    assert!(response.ok);
    assert_eq!(response.data["count"], 1);
    assert_eq!(
        server.state.lock().unwrap().msgs["recv-message"].state,
        "read"
    );
    std::fs::remove_dir_all(root).ok();
}

fn seeded_receive_peer(server: &Server, root: &Path, id: &str, worker: &str) -> String {
    let _ = root;
    register(server, worker, "%peer");
    server.commit(&[Event::Sent {
        msg: Message {
            id: id.into(),
            from: "sender".into(),
            to: worker.into(),
            mtype: "notify".into(),
            subject: Some("receive".into()),
            body: format!("body-{id}"),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    root.display().to_string()
}

#[tokio::test]
async fn receive_id_commits_the_batch_and_replays_it_after_a_lost_response() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-loss-message", "peer");
    let server = Arc::new(server);

    // First poll carries a caller-owned identity and commits the batch. The
    // caller never sees this response: it simulates a lost socket reply.
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-loss-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");
    assert_eq!(first.data["receive_id"], "receive-loss-1");
    assert_eq!(first.data["replayed"], false);
    assert_eq!(first.data["messages"][0]["id"], "receive-loss-message");
    assert_eq!(
        server.state.lock().unwrap().msgs["receive-loss-message"].state,
        "read",
        "the receive identity commits the consumption in the same transaction"
    );

    // The same identity must return the exact committed batch, not an empty
    // inbox, so a lost response is recoverable by the same caller.
    let replayed = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-loss-1".into()),
    )
    .await;
    assert!(replayed.ok, "{replayed:?}");
    assert_eq!(replayed.data["replayed"], true);
    assert_eq!(replayed.data["count"], 1);
    assert_eq!(replayed.data["messages"][0]["id"], "receive-loss-message");
    assert_eq!(
        replayed.data["messages"][0]["body"],
        "body-receive-loss-message"
    );
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn receive_id_replay_survives_a_truncated_journal_restart() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-restart-message", "peer");
    let server = Arc::new(server);
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-restart-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");
    assert_eq!(first.data["messages"][0]["id"], "receive-restart-message");
    drop(server);

    // A fresh reducer replayed from the committed journal must still answer
    // the same identity with the same batch instead of an unread inbox.
    let restored = replay(&root).expect("replay committed journal");
    let receipt = restored
        .receive_receipts
        .get("receive-restart-1")
        .expect("receive receipt must be durable");
    assert_eq!(receipt.worker_id, "peer");
    assert_eq!(
        receipt.message_ids,
        vec!["receive-restart-message".to_owned()]
    );
    assert!(restored.msgs["receive-restart-message"].state == "read");
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn receive_id_rejects_another_actor_and_another_route() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-owner-message", "peer");
    register(&server, "other", "%other");
    let server = Arc::new(server);
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-owner-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");

    let foreign = handle_poll_async_with_context(
        server.clone(),
        "other".into(),
        None,
        0,
        None,
        Some("receive-owner-1".into()),
    )
    .await;
    assert!(!foreign.ok, "{foreign:?}");
    assert!(
        foreign
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RECEIVE_IDENTITY_MISMATCH")),
        "{foreign:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_sent_event_replay_classifies_message_without_reply_reference() {
    let (_server, root) = test_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(
        &journal_path,
        r#"{"ev":"Sent","msg":{"id":"legacy-message","from":"peer-a","to":"peer-b","type":"request","subject":"legacy","body":"legacy body","created_ms":1,"state":"pending"}}
"#,
    )
    .unwrap();

    let state = replay(&root).expect("legacy Sent event must replay");
    let message = state
        .msgs
        .get("legacy-message")
        .expect("replay must retain the legacy message");
    assert_eq!(message.in_reply_to, None);
    assert_eq!(message.mtype, "request");
    assert_eq!(
        state
            .inbox_of("peer-b")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy-message"]
    );
    assert!(!state.answered("legacy-message"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_journal_replay_fails_fast() {
    let (_server, root) = test_server();
    std::fs::write(
        root.join(".agent-collab/server/journal.jsonl"),
        "{manual-edit\n",
    )
    .unwrap();
    let error = replay(&root).err().expect("malformed journal must fail");
    assert!(error
        .to_string()
        .contains("manual journal edits are unsupported"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn concatenated_journal_events_replay_and_self_heal() {
    let (_server, root) = test_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let event1 = json!({"ev":"KeepaliveUpdated","worker_id":"w1","record":{"observed":"unknown","idle_since_ms":100,"activity_ms":50,"last_notice_ms":0,"last_notice_id":null,"unacked":0,"suspected_offline":false}}).to_string();
    let event2 = json!({"ev":"KeepaliveUpdated","worker_id":"w2","record":{"observed":"unknown","idle_since_ms":200,"activity_ms":150,"last_notice_ms":0,"last_notice_id":null,"unacked":0,"suspected_offline":false}}).to_string();
    std::fs::write(&journal_path, format!("{}{}\n", event1, event2)).unwrap();

    let state = replay(&root).expect("concatenated journal events must self-heal and replay");
    assert_eq!(state.keepalives.len(), 2);
    assert_eq!(state.keepalives["w1"].idle_since_ms, 100);
    assert_eq!(state.keepalives["w2"].idle_since_ms, 200);

    let content = std::fs::read_to_string(&journal_path).unwrap();
    let lines: Vec<_> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn worktree_path_budget_accepts_short_slug_and_rejects_escape() {
    let root = std::env::temp_dir().join(format!(
        "collab-worktree-path-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(root.join("playground")).unwrap();
    assert!(validate_worktree_path(&root, "./playground/ar03-0828").is_ok());
    assert!(validate_worktree_path(
        &root,
        "./playground/v3-direct-sse-terminal-observability-20260827-long-run-id"
    )
    .is_err());
    assert!(validate_worktree_path(&root, "./playground/../outside").is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp", root.join("playground/link")).unwrap();
        assert!(validate_worktree_path(&root, "./playground/link/escape").is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn appserver_notification_contains_id_subject_and_original_body() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("release".into()),
        body: "RESOURCE_RELEASED feature=shared".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();
    assert_eq!(
        text,
        "COLLAB_NOTIFY message-id [release] RESOURCE_RELEASED feature=shared | P1 ACTION: the resource is free; resume the task that waited on it. Details: collab msg message-id. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work."
    );
    assert!(!text.contains("ACK this notice"));
    assert!(text.contains("READ IS NOT DONE"));
}

#[test]
fn appserver_notification_classifies_priority_and_names_one_action() {
    let message = |from: &str, mtype: &str, subject: &str| {
        notification_text(&Message {
            id: "m1".into(),
            from: from.into(),
            to: "master".into(),
            mtype: mtype.into(),
            subject: Some(subject.into()),
            body: "body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        })
        .unwrap()
    };
    let notify = |subject: &str| message("peer", "notify", subject);
    let internal = |subject: &str| message("collab-server", "notification", subject);

    assert!(notify("worker-idle: w1").contains("P1 ACTION: dispatch work to this idle capacity"));
    assert!(notify("master-idle: master").contains("P1 ACTION: run the scheduling pass"));
    assert!(notify("worker-unresponsive: w1").contains("P1 ACTION: snapshot the thread"));
    assert!(notify("task-keepalive 1/3").contains("P1 ACTION: continue your own task"));
    assert!(notify("blocker:task").contains("P1 ACTION:"));
    assert!(notify("unblock:task").contains("P1 ACTION:"));
    assert!(notify("wait-timeout:task").contains("P1 ACTION:"));
    assert!(notify("scheduling-blocker: queue stalled").contains("P1 ACTION:"));
    assert!(compose_notification("batch", "notification-batch", "body").contains("P1 ACTION:"));
    assert!(
        notify("goal:plan.md").contains("P1 ACTION: do the in-scope action the message asks for")
    );
    assert!(notify("goal:<id>").contains("P1 ACTION:"));
    assert!(notify("deadline:<id>").contains("P1 ACTION:"));
    assert!(!notify("goal:plan.md").contains("P0 ACTION:"));
    assert!(!notify("deadline:<id>").contains("P0 ACTION:"));
    let goal = internal("goal:plan.md");
    assert!(goal.starts_with("COLLAB_NOTIFY m1 [goal:plan.md] body | P0 ACTION:"));
    assert!(goal.contains("P0 ACTION: run the long-horizon briefing"));
    assert!(internal("goal:<id>").contains("P0 ACTION: run the long-horizon briefing"));
    assert!(internal("deadline:<id>").contains("P0 ACTION: run the long-horizon briefing"));
    assert!(!internal("goal:plan.md").contains("P1 ACTION:"));
    assert!(!internal("deadline:<id>").contains("P1 ACTION:"));
    assert!(message("peer", "notification", "goal:plan.md").contains("P1 ACTION:"));
    assert!(message("peer", "notification", "deadline:<id>").contains("P1 ACTION:"));
    assert!(message("collab-server", "notify", "goal:plan.md").contains("P1 ACTION:"));
    assert!(message("collab-server", "notify", "deadline:<id>").contains("P1 ACTION:"));
    assert!(internal("goal").contains("P1 ACTION:"));
    assert!(internal("deadline").contains("P1 ACTION:"));
    assert!(notify("goalpost: reached").contains("P1 ACTION:"));
    assert!(notify("deadline-notice: task due").contains("P1 ACTION:"));
    assert!(!notify("goalpost: reached").contains("P0 ACTION:"));
    assert!(!notify("deadline-notice: task due").contains("P0 ACTION:"));
    assert!(notify("Settings delivery recorded").contains("P2 ACTION: note it"));

    // Every class carries the resume protocol, not just the operational ones.
    for subject in [
        "worker-idle: w1",
        "goal:plan.md",
        "Settings delivery recorded",
    ] {
        assert!(notify(subject).contains("resume your current task"));
    }
    assert!(internal("goal:plan.md").contains("resume your current task"));
}

#[test]
fn appserver_notification_truncates_body_without_dropping_the_action_contract() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("release".into()),
        body: "x".repeat(4000),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();

    assert!(text.chars().count() <= 1024);
    assert!(text.contains("P1 ACTION:"));
    assert!(text.contains("READ IS NOT DONE"));
    assert!(text.ends_with("run `appsdk longhorizon show` and take work."));
}

#[test]
fn appserver_notification_abbreviates_subject_and_escapes_body_controls() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some(
            "this subject is deliberately longer than forty eight visible characters".into(),
        ),
        body: "line one\nline two\t中文".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();
    assert_eq!(
        text,
        "COLLAB_NOTIFY message-id [this subject is deliberately longer than forty …] line one\\nline two\\t中文 | P1 ACTION: do the in-scope action the message asks for. Details: collab msg message-id. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work."
    );
}

#[test]
fn appserver_notification_long_goal_deadline_subjects_keep_the_typed_prefix() {
    for prefix in ["goal:", "deadline:"] {
        let subject = format!("{prefix}{}", "x".repeat(80));
        let text = notification_text(&Message {
            id: "message-id".into(),
            from: "collab-server".into(),
            to: "master".into(),
            mtype: "notification".into(),
            subject: Some(subject),
            body: "body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        })
        .unwrap();
        assert!(
            text.starts_with(&format!("COLLAB_NOTIFY message-id [{prefix}")),
            "{text}"
        );
        assert!(text.contains("] body | P0 ACTION: run the long-horizon briefing"));
    }
}

#[test]
fn authenticated_send_cannot_claim_internal_goal_deadline_owner() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    assert!(register(&server, "sender", "%sender").ok);
    assert!(register(&server, "recipient", "%recipient").ok);

    let mut request = authenticated_send(&root, "sender", "recipient", "goal:plan.md");
    if let Req::Send { mtype, .. } = &mut request {
        *mtype = "notification".into();
    }
    let response = dispatch(&server, request);
    assert_eq!(
        response.error.as_deref(),
        Some("peer messaging requires type notify")
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sendmessage_requires_subject_before_state_mutation() {
    let (server, root) = test_server();
    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        None,
        "The candidate is ready.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("MESSAGE_SUBJECT_REQUIRED: sendmessage requires --subject")
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn send_without_subscription_is_mailbox_only_and_deduplicated() {
    let (server, root) = test_server();
    register(&server, "sender", "%collab-missing-sender");
    register(&server, "recipient", "%collab-missing-recipient");
    let subscription_id = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.worker_id == "recipient")
        .unwrap()
        .id
        .clone();
    server.commit(&[Event::NotificationStatus {
        subscription_id,
        status: "cancelled".into(),
        updated_ms: now_ms(),
    }]);
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("occupied".into()),
        "RESOURCE_OCCUPIED feature=shared".into(),
        None,
        "immediate".into(),
    );
    assert!(first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    assert_eq!(server.state.lock().unwrap().msgs.len(), 1);
    assert!(root
        .join(".agent-collab/mailbox")
        .join(format!("{message_id}.json"))
        .exists());
    let jsonl = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    assert!(std::fs::read_to_string(&jsonl)
        .unwrap()
        .lines()
        .any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .map(|record| {
                    record["schema_version"] == 1
                        && record["record_type"] == "message"
                        && record["recipient"] == "recipient"
                        && record["category"] == "direct"
                        && record["task_ids"].is_array()
                        && record["created_ms"].is_i64()
                        && record["window_start_ms"].is_null()
                        && record["window_end_ms"].is_null()
                        && record["state"] == "pending"
                        && record["exact_error"].as_str().is_some_and(|error| {
                            error.starts_with("MAILBOX_SCOPE_BINDING_UNAVAILABLE:")
                        })
                        && record["message"]["id"] == message_id
                })
                .unwrap_or(false)
        }));
    assert_eq!(
        replay(&root).unwrap().msgs[&message_id].body,
        "RESOURCE_OCCUPIED feature=shared"
    );
    assert_eq!(first.data["notification"], "mailbox-only-no-subscription");
    assert_eq!(
        server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
        0
    );
    assert!(!server.log_path().exists());

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("occupied".into()),
        "RESOURCE_OCCUPIED feature=shared".into(),
        None,
        "immediate".into(),
    );
    assert!(duplicate.ok);
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(duplicate.data["deduplicated"], true);
    assert_eq!(server.state.lock().unwrap().msgs.len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_peer_notification_accepts_arbitrary_durable_body() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok);
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.msgs[message_id].body,
        "The candidate is ready for your review."
    );
    assert_eq!(state.msgs[message_id].subject.as_deref(), Some("review"));
    assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
    assert_eq!(response.data["notification"], "sent");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_reports_appserver_rejection_after_durable_commit() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _| {
        Err(
            "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
                .into(),
        )
    });

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some(
            "APPSERVER_NOTIFICATION_REJECTED: ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
        )
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    assert_eq!(
        response.data["notification_error"],
        "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
    );
    assert_eq!(response.data["failure"], "notification_delivery_failed");
    assert_eq!(response.data["repair_required"], true);
    assert!(response.data["escalation"]
        .as_str()
        .unwrap()
        .contains("live master"));
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
    assert_eq!(state.msgs[message_id].state, "pending");
    assert_eq!(
        state.notification_delivery_failures[message_id].error,
        "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
    );
    drop(state);
    let replayed = replay(&root).unwrap();
    let failure = &replayed.notification_delivery_failures[message_id];
    assert_eq!(failure.operation, "notification.emitted");
    assert_eq!(
        failure.error,
        "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
    );
    assert!(failure.failed_ms > 0);

    {
        let state = server.state.lock().unwrap();
        server.rewrite_journal_locked(&state).unwrap();
    }
    let compacted = replay(&root).unwrap();
    assert_eq!(
        compacted.notification_delivery_failures[message_id],
        *failure
    );
    assert!(root
        .join(".agent-collab/mailbox")
        .join(format!("{message_id}.json"))
        .exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_duplicate_after_rejection_retries_the_same_message_once() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let attempts = Arc::new(AtomicU32::new(0));
    let attempts_for_sink = Arc::clone(&attempts);
    server.appserver_notification_sink = Arc::new(move |_, _, _, _, _| {
        let attempt = attempts_for_sink.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            Err(
                "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
                    .into(),
            )
        } else {
            Ok(json!({"accepted": true}))
        }
    });

    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert!(!state.msgs[&message_id].retry_attempted);
        assert!(state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(duplicate.ok, "{duplicate:?}");
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(duplicate.data["durable"], true);
    assert_eq!(duplicate.data["deduplicated"], true);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 2);
        assert!(state.msgs[&message_id].retry_attempted);
    }

    let third = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!third.ok);
    assert_eq!(third.data["msg_id"], message_id);
    assert_eq!(
        third.error.as_deref(),
        Some("APPSERVER_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs[&message_id].wake_attempt_count, 2);
    assert!(replayed.msgs[&message_id].retry_attempted);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_an_accepted_original_attempt() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "accepted once".into(),
        None,
        "immediate".into(),
    );
    assert!(first.ok, "{first:?}");
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert!(!state.msgs[&message_id].retry_attempted);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "accepted once".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok, "{duplicate:?}");
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("APPSERVER_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_an_unknown_original_attempt() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink =
        Arc::new(|_, _, _, _, _| Err("ADAPTER_TIMEOUT: turn/start timed out".into()));
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "unknown outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(!state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "unknown outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("APPSERVER_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_a_decode_failure() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _| {
        Err("ADAPTER_UNKNOWN: decode response: missing result payload".into())
    });
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "undecodable outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(!state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "undecodable outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("APPSERVER_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn reregistration_does_not_overwrite_a_stale_default_target() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    {
        let mut state = server.state.lock().unwrap();
        state
            .notification_subscriptions
            .get_mut("sub-default-direct-message-peer")
            .unwrap()
            .target = "thread-stale".into();
    }
    assert!(register(&server, "peer", "%peer").ok);
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"].target,
        "thread-stale"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_reports_when_subscription_transport_mismatches() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("recipient")
        .unwrap()
        .transport = Some(test_appserver_transport("mismatched-thread-recipient"));

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some(
            "APPSERVER_NOTIFICATION_REJECTED: subscription does not match the selected App Server transport"
        )
    );
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    assert_eq!(
        response.data["notification_error"],
        "subscription does not match the selected App Server transport"
    );
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        let failure = &state.notification_delivery_failures[message_id];
        assert_eq!(failure.operation, "notification.not_attempted");
        assert_eq!(
            failure.error,
            "subscription does not match the selected App Server transport"
        );
    }
    assert_eq!(
        replay(&root).unwrap().notification_delivery_failures[message_id].error,
        "subscription does not match the selected App Server transport"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_records_latest_delivery_and_journal_replay() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let id = "jsonl-message";
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: id.into(),
                from: "sender".into(),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("progress".into()),
                body: "task-jsonl progress".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Delivered {
            ids: vec![id.into()],
        },
        Event::Acked {
            ids: vec![id.into()],
        },
    ]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    let records = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["message"]["id"] == id)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3, "Sent/Delivered/Acked append snapshots");
    for record in &records {
        assert_eq!(record["schema_version"], 1);
        assert_eq!(record["record_type"], "message");
        assert_eq!(record["recipient"], "recipient");
        assert_eq!(record["category"], "progress");
        assert!(record["task_ids"].is_array());
        assert!(record["created_ms"].is_i64());
        assert!(record["window_start_ms"].is_null());
        assert!(record["window_end_ms"].is_null());
        assert!(record["exact_error"]
            .as_str()
            .is_some_and(|error| { error.starts_with("MAILBOX_SCOPE_BINDING_UNAVAILABLE:") }));
    }
    assert_eq!(records.last().unwrap()["message"]["state"], "read");
    assert_eq!(replay(&root).unwrap().msgs[id].state, "read");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::fs::write(
        &path,
        format!("{}{{\"partial\":", std::fs::read_to_string(&path).unwrap()),
    )
    .unwrap();
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 3);
    assert!(projection.partial_tail);
    assert!(projection.unterminated_tail);
    std::fs::write(&path, "{\"bad\":true}\nnot-json\n").unwrap();
    assert!(read_recipient_mailbox(&path, "recipient").is_err());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_failure_is_logged_without_panicking() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    std::fs::create_dir_all(root.join(".agent-collab/mailbox/recipient-recipient.jsonl")).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "jsonl-failure".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("failure".into()),
            body: "preserve journal truth".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/log.txt"))
            .unwrap()
            .contains("MAILBOX_JSONL_WRITE_FAILED")
    );
    assert_eq!(
        replay(&root).unwrap().msgs["jsonl-failure"].state,
        "pending"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_accepts_legacy_bare_message_before_new_append() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = Message {
        id: "legacy-message".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("progress".into()),
        body: "legacy record".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&legacy).unwrap()),
    )
    .unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "new-message".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert_eq!(projection.records[0]["schema_version"], 1);
    assert_eq!(projection.records[0]["window_source"], "legacy-message");
    assert_eq!(projection.records[1]["message"]["id"], "new-message");
    assert_eq!(replay(&root).unwrap().msgs["new-message"].state, "pending");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_separates_complete_unterminated_legacy_record() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = Message {
        id: "legacy-without-newline".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("progress".into()),
        body: "complete record without separator".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-legacy-without-newline".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert!(!projection.partial_tail);
    assert!(!projection.unterminated_tail);
    assert_eq!(
        projection.records[0]["message"]["id"],
        "legacy-without-newline"
    );
    assert_eq!(
        projection.records[1]["message"]["id"],
        "after-legacy-without-newline"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn partial_recipient_tail_is_repaired_before_append_and_replay_preserves_assignment() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "partial-assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("partial-mailbox".into()),
            worktree_path: None,
            branch: None,
            base_commit: Some("partial-base".into()),
            priority: "p0".into(),
            status: "working".into(),
            next_step: Some("replay after restart".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "before-partial".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "before partial tail".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("{\"partial\":");
    std::fs::write(&path, content).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-partial".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "after partial tail".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert!(!projection.partial_tail);
    assert!(!projection.unterminated_tail);
    assert_eq!(projection.records[0]["message"]["id"], "before-partial");
    assert_eq!(projection.records[1]["message"]["id"], "after-partial");
    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs["before-partial"].state, "pending");
    assert_eq!(replayed.msgs["after-partial"].state, "pending");
    assert_eq!(replayed.tasks["partial-assigned-task"].owner, "recipient");
    assert_eq!(
        replayed.tasks["partial-assigned-task"]
            .feature_id
            .as_deref(),
        Some("partial-mailbox")
    );
    assert_eq!(
        replayed.tasks["partial-assigned-task"].next_step.as_deref(),
        Some("replay after restart")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_recipient_jsonl_does_not_block_future_append_or_journal_replay() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("mailbox-envelope".into()),
            worktree_path: None,
            branch: None,
            base_commit: Some("base-commit".into()),
            priority: "p0".into(),
            status: "working".into(),
            next_step: Some("consume mailbox".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"bad\":true}\n").unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "journal remains authoritative".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.lines().any(|line| line.contains("after-malformed")));
    assert_eq!(content.matches("{\"bad\":true}").count(), 1);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 1);
    assert_eq!(projection.records[0]["message"]["id"], "after-malformed");
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/log.txt"))
            .unwrap()
            .contains("MAILBOX_JSONL_RECOVERABLE")
    );
    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs["after-malformed"].state, "pending");
    let assignment = &replayed.tasks["assigned-task"];
    assert_eq!(assignment.owner, "recipient");
    assert_eq!(assignment.feature_id.as_deref(), Some("mailbox-envelope"));
    assert_eq!(assignment.base_commit.as_deref(), Some("base-commit"));
    assert_eq!(assignment.status, "working");
    assert_eq!(assignment.next_step.as_deref(), Some("consume mailbox"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn interior_malformed_recipient_jsonl_preserves_bad_line_and_appends_later_message() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "interior-assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("interior-mailbox-recovery".into()),
            worktree_path: Some("./playground/interior-mailbox-recovery".into()),
            branch: Some("codex/interior-mailbox-recovery".into()),
            base_commit: Some("interior-base".into()),
            priority: "p1".into(),
            status: "working".into(),
            next_step: Some("consume preserved mailbox".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);

    server.commit(&[Event::Sent {
        msg: Message {
            id: "before-interior-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "before malformed interior record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let malformed_line = "not-json-interior-record-preserve-this-line";
    server.commit(&[Event::Sent {
        msg: Message {
            id: "later-valid-record".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "later valid record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let content = std::fs::read_to_string(&path).unwrap();
    let (first, remaining) = content.split_once('\n').unwrap();
    std::fs::write(&path, format!("{first}\n{malformed_line}\n{remaining}")).unwrap();

    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-interior-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new append after malformed interior record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.lines().any(|line| line == malformed_line));
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(
        projection
            .records
            .iter()
            .filter_map(|record| record["message"]["id"].as_str())
            .collect::<Vec<_>>(),
        vec![
            "before-interior-malformed",
            "later-valid-record",
            "after-interior-malformed"
        ]
    );
    assert_eq!(projection.recoverable_errors.len(), 1);
    assert!(projection.recoverable_errors[0].contains("record 2"));

    let server = Arc::new(server);
    let response = dispatch(
        &server,
        Req::MailboxRead {
            all: false,
            sort: Some("time-asc".into()),
            worker_id: Some("recipient".into()),
        },
    );
    assert!(response.ok);
    assert_eq!(
        response.data["recipient_jsonl"]["status"],
        "recoverable-error"
    );
    assert!(response.data["recipient_jsonl"]["exact_error"]
        .as_str()
        .unwrap()
        .contains("record 2"));
    assert_eq!(
        response.data["recipient_jsonl"]["records"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(
        replayed.msgs["later-valid-record"].body,
        "later valid record"
    );
    assert_eq!(
        replayed.msgs["after-interior-malformed"].body,
        "new append after malformed interior record"
    );
    let assignment = &replayed.tasks["interior-assigned-task"];
    assert_eq!(assignment.owner, "recipient");
    assert_eq!(
        assignment.feature_id.as_deref(),
        Some("interior-mailbox-recovery")
    );
    assert_eq!(
        assignment.worktree_path.as_deref(),
        Some("./playground/interior-mailbox-recovery")
    );
    assert_eq!(
        assignment.branch.as_deref(),
        Some("codex/interior-mailbox-recovery")
    );
    assert_eq!(assignment.base_commit.as_deref(), Some("interior-base"));
    assert_eq!(assignment.status, "working");
    assert_eq!(
        assignment.next_step.as_deref(),
        Some("consume preserved mailbox")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn removed_role_and_dispatch_commands_fail_fast() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(
        handle_task_dispatch(&server, "peer".into(), "token-peer".into())
            .error
            .unwrap()
            .contains("deprecated")
    );
    assert!(handle_task_claim(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-task".into(),
    )
    .error
    .unwrap()
    .contains("deprecated"));
    let server = Arc::new(server);
    for request in [
        Req::Role {
            worker_id: "peer".into(),
        },
        Req::TransferMaster {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            target_id: "peer".into(),
        },
    ] {
        assert!(!dispatch(&server, request).ok);
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn lifecycle_cannot_bypass_delivery_or_review() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(create_task(&server, "peer", "blocked-task", "blocked-feature").ok);
    assert!(create_task(&server, "peer", "working-task", "working-feature").ok);
    let early_review = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "not delivered".into(),
    );
    assert!(!early_review.ok);
    assert_eq!(
        early_review.error.as_deref(),
        Some("task task must be delivered before review (current: working)")
    );
    let missing_evidence_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        None,
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        missing_evidence_delivery.error.as_deref(),
        Some("task deliver requires non-empty --evidence")
    );
    let working_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        Some("working task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(working_delivery.ok, "{working_delivery:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["working-task"].status,
        "delivered"
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "blocked-task".into(),
            Some("blocked".into()),
            None,
        )
        .ok
    );
    let blocked_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "blocked-task".into(),
        Some("blocked task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        blocked_delivery.error.as_deref(),
        Some(
            "task blocked-task must be owned and working, verifying, reviewed, or rework before delivery (current: blocked)"
        )
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    let skipped_delivery = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        skipped_delivery.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "verifying"
    );
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let accepted = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_accepted_candidate_can_record_merge() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let merged = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        Some("merged".into()),
        Some("integration recorded".into()),
    );
    assert!(
        merged.ok,
        "legacy accepted task must be mergeable: {merged:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    assert_eq!(
        replay(&root).unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn merged_task_without_lifecycle_edges_cannot_close() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "accepted-task".into(),
            Some("merged".into()),
            Some("legacy merge compatibility".into()),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        false,
        None,
    );
    assert_eq!(
        closed.error.as_deref(),
        Some("task accepted-task cannot close before delivery, review, and integration evidence")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_the_main_tip() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let head = current_head(&root);

    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        head.clone(),
        "main tip integration".into(),
    );
    assert!(integrated.ok, "{integrated:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    assert_eq!(
        state.task_lifecycle["task"].integration_commit.as_deref(),
        Some(head.as_str())
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_a_real_merge_commit_and_the_merged_candidate() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);

    git_ok(&root, &["checkout", "-q", "-b", "candidate"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(
        &root,
        &[
            "merge",
            "--no-ff",
            "-q",
            "candidate",
            "-m",
            "merge candidate",
        ],
    );
    let merge_commit = current_head(&root);
    assert_ne!(candidate, merge_commit);
    // Main keeps moving after the merge, so neither the merge commit nor the
    // candidate is the current main tip when integration is recorded.
    git_ok(
        &root,
        &["commit", "--allow-empty", "-q", "-m", "main moves on"],
    );
    let main_tip = current_head(&root);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_tip);
    assert_ne!(main_tip, merge_commit);

    let via_merge = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        merge_commit.clone(),
        "merge commit integration".into(),
    );
    assert!(via_merge.ok, "{via_merge:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(merge_commit.as_str())
    );

    // The same accepted task can be re-recorded with the candidate SHA that
    // the merge brought in, because it is reachable from refs/heads/main.
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let via_candidate = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        candidate.clone(),
        "candidate SHA integration".into(),
    );
    assert!(via_candidate.ok, "{via_candidate:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(candidate.as_str())
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_rejects_a_commit_not_reachable_from_main_with_an_actionable_error() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "--orphan", "orphan"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "orphan"]);
    let orphan = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);

    let rejected = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        orphan.clone(),
        "orphan integration".into(),
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert_eq!(rejected.data["provided"], orphan);
    assert_eq!(rejected.data["main_head"], main_head);
    assert!(
        rejected.data["expected"]
            .as_str()
            .unwrap_or_default()
            .contains("reachable from refs/heads/main"),
        "expected value must name the reachability contract: {rejected:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_is_correct_when_root_is_not_checked_out_on_main() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "-b", "topic"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "topic"]);
    let topic_head = current_head(&root);
    assert_ne!(topic_head, main_head);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_head);

    // The recorded commit is a real ancestor of main, so reachability must be
    // judged from refs/heads/main rather than from the current checkout.
    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        topic_head.clone(),
        "integration judged from refs/heads/main".into(),
    );
    assert!(
        !integrated.ok,
        "a topic-only commit must not be accepted: {integrated:?}"
    );
    assert_eq!(
        integrated.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert!(integrated.data["expected"]
        .as_str()
        .unwrap_or_default()
        .contains("reachable from refs/heads/main"));
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );

    let merged = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        main_head.clone(),
        "main ancestor accepted while checked out on topic".into(),
    );
    assert!(merged.ok, "{merged:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(main_head.as_str())
    );
    // The checkout must not be moved by the integration record.
    assert_eq!(current_head(&root), topic_head);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn delivery_review_and_exact_main_integration_are_durable() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "outsider", "%outsider");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("reviewed".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("candidate".into()),
        )
        .ok
    );
    let denied = handle_task_review(
        &server,
        "outsider".into(),
        "token-outsider".into(),
        "task".into(),
        true,
        false,
        "outsider review".into(),
    );
    assert!(!denied.ok);
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "review gates passed".into(),
        )
        .ok
    );

    for args in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Collab Test"].as_slice(),
        ["commit", "--allow-empty", "-q", "-m", "main"].as_slice(),
        ["branch", "-M", "main"].as_slice(),
    ] {
        assert!(Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let head = String::from_utf8(
        Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            head,
            "main integration verified".into(),
        )
        .ok
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    let lifecycle = &state.task_lifecycle["task"];
    assert_eq!(
        lifecycle.delivery_evidence.as_deref(),
        Some("candidate commit and gates passed")
    );
    assert_eq!(lifecycle.reviewer.as_deref(), Some("owner"));
    assert_eq!(
        lifecycle.integration_evidence.as_deref(),
        Some("main integration verified")
    );
    drop(state);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.tasks["task"].status, "merged");
    assert_eq!(
        replayed.task_lifecycle["task"]
            .integration_evidence
            .as_deref(),
        Some("main integration verified")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accepted_task_can_return_to_rework_and_redeliver() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("first candidate".into()),
            Some("candidate".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "accepted".into(),
        )
        .ok
    );
    let direct_verifying = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("verifying".into()),
        None,
    );
    assert_eq!(
        direct_verifying.error.as_deref(),
        Some("invalid task transition accepted -> verifying")
    );
    let direct_merge = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        direct_merge.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("rework".into()),
            Some("address review findings".into()),
        )
        .ok
    );
    for status in ["working", "verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let redelivery = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("corrected candidate".into()),
        Some("candidate".into()),
    );
    assert!(redelivery.ok, "{}", redelivery.error.unwrap_or_default());
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .delivery_evidence
            .as_deref(),
        Some("corrected candidate")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_is_read_only_and_does_not_consume_notifications() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    register(&server, "peer-two", "%peer-two");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    assert!(
        handle_task_register(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            None,
            Some("feature".into()),
            Some("./playground/peer-task".into()),
            Some("peer-branch".into()),
            Some("peer-base".into()),
            default_priority(),
        )
        .ok
    );
    assert!(
        handle_task_register(
            &server,
            "peer-two".into(),
            "token-peer-two".into(),
            "other-task".into(),
            None,
            Some("other-feature".into()),
            Some("./playground/other-task".into()),
            Some("other-branch".into()),
            Some("other-base".into()),
            default_priority(),
        )
        .ok
    );
    let message_id = "notification".to_string();
    server.commit(&[Event::Sent {
        msg: Message {
            id: message_id.clone(),
            from: "peer-two".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("released:task".into()),
            body: "RESOURCE_RELEASED task=task".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok);
    assert_eq!(context.data["schema_version"], 1);
    assert_eq!(context.data["registered"], true);
    assert_eq!(context.data["registration"]["status"], "registered");
    assert!(context.data["master"].is_null());
    assert!(context.data["recorded_unusable"].is_null());
    assert_eq!(context.data["authority"]["must_obey_master"], false);
    assert_eq!(context.data["authority"]["may_decline_master_invite"], true);
    assert_eq!(context.data["inbox"]["unread"], 1);
    assert_eq!(context.data["inbox"]["messages"][0]["id"], message_id);
    assert_eq!(
        context.data["inbox"]["messages"][0]["body"],
        "RESOURCE_RELEASED task=task"
    );
    assert_eq!(context.data["identity"]["role"], "worker");
    assert_eq!(context.data["agent"]["thread_state"], "idle");
    assert_eq!(context.data["agent"]["can_accept_direct_input"], true);
    assert_eq!(context.data["tasks"][0]["id"], "task");
    assert_eq!(context.data["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(context.data["worktrees"][0]["task_id"], "task");
    for peer in context.data["peers"].as_array().unwrap() {
        assert!(peer.get("tasks").is_none());
        assert!(peer.get("transport").is_none());
        assert!(peer.get("agent").is_none());
    }
    assert!(!context.data.to_string().contains("other-task"));
    assert!(!context.data.to_string().contains("other-branch"));
    assert!(!context.data.to_string().contains("other-base"));
    assert!(context.data["subscriptions"].is_array());
    assert_eq!(context.data["daemon"]["live"], true);
    assert_eq!(
        context.data["daemon"]["pid"],
        serde_json::json!(std::process::id())
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_gives_an_idle_master_one_canonical_scheduling_action() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let context = handle_context(&server, "master".into(), "token-master".into());

    assert!(context.ok, "{}", context.error.unwrap_or_default());
    assert_eq!(context.data["identity"]["role"], "master");
    assert_eq!(context.data["master"]["worker_id"], "master");
    assert_eq!(context.data["master"]["endpoint_live"], true);
    assert!(context.data["recorded_unusable"].is_null());
    assert_eq!(
        context.data["next_actions"],
        serde_json::json!(["run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle"])
    );
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("Delivery, merge, or a review verdict is not a lifecycle endpoint; drive review/integration/cleanup/close and assign the next ready P0/P1 task.")));
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("Before ending each scheduling turn, saturate every live present peer first, then schedule managed subagents within the configured cap; never stay idle while eligible capacity remains.")));
    assert_eq!(
        context.data["role_brief"]["next_action"],
        "Run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle. Delivery or review triggers review/integration/cleanup/dispatch, not an endpoint."
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_projects_appserver_thread_and_turn_state_without_guessing() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "state-peer", "thread-state-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    let mut server = Arc::new(server);

    for (thread_state, active_flags, turn_status, expected) in [
        ("idle", vec![], Some("completed"), "idle"),
        ("active", vec![], Some("inProgress"), "working"),
        (
            "active",
            vec!["waitingOnApproval"],
            Some("inProgress"),
            "waiting_approval",
        ),
        (
            "active",
            vec!["waitingOnUserInput"],
            Some("inProgress"),
            "waiting_input",
        ),
        ("systemError", vec![], Some("failed"), "system_error"),
        ("notLoaded", vec![], None, "not_loaded"),
    ] {
        Arc::get_mut(&mut server).unwrap().appserver_thread_status =
            Arc::new(move |_, thread_id| {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": thread_state, "activeFlags": active_flags},
                        "canAcceptDirectInput": thread_state == "idle",
                        "turns": [
                            {"status": turn_status},
                            {"status": "older"}
                        ]
                    }
                }))
            });
        let context = handle_context(&server, "state-peer".into(), "token-state-peer".into());
        assert_eq!(context.data["agent"]["thread_state"], thread_state);
        assert_eq!(
            context.data["agent"]["latest_turn_status"].as_str(),
            turn_status
        );
        let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
        assert_eq!(status.data["workers"][0]["agent_state"], expected);
    }

    Arc::get_mut(&mut server).unwrap().appserver_thread_status =
        Arc::new(|_, _| Err("ADAPTER_UNKNOWN: thread/read unavailable".into()));
    let context = handle_context(&server, "state-peer".into(), "token-state-peer".into());
    assert!(context.data["agent"].is_null());
    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_status_probe_timeout_is_durable_unknown() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "timeout-peer", "thread-timeout-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status =
        Arc::new(|_, _| Err("ADAPTER_TIMEOUT: thread/read timed out".into()));
    let server = Arc::new(server);

    let context = handle_context(&server, "timeout-peer".into(), "token-timeout-peer".into());
    assert!(context.data["agent"].is_null());

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["status"], "unknown");
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    assert_eq!(status.data["workers"][0]["presence"], "unknown");
    assert!(status.data["workers"][0]["endpoint_live"].is_null());
    assert!(status.data["workers"][0]["identity_valid"].is_null());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_status_route_unavailable_is_missing_not_unknown() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "missing-peer", "thread-missing-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status = Arc::new(|_, _| {
        Err("ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded".into())
    });
    let server = Arc::new(server);

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["status"], "lost");
    assert_eq!(status.data["workers"][0]["agent_state"], "absent");
    assert_eq!(status.data["workers"][0]["presence"], "missing");
    assert_eq!(status.data["workers"][0]["endpoint_live"], false);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_identity_timeout_cannot_be_overridden_by_successful_status_probe() {
    let (mut server, root) = test_server();
    let registration =
        register_appserver(&mut server, "timeout-identity", "thread-timeout-identity");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_candidate_check =
        Arc::new(|_| Err("ADAPTER_TIMEOUT: candidate verification timed out".into()));
    let server = Arc::new(server);

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["status"], "unknown");
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    assert_eq!(status.data["workers"][0]["presence"], "unknown");
    assert!(status.data["workers"][0]["endpoint_live"].is_null());
    assert!(status.data["workers"][0]["identity_valid"].is_null());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn architecture_source_has_no_live_declared_role_or_dispatch_owner() {
    let server_source = include_str!("mod.rs");
    let state_source = include_str!("state.rs");
    let mcp_source = include_str!("../bin/collab-mcp.rs");
    for removed in [
        "#[cfg(any())]",
        "fn default_role",
        "fn handle_transfer_master",
        "fn idle_worker_ids",
        "TASK_OFFER",
        "TASK_DELIVERED",
        "master_notified",
    ] {
        assert!(
            !server_source.contains(removed),
            "removed runtime semantic remains: {removed}"
        );
    }
    assert!(!state_source.contains("pub role:"));
    assert!(!state_source.contains("pub goal_prompt:"));
    assert!(!state_source.contains("pub goal_busy:"));
    assert!(!state_source.contains("pub nudge_count:"));
    assert!(!state_source.contains("pub last_nudge_ms:"));
    for removed_tool in [
        "\"collab_role\"",
        "\"collab_root\"",
        "\"collab_task_claim\"",
        "\"collab_task_dispatch\"",
        "\"project_root\"",
        "args.get(\"pane\")",
    ] {
        assert!(
            !mcp_source.contains(removed_tool),
            "removed MCP tool remains: {removed_tool}"
        );
    }
}

#[test]
fn active_lifecycle_manifest_binds_every_registered_call_edge_to_source() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../docs/collab-v1-lifecycle.manifest.json")).unwrap();
    let call_map: serde_json::Value =
        serde_json::from_str(include_str!("../../docs/mainline-call-map.json")).unwrap();
    assert_eq!(manifest["status"], "active");
    assert_eq!(call_map["status"], "active");
    assert_eq!(manifest["lifecycle_id"], call_map["lifecycle_id"]);

    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for edge in call_map["edges"].as_array().unwrap() {
        let path = edge["path"].as_str().unwrap();
        let source = std::fs::read_to_string(project_root.join(path)).unwrap();
        for field in ["caller", "callee"] {
            let symbol = edge[field].as_str().unwrap();
            assert!(
                source.contains(symbol),
                "{field} {symbol} is not bound in {path}"
            );
        }
    }
    for path in manifest["canonical_docs"].as_array().unwrap() {
        assert!(project_root.join(path.as_str().unwrap()).is_file());
    }
}

#[test]
fn cleanup_rejects_unmerged_then_removes_only_merged_clean_worktree() {
    let root = std::env::temp_dir().join(format!(
        "collab-close-git-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).status.success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "collab test"])
        .status
        .success());
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    assert!(git(&["add", "README.md"]).status.success());
    assert!(git(&["commit", "-q", "-m", "base"]).status.success());
    assert!(
        git(&["worktree", "add", "-q", "-b", "feature", "playground/wt"])
            .status
            .success()
    );
    std::fs::write(root.join("playground/wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );

    let refused = close_task_resources(&root, Some("playground/wt"), Some("feature"));
    assert!(refused.unwrap_err().contains("not merged"));
    assert!(playground.join("wt").is_dir());

    assert!(git(&["merge", "-q", "feature"]).status.success());
    assert!(close_task_resources(&root, Some("playground/wt"), Some("feature")).is_ok());
    assert!(!playground.join("wt").exists());
    assert!(!git(&["rev-parse", "--verify", "feature"]).status.success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn codex_subagents_exchange_messages() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "first-peer", "%first");
    register(&server, "codex-peer", "%codex");
    let now = now_ms();
    let first = Record {
        id: "first-rt".into(),
        parent: "parent".into(),
        peer: "first-peer".into(),
        status: "idle".into(),
        thread_id: Some("thread-first".into()),
        profile: None,
        created_ms: now,
        ready_deadline_ms: now + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
    };
    let codex = Record {
        id: "codex-rt".into(),
        parent: "parent".into(),
        peer: "codex-peer".into(),
        status: "idle".into(),
        thread_id: Some("thread-codex".into()),
        profile: None,
        created_ms: now,
        ready_deadline_ms: now + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
    };
    server.commit(&[
        Event::SubagentUpdated {
            subagent: first.clone(),
        },
        Event::SubagentUpdated {
            subagent: codex.clone(),
        },
    ]);
    let to_codex = handle_send(
        &server,
        "first-peer".into(),
        "codex-peer".into(),
        "notify".into(),
        Some("first-to-codex".into()),
        "ping from first runtime".into(),
        None,
        "immediate".into(),
    );
    assert!(to_codex.ok, "{}", to_codex.error.unwrap_or_default());
    let to_first = handle_send(
        &server,
        "codex-peer".into(),
        "first-peer".into(),
        "notify".into(),
        Some("codex-to-first".into()),
        "pong from codex runtime".into(),
        None,
        "immediate".into(),
    );
    assert!(to_first.ok, "{}", to_first.error.unwrap_or_default());
    let to_parent = handle_send(
        &server,
        "first-peer".into(),
        "parent".into(),
        "notify".into(),
        Some("first-result".into()),
        "first finished".into(),
        None,
        "immediate".into(),
    );
    assert!(to_parent.ok, "{}", to_parent.error.unwrap_or_default());
    let from_parent_first = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "first-rt".into(),
            subject: "assign-first".into(),
            body: "task for first".into(),
        },
    );
    assert!(
        from_parent_first.ok,
        "{}",
        from_parent_first.error.unwrap_or_default()
    );
    assert!(
        crate::subagent::handle(
            &server,
            "first-peer",
            "token-first-peer",
            Action::Working {
                id: "first-rt".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "first-peer",
            "token-first-peer",
            Action::Ready {
                id: "first-rt".into()
            }
        )
        .ok
    );
    let from_parent_codex = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "codex-rt".into(),
            subject: "assign-codex".into(),
            body: "task for codex".into(),
        },
    );
    assert!(
        from_parent_codex.ok,
        "{}",
        from_parent_codex.error.unwrap_or_default()
    );
    let first_status = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "first-rt".into(),
        },
    );
    let codex_status = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "codex-rt".into(),
        },
    );
    assert!(first_status.ok);
    assert!(codex_status.ok);
    assert_eq!(first_status.data["subagent"]["runtime"], "codex");
    assert_eq!(codex_status.data["subagent"]["runtime"], "codex");
    assert_eq!(first_status.data["next_check"], "status");
    assert_eq!(first_status.data["progress"], "snapshot");
    assert_eq!(first_status.data["close_required"], false);
    let msgs: Vec<_> = server
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .cloned()
        .collect();
    assert!(
        msgs.iter().any(|m| m.from == "first-peer"
            && m.to == "codex-peer"
            && m.subject.as_deref() == Some("first-to-codex")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.from == "codex-peer"
            && m.to == "first-peer"
            && m.subject.as_deref() == Some("codex-to-first")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.from == "first-peer"
            && m.to == "parent"
            && m.subject.as_deref() == Some("first-result")),
        "{msgs:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_query_exposes_liveness_identity_and_notification_pressure() {
    let (server, root) = test_server();
    register(&server, "status-worker", "%test-status-worker");
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["id"], "status-worker");
    assert_eq!(w["transport"]["kind"], "appserver");
    assert_eq!(w["transport"]["thread_id"], "thread-test-status-worker");
    assert_eq!(w["endpoint_live"], true);
    assert_eq!(w["identity_valid"], true);
    assert_eq!(w["agent_state"], "idle");
    assert_eq!(w["status"], "idle");
    assert_eq!(w["unacked_notifications"], 0);
    assert_eq!(w["notifications_paused"], false);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_query_exposes_appserver_liveness() {
    let (mut server, root) = test_server();
    assert!(
        register_appserver(&mut server, "status-appserver", "thread-status-appserver").ok,
        "appserver registration failed"
    );
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["id"], "status-appserver");
    assert_eq!(w["transport"]["kind"], "appserver");
    assert_eq!(w["transport"]["thread_id"], "thread-status-appserver");
    assert_eq!(w["endpoint_live"], true);
    assert_eq!(w["identity_valid"], true);
    assert_eq!(w["agent_state"], "idle");
    assert_eq!(w["status"], "idle");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_query_reports_unverified_appserver_as_lost() {
    let (mut server, root) = test_server();
    assert!(
        register_appserver(&mut server, "lost-appserver", "thread-lost-appserver").ok,
        "appserver registration failed"
    );
    server.appserver_candidate_check = Arc::new(|_| {
        Err(crate::client::adapters::AdapterError::RouteUnavailable {
            detail: "test appserver verification failure".into(),
        }
        .to_string())
    });
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["transport"]["kind"], "appserver");
    assert_eq!(w["endpoint_live"], false);
    assert_eq!(w["agent_state"], "absent");
    assert_eq!(w["status"], "lost");
    assert_eq!(
        w["diagnostic"],
        "registered transport is not live; verify App Server route or thread"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bulk_ack_with_empty_ids_acknowledges_all_inbox_messages() {
    let (server, root) = test_server();
    register(&server, "sender-worker", "%test-sender-worker");
    register(&server, "bulk-worker", "%test-bulk-worker");
    let server_arc = Arc::new(server);

    // Send 2 messages to bulk-worker
    for i in 1..=2 {
        dispatch(
            &server_arc,
            Req::Send {
                from: "sender-worker".into(),
                worker_id: Some("sender-worker".into()),
                token: Some("token-sender-worker".into()),
                command: Some(send_command(&root, "sender-worker")),
                to: "bulk-worker".into(),
                mtype: "notify".into(),
                subject: Some(format!("test-{i}")),
                body: format!("body {i}"),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        );
    }

    // Deliver them
    let ids: Vec<String> = server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .map(|m| m.id.clone())
        .collect();
    server_arc.commit(&[Event::Delivered { ids: ids.clone() }]);

    // Bulk ack with empty ids
    let resp = dispatch(
        &server_arc,
        Req::Ack {
            worker_id: "bulk-worker".into(),
            token: "token-bulk-worker".into(),
            ids: vec![],
        },
    );
    assert!(resp.ok);
    assert_eq!(resp.data["acked"].as_array().unwrap().len(), 2);

    // Verify all messages are read
    let state = server_arc.state.lock().unwrap();
    assert!(state.msgs.values().all(|m| m.state == "read"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_master_wake_delivery_marks_accumulator_notified() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let now = now_ms();
    server.commit(&[
        Event::NotificationSubscribed {
            subscription: crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                target: "thread-master".into(),
                event: "worker-idle".into(),
                subject: None,
                method: "appserver".into(),
                status: "armed".into(),
                status_reason: None,
                created_ms: now,
                updated_ms: now,
                fired_count: 0,
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                expires_ms: now + 60_000,
            },
        },
        Event::MasterWakeSignal {
            signal: crate::server::state::MasterWakeSignal::WorkerIdle {
                worker_id: "worker".into(),
            },
            at_ms: now,
        },
        Event::Sent {
            msg: crate::server::state::Message {
                id: "master-wake".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notify".into(),
                subject: Some("worker-idle: worker".into()),
                body: "wake".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::NotificationSubscribed {
            subscription: crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                event: "direct-message".into(),
                subject: None,
                target: "thread-master".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now.saturating_add(300_000),
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        },
        Event::WakeBound {
            message_id: "master-wake".into(),
            subscription_id: "sub-master".into(),
        },
    ]);
    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "pending"
    );

    server.commit(&[Event::Delivered {
        ids: vec!["master-wake".into()],
    }]);

    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "notified_unconsumed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_master_wake_delivery_does_not_depend_on_legacy_master_projection() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let now = now_ms();
    server.commit(&[
        Event::MasterWakeSignal {
            signal: crate::server::state::MasterWakeSignal::WorkerIdle {
                worker_id: "worker".into(),
            },
            at_ms: now,
        },
        Event::Sent {
            msg: crate::server::state::Message {
                id: "master-wake-typed".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notify".into(),
                subject: Some("worker-idle: worker".into()),
                body: "wake".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: "master-wake-typed".into(),
            subscription_id: "sub-master".into(),
        },
    ]);
    {
        let mut state = server.state.lock().unwrap();
        state.master_worker_id = None;
        state.notification_subscriptions.insert(
            "sub-master".into(),
            crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                target: "thread-master".into(),
                event: "worker-idle".into(),
                subject: None,
                method: "appserver".into(),
                status: "armed".into(),
                status_reason: None,
                created_ms: now,
                updated_ms: now,
                fired_count: 1,
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                expires_ms: now + 60_000,
            },
        );
    }

    server.commit(&[Event::Delivered {
        ids: vec!["master-wake-typed".into()],
    }]);

    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "notified_unconsumed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_or_operator_sender_can_send_without_registration() {
    let (server, root) = test_server();
    register(&server, "recipient-worker", "%recipient");
    let resp = handle_send(
        &server,
        "external-operator".into(),
        "recipient-worker".into(),
        "notify".into(),
        Some("test-topic".into()),
        "hello from outside a registered peer".into(),
        None,
        "immediate".into(),
    );
    assert!(resp.ok);
    assert_eq!(resp.data["durable"].as_bool(), Some(true));
    let msg_id = resp.data["msg_id"].as_str().unwrap();

    let state = server.state.lock().unwrap();
    let msg = state.msgs.get(msg_id).unwrap();
    assert_eq!(msg.from, "external-operator");
    assert_eq!(msg.to, "recipient-worker");
    assert_eq!(msg.body, "hello from outside a registered peer");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_freed_transitions_notify_live_master() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "task-worker", "thread-worker");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let now = now_ms();
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-worker", "working", "task-worker"),
        },
        Event::KeepaliveUpdated {
            worker_id: "task-worker".into(),
            record: crate::server::keepalive::Record {
                observed: "working".into(),
                idle_since_ms: now - 1,
                ..Default::default()
            },
        },
        Event::TaskCreated {
            task: TaskRec {
                id: "task-worker-release".into(),
                owner: "task-worker".into(),
                created_by: "master-worker".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "working".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 1);
    server_arc.commit(&[
        Event::TaskUpdated {
            task: TaskRec {
                id: "task-worker-release".into(),
                owner: "task-worker".into(),
                created_by: "master-worker".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now + 2,
            },
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-worker", "idle", "task-worker"),
        },
    ]);

    crate::server::keepalive::tick_at(&server_arc, now + 60_002);

    let state = server_arc.state.lock().unwrap();
    let idle_alert = state
        .msgs
        .values()
        .find(|m| m.to == "master-worker" && m.subject == Some("subagent-status".into()));
    assert!(
        idle_alert.is_some(),
        "expected managed subagent-status alert sent to master"
    );
    let alert = idle_alert.unwrap();
    assert!(alert.body.contains("newly_idle=subagent:managed-worker"));
    assert!(alert.body.contains("live_idle=subagent:managed-worker"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_working_to_idle_leaves_reminder_to_the_master_idle_timer() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let now = now_ms();
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-master", "working", "master-worker"),
        },
        Event::KeepaliveUpdated {
            worker_id: "master-worker".into(),
            record: crate::server::keepalive::Record {
                observed: "working".into(),
                idle_since_ms: now - 1,
                ..Default::default()
            },
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-master", "idle", "master-worker"),
        },
    ]);

    crate::server::keepalive::tick_at(&server_arc, now + 1);

    let state = server_arc.state.lock().unwrap();
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|m| m.to == "master-worker")
            .count(),
        0,
        "keepalive must not self-wake a master idle transition; the master-idle timer owns that contract"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_idle_requires_live_master_armed_subscription_and_empty_backlog() {
    let (server, root) = test_server();
    register(&server, "unpromoted", "thread-unpromoted");
    let server_arc = std::sync::Arc::new(server);
    let now = now_ms();
    let initial_rec = crate::server::keepalive::Record {
        observed: "working".into(),
        idle_since_ms: now - 1,
        ..Default::default()
    };
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-unpromoted", "working", "unpromoted"),
        },
        Event::KeepaliveUpdated {
            worker_id: "unpromoted".into(),
            record: initial_rec.clone(),
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-unpromoted", "idle", "unpromoted"),
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 1);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: unpromoted".into())));

    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "unpromoted".into(),
            token: "token-unpromoted".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);
    server_arc.commit(&[
        Event::KeepaliveUpdated {
            worker_id: "unpromoted".into(),
            record: initial_rec.clone(),
        },
        Event::NotificationStatus {
            subscription_id: "sub-default-direct-message-unpromoted".into(),
            status: "consumed".into(),
            updated_ms: 2001,
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 2);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: unpromoted".into())));
    std::fs::remove_dir_all(root).unwrap();

    let (server, root) = test_server();
    register(&server, "busy-master", "thread-master");
    let server_arc = std::sync::Arc::new(server);
    assert!(
        dispatch(
            &server_arc,
            Req::MasterPromote {
                worker_id: "busy-master".into(),
                token: "token-busy-master".into(),
                approval: "approved".into(),
            }
        )
        .ok
    );
    create_task(&server_arc, "busy-master", "task-actionable", "feature");
    server_arc.commit(&[Event::KeepaliveUpdated {
        worker_id: "busy-master".into(),
        record: initial_rec,
    }]);
    crate::server::keepalive::tick_at(&server_arc, now + 3);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: busy-master".into())));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_unresponsive_notifies_live_master_with_snapshot_advice() {
    let (mut server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "stuck-worker", "thread-stuck");
    let stuck_live = Arc::new(AtomicBool::new(true));
    let stuck_live_for_probe = stuck_live.clone();
    server.appserver_candidate_check = Arc::new(move |candidate| {
        if candidate.thread_id == "thread-stuck" && !stuck_live_for_probe.load(Ordering::SeqCst) {
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "stuck worker route is not live".into(),
            }
            .to_string())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let baseline = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(baseline.ok, "{baseline:?}");
    assert_eq!(baseline.data["workers"][0]["status"], "idle");
    assert_eq!(
        server_arc.state.lock().unwrap().keepalives["stuck-worker"].notified_presence,
        "online"
    );
    assert!(server_arc.state.lock().unwrap().msgs.values().all(|m| {
        !(m.to == "master-worker" && m.subject == Some("worker-unresponsive: stuck-worker".into()))
    }));

    stuck_live.store(false, Ordering::SeqCst);
    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["status"], "lost");

    let state = server_arc.state.lock().unwrap();
    let offline_alerts: Vec<_> = state
        .msgs
        .values()
        .filter(|m| {
            m.to == "master-worker" && m.subject == Some("worker-unresponsive: stuck-worker".into())
        })
        .collect();
    assert_eq!(offline_alerts.len(), 1);
    assert!(offline_alerts[0].body.contains("run snapshot"));
    assert_eq!(
        state.keepalives["stuck-worker"].notified_presence,
        "offline"
    );
    assert!(state
        .master_wake
        .unresponsive_workers
        .contains(&"stuck-worker".into()));
    drop(state);

    for request in [
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
        Req::StatusAll,
        Req::Workers,
    ] {
        let response = dispatch(&server_arc, request);
        assert!(response.ok, "{response:?}");
    }
    let ack = dispatch(
        &server_arc,
        Req::Ack {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            ids: vec![],
        },
    );
    assert!(ack.ok, "{ack:?}");
    let unchanged = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(unchanged.ok, "{unchanged:?}");
    assert_eq!(
        server_arc
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-unresponsive: stuck-worker".into())
            })
            .count(),
        1,
        "unchanged offline status, status-all, workers, and ack must not duplicate"
    );

    stuck_live.store(true, Ordering::SeqCst);
    let recovered = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.data["workers"][0]["status"], "idle");
    {
        let state = server_arc.state.lock().unwrap();
        let recovered_alerts: Vec<_> = state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-recovered: stuck-worker".into())
            })
            .collect();
        assert_eq!(recovered_alerts.len(), 1);
        assert_eq!(state.keepalives["stuck-worker"].notified_presence, "online");
        assert!(!state
            .master_wake
            .unresponsive_workers
            .contains(&"stuck-worker".into()));
    }
    let duplicate_recovered = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(duplicate_recovered.ok, "{duplicate_recovered:?}");
    assert_eq!(
        server_arc
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-recovered: stuck-worker".into())
            })
            .count(),
        1,
        "unchanged online status must not duplicate recovery"
    );

    stuck_live.store(false, Ordering::SeqCst);
    let rearmed = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(rearmed.ok, "{rearmed:?}");
    assert_eq!(
        server_arc
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-unresponsive: stuck-worker".into())
            })
            .count(),
        2,
        "opposite recovery transition must re-arm the next offline notification"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_offline_status_sets_baseline_then_online_transition_notifies_once() {
    let (mut server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "cold-worker", "thread-cold");
    let cold_live = Arc::new(AtomicBool::new(false));
    let cold_live_for_probe = cold_live.clone();
    server.appserver_candidate_check = Arc::new(move |candidate| {
        if candidate.thread_id == "thread-cold" && !cold_live_for_probe.load(Ordering::SeqCst) {
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "cold worker route is not live".into(),
            }
            .to_string())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let offline = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(offline.ok, "{offline:?}");
    assert_eq!(offline.data["workers"][0]["status"], "lost");
    {
        let state = server_arc.state.lock().unwrap();
        assert_eq!(state.keepalives["cold-worker"].notified_presence, "offline");
        assert!(state.msgs.values().all(|m| {
            !(m.to == "master-worker"
                && m.subject == Some("worker-unresponsive: cold-worker".into()))
        }));
    }

    cold_live.store(true, Ordering::SeqCst);
    let online = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(online.ok, "{online:?}");
    assert_eq!(online.data["workers"][0]["status"], "idle");
    let repeated = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(repeated.ok, "{repeated:?}");
    let state = server_arc.state.lock().unwrap();
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker" && m.subject == Some("worker-recovered: cold-worker".into())
            })
            .count(),
        1,
        "offline->online status edge must notify exactly once"
    );
    assert_eq!(state.keepalives["cold-worker"].notified_presence, "online");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_presence_probe_after_reregister_does_not_notify_or_mutate_new_worker() {
    let (mut server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "edge-worker", "thread-stale");
    let root_cwd = root.display().to_string();
    let server_for_probe: Arc<StdMutex<Option<Arc<Server>>>> = Arc::new(StdMutex::new(None));
    let server_for_probe_closure = server_for_probe.clone();
    let reregistered = Arc::new(AtomicBool::new(false));
    let reregistered_closure = reregistered.clone();
    server.appserver_candidate_check = Arc::new(move |candidate| {
        if candidate.thread_id == "thread-stale"
            && !reregistered_closure.swap(true, Ordering::SeqCst)
        {
            if let Some(server) = server_for_probe_closure.lock().unwrap().as_ref() {
                server.commit(&[Event::Registered {
                    worker: WorkerRec {
                        id: "edge-worker".into(),
                        token: "token-edge-worker-new".into(),
                        cwd: root_cwd.clone(),
                        registered_ms: now_ms() + 1,
                        transport: Some(test_appserver_transport("thread-new")),
                    },
                }]);
            }
            Err(crate::client::adapters::AdapterError::RouteUnavailable {
                detail: "old registration route is no longer live".into(),
            }
            .to_string())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let server_arc = std::sync::Arc::new(server);
    *server_for_probe.lock().unwrap() = Some(server_arc.clone());
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);
    server_arc.commit(&[Event::KeepaliveUpdated {
        worker_id: "edge-worker".into(),
        record: crate::server::keepalive::Record {
            notified_presence: "online".into(),
            ..Default::default()
        },
    }]);

    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("edge-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["status"], "idle");
    assert_eq!(
        status.data["workers"][0]["transport"]["thread_id"],
        "thread-new"
    );
    let state = server_arc.state.lock().unwrap();
    assert_eq!(state.workers["edge-worker"].token, "token-edge-worker-new");
    assert_eq!(state.keepalives["edge-worker"].notified_presence, "online");
    assert!(state.msgs.values().all(|m| {
        !(m.to == "master-worker" && m.subject == Some("worker-unresponsive: edge-worker".into()))
    }));
    assert!(!state
        .master_wake
        .unresponsive_workers
        .contains(&"edge-worker".into()));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn presence_edge_probes_appserver_status_outside_state_mutex() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "edge-worker", "thread-edge");
    let mut server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let server_for_probe: Arc<StdMutex<Option<Arc<Server>>>> = Arc::new(StdMutex::new(None));
    let server_for_probe_closure = server_for_probe.clone();
    let server_mut = Arc::get_mut(&mut server_arc).unwrap();
    server_mut.appserver_thread_status = Arc::new(move |_, thread_id| {
        if let Some(server) = server_for_probe_closure.lock().unwrap().as_ref() {
            assert!(
                server.state.try_lock().is_ok(),
                "App Server status probes must not run under server.state mutex"
            );
        }
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "idle"},
                "canAcceptDirectInput": true
            }
        }))
    });
    *server_for_probe.lock().unwrap() = Some(server_arc.clone());

    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("edge-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["status"], "idle");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn closed_and_delivered_tasks_do_not_trigger_keepalives() {
    let (server, root) = test_server();
    register(&server, "worker-a", "thread-worker-a");
    let server_arc = std::sync::Arc::new(server);
    let base = now_ms();

    // Create a task that is delivered
    server_arc.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-delivered".into(),
            owner: "worker-a".into(),
            created_by: "worker-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "delivered".into(),
            next_step: None,
            wait: None,
            created_ms: base,
            updated_ms: base,
        },
    }]);

    // Tick scheduler - delivered task must NOT generate keepalive.
    crate::server::keepalive::tick_at(&server_arc, base + 900_000);
    assert!(server_arc.state.lock().unwrap().msgs.is_empty());

    // Now update task to closed
    server_arc.commit(&[Event::TaskUpdated {
        task: TaskRec {
            id: "task-delivered".into(),
            owner: "worker-a".into(),
            created_by: "worker-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "closed".into(),
            next_step: None,
            wait: None,
            created_ms: base,
            updated_ms: base,
        },
    }]);

    // Tick scheduler - closed task must NOT generate keepalive.
    crate::server::keepalive::tick_at(&server_arc, base + 1_800_000);
    assert!(server_arc.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_all_aggregates_workers_tasks_subagents_and_summary() {
    let (server, root) = test_server();
    let server_arc = Arc::new(server);

    // Register a worker
    server_arc.commit(&[Event::Registered {
        worker: WorkerRec {
            id: "worker-1".into(),
            token: "tok-1".into(),
            cwd: root.display().to_string(),
            registered_ms: 1000,
            transport: Some(test_appserver_transport("thread-worker-1")),
        },
    }]);

    // Create a task
    server_arc.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-1".into(),
            owner: "worker-1".into(),
            created_by: "master".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "normal".into(),
            status: "working".into(),
            next_step: Some("implementing".into()),
            wait: None,
            created_ms: 1000,
            updated_ms: 1000,
        },
    }]);

    let resp = dispatch(&server_arc, Req::StatusAll);
    assert!(resp.ok);
    assert_eq!(resp.data["summary"]["workers"], 1);
    assert_eq!(resp.data["summary"]["tasks"], 1);
    assert_eq!(resp.data["workers"].as_array().unwrap().len(), 1);
    assert_eq!(resp.data["workers"][0]["id"], "worker-1");
    assert_eq!(resp.data["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(resp.data["tasks"][0]["id"], "task-1");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mailbox_read_all_chronological_sort_asc_and_desc() {
    let (server, root) = test_server();
    let server_arc = Arc::new(server);

    // Send messages with different timestamps
    server_arc.commit(&[
        Event::Sent {
            msg: Message {
                id: "m-1".into(),
                from: "alice".into(),
                to: "bob".into(),
                mtype: "notify".into(),
                subject: Some("first".into()),
                body: "first body".into(),
                in_reply_to: None,
                created_ms: 1000,
                state: "delivered".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: "m-2".into(),
                from: "bob".into(),
                to: "alice".into(),
                mtype: "notify".into(),
                subject: Some("second".into()),
                body: "second body".into(),
                in_reply_to: None,
                created_ms: 2000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: "m-3".into(),
                from: "charlie".into(),
                to: "bob".into(),
                mtype: "notify".into(),
                subject: Some("third".into()),
                body: "third body".into(),
                in_reply_to: None,
                created_ms: 3000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
    ]);

    // Test time-asc (default)
    let asc_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: true,
            sort: Some("time-asc".into()),
            worker_id: None,
        },
    );
    assert!(asc_resp.ok);
    assert_eq!(asc_resp.data["count"], 3);
    let asc_msgs = asc_resp.data["messages"].as_array().unwrap();
    assert_eq!(asc_msgs[0]["id"], "m-1");
    assert_eq!(asc_msgs[1]["id"], "m-2");
    assert_eq!(asc_msgs[2]["id"], "m-3");

    // Test time-desc
    let desc_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: true,
            sort: Some("time-desc".into()),
            worker_id: None,
        },
    );
    assert!(desc_resp.ok);
    let desc_msgs = desc_resp.data["messages"].as_array().unwrap();
    assert_eq!(desc_msgs[0]["id"], "m-3");
    assert_eq!(desc_msgs[1]["id"], "m-2");
    assert_eq!(desc_msgs[2]["id"], "m-1");

    // Test worker filter
    let worker_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: false,
            sort: Some("time-asc".into()),
            worker_id: Some("charlie".into()),
        },
    );
    assert!(worker_resp.ok);
    assert_eq!(worker_resp.data["count"], 1);
    assert_eq!(worker_resp.data["messages"][0]["id"], "m-3");

    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn recv_clears_keepalive_unacked_counter() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    server.commit(&[Event::Sent {
        msg: Message {
            id: "wake-message".into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("wake".into()),
            body: "message".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let mut keepalive = crate::server::keepalive::Record::default();
    keepalive.unacked = 3;
    keepalive.last_notice_id = Some("stale-notice".into());
    keepalive.suspected_offline = true;
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "peer".into(),
        record: keepalive,
    }]);
    let server = Arc::new(server);
    let response = handle_poll_async(server.clone(), "peer".into(), 100).await;
    assert!(response.ok);
    let record = server.state.lock().unwrap().keepalives["peer"].clone();
    assert_eq!(record.unacked, 0);
    assert_eq!(record.last_notice_id, None);
    assert!(!record.suspected_offline);
    std::fs::remove_dir_all(root).ok();
}
