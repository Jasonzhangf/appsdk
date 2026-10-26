use super::state::Message;
use super::*;
use crate::ceo::CeoGrant;
use crate::ceo_report_state::{CeoNoticeGate, CeoProgressContent};
use crate::server::global_state::GlobalState;
use crate::server::name_resolution::build_name_resolution_events;
use crate::server::name_state::NameOccupancy;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Weak};

const CEO_WORKER: &str = "ceo";
const CEO_NAME: &str = "CEO";
const CEO_THREAD: &str = "thread-ceo";
const NOTICE_SUBJECT: &str = "ceo-report-due";
const PATROL_INTERVAL_MS: u64 = 900_000;
const BEIJING_OFFSET_MS: u64 = 28_800_000;
const DAY_MS: u64 = 86_400_000;

type ThreadStatus = dyn Fn(&crate::proto::SelectedTransport, &str) -> Result<serde_json::Value, String>
    + Send
    + Sync;

struct PatrolFixture {
    server: Arc<Server>,
    manager: Arc<ProjectRuntimeManager>,
    root: PathBuf,
    probes: Arc<AtomicUsize>,
}

impl Drop for PatrolFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn beijing_at(minute: u32) -> i64 {
    let utc_day = 1_700_000_000_000_u64 / DAY_MS * DAY_MS;
    utc_day
        .saturating_add(u64::from(minute) * 60_000)
        .saturating_sub(BEIJING_OFFSET_MS) as i64
}

fn idle_status(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "thread": {
            "id": thread_id,
            "status": {"type": "idle"},
            "canAcceptDirectInput": true
        }
    })
}

fn busy_status(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "thread": {
            "id": thread_id,
            "status": {"type": "active", "activeFlags": []},
            "canAcceptDirectInput": false
        }
    })
}

fn patrol_fixture(status: Arc<ThreadStatus>, probes: Arc<AtomicUsize>) -> PatrolFixture {
    patrol_fixture_inner(status, probes, None)
}

fn patrol_fixture_inner(
    status: Arc<ThreadStatus>,
    probes: Arc<AtomicUsize>,
    weak_server: Option<Arc<std::sync::OnceLock<Weak<Server>>>>,
) -> PatrolFixture {
    let (mut server, root) = peer_tests::test_server();
    server.appserver_thread_status = status;
    let canonical_root = root.canonicalize().unwrap();
    server.root = canonical_root.clone();
    server.storage_root = canonical_root.clone();
    let server = Arc::new(server);
    if let Some(slot) = weak_server {
        slot.set(Arc::downgrade(&server)).ok();
    }
    let worker =
        peer_tests::register_appserver_worker(&server, &canonical_root, CEO_WORKER, CEO_THREAD);
    let project_scope = GlobalState::canonical_project_scope(&server.root).unwrap();
    let session_id = worker
        .transport
        .as_ref()
        .and_then(|transport| transport.session_id.clone())
        .unwrap_or_else(|| format!("session-{CEO_THREAD}"));
    {
        let mut state = server.state.lock().unwrap();
        let occupancy = NameOccupancy {
            name: CEO_NAME.into(),
            worker_id: CEO_WORKER.into(),
            session_id,
            occupancy_revision: 1,
            updated_ms: now_ms(),
        };
        let events = build_name_resolution_events(&state, project_scope.as_str(), &occupancy);
        server.commit_locked_checked(&mut state, &events).unwrap();
    }
    let workspace = server.root.to_string_lossy().into_owned();
    server.commit(&[Event::CeoGrantSet {
        grant: Some(CeoGrant {
            workspace: workspace.clone(),
            app_scope_id: "tui-default".into(),
            name: CEO_NAME.into(),
            granted_by: "user".into(),
            approval: "approved".into(),
            granted_ms: now_ms(),
        }),
    }]);
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .name_occupancy(&workspace, CEO_NAME)
            .unwrap()
            .worker_id,
        CEO_WORKER
    );
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    manager.install_runtime(
        &("tui-default".to_owned(), project_scope.as_str().to_owned()),
        server.clone(),
        None,
    );
    PatrolFixture {
        server,
        manager,
        root,
        probes,
    }
}

fn idle_fixture() -> PatrolFixture {
    let probes = Arc::new(AtomicUsize::new(0));
    let callback_probes = probes.clone();
    let status: Arc<ThreadStatus> = Arc::new(move |_, thread_id| {
        callback_probes.fetch_add(1, Ordering::SeqCst);
        Ok(idle_status(thread_id))
    });
    patrol_fixture(status, probes)
}

