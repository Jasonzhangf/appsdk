use super::*;
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(2500);
const POLL: Duration = Duration::from_secs(10);
const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

struct PeerFacts {
    master: String,
    peer: String,
    peer_name: String,
    peer_session: String,
    project: String,
}

fn dispatch_gap_notices(state: &Value, master: &str) -> Vec<Value> {
    state["msgs"].as_object().map_or(Vec::new(), |msgs| {
        msgs.values()
            .filter(|message| {
                message["to"].as_str() == Some(master)
                    && message["subject"].as_str() == Some("master-dispatch-gap")
            })
            .cloned()
            .collect::<Vec<_>>()
    })
}

fn wait_for_gap_notices(fixture: &AppFixture, master: &str, want: usize) -> Vec<Value> {
    let deadline = Instant::now() + POLL;
    loop {
        let notices = dispatch_gap_notices(&fixture.current_state(), master);
        if notices.len() >= want {
            return notices;
        }
        assert!(
            Instant::now() < deadline,
            "expected {want} master-dispatch-gap notice(s), got {}",
            notices.len()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn setup_master_and_peer(fixture: &mut AppFixture) -> PeerFacts {
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker B")
        .to_owned();
    let promoted = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture keeps worker B as master"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(promoted["master"]["worker_id"], worker_b);
    let transport = fixture.current_state()["workers"][&worker_a]["transport"].clone();
    let peer_session = transport["session_id"]
        .as_str()
        .expect("peer transport carries a session id")
        .to_owned();
    let project = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    PeerFacts {
        master: worker_b,
        peer: worker_a,
        peer_name: context_a["name"]
            .as_str()
            .expect("stable peer name")
            .to_owned(),
        peer_session,
        project,
    }
}

fn seed_peer_activity(
    fixture: &mut AppFixture,
    project: &str,
    peer: &str,
    session: &str,
    presence: &str,
    activity: &str,
) {
    fixture.update_current_state(|state| {
        state["agent_presence"][project][peer] = json!({
            "project_scope": project,
            "worker_id": peer,
            "session_id": session,
            "presence": presence,
            "reason": "fixture",
            "observed_ms": 1
        });
        state["agent_activity"][project][peer] = json!({
            "project_scope": project,
            "worker_id": peer,
            "session_id": session,
            "activity": activity,
            "background": false,
            "reason": "fixture",
            "observed_ms": 2,
            "idle_since_ms": 2
        });
    });
}

fn publish_waiting_assignment_ticket(
    fixture: &AppFixture,
    id: &str,
    thread: &str,
    extra_policy: &[&str],
) {
    let output = fixture.run_public(
        &[
            "board",
            "publish",
            id,
            "--title",
            id,
            "--description",
            "dispatch gap smoke",
            "--delivery-condition",
            "name continues work",
            "--test-condition",
            "public CLI assertions",
        ]
        .iter()
        .chain(extra_policy)
        .copied()
        .collect::<Vec<_>>()
        .as_slice(),
        thread,
    );
    let state = fixture.current_state();
    let details = state["board_details"][id].clone();
    assert_eq!(
        details["public_visibility"], true,
        "publish created a public task: {output:?}"
    );
    let phase = state["ticket_phases"]
        .as_array()
        .expect("state carries the ticket phase projection")
        .iter()
        .find(|phase| phase["task_id"] == id)
        .cloned()
        .expect("admitted public ticket carries an execute phase");
    assert_eq!(
        phase["status"], "waiting_assignment",
        "an unavailable board must leave the execute phase waiting for a slot: {output:?}"
    );
}

#[test]
fn dispatch_gap_reminder_is_delivered_once_and_idempotent_for_600000ms() {
    let mut fixture = AppFixture::new();
    let facts = setup_master_and_peer(&mut fixture);
    let task_id = "dispatch-gap-task";
    // The only peer is offline at publish, so admission cannot place a slot
    // and the execute phase stays waiting_assignment: a real undispatched
    // ticket rather than one the publish flow already claimed.
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "offline",
        "idle",
    );
    publish_waiting_assignment_ticket(&mut fixture, task_id, THREAD_B, &[]);

    // An offline peer is not idle capacity, so the gap is not reportable yet.
    thread::sleep(SETTLE);
    assert!(
        dispatch_gap_notices(&fixture.current_state(), &facts.master).is_empty(),
        "an offline peer must not open a dispatch gap"
    );

    // Free the peer: the admitted ticket now has a real eligible idle peer.
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "online",
        "idle",
    );
    let notices = wait_for_gap_notices(&fixture, &facts.master, 1);
    assert_eq!(notices.len(), 1, "one durable reminder per gap");
    assert_eq!(notices[0]["to"], facts.master);
    assert_eq!(notices[0]["subject"], "master-dispatch-gap");

    let wake = fixture.current_state()["master_wake"].clone();
    assert_eq!(wake["dispatch_gap_task_ids"], json!([task_id]));
    assert_eq!(wake["dispatch_gap_idle_peer_ids"], json!([facts.peer]));
    assert!(wake["last_dispatch_gap_notice_ms"].as_i64().unwrap_or(0) > 0);
    assert_eq!(wake["dispatch_gap_delivery_state"], "delivered");

    let durable = fixture.current_state()["tasks"][task_id].clone();
    assert_eq!(durable["owner"], facts.master);
    let phase = fixture.current_state()["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == task_id)
        .cloned()
        .unwrap();
    assert_eq!(
        phase["status"], "waiting_assignment",
        "the reminder must not auto-dispatch or mutate the phase"
    );
    assert_eq!(durable["owner"], facts.master);

    thread::sleep(SETTLE);
    let notices = dispatch_gap_notices(&fixture.current_state(), &facts.master);
    assert_eq!(
        notices.len(),
        1,
        "the same condition must not re-notify inside the 600000ms window"
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    fixture.update_current_state(|state| {
        state["master_wake"]["last_dispatch_gap_notice_ms"] = json!(now - 600_000);
    });
    let notices = wait_for_gap_notices(&fixture, &facts.master, 2);
    assert_eq!(
        notices.len(),
        2,
        "the condition may notify again once the 600000ms window closes"
    );
    let wake = fixture.current_state()["master_wake"].clone();
    assert_eq!(wake["dispatch_gap_task_ids"], json!([task_id]));
    assert_eq!(wake["dispatch_gap_idle_peer_ids"], json!([facts.peer]));

    drop(fixture);
}

#[test]
fn dispatch_gap_reminder_skips_when_no_undispatched_ticket_exists() {
    let mut fixture = AppFixture::new();
    let facts = setup_master_and_peer(&mut fixture);
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "online",
        "idle",
    );

    thread::sleep(SETTLE);
    let notices = dispatch_gap_notices(&fixture.current_state(), &facts.master);
    assert!(
        notices.is_empty(),
        "no undispatched ticket means no reminder: {notices:?}"
    );

    // Admit a ticket while the only peer is offline, then free it: this is a
    // real undispatched ticket and must open the reminder.
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "offline",
        "idle",
    );
    publish_waiting_assignment_ticket(&mut fixture, "dispatch-gap-task", THREAD_B, &[]);
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "online",
        "idle",
    );
    let notices = wait_for_gap_notices(&fixture, &facts.master, 1);
    assert_eq!(notices.len(), 1);

    drop(fixture);
}

