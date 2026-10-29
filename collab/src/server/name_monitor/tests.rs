use super::*;
use crate::client::adapters::tmux::{AgentActivity, AgentPresence};
use crate::proto::{SelectedTransport, TransportKind};
use crate::scope::HostPaths;
use crate::server::name_state::{
    AgentActivity as Activity, AgentActivityRecord, AgentPresence as Presence,
};
use crate::server::state::{State, WorkerRec};
use std::sync::{Arc, Mutex};

fn server() -> Arc<Server> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "collab-name-monitor-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let journal_path = root.join("journal.jsonl");
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
        .unwrap();
    let server = Arc::new(Server {
        config: Default::default(),
        root: root.clone(),
        storage_root: root.clone(),
        journal_path,
        host_paths: HostPaths::for_state_root(root.join("host")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        appserver_candidate_check: Arc::new(|_| unreachable!()),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| unreachable!()),
        appserver_thread_status: Arc::new(|_, _| unreachable!()),
        appserver_thread_archive: Arc::new(|_, _| unreachable!()),
        mailbox_notify: tokio::sync::Notify::new(),
    });
    server.state.lock().unwrap().workers.insert(
        "worker".into(),
        WorkerRec {
            id: "worker".into(),
            token: "test".into(),
            cwd: root.to_string_lossy().into(),
            registered_ms: 0,
            transport: Some(SelectedTransport {
                kind: TransportKind::Tmux,
                endpoint: None,
                namespace: None,
                session_id: Some("agent-session".into()),
                thread_id: None,
                tmux_endpoint: Some(TmuxEndpoint {
                    socket_path: "/explicit/test.sock".into(),
                    server_pid: 123,
                    tmux_session_id: "$7".into(),
                    pane_id: "%8".into(),
                    pane_pid: 456,
                    codex_session_id: Some("agent-session".into()),
                    codex_thread_id: None,
                }),
                capabilities: vec![],
                self_check: "test".into(),
            }),
        },
    );
    server
}

fn observation(presence: AgentPresence, activity: AgentActivity) -> AgentObservation {
    AgentObservation {
        agent_presence: presence,
        activity,
        background: true,
        foreground_command: Some("codex".into()),
        reason: "observed".into(),
    }
}

fn scope(server: &Server) -> String {
    server.root.canonicalize().unwrap().to_string_lossy().into()
}

#[test]
fn ordinary_worker_maps_activity_and_deduplicates_facts() {
    let server = server();
    for (index, (source, expected)) in [
        (AgentActivity::Busy, Activity::Busy),
        (AgentActivity::Idle, Activity::Idle),
        (AgentActivity::WaitingUser, Activity::WaitingUser),
        (AgentActivity::Unknown, Activity::Unknown),
    ]
    .into_iter()
    .enumerate()
    {
        tick_with_observer(&server, index as i64, &|endpoint| {
            assert_eq!(endpoint.socket_path, "/explicit/test.sock");
            assert!(
                server.state.try_lock().is_ok(),
                "probe must release State lock"
            );
            Ok(observation(AgentPresence::Online, source))
        });
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.agent_presence[&scope(&server)]["worker"].presence,
            Presence::Online
        );
        assert_eq!(
            state.agent_presence[&scope(&server)]["worker"].observed_ms,
            0
        );
        let activity = &state.agent_activity[&scope(&server)]["worker"];
        assert_eq!(activity.activity, expected);
        assert!(activity.background);
        assert_eq!(activity.observed_ms, index as i64);
        drop(state);
        let before = std::fs::read(&server.journal_path).unwrap();
        tick_with_observer(&server, 100, &|_| {
            Ok(observation(AgentPresence::Online, source))
        });
        assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    }
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("worker")
        .unwrap()
        .transport
        .as_mut()
        .unwrap()
        .session_id = None;
    let before = std::fs::read(&server.journal_path).unwrap();
    tick_with_observer(&server, 101, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Unknown))
    });
    assert_eq!(
        std::fs::read(&server.journal_path).unwrap(),
        before,
        "endpoint codex_session_id must preserve the same agent identity"
    );
    std::fs::remove_dir_all(&server.root).unwrap();
}

#[test]
fn idle_interval_restarts_from_the_first_post_restart_observation() {
    let server = server();
    // A journal written by the previous daemon process records a long idle run
    // for the same binding and session.
    server.commit(&[crate::server::state::Event::AgentActivityObserved {
        activity: AgentActivityRecord {
            project_scope: scope(&server),
            worker_id: "worker".into(),
            session_id: "agent-session".into(),
            activity: Activity::Idle,
            background: true,
            reason: "observed before restart".into(),
            observed_ms: 500,
            idle_since_ms: Some(500),
        },
    }]);
    tick_with_observer(&server, 1_000_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(1_000_000),
        "the first observation after a daemon restart starts a fresh interval"
    );
    tick_with_observer(&server, 1_060_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(1_000_000),
        "later observations in the same process keep the interval"
    );
    std::fs::remove_dir_all(&server.root).unwrap();
}

#[test]
fn legacy_activity_record_without_idle_start_decodes_as_untracked() {
    let legacy = serde_json::json!({
        "project_scope": "/tmp/legacy",
        "worker_id": "worker",
        "session_id": "agent-session",
        "activity": "idle",
        "background": false,
        "reason": "legacy",
        "observed_ms": 42,
    });
    let record: AgentActivityRecord = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        record.idle_since_ms, None,
        "an old journal never backfills a trusted idle start from observed_ms"
    );
}