fn record_progress(server: &Server, subject: &str, body: &str, urgent: bool) {
    let project = server.root.to_string_lossy().into_owned();
    server.commit(&[Event::CeoProgressRecorded {
        project,
        content: CeoProgressContent::new(subject, body),
        urgent,
    }]);
}

fn set_gate(server: &Server, gate: CeoNoticeGate) {
    server.commit(&[Event::CeoReportNoticeGateUpdated { gate }]);
}

fn queued_reservation(server: &Server, message_id: &str) {
    server.commit(&[Event::Sent {
        msg: Message {
            id: message_id.into(),
            from: "collab-server".into(),
            to: CEO_WORKER.into(),
            mtype: "notify".into(),
            subject: Some(NOTICE_SUBJECT.into()),
            body: "already queued".into(),
            in_reply_to: None,
            created_ms: beijing_at(720),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
}

fn notice_messages(server: &Server) -> Vec<Message> {
    let state = server.state.lock().unwrap();
    state
        .msgs
        .values()
        .filter(|message| message.subject.as_deref() == Some(NOTICE_SUBJECT))
        .cloned()
        .collect()
}

fn event_variants_for(server: &Server, message_id: &str) -> Vec<String> {
    std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
        .filter(|line| line.contains(message_id))
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value
                .get("ev")
                .and_then(|tag| tag.as_str())
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn ceo_report_patrol_quiet_ordinary_stays_quiet_and_new_urgent_fires_once() {
    let fixture = idle_fixture();
    let quiet = beijing_at(120);
    record_progress(&fixture.server, "ordinary", "quiet details", false);
    fixture
        .manager
        .tick_ceo_report_patrol(quiet)
        .expect("quiet ordinary tick");
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 0);
    assert!(notice_messages(&fixture.server).is_empty());
    assert_eq!(
        fixture
            .server
            .state
            .lock()
            .unwrap()
            .ceo_report_state
            .notice_gate,
        CeoNoticeGate::default()
    );

    record_progress(&fixture.server, "urgent", "urgent details", true);
    fixture
        .manager
        .tick_ceo_report_patrol(quiet)
        .expect("urgent tick");
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 1);
    let notices = notice_messages(&fixture.server);
    assert_eq!(notices.len(), 1);
    let notice = &notices[0];
    assert_eq!(notice.to, CEO_WORKER);
    assert!(notice.body.contains("carries no prebound content"));
    assert!(!notice.body.contains("urgent details"));
    let state = fixture.server.state.lock().unwrap();
    assert!(state.ceo_report_state.batches.is_empty());
    assert_eq!(state.ceo_report_state.notice_gate.urgent_generation_seen, 1);
    assert_eq!(
        state.ceo_report_state.notice_gate.next_due_ms,
        quiet as u64 + PATROL_INTERVAL_MS
    );
    assert!(state
        .ceo_report_state
        .notice_gate
        .pending_message_id
        .is_none());
    assert_eq!(state.msgs[&notice.id].wake_attempt_count, 1);
    drop(state);

    let variants = event_variants_for(&fixture.server, &notice.id);
    let mode = variants.iter().position(|name| name == "DeliveryMode");
    let sent = variants.iter().position(|name| name == "Sent");
    assert!(mode.is_some(), "missing DeliveryMode in {variants:?}");
    assert!(sent.is_some(), "missing Sent in {variants:?}");
    assert!(mode < sent, "delivery mode must precede Sent: {variants:?}");
}

#[test]
fn ceo_report_patrol_busy_skips_without_catchup_and_new_window_uses_new_id() {
    let probes = Arc::new(AtomicUsize::new(0));
    let busy = Arc::new(AtomicBool::new(true));
    let callback_probes = probes.clone();
    let callback_busy = busy.clone();
    let status: Arc<ThreadStatus> = Arc::new(move |_, thread_id| {
        callback_probes.fetch_add(1, Ordering::SeqCst);
        Ok(if callback_busy.load(Ordering::SeqCst) {
            busy_status(thread_id)
        } else {
            idle_status(thread_id)
        })
    });
    let fixture = patrol_fixture(status, probes);
    let start = beijing_at(720);
    record_progress(&fixture.server, "ordinary", "first", false);
    fixture.manager.tick_ceo_report_patrol(start).unwrap();
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 1);
    assert!(notice_messages(&fixture.server).is_empty());
    assert_eq!(
        fixture
            .server
            .state
            .lock()
            .unwrap()
            .ceo_report_state
            .notice_gate
            .next_due_ms,
        start as u64 + PATROL_INTERVAL_MS
    );

    busy.store(false, Ordering::SeqCst);
    fixture
        .manager
        .tick_ceo_report_patrol(start + 300_000)
        .unwrap();
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 1);
    assert!(notice_messages(&fixture.server).is_empty());

    fixture
        .manager
        .tick_ceo_report_patrol(start + PATROL_INTERVAL_MS as i64)
        .unwrap();
    let first = notice_messages(&fixture.server)
        .pop()
        .expect("due notice after the skip window");
    assert!(fixture
        .server
        .state
        .lock()
        .unwrap()
        .notification_delivery_accepted
        .contains_key(&first.id));

    record_progress(&fixture.server, "ordinary", "second", false);
    fixture
        .manager
        .tick_ceo_report_patrol(start + 2 * PATROL_INTERVAL_MS as i64)
        .unwrap();
    let notices = notice_messages(&fixture.server);
    assert_eq!(notices.len(), 2);
    assert!(notices.iter().any(|message| message.id == first.id));
    assert!(notices.iter().any(|message| message.id != first.id));
}

#[test]
fn ceo_report_patrol_recovery_clears_queued_and_rechecks_unqueued_policy_and_activity() {
    let day = beijing_at(720);
    let quiet = beijing_at(120);

    let queued = idle_fixture();
    queued_reservation(&queued.server, "reserved-queued");
    set_gate(
        &queued.server,
        CeoNoticeGate {
            pending_message_id: Some("reserved-queued".into()),
            pending_worker_id: Some(CEO_WORKER.into()),
            ..CeoNoticeGate::default()
        },
    );
    queued.manager.tick_ceo_report_patrol(day).unwrap();
    assert_eq!(queued.probes.load(Ordering::SeqCst), 0);
    assert_eq!(
        queued.server.state.lock().unwrap().msgs["reserved-queued"].wake_attempt_count,
        0
    );
    assert!(queued
        .server
        .state
        .lock()
        .unwrap()
        .ceo_report_state
        .notice_gate
        .pending_message_id
        .is_none());

    let unqueued_idle = idle_fixture();
    record_progress(&unqueued_idle.server, "ordinary", "idle recovery", false);
    set_gate(
        &unqueued_idle.server,
        CeoNoticeGate {
            pending_message_id: Some("reserved-idle".into()),
            pending_worker_id: Some(CEO_WORKER.into()),
            ..CeoNoticeGate::default()
        },
    );
    unqueued_idle.manager.tick_ceo_report_patrol(day).unwrap();
    assert_eq!(unqueued_idle.probes.load(Ordering::SeqCst), 1);
    assert!(unqueued_idle
        .server
        .state
        .lock()
        .unwrap()
        .msgs
        .contains_key("reserved-idle"));

    let probes = Arc::new(AtomicUsize::new(0));
    let callback_probes = probes.clone();
    let status: Arc<ThreadStatus> = Arc::new(move |_, thread_id| {
        callback_probes.fetch_add(1, Ordering::SeqCst);
        Ok(busy_status(thread_id))
    });
    let unqueued_busy = patrol_fixture(status, probes);
    record_progress(&unqueued_busy.server, "ordinary", "busy recovery", false);
    set_gate(
        &unqueued_busy.server,
        CeoNoticeGate {
            pending_message_id: Some("reserved-busy".into()),
            pending_worker_id: Some(CEO_WORKER.into()),
            ..CeoNoticeGate::default()
        },
    );
    unqueued_busy.manager.tick_ceo_report_patrol(day).unwrap();
    assert_eq!(unqueued_busy.probes.load(Ordering::SeqCst), 1);
    let state = unqueued_busy.server.state.lock().unwrap();
    assert!(!state.msgs.contains_key("reserved-busy"));
    assert_eq!(
        state.ceo_report_state.notice_gate.next_due_ms,
        day as u64 + PATROL_INTERVAL_MS
    );
    assert!(state
        .ceo_report_state
        .notice_gate
        .pending_message_id
        .is_none());
    drop(state);

    let quiet_recovery = idle_fixture();
    record_progress(&quiet_recovery.server, "ordinary", "quiet recovery", false);
    set_gate(
        &quiet_recovery.server,
        CeoNoticeGate {
            pending_message_id: Some("reserved-quiet".into()),
            pending_worker_id: Some(CEO_WORKER.into()),
            ..CeoNoticeGate::default()
        },
    );
    quiet_recovery
        .manager
        .tick_ceo_report_patrol(quiet)
        .unwrap();
    assert_eq!(quiet_recovery.probes.load(Ordering::SeqCst), 0);
    let state = quiet_recovery.server.state.lock().unwrap();
    assert!(!state.msgs.contains_key("reserved-quiet"));
    assert!(state
        .ceo_report_state
        .notice_gate
        .pending_message_id
        .is_none());
    assert_eq!(
        state.ceo_report_state.notice_gate.next_due_ms,
        quiet as u64 + PATROL_INTERVAL_MS
    );
}

#[test]
fn ceo_report_patrol_revalidates_route_after_probe_and_preserves_probe_error() {
    let day = beijing_at(720);

    let probes = Arc::new(AtomicUsize::new(0));
    let callback_probes = probes.clone();
    let changed = Arc::new(AtomicBool::new(false));
    let callback_changed = changed.clone();
    let server_weak = Arc::new(std::sync::OnceLock::<Weak<Server>>::new());
    let status_server = server_weak.clone();
    let status: Arc<ThreadStatus> = Arc::new(move |_, thread_id| {
        callback_probes.fetch_add(1, Ordering::SeqCst);
        if !callback_changed.swap(true, Ordering::SeqCst) {
            let server = status_server
                .get()
                .and_then(Weak::upgrade)
                .expect("fixture server remains alive");
            let mut state = server.state.lock().unwrap();
            let worker = state.workers.get_mut(CEO_WORKER).unwrap();
            let transport = worker.transport.as_mut().unwrap();
            transport.thread_id = Some("thread-changed-during-probe".into());
            transport.session_id = Some("session-changed-during-probe".into());
        }
        Ok(idle_status(thread_id))
    });
    let binding_change = patrol_fixture_inner(status, probes, Some(server_weak));
    record_progress(&binding_change.server, "ordinary", "binding change", false);
    binding_change
        .manager
        .tick_ceo_report_patrol(day)
        .expect("changed route is a skip, not leaked native input");
    assert_eq!(binding_change.probes.load(Ordering::SeqCst), 1);
    assert!(notice_messages(&binding_change.server).is_empty());
    let state = binding_change.server.state.lock().unwrap();
    assert_eq!(
        state.ceo_report_state.notice_gate.next_due_ms,
        day as u64 + PATROL_INTERVAL_MS
    );
    assert!(state
        .ceo_report_state
        .notice_gate
        .pending_message_id
        .is_none());
    drop(state);

    let error_probes = Arc::new(AtomicUsize::new(0));
    let callback_error_probes = error_probes.clone();
    let error_status: Arc<ThreadStatus> = Arc::new(move |_, _| {
        callback_error_probes.fetch_add(1, Ordering::SeqCst);
        Err("native thread read exploded".into())
    });
    let error_fixture = patrol_fixture(error_status, error_probes);
    record_progress(&error_fixture.server, "ordinary", "probe error", false);
    let error = error_fixture
        .manager
        .tick_ceo_report_patrol(day)
        .expect_err("probe errors must remain visible");
    assert!(error.contains("native thread read exploded"), "{error}");
    assert_eq!(error_fixture.probes.load(Ordering::SeqCst), 1);
    assert!(notice_messages(&error_fixture.server).is_empty());
    assert_eq!(
        error_fixture
            .server
            .state
            .lock()
            .unwrap()
            .ceo_report_state
            .notice_gate
            .next_due_ms,
        day as u64 + PATROL_INTERVAL_MS
    );
    error_fixture
        .manager
        .tick_ceo_report_patrol(day)
        .expect("skip window suppresses a second probe");
    assert_eq!(error_fixture.probes.load(Ordering::SeqCst), 1);
}

#[test]
fn ceo_report_patrol_concurrent_ticks_at_beijing_eight_queue_one_notice() {
    let fixture = idle_fixture();
    let now = beijing_at(480);
    record_progress(&fixture.server, "ordinary", "automatic", false);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let manager = fixture.manager.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            manager.tick_ceo_report_patrol(now)
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap().expect("concurrent tick");
    }
    let notices = notice_messages(&fixture.server);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].wake_attempt_count, 1);
}