#[test]
fn dispatch_gap_reminder_skips_busy_offline_and_waiting_peers() {
    let mut fixture = AppFixture::new();
    let facts = setup_master_and_peer(&mut fixture);
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "offline",
        "idle",
    );
    publish_waiting_assignment_ticket(&mut fixture, "dispatch-gap-task", THREAD_B, &[]);

    for (presence, activity) in [
        ("offline", "idle"),
        ("online", "busy"),
        ("online", "waiting_user"),
        ("unknown", "idle"),
    ] {
        seed_peer_activity(
            &mut fixture,
            &facts.project,
            &facts.peer,
            &facts.peer_session,
            presence,
            activity,
        );
        thread::sleep(SETTLE);
        let notices = dispatch_gap_notices(&fixture.current_state(), &facts.master);
        assert!(
            notices.is_empty(),
            "{presence}/{activity} is not idle capacity: {notices:?}"
        );
    }

    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "online",
        "idle",
    );
    let notices = wait_for_gap_notices(&fixture, &facts.master, 1);
    assert_eq!(notices.len(), 1);

    drop(fixture);
}

#[test]
fn dispatch_gap_reminder_requires_pinned_executor_to_be_idle_and_available() {
    let mut fixture = AppFixture::new();
    let facts = setup_master_and_peer(&mut fixture);
    let unrelated = fixture.run_public(&["context"], THREAD_C);
    let unrelated_worker = unrelated["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names the unrelated peer")
        .to_owned();
    let unrelated_session = fixture.current_state()["workers"][&unrelated_worker]["transport"]
        ["session_id"]
        .as_str()
        .expect("unrelated peer transport carries a session id")
        .to_owned();
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &unrelated_worker,
        &unrelated_session,
        "online",
        "idle",
    );
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &unrelated_worker,
        &unrelated_session,
        "online",
        "idle",
    );
    // The pinned executor is offline at publish so admission cannot place the
    // phase and the ticket is genuinely undispatched.
    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "offline",
        "idle",
    );
    publish_waiting_assignment_ticket(
        &mut fixture,
        "pinned-dispatch-gap",
        THREAD_B,
        &["--executor", facts.peer_name.as_str()],
    );

    thread::sleep(SETTLE);
    let notices = dispatch_gap_notices(&fixture.current_state(), &facts.master);
    assert!(
        notices.is_empty(),
        "an unrelated idle peer must not report a task pinned to a busy executor: {notices:?}"
    );

    seed_peer_activity(
        &mut fixture,
        &facts.project,
        &facts.peer,
        &facts.peer_session,
        "online",
        "idle",
    );
    let notices = wait_for_gap_notices(&fixture, &facts.master, 1);
    assert_eq!(notices.len(), 1);
    let wake = fixture.current_state()["master_wake"].clone();
    assert_eq!(
        wake["dispatch_gap_idle_peer_ids"],
        json!([facts.peer]),
        "only the pinned executor is eligible idle capacity"
    );

    drop(fixture);
}
