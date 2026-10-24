//! Idle reminder behaviour for durable reply obligations.
//!
//! Every case drives the real keepalive tick with typed agent observation.
//! No pane text is parsed and no production status is faked.

use super::super::name_state::{
    AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord,
};
use super::super::peer_tests::{register_synthetic_peer, test_server};
use super::super::state::{now_ms, Event, Message, ReplyObligation, State};
use super::{tick_at, Server};
use crate::proto::ReplyPolicy;
use std::path::PathBuf;

fn observe(server: &Server, worker_id: &str, activity: AgentActivity, session: &str) {
    let scope = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    server.commit(&[
        Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: scope.clone(),
                worker_id: worker_id.into(),
                session_id: session.into(),
                presence: AgentPresence::Online,
                reason: "fixture".into(),
                observed_ms: 1,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: scope,
                worker_id: worker_id.into(),
                session_id: session.into(),
                activity,
                background: false,
                reason: "fixture".into(),
                observed_ms: 1,
            },
        },
    ]);
}

fn setup() -> (Server, PathBuf) {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "worker", "thread-worker");
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
        .session_id = Some("current".into());
    (server, root)
}

fn seed_obligation(server: &Server, message_id: &str, from: &str, policy: ReplyPolicy) {
    let project = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    server.commit(&[Event::ReplyObligationSet {
        obligation: ReplyObligation {
            message_id: message_id.into(),
            project,
            from_project: None,
            from_worker_id: from.into(),
            from_name: Some(from.into()),
            recipient_worker_id: "worker".into(),
            recipient_name: Some("worker".into()),
            policy,
            created_ms: now_ms(),
            reminded_idle_episode: false,
        },
    }]);
}

fn reminders(server: &Server) -> Vec<Message> {
    server
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .filter(|message| {
            message.to == "worker"
                && message.from == "collab-server"
                && message.subject.as_deref() == Some("reply-pending")
        })
        .cloned()
        .collect()
}

fn obligation_view(server: &Server) -> Vec<serde_json::Value> {
    server.state.lock().unwrap().reply_obligation_view("worker")
}

fn replayed_state(server: &Server) -> State {
    let mut state = State::default();
    for line in std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
    {
        state
            .apply_checked(&serde_json::from_str::<Event>(line).unwrap())
            .unwrap();
    }
    state
}

fn idle_episode(server: &Server, now: i64) {
    observe(server, "worker", AgentActivity::Busy, "current");
    tick_at(server, now);
    observe(server, "worker", AgentActivity::Idle, "current");
    tick_at(server, now + 1);
}

#[test]
fn cross_project_idle_reminder_uses_ceo_reply_entry() {
    let (server, root) = setup();
    seed_obligation(
        &server,
        "ceo-request",
        "global-ceo",
        ReplyPolicy::OnCompletion,
    );
    let mut obligation = server.state.lock().unwrap().reply_obligations["ceo-request"].clone();
    obligation.from_project = Some("/workspace".into());
    server.commit(&[Event::ReplyObligationSet { obligation }]);
    idle_episode(&server, now_ms());
    let notices = reminders(&server);
    assert_eq!(notices.len(), 1);
    let body = &notices[0].body;
    assert!(
        body.contains("collab ceo reply"),
        "cross-project reminder used a local reply address: {body}"
    );
    assert!(!body.contains("collab sendmessage"), "{body}");
    assert!(
        body.contains("ceo-request") && body.contains("on_completion"),
        "{body}"
    );
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .reply_obligations
            .contains_key("ceo-request"),
        "a reminder must not clear the obligation"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_waiting_reminds_once_with_original_fields() {
    let (server, root) = setup();
    seed_obligation(&server, "msg-a", "sender", ReplyPolicy::Required);
    idle_episode(&server, now_ms());

    let notices = reminders(&server);
    assert_eq!(notices.len(), 1, "one reminder per idle episode");
    let body = &notices[0].body;
    assert!(body.contains("message_id=msg-a"), "{body}");
    assert!(body.contains("from=sender"), "{body}");
    assert!(body.contains("policy=required"), "{body}");
    assert!(body.contains("collab msg"), "{body}");
    assert!(body.contains("--in-reply-to"), "{body}");
    assert!(!body.contains("--body"), "{body}");
    assert_eq!(obligation_view(&server)[0]["reminded_idle_episode"], true);

    tick_at(&server, now_ms() + 60_000);
    assert_eq!(
        reminders(&server).len(),
        1,
        "the same idle episode must not spam"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_working_and_unknown_do_not_remind() {
    let (server, root) = setup();
    seed_obligation(&server, "msg-b", "sender", ReplyPolicy::Required);

    observe(&server, "worker", AgentActivity::Busy, "current");
    tick_at(&server, now_ms());
    assert!(reminders(&server).is_empty());

    observe(&server, "worker", AgentActivity::Unknown, "current");
    tick_at(&server, now_ms() + 1);
    assert!(reminders(&server).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_working_again_rearms_the_next_idle_episode() {
    let (server, root) = setup();
    seed_obligation(&server, "msg-c", "sender", ReplyPolicy::OnCompletion);
    let now = now_ms();
    idle_episode(&server, now);
    assert_eq!(reminders(&server).len(), 1);
    assert_eq!(obligation_view(&server)[0]["reminded_idle_episode"], true);

    observe(&server, "worker", AgentActivity::Busy, "current");
    tick_at(&server, now + 2);
    assert_eq!(reminders(&server).len(), 1);
    assert_eq!(obligation_view(&server)[0]["reminded_idle_episode"], false);

    observe(&server, "worker", AgentActivity::Idle, "current");
    tick_at(&server, now + 3);
    assert_eq!(reminders(&server).len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_without_obligation_stays_quiet() {
    let (server, root) = setup();
    idle_episode(&server, now_ms());
    assert!(reminders(&server).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_reminder_and_plain_result_add_no_obligation() {
    let (server, root) = setup();
    seed_obligation(&server, "msg-d", "sender", ReplyPolicy::Required);
    idle_episode(&server, now_ms());

    let notice = reminders(&server).remove(0);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.reply_obligations.len(), 1);
        assert!(!state.reply_obligations.contains_key(&notice.id));
    }

    let result_id = "result-msg-d";
    server.commit(&[Event::Sent {
        msg: Message {
            id: result_id.into(),
            from: "worker".into(),
            to: "sender".into(),
            mtype: "notify".into(),
            subject: Some("done".into()),
            body: "handled".into(),
            in_reply_to: Some("msg-d".into()),
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let state = server.state.lock().unwrap();
    assert_eq!(state.reply_obligations.len(), 1);
    assert!(!state.reply_obligations.contains_key(result_id));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reply_idle_replayed_reminder_state_does_not_repeat() {
    let (server, root) = setup();
    seed_obligation(&server, "msg-e", "sender", ReplyPolicy::Required);
    idle_episode(&server, now_ms());
    assert_eq!(reminders(&server).len(), 1);

    *server.state.lock().unwrap() = replayed_state(&server);
    assert_eq!(obligation_view(&server)[0]["reminded_idle_episode"], true);

    tick_at(&server, now_ms() + 60_000);
    assert_eq!(
        reminders(&server).len(),
        1,
        "replayed reminder state must not repeat"
    );
    std::fs::remove_dir_all(root).unwrap();
}
