use super::*;
use crate::proto::TmuxCandidate;
use crate::server::operation_journal::{inject_next_append_fault, OperationJournal};
use crate::server::peer_tests::test_server;
use std::sync::Arc;

fn persisted_context_fixture() -> (
    Arc<Server>,
    Arc<Server>,
    Arc<ProjectRuntimeManager>,
    PathBuf,
    PathBuf,
    IdentityFacts,
    identity::Identity,
) {
    let (host, host_root) = test_server();
    let (runtime, root) = test_server();
    let host = Arc::new(host);
    let runtime = Arc::new(runtime);
    let mut candidate = tmux_anchor();
    candidate.cwd = root.display().to_string();
    let app = AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap();
    // Durable prior state: this fixture establishes an already-registered owner
    // so the ordinary-restore and role tests start from a committed identity.
    // It is not an initial-registration test, so it seeds defined typed state
    // instead of driving a live participant return. The public owner is
    // exercised after this precondition.
    let transport = crate::proto::SelectedTransport {
        kind: crate::proto::TransportKind::Tmux,
        endpoint: Some(candidate.endpoint.socket_path.clone()),
        namespace: Some(candidate.endpoint.tmux_session_id.clone()),
        session_id: candidate.endpoint.codex_session_id.clone(),
        thread_id: candidate.endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(candidate.endpoint.clone()),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "persisted context fixture owner".into(),
    };
    let registered = crate::server::peer_tests::seed_committed_owner(
        &runtime,
        "saved-peer",
        "saved-token",
        &transport,
        &root.display().to_string(),
        Some(app.clone()),
    );
    assert!(registered.ok, "{registered:?}");
    commit_current_thread_route_for_runtime(
        &host,
        &runtime,
        "saved-peer",
        &root.display().to_string(),
        Some(&app),
    )
    .unwrap();
    let (binding, transport) =
        identity::registration_from_receipt(&registered.data, "saved-peer", &root).unwrap();
    let mut saved = identity::Identity {
        worker_id: "saved-peer".into(),
        token: "saved-token".into(),
        project_scope: Some(GlobalState::canonical_project_scope(&root).unwrap()),
        runtime: None,
        transport: None,
    };
    identity::persist_registration_at(
        &host.host_paths,
        &Scope { root: root.clone() },
        &mut saved,
        binding,
        transport,
    )
    .unwrap();
    let manager = recovery_manager(host.clone());
    let context = recovery_context(&root);
    manager.install_runtime(
        &(app.as_str().into(), context.project_scope.as_str().into()),
        runtime.clone(),
        None,
    );
    (
        host,
        runtime,
        manager,
        host_root,
        root,
        IdentityFacts {
            session_id: candidate.endpoint.codex_session_id.clone(),
            thread_id: candidate.endpoint.codex_thread_id.clone(),
            tmux: Some(candidate),
            ..Default::default()
        },
        saved,
    )
}

