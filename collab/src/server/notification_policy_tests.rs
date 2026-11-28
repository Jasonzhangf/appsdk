//! Red-to-green contract tests for WP-C2:
//! - Periodic-first wakes and non-steering submission for master, patrol, and memory.
//! - True fixed 120_000ms [t0, t0 + 120_000) coalescing across transports with complete membership.
//! - Deferred batching never mis-classified as NotificationDeliveryFailed.
//! - Complete ablation of upward ordinary peer idle->master reporting and churn-resistant dispatch-gap.

use super::keepalive::{dispatch_gap_check_at, tick_at as keepalive_tick_at};
use super::mailbox::default_direct_message_id;
use super::name_state::{
    AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord, NameOccupancy,
};
use super::peer_tests::{promote_master, register_synthetic_peer, test_server};
use super::state::{Event, Message, NotificationSubscription, TaskRec};
use super::ticket_state::{TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus};
use super::{
    attempt_notification_detailed_with_at, NotificationAttempt, SelectedTransport, Server,
    TransportKind,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct CapturedSinkCall {
    transport: TransportKind,
    mode: String,
    body: String,
    message_id: String,
}

fn new_arc_server() -> (Arc<Server>, std::path::PathBuf) {
    let (server, root) = test_server();
    (Arc::new(server), root)
}

fn install_policy_sink(server: &mut Arc<Server>) -> Arc<Mutex<Vec<CapturedSinkCall>>> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&calls);
    Arc::get_mut(server)
        .expect("unique test server")
        .appserver_notification_sink =
        Arc::new(move |transport, _, body, message_id, _, mode| {
            observed.lock().unwrap().push(CapturedSinkCall {
                transport: transport.kind.clone(),
                mode: mode.to_owned(),
                body: body.to_owned(),
                message_id: message_id.to_owned(),
            });
            Ok(serde_json::json!({
                "accepted": true,
                "messageId": message_id,
            }))
        });
    Arc::get_mut(server)
        .expect("unique test server")
        .appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": { "type": "idle", "activeFlags": [] }
            }
        }))
    });
    calls
}

fn seed_worker_with_transport(
    server: &Server,
    worker_id: &str,
    transport: SelectedTransport,
    now: i64,
) -> String {
    let subscription_id = default_direct_message_id(worker_id);
    let project_scope = super::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();
    let method = transport.kind.as_str().to_owned();
    let target = transport
        .thread_id
        .clone()
        .or_else(|| transport.session_id.clone())
        .unwrap_or_else(|| format!("target-{worker_id}"));
    let session_id = transport
        .session_id
        .clone()
        .unwrap_or_else(|| format!("session-{worker_id}"));
    server.commit(&[
        Event::Registered {
            worker: super::state::WorkerRec {
                id: worker_id.into(),
                token: format!("token-{worker_id}"),
                cwd: server.root.display().to_string(),
                registered_ms: now,
                transport: Some(transport),
            },
        },
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: worker_id.into(),
                event: "direct-message".into(),
                subject: None,
                target,
                method,
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 3_600_000,
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
            presence: AgentPresenceRecord {
                project_scope: project_scope.clone(),
                worker_id: worker_id.into(),
                session_id: session_id.clone(),
                presence: AgentPresence::Online,
                reason: "fixture online".into(),
                observed_ms: now,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope,
                worker_id: worker_id.into(),
                session_id,
                activity: AgentActivity::Idle,
                background: false,
                reason: "fixture idle".into(),
                observed_ms: now,
                idle_since_ms: Some(now),
            },
        },
    ]);
    subscription_id
}

fn appserver_transport(worker_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/policy-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{worker_id}")),
        thread_id: Some(format!("thread-{worker_id}")),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    }
}

fn dsh_transport(worker_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some("unix:///tmp/policy-test-dsh.sock".into()),
        namespace: Some("dsh".into()),
        session_id: Some(format!("session-{worker_id}")),
        thread_id: Some(format!("thread-{worker_id}")),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["enqueue".into()],
        self_check: "test dsh".into(),
    }
}

fn desktop_transport(worker_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::DesktopBridge,
        endpoint: Some("http://127.0.0.1:19999".into()),
        namespace: Some("codex_desktop".into()),
        session_id: Some(format!("session-{worker_id}")),
        thread_id: Some(format!("thread-{worker_id}")),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message".into()],
        self_check: "test desktop".into(),
    }
}