#[test]
fn shell_offline_and_probe_error_unknown() {
    let server = server();
    tick_with_observer(&server, 1, &|_| {
        Ok(observation(AgentPresence::Offline, AgentActivity::Unknown))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_presence[&scope(&server)]["worker"].presence,
        Presence::Offline
    );
    tick_with_observer(&server, 2, &|_| Err("probe failed".into()));
    let state = server.state.lock().unwrap();
    let presence = &state.agent_presence[&scope(&server)]["worker"];
    assert_eq!(presence.presence, Presence::Unknown);
    assert_eq!(presence.reason, "probe failed");
    assert_eq!(
        state.agent_activity[&scope(&server)]["worker"].activity,
        Activity::Unknown
    );
    drop(state);
    std::fs::remove_dir_all(&server.root).unwrap();
}

#[test]
fn changed_session_or_endpoint_discards_stale_observation() {
    for change_session in [true, false] {
        let server = server();
        tick_with_observer(&server, 1, &|_| {
            let mut state = server.state.lock().unwrap();
            let transport = state
                .workers
                .get_mut("worker")
                .unwrap()
                .transport
                .as_mut()
                .unwrap();
            if change_session {
                transport.session_id = Some("new-session".into());
            } else {
                transport.tmux_endpoint.as_mut().unwrap().pane_id = "%99".into();
            }
            Ok(observation(AgentPresence::Offline, AgentActivity::Unknown))
        });
        let state = server.state.lock().unwrap();
        assert!(state.agent_presence.is_empty());
        assert!(state.agent_activity.is_empty());
        drop(state);
        assert!(std::fs::read(&server.journal_path).unwrap().is_empty());
        std::fs::remove_dir_all(&server.root).unwrap();
    }
}

#[test]
fn only_tmux_with_agent_session_is_observed() {
    let server = server();
    {
        let mut state = server.state.lock().unwrap();
        let transport = state
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap();
        transport.session_id = None;
        transport.tmux_endpoint.as_mut().unwrap().codex_session_id = None;
    }
    tick_with_observer(&server, 1, &|_| {
        panic!("tmux session is not agent identity")
    });
    {
        let mut state = server.state.lock().unwrap();
        let transport = state
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap();
        transport.session_id = Some("agent-session".into());
        transport.kind = TransportKind::AppServer;
    }
    tick_with_observer(&server, 2, &|_| {
        panic!("other transport must remain untouched")
    });
    assert!(server.state.lock().unwrap().agent_presence.is_empty());
    std::fs::remove_dir_all(&server.root).unwrap();
}

#[test]
fn idle_interval_requires_current_binding_and_continuity() {
    let server = server();
    tick_with_observer(&server, 1_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(1_000),
        "first trusted online idle observation starts the trusted interval"
    );

    tick_with_observer(&server, 6_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(1_000),
        "continuous idle under the same binding preserves the trusted start"
    );

    for (source, label) in [
        (AgentActivity::Busy, "busy"),
        (AgentActivity::WaitingUser, "waiting_user"),
    ] {
        tick_with_observer(&server, 7_000, &|_| {
            Ok(observation(AgentPresence::Online, source))
        });
        assert_eq!(
            server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
            None,
            "{label} must clear the trusted idle start"
        );
        tick_with_observer(&server, 8_000, &|_| {
            Ok(observation(AgentPresence::Online, AgentActivity::Idle))
        });
        assert_eq!(
            server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
            Some(8_000),
            "{label} break starts a fresh trusted interval"
        );
    }

    tick_with_observer(&server, 9_000, &|_| {
        Ok(observation(AgentPresence::Unknown, AgentActivity::Unknown))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        None,
        "unknown activity must clear the trusted idle start"
    );
    tick_with_observer(&server, 10_000, &|_| {
        Ok(observation(AgentPresence::Offline, AgentActivity::Unknown))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        None,
        "offline observation must stay untrusted"
    );

    tick_with_observer(&server, 11_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    {
        let mut state = server.state.lock().unwrap();
        let transport = state
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap();
        transport.session_id = Some("new-session".into());
    }
    tick_with_observer(&server, 12_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(12_000),
        "a new session starts a fresh trusted interval instead of reusing the old start"
    );

    tick_with_observer(&server, 13_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    {
        let mut state = server.state.lock().unwrap();
        let activity = state
            .agent_activity
            .get_mut(&scope(&server))
            .expect("scope present")
            .get_mut("worker")
            .unwrap();
        activity.observed_ms = 20_000;
    }
    tick_with_observer(&server, 14_000, &|_| {
        Ok(observation(AgentPresence::Online, AgentActivity::Idle))
    });
    assert_eq!(
        server.state.lock().unwrap().agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(14_000),
        "a clock rollback must not keep a stale trusted idle start"
    );

    let mut replayed = State::default();
    for line in std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let event: crate::server::state::Event = serde_json::from_str(line).unwrap();
        replayed.apply(&event);
    }
    assert_eq!(
        replayed.agent_activity[&scope(&server)]["worker"].idle_since_ms,
        Some(14_000),
        "typed activity continuity replays through the shared journal"
    );
    std::fs::remove_dir_all(&server.root).unwrap();
}