#[test]
fn session_move_retargets_owned_deadline_without_resetting_its_schedule() {
    let (host, runtime, manager, host_root, root, mut facts, saved) = persisted_context_fixture();
    let registered =
        ProjectContext::for_registered_route(&root, saved.runtime.as_ref().unwrap()).unwrap();
    let (_, subscribed) = manager.dispatch_sync(
        Some(registered),
        Req::NotificationSubscribe {
            worker_id: saved.worker_id.clone(),
            token: saved.token.clone(),
            event: "deadline".into(),
            subject: Some("placement-patrol".into()),
            trigger_ms: Some(now_ms() + 60_000),
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            ttl_seconds: 120,
        },
    );
    assert!(subscribed.ok, "{subscribed:?}");
    let before = runtime
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some("placement-patrol"))
        .unwrap()
        .clone();
    let mut cancelled = before.clone();
    cancelled.id.push_str("-cancelled");
    cancelled.status = "cancelled".into();
    runtime
        .try_commit(&[Event::NotificationSubscribed {
            subscription: cancelled.clone(),
        }])
        .unwrap();
    facts.tmux.as_mut().unwrap().endpoint.pane_id = "%77".into();
    facts.tmux.as_mut().unwrap().endpoint.pane_pid += 1;
    let (_, restored) = manager.dispatch_sync(
        Some(recovery_context(&root)),
        Req::IdentityContext {
            facts,
            identity_context: None,
        },
    );
    assert!(restored.ok, "{restored:?}");
    let after = runtime.state.lock().unwrap().notification_subscriptions[&before.id].clone();
    assert_eq!(after.target, "saved-agent-thread");
    let endpoint = restored.data["identity_receipt"]["transport"]["tmux_endpoint"].clone();
    assert_eq!(endpoint["pane_id"], "%77");
    assert_eq!(after.status, "armed");
    assert_eq!(after.created_ms, before.created_ms);
    assert_eq!(after.expires_ms, before.expires_ms);
    assert_eq!(after.trigger_ms, before.trigger_ms);
    assert_eq!(after.repeat_count, before.repeat_count);
    assert_eq!(after.fired_count, before.fired_count);
    let retained = runtime.state.lock().unwrap().notification_subscriptions[&cancelled.id].clone();
    assert_eq!(
        serde_json::to_value(retained).unwrap(),
        serde_json::to_value(cancelled).unwrap()
    );
    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn new_tmux_runtime_id_does_not_encode_physical_placement() {
    let (host, runtime, manager, host_root, root, _, saved) = persisted_context_fixture();
    let (other, other_root) = test_server();
    let mut transport = saved.transport.clone().unwrap();
    let endpoint = transport.tmux_endpoint.as_mut().unwrap();
    endpoint.socket_path = other_root
        .join("different.sock")
        .to_string_lossy()
        .into_owned();
    endpoint.server_pid += 1;
    endpoint.pane_id = "%99".into();
    endpoint.pane_pid += 1;
    transport.endpoint = Some(endpoint.socket_path.clone());
    let receipt = crate::server::peer_tests::seed_committed_owner(
        &other,
        &saved.worker_id,
        &saved.token,
        &transport,
        &other_root.to_string_lossy(),
        Some(AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap()),
    );
    assert!(receipt.ok, "{receipt:?}");
    let (binding, _) =
        identity::registration_from_receipt(&receipt.data, &saved.worker_id, &other_root).unwrap();
    assert_eq!(
        binding.runtime_id,
        saved.runtime.as_ref().unwrap().runtime_id
    );
    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

#[test]
fn pane_movement_preserves_selected_native_transport() {
    let (host, runtime, manager, host_root, root, facts, mut saved) = persisted_context_fixture();
    let context = recovery_context(&root);
    let mut binding = manager
        .current_anchor_binding(&context, &facts)
        .unwrap()
        .unwrap();
    binding.endpoint_generation += 1;
    binding.session_id = Some(identity::SessionId::new("native-session").unwrap());
    binding.native_thread_id = Some(NativeThreadId::new("native-thread").unwrap());
    let mut worker = runtime.state.lock().unwrap().workers[&saved.worker_id].clone();
    let transport = worker.transport.as_mut().unwrap();
    transport.kind = TransportKind::AppServer;
    transport.endpoint = Some("unix:///tmp/selected-native.sock".into());
    transport.namespace = Some("codex_tui".into());
    transport.session_id = Some("native-session".into());
    transport.thread_id = Some("native-thread".into());
    transport.capabilities = vec!["send_message_to_thread".into()];
    let selected = transport.clone();
    runtime
        .try_commit(&[
            Event::GlobalRuntimeBound {
                binding: binding.clone(),
            },
            Event::Registered { worker },
        ])
        .unwrap();
    saved.runtime.as_mut().unwrap().endpoint_generation = binding.endpoint_generation;
    saved.runtime.as_mut().unwrap().session_id = binding.session_id.clone();
    saved.runtime.as_mut().unwrap().native_thread_id = binding.native_thread_id.clone();
    saved.transport = Some(selected.clone());
    let mut moved = facts.tmux.unwrap();
    moved.endpoint.pane_id = "%77".into();
    moved.endpoint.pane_pid += 1;
    let refreshed = manager
        .refresh_session_endpoint(
            &runtime,
            &binding,
            &moved,
            &mut saved,
            &Scope { root: root.clone() },
        )
        .unwrap();
    let actual = saved.transport.as_ref().unwrap();
    assert_eq!(actual.kind, TransportKind::AppServer);
    assert_eq!(actual.endpoint, selected.endpoint);
    assert_eq!(actual.session_id, selected.session_id);
    assert_eq!(actual.thread_id, selected.thread_id);
    assert_eq!(refreshed.session_id, binding.session_id);
    assert_eq!(refreshed.native_thread_id, binding.native_thread_id);
    assert_eq!(actual.tmux_endpoint.as_ref(), Some(&moved.endpoint));
    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn ordinary_persisted_context_has_no_registration_effects() {
    let (host, runtime, manager, host_root, root, mut facts, saved) = persisted_context_fixture();
    let context = recovery_context(&root);
    let before = std::fs::read(&runtime.journal_path).unwrap();
    // An unavailable adjacent adaptor must never be admitted on restore.
    facts.endpoint = Some("unix:///tmp/unavailable-restore-adaptor.sock".into());
    facts.namespace = Some("codex_tui".into());
    let (_, restored) = manager.dispatch_sync(
        Some(context),
        Req::IdentityContext {
            facts,
            identity_context: None,
        },
    );
    assert!(restored.ok, "{restored:?}");
    assert_eq!(restored.data["identity_receipt"], json!(saved));
    assert_eq!(
        std::fs::read(&runtime.journal_path).unwrap(),
        before,
        "ordinary restore must not Register or select/admit an adaptor"
    );
    // The caller supplies its observed session even while delivery is offline.
    // A historical socket + pane alone is no longer an identity selector.
    let stable = IdentityFacts {
        session_id: saved.transport.as_ref().unwrap().session_id.clone(),
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: saved
                .transport
                .as_ref()
                .unwrap()
                .tmux_endpoint
                .clone()
                .unwrap(),
            cwd: root.to_string_lossy().into_owned(),
        }),
        ..IdentityFacts::default()
    };
    let (_, offline) = manager.dispatch_sync(
        Some(recovery_context(&root)),
        Req::IdentityContext {
            facts: stable,
            identity_context: None,
        },
    );
    assert!(offline.ok, "{offline:?}");
    assert_eq!(offline.data["identity_receipt"], json!(saved));
    assert_eq!(std::fs::read(&runtime.journal_path).unwrap(), before);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn minimal_restore_master_grant_derives_current_identity_and_scope() {
    let (host, _runtime, manager, host_root, root, facts, saved) = persisted_context_fixture();
    let context = recovery_context(&root);
    let request = IdentityContextRequest {
        operation_id: "ctxop-minimal-restore-master".into(),
        invocation: "approved_recovery".into(),
        action: "context".into(),
        facts,
        approval: None,
        grant_approval: Some(json!({
            "action": "restore_master",
            "authorization": "user explicitly approved current identity as master"
        })),
        query: false,
        query_capability: "minimal-master-capability".into(),
        invocation_ticket: String::new(),
    };
    // Role restoration now uses the stable-name owner, not credential takeover.
    assert!(matches!(
        manager.approved_recovery_admission(&context, &request),
        Ok(None)
    ));
    let (_, response) = manager.identity_context(context.clone(), request);
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["result"]["outcome"], "completed");
    let snapshot = &response.data["result"]["snapshot"];
    assert_eq!(snapshot["identity"]["worker_id"], saved.worker_id);
    assert_eq!(snapshot["identity"]["role"], "master");
    assert_eq!(
        snapshot["scope"]["project_scope"],
        context.project_scope.as_str()
    );
    assert_eq!(
        snapshot["scope"]["app_scope_id"],
        context.app_scope_id.as_str()
    );
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn same_session_endpoint_move_reuses_identity_without_register() {
    let (host, runtime, manager, host_root, root, mut facts, saved) = persisted_context_fixture();
    assert!(
        handle_master_promote(
            &runtime,
            saved.worker_id.clone(),
            saved.token.clone(),
            "user assigned current session master".into()
        )
        .ok
    );
    let before = std::fs::read_to_string(&runtime.journal_path).unwrap();
    facts.tmux.as_mut().unwrap().endpoint.pane_id = "%77".into();
    facts.tmux.as_mut().unwrap().endpoint.pane_pid += 1;
    let (_, response) = manager.dispatch_sync(
        Some(recovery_context(&root)),
        Req::IdentityContext {
            facts: facts.clone(),
            identity_context: None,
        },
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.data["snapshot"]["identity"]["role"], "master",
        "{response:?}"
    );
    let restored: identity::Identity =
        serde_json::from_value(response.data["identity_receipt"].clone()).unwrap();
    assert_eq!(restored.worker_id, saved.worker_id);
    assert_eq!(restored.token, saved.token);
    assert_eq!(
        restored.transport.unwrap().tmux_endpoint,
        Some(facts.tmux.unwrap().endpoint)
    );
    let after = std::fs::read_to_string(&runtime.journal_path).unwrap();
    assert_eq!(
        after.matches("CommandStarted").count(),
        before.matches("CommandStarted").count(),
        "endpoint move must not dispatch Register"
    );
    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn context_binding_public_context_restore_reconciles_worker_projection() {
    let (host, runtime, manager, host_root, root, facts, saved) = persisted_context_fixture();
    let expected_endpoint = facts.tmux.as_ref().unwrap().endpoint.clone();
    let authoritative = manager
        .current_anchor_binding(&recovery_context(&root), &facts)
        .unwrap()
        .unwrap();
    let expected_generation = authoritative.endpoint_generation;
    let host_journal_before = std::fs::read(&host.journal_path).unwrap();
    runtime
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: authoritative.clone(),
        }])
        .unwrap();

    let mut stale_worker = runtime.state.lock().unwrap().workers[&saved.worker_id].clone();
    let stale_transport = stale_worker.transport.as_mut().unwrap();
    stale_transport.endpoint = Some("/tmp/stale-worker.sock".into());
    let stale_endpoint = stale_transport.tmux_endpoint.as_mut().unwrap();
    stale_endpoint.socket_path = "/tmp/stale-worker.sock".into();
    stale_endpoint.pane_id = "%99".into();
    stale_endpoint.pane_pid += 1;
    runtime
        .try_commit(&[Event::Registered {
            worker: stale_worker,
        }])
        .unwrap();

    let registered =
        ProjectContext::for_registered_route(&root, saved.runtime.as_ref().unwrap()).unwrap();
    let before_status_tasks = serde_json::to_value(&runtime.state.lock().unwrap().tasks).unwrap();
    let before_status_projects =
        serde_json::to_value(&runtime.state.lock().unwrap().global.projects).unwrap();
    let before_status_workers =
        serde_json::to_value(&runtime.state.lock().unwrap().workers).unwrap();
    let before_status_subscriptions =
        serde_json::to_value(&runtime.state.lock().unwrap().notification_subscriptions).unwrap();
    let before_status_runtime_journal = std::fs::read(&runtime.journal_path).unwrap();
    let before_status_host_journal = std::fs::read(&host.journal_path).unwrap();
    let (_, status) = manager.dispatch_sync(
        Some(registered.clone()),
        Req::WorkerStatus {
            worker_id: Some(saved.worker_id.clone()),
        },
    );
    assert!(status.ok, "{status:?}");
    let owner = status.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == saved.worker_id)
        .expect("owner status");
    assert_eq!(owner["identity_valid"], false, "{status:?}");
    assert_eq!(owner["status"], "identity-mismatch", "{status:?}");
    let diagnostic = owner["diagnostic"].as_str().unwrap_or_default();
    assert!(
        diagnostic.starts_with("SESSION_THREAD_BINDING_MISMATCH:"),
        "worker status must return a typed binding diagnosis: {diagnostic}"
    );
    assert!(
        diagnostic.contains("preserve the registered binding"),
        "worker status must preserve the binding: {diagnostic}"
    );
    assert!(
        diagnostic.contains("collab context"),
        "worker status must direct repair through collab context: {diagnostic}"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().tasks).unwrap(),
        before_status_tasks,
        "worker status diagnosis must not mutate task ownership"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().global.projects).unwrap(),
        before_status_projects,
        "worker status diagnosis must not repair or rewrite the binding"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().workers).unwrap(),
        before_status_workers,
        "worker status diagnosis must preserve the worker projection"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().notification_subscriptions).unwrap(),
        before_status_subscriptions,
        "worker status diagnosis must not create or change subscriptions"
    );
    assert_eq!(
        std::fs::read(&runtime.journal_path).unwrap(),
        before_status_runtime_journal,
        "worker status diagnosis must not append runtime events"
    );
    assert_eq!(
        std::fs::read(&host.journal_path).unwrap(),
        before_status_host_journal,
        "worker status diagnosis must not append host events"
    );

    let (_, restored) = manager.dispatch_sync(
        Some(recovery_context(&root)),
        Req::IdentityContext {
            facts: facts.clone(),
            identity_context: None,
        },
    );
    assert!(restored.ok, "{restored:?}");

    let actual = runtime.state.lock().unwrap().workers[&saved.worker_id]
        .transport
        .clone()
        .expect("selected worker transport");
    assert_eq!(
        actual.tmux_endpoint.as_ref(),
        Some(&expected_endpoint),
        "ordinary context must reconcile the worker projection to the authoritative project binding"
    );
    assert_eq!(
        actual.endpoint.as_deref(),
        Some(expected_endpoint.socket_path.as_str())
    );
    assert_eq!(
        manager
            .current_anchor_binding(&recovery_context(&root), &facts)
            .unwrap()
            .unwrap()
            .endpoint_generation,
        expected_generation,
        "repairing the worker projection must not manufacture a new binding generation"
    );
    assert_eq!(
        std::fs::read(&host.journal_path).unwrap(),
        host_journal_before,
        "repairing a same-binding worker projection must not rewrite the host route journal"
    );

    let (_, status) = manager.dispatch_sync(
        Some(registered.clone()),
        Req::WorkerStatus {
            worker_id: Some(saved.worker_id.clone()),
        },
    );
    assert!(status.ok, "{status:?}");
    let owner = status.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == saved.worker_id)
        .expect("owner status");
    assert_eq!(owner["identity_valid"], true, "{status:?}");
    assert_eq!(owner["presence"], "present", "{status:?}");

    let (_, published) = manager.dispatch_sync(
        Some(registered.clone()),
        Req::Board {
            worker_id: saved.worker_id.clone(),
            token: saved.token.clone(),
            command: crate::board::BoardCommand::Publish {
                id: "context-binding-atomic".into(),
                title: "context binding atomic".into(),
                description: "admit work through the repaired binding".into(),
                delivery_condition: "context, status, and board use one binding".into(),
                test_condition: "the public in-process regression passes".into(),
                priority: "p1".into(),
            },
        },
    );
    assert!(published.ok, "{published:?}");
    let (_, claimed) = manager.dispatch_sync(
        Some(registered),
        Req::Board {
            worker_id: saved.worker_id.clone(),
            token: saved.token.clone(),
            command: crate::board::BoardCommand::Claim {
                id: "context-binding-atomic".into(),
                expected_revision: 1,
            },
        },
    );
    assert!(claimed.ok, "{claimed:?}");

    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn context_binding_public_stale_runtime_rejects_then_context_repair_succeeds() {
    let (host, runtime, manager, host_root, root, facts, saved) = persisted_context_fixture();
    let retained_context =
        ProjectContext::for_registered_route(&root, saved.runtime.as_ref().unwrap()).unwrap();
    let retained_generation = retained_context
        .runtime_context
        .as_ref()
        .unwrap()
        .endpoint_generation;
    let authoritative = manager
        .current_anchor_binding(&recovery_context(&root), &facts)
        .unwrap()
        .unwrap();

    let mut moved_facts = facts.clone();
    let moved_endpoint = moved_facts.tmux.as_mut().unwrap();
    moved_endpoint.endpoint.pane_id = "%4".into();
    moved_endpoint.endpoint.pane_pid += 1;
    let moved_candidate = moved_facts.tmux.as_ref().unwrap();
    let mut advanced_identity = saved.clone();
    let advanced = manager
        .refresh_session_endpoint(
            &runtime,
            &authoritative,
            moved_candidate,
            &mut advanced_identity,
            &Scope { root: root.clone() },
        )
        .unwrap();
    assert_eq!(
        advanced.endpoint_generation,
        retained_generation + 1,
        "the supported pane move must advance the authoritative generation"
    );

    let subscription_subject = "stale-context-repair";
    let stale_request = Req::NotificationSubscribe {
        worker_id: saved.worker_id.clone(),
        token: saved.token.clone(),
        event: "deadline".into(),
        subject: Some(subscription_subject.into()),
        trigger_ms: Some(now_ms() + 60_000),
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        ttl_seconds: 120,
    };
    let before_tasks = serde_json::to_value(&runtime.state.lock().unwrap().tasks).unwrap();
    let before_worktrees =
        serde_json::to_value(&runtime.state.lock().unwrap().worktree_bindings).unwrap();
    let before_workers = serde_json::to_value(&runtime.state.lock().unwrap().workers).unwrap();
    let before_subscriptions =
        serde_json::to_value(&runtime.state.lock().unwrap().notification_subscriptions).unwrap();
    let before_runtime_journal = std::fs::read(&runtime.journal_path).unwrap();
    let before_host_journal = std::fs::read(&host.journal_path).unwrap();

    let (_, rejected) = manager.dispatch_sync(Some(retained_context), stale_request.clone());
    assert!(!rejected.ok, "{rejected:?}");
    let error = rejected.error.as_deref().unwrap_or_default();
    assert!(
        error.starts_with("SESSION_THREAD_BINDING_MISMATCH:"),
        "stale generation must be a typed rejection: {error}"
    );
    assert!(
        error.contains(&format!("expected {}", retained_generation + 1))
            && error.contains(&format!("observed {retained_generation}")),
        "typed rejection must identify expected and observed generations: {error}"
    );
    assert!(
        error.contains("collab context"),
        "typed rejection must direct repair through collab context: {error}"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().tasks).unwrap(),
        before_tasks,
        "stale request must not mutate task ownership"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().worktree_bindings).unwrap(),
        before_worktrees,
        "stale request must not allocate resources"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().workers).unwrap(),
        before_workers,
        "stale request must not rewrite identity"
    );
    assert_eq!(
        serde_json::to_value(&runtime.state.lock().unwrap().notification_subscriptions).unwrap(),
        before_subscriptions,
        "stale request must not create a subscription"
    );
    assert_eq!(
        std::fs::read(&runtime.journal_path).unwrap(),
        before_runtime_journal
    );
    assert_eq!(
        std::fs::read(&host.journal_path).unwrap(),
        before_host_journal
    );

    let (_, repaired) = manager.dispatch_sync(
        Some(recovery_context(&root)),
        Req::IdentityContext {
            facts: moved_facts,
            identity_context: None,
        },
    );
    assert!(repaired.ok, "{repaired:?}");
    let receipt: identity::Identity =
        serde_json::from_value(repaired.data["identity_receipt"].clone()).unwrap();
    assert_eq!(
        receipt.runtime.as_ref().unwrap().endpoint_generation,
        retained_generation + 1
    );
    let repaired_context =
        ProjectContext::for_registered_route(&root, receipt.runtime.as_ref().unwrap()).unwrap();
    let (_, accepted) = manager.dispatch_sync(Some(repaired_context), stale_request);
    assert!(accepted.ok, "{accepted:?}");
    assert!(
        runtime
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .any(|subscription| subscription.subject.as_deref() == Some(subscription_subject)),
        "the retry with the context-returned binding must succeed"
    );

    drop(manager);
    drop(runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn current_project_session_does_not_process_foreign_pane_owner() {
    let (host, old_runtime, manager, host_root, old_root, facts, old_identity) =
        persisted_context_fixture();
    assert!(
        handle_master_promote(
            &old_runtime,
            old_identity.worker_id.clone(),
            old_identity.token.clone(),
            "user assigned former project master".into()
        )
        .ok
    );
    let (new_runtime, root) = test_server();
    let new_runtime = Arc::new(new_runtime);
    let context = recovery_context(&root);
    let mut transport = old_identity.transport.clone().unwrap();
    transport.tmux_endpoint.as_mut().unwrap().pane_id = "%78".into();
    let registered = crate::server::peer_tests::seed_committed_owner(
        &new_runtime,
        "current-project-peer",
        "current-project-token",
        &transport,
        &root.display().to_string(),
        Some(context.app_scope_id.clone()),
    );
    assert!(registered.ok, "{registered:?}");
    commit_current_thread_route_for_runtime(
        &host,
        &new_runtime,
        "current-project-peer",
        &root.display().to_string(),
        Some(&context.app_scope_id),
    )
    .unwrap();
    let (binding, transport) =
        identity::registration_from_receipt(&registered.data, "current-project-peer", &root)
            .unwrap();
    let mut current = identity::Identity {
        worker_id: "current-project-peer".into(),
        token: "current-project-token".into(),
        project_scope: Some(context.project_scope.clone()),
        runtime: None,
        transport: None,
    };
    identity::persist_registration_at(
        &host.host_paths,
        &Scope { root: root.clone() },
        &mut current,
        binding,
        transport,
    )
    .unwrap();
    manager.install_runtime(
        &(
            context.app_scope_id.as_str().into(),
            context.project_scope.as_str().into(),
        ),
        new_runtime.clone(),
        None,
    );
    let before = std::fs::read_to_string(&new_runtime.journal_path).unwrap();
    let (_, restored) = manager.dispatch_sync(
        Some(context.clone()),
        Req::IdentityContext {
            facts,
            identity_context: None,
        },
    );
    assert!(restored.ok, "{restored:?}");
    assert_eq!(
        restored.data["identity_receipt"]["worker_id"],
        current.worker_id
    );
    assert_eq!(restored.data["snapshot"]["identity"]["role"], "worker");
    let state = old_runtime.state.lock().unwrap();
    assert!(state.workers.contains_key(&old_identity.worker_id));
    let route_scope = RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: old_identity.project_scope.clone().unwrap(),
    };
    assert!(current_master_grant(&state, Some(&route_scope)).is_some());
    let old_binding = state
        .global
        .lookup_binding_for(&route_scope, &old_identity.runtime.unwrap().binding_id)
        .unwrap();
    assert!(old_binding.tmux_endpoint.is_some());
    drop(state);
    let after = std::fs::read_to_string(&new_runtime.journal_path).unwrap();
    assert_eq!(
        after.matches("CommandStarted").count(),
        before.matches("CommandStarted").count()
    );
    let host_state = host.state.lock().unwrap();
    assert!(!host_state
        .global
        .current_thread_routes
        .values()
        .any(|route| route.agent_id.as_str() == old_identity.worker_id));
    drop(host_state);
    drop(manager);
    drop(new_runtime);
    drop(old_runtime);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(old_root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

#[test]
fn minimal_role_intent_is_an_explicit_authority_approval() {
    let (host, runtime, manager, host_root, root, facts, saved) = persisted_context_fixture();
    let context = recovery_context(&root);
    let app = context.app_scope_id.clone();
    let mut other = facts.tmux.clone().unwrap();
    other.endpoint.pane_id = "%4".into();
    other.endpoint.tmux_session_id = "$4".into();
    other.endpoint.codex_session_id = Some("unrelated-agent-session".into());
    other.endpoint.codex_thread_id = Some("unrelated-agent-thread".into());
    // The unrelated master is durable prior state, so seed a committed owner
    // instead of driving an initial Register: a tmux candidate can only finish
    // registration through a live participant return, which this fixture does
    // not model.
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(other.endpoint.socket_path.clone()),
        namespace: Some(other.endpoint.tmux_session_id.clone()),
        session_id: other
            .endpoint
            .codex_session_id
            .clone()
            .or_else(|| Some(other.endpoint.tmux_session_id.clone())),
        thread_id: other
            .endpoint
            .codex_thread_id
            .clone()
            .or_else(|| Some(other.endpoint.pane_id.clone())),
        tmux_endpoint: Some(other.endpoint.clone()),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "prior committed unrelated master fixture".into(),
    };
    let registered = crate::server::peer_tests::seed_committed_owner(
        &runtime,
        "unrelated-master",
        "other-token",
        &transport,
        &root.display().to_string(),
        Some(app),
    );
    assert!(registered.ok, "{registered:?}");
    assert!(
        handle_master_promote(
            &runtime,
            "unrelated-master".into(),
            "other-token".into(),
            "user assigned unrelated master".into()
        )
        .ok
    );
    commit_current_thread_route_for_runtime(
        &host,
        &runtime,
        "unrelated-master",
        &root.display().to_string(),
        Some(&context.app_scope_id),
    )
    .unwrap();
    let role_scope = RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: context.project_scope.clone(),
    };
    let unrelated =
        current_master_grant(&runtime.state.lock().unwrap(), Some(&role_scope)).unwrap();
    let (_, peer) = manager.dispatch_sync(
        Some(context.clone()),
        Req::IdentityContext {
            facts: facts.clone(),
            identity_context: None,
        },
    );
    assert!(peer.ok, "{peer:?}");
    assert_eq!(peer.data["snapshot"]["identity"]["role"], "worker");
    assert_eq!(
        peer.data["snapshot"]["master"]["worker_id"],
        "unrelated-master"
    );
    assert!(peer.data["snapshot"]["next_actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action
            .as_str()
            .is_some_and(|text| text.contains("--approve-grant"))));
    for (step, role) in ["peer", "master", "peer"].into_iter().enumerate() {
        let mut stale = saved.clone();
        stale.token = "stale-token".into();
        let identity_path = host
            .host_paths
            .state_root()
            .join("identities/saved-peer/identity.json");
        std::fs::write(&identity_path, serde_json::to_vec(&stale).unwrap()).unwrap();
        let mut request = IdentityContextRequest {
            operation_id: format!("ctxop-role-{step}"),
            invocation: "approved_recovery".into(),
            action: "context".into(),
            facts: facts.clone(),
            approval: Some(
                json!({"decision":"approved", "target_identity":"saved-peer", "action":format!("restore_{role}"), "authorization":format!("user requests {role}"), "expected_incumbent":{"binding_id":"obsolete-caller-fence","endpoint_generation":999}}),
            ),
            grant_approval: None,
            query: false,
            query_capability: "role-test-capability".into(),
            invocation_ticket: String::new(),
        };
        if role == "master" {
            request.grant_approval = request.approval.take();
            request.grant_approval.as_mut().unwrap()["replace_confirmed"] = json!(true);
        }
        let (_, result) = manager.identity_context(context.clone(), request.clone());
        assert!(result.ok, "{result:?}");
        assert_eq!(result.data["result"]["outcome"], "completed", "{result:?}");
        assert_eq!(
            result.data["result"]["snapshot"]["identity"]["role"],
            if role == "master" { "master" } else { "worker" }
        );
        let grant = current_master_grant(&runtime.state.lock().unwrap(), Some(&role_scope));
        if step == 0 {
            assert_eq!(grant, Some(unrelated.clone()));
        } else if role == "master" {
            assert_eq!(grant.unwrap().agent_id.as_str(), "saved-peer");
        } else {
            assert!(grant.is_none());
        }
        let before = (
            std::fs::read(&host.journal_path).unwrap(),
            std::fs::read(&runtime.journal_path).unwrap(),
            std::fs::read(&identity_path).unwrap(),
        );
        let (_, replay) = manager.identity_context(context.clone(), request);
        assert!(replay.ok, "{replay:?}");
        assert_eq!(
            before,
            (
                std::fs::read(&host.journal_path).unwrap(),
                std::fs::read(&runtime.journal_path).unwrap(),
                std::fs::read(&identity_path).unwrap()
            ),
            "same-key replay must not duplicate effects"
        );
    }
    let before = std::fs::read(&runtime.journal_path).unwrap();
    // A real read-only descriptor makes the atomic name/authority append fail.
    *runtime.journal.lock().unwrap() = std::fs::File::open(&runtime.journal_path).unwrap();
    let (_, failed) = manager.identity_context(context.clone(), IdentityContextRequest {
        operation_id: "ctxop-role-write-failure".into(), invocation: "approved_recovery".into(), action: "context".into(), facts,
        approval: Some(json!({"decision":"approved", "target_identity":"saved-peer", "action":"restore_master", "authorization":"user requires master"})),
        grant_approval: None, query: false, query_capability: "role-failure-capability".into(), invocation_ticket: String::new(),
    });
    assert!(!failed.ok, "{failed:?}");
    assert!(
        failed.data["result"]["owner_readback"]["name_resolution"]["error"]
            .as_str()
            .unwrap()
            .contains("NAME_DURABILITY_FAILED"),
        "{failed:?}"
    );
    assert!(!failed.data["result"]["committed_phases"]
        .as_array()
        .unwrap()
        .contains(&json!("grant")));
    assert!(current_master_grant(&runtime.state.lock().unwrap(), Some(&role_scope)).is_none());
    assert_eq!(std::fs::read(&runtime.journal_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(host_root).unwrap();
}

fn facts_from_saved_pane(saved: &identity::Identity) -> IdentityFacts {
    let endpoint = saved
        .transport
        .as_ref()
        .unwrap()
        .tmux_endpoint
        .as_ref()
        .unwrap();
    IdentityFacts {
        session_id: saved.transport.as_ref().unwrap().session_id.clone(),
        tmux_anchor: Some(crate::proto::TmuxPaneAnchor {
            socket_path: endpoint.socket_path.clone(),
            pane_id: endpoint.pane_id.clone(),
        }),
        ..Default::default()
    }
}

#[test]
fn context_success_receipts_use_the_registered_project_owner() {
    for kind in [TransportKind::AppServer, TransportKind::Tmux] {
        let (host, host_root) = test_server();
        let (runtime, root) = test_server();
        let worker = "receipt-owner";
        let scope = Scope { root: root.clone() };
        let app = AppServerId::new("registered-app").unwrap();
        let mut transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/receipt-owner.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("receipt-session".into()),
            thread_id: Some("receipt-thread".into()),
            capabilities: Default::default(),
            tmux_endpoint: None,
            self_check: "fixture".into(),
        };
        if kind == TransportKind::Tmux {
            let endpoint = tmux_anchor().endpoint;
            transport.kind = kind;
            transport.endpoint = Some(endpoint.socket_path.clone());
            transport.namespace = Some(endpoint.tmux_session_id.clone());
            transport.session_id = endpoint.codex_session_id.clone();
            transport.thread_id = endpoint.codex_thread_id.clone();
            transport.tmux_endpoint = Some(endpoint);
        }
        let prepared = runtime
            .prepare_register_envelope_for_scope(
                worker,
                "receipt-token",
                &transport,
                GlobalState::canonical_project_scope(&root).unwrap(),
                &root.to_string_lossy(),
                app.clone(),
                false,
            )
            .unwrap();
        let nested = (
            prepared.nested_command_id.clone(),
            prepared.nested_operation_id.clone(),
        );
        let receipt = consume_prepared_register_typed(&runtime, worker, &transport, prepared, None);
        assert!(receipt.ok, "{receipt:?}");
        commit_current_thread_route_for_runtime(
            &host,
            &runtime,
            worker,
            &root.to_string_lossy(),
            Some(&app),
        )
        .unwrap();
        let (binding, transport) =
            identity::registration_from_receipt(&receipt.data, worker, &root).unwrap();
        let mut ident = identity::Identity {
            worker_id: worker.into(),
            token: "receipt-token".into(),
            project_scope: Some(GlobalState::canonical_project_scope(&root).unwrap()),
            runtime: None,
            transport: None,
        };
        identity::persist_registration_at(&host.host_paths, &scope, &mut ident, binding, transport)
            .unwrap();
        let response = Resp::data(json!({"identity_receipt":ident}));
        let context = ProjectContext::for_registered_root_with_app(
            &root,
            AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap();
        assert_eq!(
            business_receipts_for_success(
                &host,
                &runtime,
                &context,
                None,
                &response,
                Some(&nested)
            ),
            vec!["nested_register", "route", "credential", "lease"]
        );
        // An explicit owner re-subscription uses a generated id after the
        // system-owned default lease was explicitly unsubscribed. The live
        // direct-message channel still proves the lease receipt.
        {
            let mut state = runtime.state.lock().unwrap();
            let default_id = crate::server::mailbox::default_direct_message_id(worker);
            let mut lease = state
                .notification_subscriptions
                .remove(&default_id)
                .expect("registration created the default direct-message lease");
            lease.id = "sub-explicit-direct-message".into();
            lease.status = "armed".into();
            lease.status_reason = None;
            lease.expires_ms = crate::server::state::now_ms() + 60_000;
            state
                .notification_subscriptions
                .insert(lease.id.clone(), lease);
        }
        assert_eq!(
            business_receipts_for_success(
                &host,
                &runtime,
                &context,
                None,
                &response,
                Some(&nested)
            ),
            vec!["nested_register", "route", "credential", "lease"]
        );
        {
            let mut state = runtime.state.lock().unwrap();
            state
                .notification_subscriptions
                .get_mut("sub-explicit-direct-message")
                .unwrap()
                .status = "cancelled".into();
        }
        assert_eq!(
            business_receipts_for_success(
                &host,
                &runtime,
                &context,
                None,
                &response,
                Some(&nested)
            ),
            vec!["nested_register", "route", "credential"]
        );
        let mut stale = Resp::data(response.data.clone());
        stale.data["identity_receipt"]["runtime"]["endpoint_generation"] = json!(2);
        assert_eq!(
            business_receipts_for_success(&host, &runtime, &context, None, &stale, Some(&nested)),
            vec!["nested_register"]
        );
        let mut cross_scope = Resp::data(response.data.clone());
        cross_scope.data["identity_receipt"]["runtime"]["appserver_id"] = json!("other-app");
        assert_eq!(
            business_receipts_for_success(
                &host,
                &runtime,
                &context,
                None,
                &cross_scope,
                Some(&nested)
            ),
            vec!["nested_register"]
        );
        let identity_path = host
            .host_paths
            .state_root()
            .join("identities")
            .join(worker)
            .join("identity.json");
        let identity_bytes = std::fs::read(&identity_path).unwrap();
        let mut wrong_credential: serde_json::Value =
            serde_json::from_slice(&identity_bytes).unwrap();
        wrong_credential["token"] = json!("not-the-current-project-credential");
        std::fs::write(
            &identity_path,
            serde_json::to_vec(&wrong_credential).unwrap(),
        )
        .unwrap();
        assert_eq!(
            business_receipts_for_success(
                &host,
                &runtime,
                &context,
                None,
                &response,
                Some(&nested)
            ),
            vec!["nested_register", "route"],
            "stale credential must not prove completion"
        );
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(host_root).unwrap();
    }
}

fn tmux_anchor() -> TmuxCandidate {
    TmuxCandidate {
        endpoint: crate::proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-test.sock".into(),
            server_pid: 42,
            tmux_session_id: "$7".into(),
            pane_id: "%3".into(),
            pane_pid: 99,
            codex_session_id: Some("saved-agent-session".into()),
            codex_thread_id: Some("saved-agent-thread".into()),
        },
        cwd: "/tmp/project".into(),
    }
}

/// A dsh anchor with no observed App Server endpoint is complete on its own.
#[test]
fn dsh_anchor_alone_requires_no_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// A tmux anchor with no observed endpoint is complete on its own.
#[test]
fn tmux_placement_requires_agent_session_but_no_appserver_fields() {
    let facts = IdentityFacts {
        tmux: Some(tmux_anchor()),
        ..IdentityFacts::default()
    };
    assert_eq!(required_fields(&facts), vec!["session_id"]);
    let facts = IdentityFacts {
        session_id: Some("current-agent".into()),
        ..facts
    };
    assert!(required_fields(&facts).is_empty());
}

/// Native selection requires four fields; the higher-priority pane does not.
#[test]
fn an_observed_endpoint_still_requires_the_four_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert_eq!(required_fields(&facts), vec!["thread_id", "namespace"]);
    let facts = IdentityFacts {
        session_id: Some("current-agent".into()),
        tmux: Some(tmux_anchor()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// With no anchor and no endpoint all four facts are requested.
#[test]
fn no_anchor_requests_all_four_fields() {
    let facts = IdentityFacts::default();
    assert_eq!(
        required_fields(&facts),
        vec!["session_id", "thread_id", "endpoint", "namespace"]
    );
}

#[test]
fn typed_operation_request_preserves_action_intent_and_capability_shape() {
    let request = IdentityContextRequest {
        operation_id: "ctxop-typed".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    assert!(validate_operation_request(&request).is_ok());
    assert_eq!(
        serde_json::to_value(&request).unwrap()["operation_id"],
        "ctxop-typed"
    );
}

#[test]
fn query_and_missing_capability_are_rejected_before_operation_admission() {
    let query = IdentityContextRequest {
        operation_id: "ctxop-query".into(),
        invocation: "query".into(),
        action: "query".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: true,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    assert_eq!(
        validate_operation_request(&query).unwrap_err(),
        "IDENTITY_OPERATION_QUERY_SHAPE_INVALID"
    );

    let missing_capability = IdentityContextRequest {
        invocation: "automatic".into(),
        action: "context".into(),
        query: false,
        query_capability: String::new(),
        invocation_ticket: String::new(),
        ..query
    };
    assert_eq!(
        validate_operation_request(&missing_capability).unwrap_err(),
        "IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability"
    );
}

#[test]
fn s23_dispatch_preserves_typed_identity_operation() {
    let (server, root) = test_server();
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    let manager = ProjectRuntimeManager::new_with_operation_journal(
        Arc::new(server),
        &host_paths,
        journal.clone(),
    )
    .unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let request = IdentityContextRequest {
        operation_id: "ctxop-dispatch".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts {
            session_id: Some("dispatch-agent-session".into()),
            tmux: Some(TmuxCandidate {
                endpoint: crate::proto::TmuxEndpoint {
                    socket_path: "/tmp/tmux-ctxop.sock".into(),
                    server_pid: 42,
                    tmux_session_id: "$7".into(),
                    pane_id: "%3".into(),
                    pane_pid: 99,
                    codex_session_id: None,
                    codex_thread_id: None,
                },
                cwd: root.display().to_string(),
            }),
            ..IdentityFacts::default()
        },
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    inject_next_append_fault();
    let (_, response) = manager.identity_context(context.clone(), request.clone());
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("IDENTITY_OPERATION_DURABILITY_FAILED: injected operation journal append failure")
    );
    let replay = OperationJournal::open(host_paths.journal_path()).unwrap();
    let query = replay
        .query(
            &IdentityContextRequest {
                operation_id: request.operation_id.clone(),
                invocation: "query".into(),
                action: "query".into(),
                facts: IdentityFacts::default(),
                approval: None,
                grant_approval: None,
                query: true,
                query_capability: request.query_capability.clone(),
                invocation_ticket: String::new(),
            },
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
        )
        .unwrap_err();
    assert!(query.starts_with("IDENTITY_OPERATION_UNKNOWN"), "{query}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn same_key_retry_returns_the_retained_projection_without_reconciliation() {
    let (server, root) = test_server();
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    let manager = ProjectRuntimeManager::new_with_operation_journal(
        Arc::new(server),
        &host_paths,
        journal.clone(),
    )
    .unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let request = IdentityContextRequest {
        operation_id: "ctxop-noop".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let admitted = journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: request.operation_id.clone(),
            project_scope: context.project_scope.as_str().to_owned(),
            app_scope_id: context.app_scope_id.as_str().to_owned(),
            action: request.action.clone(),
            invocation: request.invocation.clone(),
            intent_digest: identity_operation_intent_digest(&context, &request),
            query_capability_hash: crate::server::operation_journal::capability_hash(
                &request.query_capability,
            ),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let before = std::fs::read(host_paths.journal_path()).unwrap();
    let (_, response) = manager.identity_context(context, request);
    let after = std::fs::read(host_paths.journal_path()).unwrap();
    // The retained projection is honestly incomplete: outer `ok` is false
    // for the `unknown` outcome, and the same key neither appends nor
    // dispatches a side effect.
    assert!(!response.ok, "{:?}", response.error);
    assert_eq!(before, after, "same-key retry must not append or dispatch");
    assert_eq!(
        response.data["result"]["operation_id"],
        admitted.operation_id
    );
    assert_eq!(response.data["result"]["outcome"], "unknown");
    std::fs::remove_dir_all(&root).unwrap();
}

fn b03_transport(thread: &str) -> crate::proto::SelectedTransport {
    crate::server::peer_tests::test_appserver_transport(thread)
}

fn b03_journal(server: &crate::server::Server) -> Arc<OperationJournal> {
    Arc::new(OperationJournal::open(server.host_paths.journal_path()).unwrap())
}

/// The exact prepared envelope is what gets consumed: the receipt's nested
/// command/operation IDs equal the IDs carried on the prepared envelope.
#[test]
fn s23_b03_prepared_register_envelope_ids_equal_the_consumed_receipt() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-1";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-1");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let prepared = server
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope.clone(),
            false,
        )
        .unwrap();
    let prepared_command = prepared.nested_command_id.clone();
    let prepared_operation = prepared.nested_operation_id.clone();
    let response = consume_prepared_register_typed(&server, worker_id, &transport, prepared, None);
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["command_id"], prepared_command);
    assert_eq!(response.data["operation_id"], prepared_operation);
    assert_eq!(response.data["replayed"], false);
    std::fs::remove_dir_all(root).unwrap();
}

/// If the durable outer `Validating` sync fails, Register is never
/// consumed: no worker is registered and no nested receipt exists.
#[test]
fn s23_b03_outer_sync_failure_prevents_register_consume() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-2";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-2");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let journal = b03_journal(&server);
    journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: "ctxop-b03-bind".into(),
            project_scope: project_scope.as_str().to_owned(),
            app_scope_id: app_scope.as_str().to_owned(),
            action: "context".into(),
            invocation: "automatic".into(),
            intent_digest: "digest".into(),
            query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let binding = RegisterOuterBinding::new(journal, "ctxop-b03-bind".into());
    inject_next_append_fault();
    let mut before_consume =
        |prepared: &PreparedRegisterEnvelope| binding.bind_validating(prepared);
    let response = register_typed_observed(
        &server,
        worker_id,
        token,
        &transport,
        &root.to_string_lossy(),
        Some(project_scope),
        Some(app_scope),
        false,
        None,
        Some(&mut before_consume),
    );
    assert!(!response.ok, "{response:?}");
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("IDENTITY_OPERATION_DURABILITY_FAILED")),
        "{:?}",
        response.error
    );
    assert!(binding.bound().is_none());
    assert!(!server.state.lock().unwrap().workers.contains_key(worker_id));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