fn commit_notice(
    server: &Server,
    id: &str,
    to: &str,
    subscription_id: &str,
    subject: &str,
    delivery_mode: Option<&str>,
    created_ms: i64,
) {
    let mut events = vec![
        Event::Sent {
            msg: Message {
                id: id.into(),
                from: "sender".into(),
                to: to.into(),
                mtype: "notify".into(),
                subject: Some(subject.into()),
                body: format!("body-{id}"),
                in_reply_to: None,
                created_ms,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: id.into(),
            subscription_id: subscription_id.into(),
        },
    ];
    if let Some(mode) = delivery_mode {
        events.push(Event::DeliveryMode {
            msg_id: id.into(),
            mode: mode.into(),
            source_thread_id: None,
        });
    }
    server.commit(&events);
}

#[test]
fn ordinary_burst_waits_for_full_two_minute_window_across_transports() {
    let base = 1_795_000_000_000_i64;
    for (label, mut transport) in [
        ("appserver", appserver_transport("peer")),
        ("dsh", dsh_transport("peer")),
        ("desktop", desktop_transport("peer")),
    ] {
        let (mut server, root) = new_arc_server();
        let _gateway = (label == "dsh").then(|| {
            let gateway = super::dsh_channel_tests::registration_gateway(
                "policy-dsh",
                &root,
                None,
                super::dsh_channel_tests::wake_reply,
            );
            transport.endpoint = Some(gateway.endpoint());
            transport.namespace = Some("rt-1".into());
            transport.session_id = Some("ses-1".into());
            transport.thread_id = Some("ses-1".into());
            gateway
        });
        let calls = install_policy_sink(&mut server);
        seed_worker_with_transport(&server, "sender", appserver_transport("sender"), base);
        let sub_id = seed_worker_with_transport(&server, "peer", transport, base);

        let t0 = base + 1_000;
        commit_notice(&server, "m1", "peer", &sub_id, "dispatch-1", Some("queued"), t0);
        let attempt_t0 = attempt_notification_detailed_with_at(&server, "m1", &sub_id, t0);
        assert!(
            matches!(attempt_t0, NotificationAttempt::Deferred),
            "{label}: first message at t0 must defer without leading-edge wake, got {attempt_t0:?}"
        );
        {
            let state = server.state.lock().unwrap();
            assert!(
                !state.notification_delivery_failures.contains_key("m1"),
                "{label}: deferred window must not record NotificationDeliveryFailed"
            );
            assert_eq!(state.msgs["m1"].wake_attempt_count, 0);
        }

        // Second message within [t0, t0 + 120_000) plus one exactly at boundary t0 + 120_000
        commit_notice(
            &server,
            "m2",
            "peer",
            &sub_id,
            "dispatch-2",
            Some("queued"),
            t0 + 30_000,
        );
        commit_notice(
            &server,
            "m3",
            "peer",
            &sub_id,
            "dispatch-3",
            Some("queued"),
            t0 + 120_000,
        );

        super::timers::tick_with_idle_at(&server, t0 + 119_999, &|_| true);
        assert_eq!(
            calls.lock().unwrap().len(),
            0,
            "{label}: zero adapter calls before 120_000ms elapses"
        );

        // At t0 + 120_000: m1 and m2 deliver together as 1 batch; m3 (created at t0 + 120_000) stays in second window
        super::timers::tick_with_idle_at(&server, t0 + 120_000, &|_| true);
        {
            let calls = calls.lock().unwrap();
            assert_eq!(
                calls.len(),
                1,
                "{label}: expected 1 batched wake at t0+120_000, got {calls:?}"
            );
            let body = &calls[0].body;
            assert!(body.contains("m1") && body.contains("m2"), "{label}: {body}");
            assert!(
                !body.contains("m3"),
                "{label}: boundary message created at t0+120_000 must stay in second window: {body}"
            );
            let _ = (&calls[0].transport, &calls[0].message_id);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn dispatch_batch_exceeding_three_members_delivers_all_without_stranding() {
    let base = 1_795_000_000_000_i64;
    let (mut server, root) = new_arc_server();
    let calls = install_policy_sink(&mut server);
    let sub_id = seed_worker_with_transport(&server, "peer", appserver_transport("peer"), base);

    let t0 = base + 1_000;
    for i in 0..5_i64 {
        let id = format!("burst-msg-{i}-{}", "x".repeat(40));
        commit_notice(
            &server,
            &id,
            "peer",
            &sub_id,
            &format!("dispatch-{i}"),
            None,
            t0 + i * 1_000,
        );
    }

    super::timers::tick_with_idle_at(&server, t0 + 120_000, &|_| true);
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "all 5 burst entries must fire in one submission"
    );
    let state = server.state.lock().unwrap();
    for i in 0..5_i64 {
        let id = format!("burst-msg-{i}-{}", "x".repeat(40));
        assert!(
            state.notification_delivery_accepted.contains_key(&id),
            "member {id} was stranded outside batch acceptance"
        );
        assert_eq!(state.msgs[&id].wake_attempt_count, 1);
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn protected_roles_events_do_not_wake_before_periodic_due_and_never_steer() {
    let base = 1_795_000_000_000_i64;
    let (mut server, root) = new_arc_server();
    let calls = install_policy_sink(&mut server);
    let project_scope = super::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();

    super::peer_tests::register_appserver_peer(
        &server,
        "master",
        &appserver_transport("master"),
    );
    let sub_master = default_direct_message_id("master");
    promote_master(&server, "master", "user approved master");
    let sub_patrol = seed_worker_with_transport(
        &server,
        "patrol-peer",
        appserver_transport("patrol-peer"),
        base,
    );
    let sub_memory = seed_worker_with_transport(
        &server,
        "memory-peer",
        appserver_transport("memory-peer"),
        base,
    );

    server.commit(&[
        Event::NameWorkRolesUpdated {
            project_scope: project_scope.clone(),
            record: crate::board::NameWorkRoles {
                name: "PatrolAgent".into(),
                work_roles: vec!["巡察者".into()],
            },
        },
        Event::NameResolutionCommitted {
            project_scope: project_scope.clone(),
            occupancy: NameOccupancy {
                name: "PatrolAgent".into(),
                worker_id: "patrol-peer".into(),
                session_id: "session-patrol-peer".into(),
                occupancy_revision: 1,
                updated_ms: base,
                pane_anchor: None,
            },
            assignments: vec![],
        },
        Event::NameWorkRolesUpdated {
            project_scope: project_scope.clone(),
            record: crate::board::NameWorkRoles {
                name: "MemoryAgent".into(),
                work_roles: vec!["记忆 agent".into()],
            },
        },
        Event::NameResolutionCommitted {
            project_scope: project_scope.clone(),
            occupancy: NameOccupancy {
                name: "MemoryAgent".into(),
                worker_id: "memory-peer".into(),
                session_id: "session-memory-peer".into(),
                occupancy_revision: 1,
                updated_ms: base,
                pane_anchor: None,
            },
            assignments: vec![],
        },
    ]);

    let t0 = base + 5_000;
    commit_notice(
        &server,
        "evt-master",
        "master",
        &sub_master,
        "board-changed:t1",
        Some("queued"),
        t0,
    );
    commit_notice(
        &server,
        "evt-patrol",
        "patrol-peer",
        &sub_patrol,
        "workflow-template-changed",
        Some("queued"),
        t0,
    );
    commit_notice(
        &server,
        "evt-memory",
        "memory-peer",
        &sub_memory,
        "work-roles-changed",
        Some("queued"),
        t0,
    );

    // Even after 120_000ms elapses, protected roles without a due patrol schedule must NOT wake on routine events
    for (msg_id, sub_id) in [
        ("evt-master", sub_master.as_str()),
        ("evt-patrol", sub_patrol.as_str()),
        ("evt-memory", sub_memory.as_str()),
    ] {
        let outcome =
            attempt_notification_detailed_with_at(&server, msg_id, sub_id, t0 + 130_000);
        assert!(
            matches!(outcome, NotificationAttempt::Deferred),
            "protected role {msg_id} must defer without periodic due, got {outcome:?}"
        );
    }
    super::timers::tick_with_idle_at(&server, t0 + 130_000, &|_| true);
    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "protected roles must receive zero native wakes before periodic due: {:?}",
        calls.lock().unwrap()
    );
    {
        let state = server.state.lock().unwrap();
        assert!(state.notification_delivery_failures.is_empty());
        assert!(state.message_receivable_by("master", &state.msgs["evt-master"]));
        assert!(state.message_receivable_by("patrol-peer", &state.msgs["evt-patrol"]));
        assert!(state.message_receivable_by("memory-peer", &state.msgs["evt-memory"]));
    }

    // Arm a due periodic deadline for patrol-peer while Idle -> wakes via non-steering queued mode
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-patrol-periodic".into(),
            worker_id: "patrol-peer".into(),
            event: "deadline".into(),
            subject: Some("periodic-patrol".into()),
            target: "thread-patrol-peer".into(),
            method: "appserver".into(),
            trigger_ms: Some(t0 + 200_000),
            trigger_times_ms: vec![t0 + 200_000],
            interval_ms: Some(900_000),
            repeat_count: 100,
            fired_count: 0,
            expires_ms: t0 + 3_600_000,
            status: "armed".into(),
            created_ms: base,
            updated_ms: base,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: true,
            goal_content: None,
        },
    }]);
    super::timers::tick_with_idle_at(&server, t0 + 200_000, &|_| true);
    {
        let calls = calls.lock().unwrap();
        assert!(
            !calls.is_empty(),
            "due periodic schedule for protected role must deliver"
        );
        for call in calls.iter() {
            assert_eq!(
                call.mode, "queued",
                "protected role due wake must use queued mode, never turn/steer: {call:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_idle_is_silent_to_master_and_dispatch_gap_ignores_key_churn() {
    let base = 1_795_000_000_000_i64;
    let (server, root) = test_server();
    register_synthetic_peer(&server, "master", "thread-master");
    promote_master(&server, "master", "user approved master");
    register_synthetic_peer(&server, "worker", "thread-worker");

    let scope = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let observe = |worker: &str, session: &str, activity: AgentActivity, at: i64| {
        server.commit(&[
            Event::AgentPresenceObserved {
                presence: AgentPresenceRecord {
                    project_scope: scope.clone(),
                    worker_id: worker.into(),
                    session_id: session.into(),
                    presence: AgentPresence::Online,
                    reason: "fixture".into(),
                    observed_ms: at,
                },
            },
            Event::AgentActivityObserved {
                activity: AgentActivityRecord {
                    project_scope: scope.clone(),
                    worker_id: worker.into(),
                    session_id: session.into(),
                    activity,
                    background: false,
                    reason: "fixture".into(),
                    observed_ms: at,
                    idle_since_ms: Some(at),
                },
            },
        ]);
    };
    // Worker goes Working -> Idle while master is Online + Idle
    observe("worker", "session-thread-worker", AgentActivity::Busy, base);
    observe("master", "session-thread-master", AgentActivity::Idle, base);
    keepalive_tick_at(&server, base);
    observe("worker", "session-thread-worker", AgentActivity::Idle, base + 1);
    keepalive_tick_at(&server, base + 1);
    keepalive_tick_at(&server, base + 200_000);
    {
        let state = server.state.lock().unwrap();
        let idle_notices: Vec<_> = state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master"
                    && matches!(
                        m.subject.as_deref(),
                        Some("worker-idle" | "master-wake-batch")
                    )
            })
            .collect();
        assert!(
            idle_notices.is_empty(),
            "ordinary peer idle must no longer emit idle notices to master: {idle_notices:?}"
        );
    }

    // Dispatch gap: first occurrence admits durable notice, key churn within 600_000ms must NOT reopen
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "T1".into(),
                owner: "master".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "pending".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: base,
                updated_ms: base,
            },
        },
        Event::TicketPhaseRecorded {
            phase: TicketPhaseRecord {
                task_id: "T1".into(),
                phase: TicketPhaseKind::Execute,
                batch: 1,
                priority: "p2".into(),
                status: TicketPhaseStatus::WaitingAssignment,
                slot_name: None,
                binding_generation: 0,
                enqueue_seq: 1,
                attempt_generation: 0,
                depends_on: Vec::new(),
                route_group: None,
                wait_reason: None,
                result: None,
            },
        },
    ]);
    server.state.lock().unwrap().board_details.insert(
        "T1".into(),
        crate::board::BoardTaskDetails::legacy("T1", true),
    );
    let gap_count = || {
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| m.to == "master" && m.subject.as_deref() == Some("master-dispatch-gap"))
            .count()
    };
    dispatch_gap_check_at(&server, base + 300_000);
    assert_eq!(gap_count(), 1);

    // Adding T2 changes condition_key, must still stay suppressed inside 600_000ms window
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "T2".into(),
                owner: "master".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "pending".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: base + 300_001,
                updated_ms: base + 300_001,
            },
        },
        Event::TicketPhaseRecorded {
            phase: TicketPhaseRecord {
                task_id: "T2".into(),
                phase: TicketPhaseKind::Execute,
                batch: 1,
                priority: "p2".into(),
                status: TicketPhaseStatus::WaitingAssignment,
                slot_name: None,
                binding_generation: 0,
                enqueue_seq: 2,
                attempt_generation: 0,
                depends_on: Vec::new(),
                route_group: None,
                wait_reason: None,
                result: None,
            },
        },
    ]);
    server.state.lock().unwrap().board_details.insert(
        "T2".into(),
        crate::board::BoardTaskDetails::legacy("T2", true),
    );
    dispatch_gap_check_at(&server, base + 300_010);
    assert_eq!(
        gap_count(),
        1,
        "task/peer condition-key churn must not bypass 600_000ms cooldown"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn protected_roles_non_queueable_desktop_defer_while_busy_and_deliver_idle() {
    let base = 1_795_000_000_000_i64;
    let (mut server, root) = new_arc_server();
    let calls = install_policy_sink(&mut server);
    let project_scope = super::GlobalState::canonical_project_scope(&server.root)
        .unwrap()
        .as_str()
        .to_owned();
    let mut transport = desktop_transport("desktop-patrol");
    transport.session_id = Some("session-desktop-patrol".into());
    transport.thread_id = Some("thread-desktop-patrol".into());
    seed_worker_with_transport(&server, "desktop-patrol", transport, base);
    server.commit(&[
        Event::NameWorkRolesUpdated {
            project_scope: project_scope.clone(),
            record: crate::board::NameWorkRoles {
                name: "DesktopPatrol".into(),
                work_roles: vec!["巡察者".into()],
            },
        },
        Event::NameResolutionCommitted {
            project_scope: project_scope.clone(),
            occupancy: NameOccupancy {
                name: "DesktopPatrol".into(),
                worker_id: "desktop-patrol".into(),
                session_id: "session-desktop-patrol".into(),
                occupancy_revision: 1,
                updated_ms: base,
                pane_anchor: None,
            },
            assignments: vec![],
        },
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-desktop-periodic".into(),
                worker_id: "desktop-patrol".into(),
                event: "deadline".into(),
                subject: Some("periodic-patrol".into()),
                target: "thread-desktop-patrol".into(),
                method: "desktop-bridge".into(),
                trigger_ms: Some(base + 10_000),
                trigger_times_ms: vec![base + 10_000],
                interval_ms: Some(900_000),
                repeat_count: 100,
                fired_count: 0,
                expires_ms: base + 3_600_000,
                status: "armed".into(),
                created_ms: base,
                updated_ms: base,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: true,
                goal_content: None,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: project_scope.clone(),
                worker_id: "desktop-patrol".into(),
                session_id: "session-desktop-patrol".into(),
                activity: AgentActivity::Busy,
                background: false,
                reason: "fixture busy".into(),
                observed_ms: base + 5_000,
                idle_since_ms: None,
            },
        },
    ]);
    super::timers::tick_with_idle_at(&server, base + 10_000, &|_| true);
    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "non-queueable protected role must defer while Busy"
    );
    server.commit(&[Event::AgentActivityObserved {
        activity: AgentActivityRecord {
            project_scope,
            worker_id: "desktop-patrol".into(),
            session_id: "session-desktop-patrol".into(),
            activity: AgentActivity::Idle,
            background: false,
            reason: "fixture idle".into(),
            observed_ms: base + 12_000,
            idle_since_ms: Some(base + 12_000),
        },
    }]);
    super::timers::tick_with_idle_at(&server, base + 12_000, &|_| true);
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "once Idle, non-queueable protected role receives due periodic notice"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_is_not_failure_and_does_not_consume_attempt_budget() {
    let base = 1_795_000_000_000_i64;
    let (mut server, root) = new_arc_server();
    let calls = install_policy_sink(&mut server);
    let sub_id = seed_worker_with_transport(&server, "peer", appserver_transport("peer"), base);
    let t0 = base + 1_000;
    commit_notice(&server, "m1", "peer", &sub_id, "dispatch-1", None, t0);
    for step in [0, 30_000, 60_000, 90_000, 119_999] {
        let outcome = attempt_notification_detailed_with_at(&server, "m1", &sub_id, t0 + step);
        assert!(matches!(outcome, NotificationAttempt::Deferred));
        let state = server.state.lock().unwrap();
        assert!(state.notification_delivery_failures.is_empty());
        assert_eq!(state.msgs["m1"].wake_attempt_count, 0);
        assert!(!state.msgs["m1"].retry_attempted);
    }
    let final_outcome =
        attempt_notification_detailed_with_at(&server, "m1", &sub_id, t0 + 120_000);
    assert!(final_outcome.accepted(), "{final_outcome:?}");
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(server.state.lock().unwrap().msgs["m1"].wake_attempt_count, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepted_unread_batch_does_not_reseed_or_block_next_window() {
    let base = 1_795_000_000_000_i64;
    let (mut server, root) = new_arc_server();
    let calls = install_policy_sink(&mut server);
    let sub_id = seed_worker_with_transport(&server, "peer", appserver_transport("peer"), base);
    let t0 = base + 1_000;
    commit_notice(&server, "m1", "peer", &sub_id, "d1", None, t0);
    commit_notice(&server, "m2", "peer", &sub_id, "d2", None, t0 + 10_000);
    assert!(
        attempt_notification_detailed_with_at(&server, "m1", &sub_id, t0 + 120_000).accepted()
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    {
        let state = server.state.lock().unwrap();
        assert!(state.notification_delivery_accepted.contains_key("m1"));
        assert!(state.notification_delivery_accepted.contains_key("m2"));
    }

    // New burst starts while m1/m2 are still accepted/delivered (unread)
    let t3 = t0 + 150_000;
    commit_notice(&server, "m3", "peer", &sub_id, "d3", None, t3);
    assert!(matches!(
        attempt_notification_detailed_with_at(&server, "m3", &sub_id, t3 + 60_000),
        NotificationAttempt::Deferred
    ));
    assert!(
        attempt_notification_detailed_with_at(&server, "m3", &sub_id, t3 + 120_000).accepted()
    );
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[1].body.contains("d3") || recorded[1].message_id == "m3");
    assert!(!recorded[1].body.contains("d1") && !recorded[1].body.contains("d2"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn removed_master_wake_fields_do_not_reappear_after_state_round_trip() {
    let mut state = super::state::State::default();
    let legacy_signal: super::notification_state::MasterWakeSignal =
        serde_json::from_str(r#"{"WorkerIdle":{"worker_id":"p1"}}"#).unwrap();
    state.apply(&Event::MasterWakeSignal {
        signal: legacy_signal,
        at_ms: 10,
    });
    state.apply(&Event::MasterWakeSignal {
        signal: super::notification_state::MasterWakeSignal::GoalDue { revision: 2 },
        at_ms: 20,
    });
    let serialized = serde_json::to_value(&state.master_wake).unwrap();
    let obj = serialized.as_object().unwrap();
    for removed in [
        "idle_workers",
        "newly_idle_workers",
        "unresponsive_workers",
        "blocked_or_timed_out_tasks",
        "completed_or_freed_tasks",
        "last_notified_newly_idle",
        "last_notified_live_idle",
    ] {
        assert!(
            !obj.contains_key(removed),
            "removed MasterWakeAccumulator field {removed} must not serialize: {serialized}"
        );
    }
    let mut legacy_acc = serialized.clone();
    legacy_acc["idle_workers"] = serde_json::json!(["old"]);
    legacy_acc["newly_idle_workers"] = serde_json::json!(["old"]);
    let decoded: super::notification_state::MasterWakeAccumulator =
        serde_json::from_value(legacy_acc).unwrap();
    assert!(decoded.goal_due);
    assert_eq!(decoded.active_goal_revision, Some(2));
}