/// The pure readback returns the real inner receipt only when the nested
/// command id exists and the nested operation id matches exactly.
#[test]
fn s23_b03_inner_receipt_readback_requires_exact_nested_ids() {
    let (server, root) = test_server();
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    let manager = ProjectRuntimeManager::new_with_operation_journal(
        Arc::new(server),
        &host_paths,
        journal.clone(),
    )
    .unwrap();
    let worker_id = "b03-worker-3";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-3");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let context = ProjectContext::for_registered_root_with_app(&root, app_scope.clone()).unwrap();
    manager.install_runtime(
        &(
            app_scope.as_str().to_owned(),
            project_scope.as_str().to_owned(),
        ),
        manager.host.clone(),
        None,
    );
    let prepared = manager
        .host
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope,
            false,
        )
        .unwrap();
    let command_id = prepared.nested_command_id.clone();
    let operation_id = prepared.nested_operation_id.clone();
    let response =
        consume_prepared_register_typed(&manager.host, worker_id, &transport, prepared, None);
    assert!(
        response.ok,
        "error={:?} data={}",
        response.error, response.data
    );

    let projection = crate::proto::IdentityOperationProjection {
        operation_id: "ctxop-b03-read".into(),
        phase: crate::proto::IdentityOperationPhase::InnerDispatched,
        outcome: "unknown".into(),
        committed_phases: vec![crate::proto::IdentityOperationPhase::Validating],
        business_receipts: Vec::new(),
        nested_command_id: Some(command_id.clone()),
        nested_operation_id: Some(operation_id.clone()),
    };
    let readback = manager
        .inner_register_receipt_readback(&context, &projection)
        .unwrap();
    assert_eq!(readback["command_id"], command_id);
    assert_eq!(readback["operation_id"], operation_id);

    // A mismatched nested operation id must not be shown.
    let wrong = crate::proto::IdentityOperationProjection {
        nested_operation_id: Some("register-op-wrong".into()),
        ..projection.clone()
    };
    assert!(manager
        .inner_register_receipt_readback(&context, &wrong)
        .is_none());

    // A missing command id is never guessed.
    let missing = crate::proto::IdentityOperationProjection {
        nested_command_id: None,
        ..projection
    };
    assert!(manager
        .inner_register_receipt_readback(&context, &missing)
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

/// An explicitly requested revision check rejects a change between prepare
/// and consume: no worker is registered, no receipt is created, and the
/// prepared nested IDs are not rewritten or retried.
#[test]
fn s23_b03_stale_prepared_revision_is_rejected_without_effect_or_reprepare() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-4";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-4");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let journal = b03_journal(&server);
    journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: "ctxop-b03-stale".into(),
            project_scope: project_scope.as_str().to_owned(),
            app_scope_id: app_scope.as_str().to_owned(),
            action: "context".into(),
            invocation: "automatic".into(),
            intent_digest: "digest".into(),
            query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let binding = RegisterOuterBinding::new(journal, "ctxop-b03-stale".into());
    let mut prepared = server
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope.clone(),
            false,
        )
        .unwrap();
    prepared.typed.envelope.expected_revision = Some(server.state.lock().unwrap().revision);
    let prepared_command = prepared.nested_command_id.clone();
    let prepared_operation = prepared.nested_operation_id.clone();
    binding.bind_validating(&prepared).unwrap();
    assert_eq!(
        binding
            .bound()
            .as_ref()
            .map(|(command, _)| command.as_str()),
        Some(prepared_command.as_str())
    );
    // A concurrent project mutation advances the reducer revision after
    // prepare, so the prepared envelope's CAS must now fail.
    server
        .commit_checked(&[Event::KeepaliveUpdated {
            worker_id: "b03-other-worker".into(),
            record: crate::server::keepalive::Record::default(),
        }])
        .unwrap();
    let response = server.consume_prepared_register(prepared).unwrap_err();
    assert!(response.to_string().contains("revision"), "{response}");
    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key(worker_id));
    assert!(!state
        .global
        .lookup_command_receipt(&CommandId::new(prepared_command.clone()).unwrap())
        .is_some());
    drop(state);
    // The bound IDs stay exactly as prepared; no reprepare/new suffix.
    assert_eq!(
        binding.bound(),
        Some((prepared_command, prepared_operation))
    );
    std::fs::remove_dir_all(root).unwrap();
}

include!("required_fields_tests_tail.rs");
