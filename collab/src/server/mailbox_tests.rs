//! Tests for the mailbox projection and batching helpers.
use super::*;
use crate::server::state::{now_ms, Event, Message};
use std::sync::Mutex;

fn test_server() -> (crate::server::Server, PathBuf) {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-mailbox-projection-{}-{sequence}",
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
        crate::server::Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(crate::server::state::State::default()),
            journal: Mutex::new(journal),
            appserver_candidate_check: crate::server::default_appserver_candidate_check(),
            appserver_notification_sink: crate::server::default_appserver_notification_sink(),
            appserver_thread_status: crate::server::default_appserver_thread_status(),
            appserver_thread_archive: crate::server::default_appserver_thread_archive(),
            mailbox_notify: tokio::sync::Notify::new(),
        },
        root,
    )
}

fn msg(id: &str) -> Message {
    Message {
        id: id.into(),
        from: "server".into(),
        to: "master".into(),
        mtype: "notify".into(),
        subject: Some(format!("worker-idle:{id}")),
        body: format!("body {id}"),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    }
}

#[test]
fn batch_selection_is_bounded_and_returns_overflow_count() {
    let candidates = (0..5)
        .map(|index| {
            let id = format!("m{index}");
            let text = notification_text(&msg(&id)).unwrap();
            (index, id, "sub".into(), "direct-message".into(), text)
        })
        .collect::<Vec<_>>();
    let (batch, remaining) = select_batch(candidates, 120_000, 0);
    assert_eq!(batch.len(), 3);
    assert_eq!(remaining, 2);
    assert!(batch.first().is_some_and(|item| item.1 == "m2"));
}

#[test]
fn mailbox_repair_removes_only_trailing_malformed_records() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-test-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mailbox = dir.join(".agent-collab/mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    let path = mailbox.join("recipient-master.jsonl");
    let message = msg("m1");
    std::fs::write(&path, "not-json\n").unwrap();
    assert!(recover_malformed_mailbox_tail(&path, "master").is_ok());
    assert!(recover_malformed_mailbox_tail(&path, "master").is_ok());
    backup_message(&dir, &message).unwrap();
    let read = read_recipient_mailbox(&path, "master").unwrap();
    assert_eq!(read.records.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mailbox_repair_preserves_syntactically_valid_invalid_schema_tail() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-valid-json-invalid-schema-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mailbox = dir.join(".agent-collab/mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    let path = mailbox.join("recipient-master.jsonl");
    let raw = b"{\"schema_version\":999,\"record_type\":\"unknown\"}\n";
    std::fs::write(&path, raw).unwrap();

    assert!(!recover_malformed_mailbox_tail(&path, "master").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert!(read_recipient_mailbox(&path, "master").is_err());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mailbox_repair_truncates_only_syntactically_malformed_tail() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-malformed-tail-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mailbox = dir.join(".agent-collab/mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    let path = mailbox.join("recipient-master.jsonl");
    let prefix = b"{\"schema_version\":999,\"record_type\":\"unknown\"}\n";
    let mut raw = prefix.to_vec();
    raw.extend_from_slice(b"{\"unterminated\":");
    std::fs::write(&path, &raw).unwrap();

    assert!(recover_malformed_mailbox_tail(&path, "master").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), prefix);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mailbox_rejects_non_basename_recipients_before_writing() {
    for recipient in ["", ".", "..", "/", "../", "worker\n"] {
        let dir = std::env::temp_dir().join(format!(
            "collab-mailbox-invalid-recipient-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let mut message = msg("invalid-recipient");
        message.to = recipient.into();

        let error = backup_message(&dir, &message).unwrap_err();
        assert!(error.contains("mailbox recipient"), "{error}");
        assert!(!dir.exists(), "invalid recipient created {dir:?}");
    }
}

#[test]
fn mailbox_keeps_valid_recipient_projection_path() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-valid-recipient-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let message = msg("valid-recipient");
    backup_message(&dir, &message).unwrap();

    assert!(dir
        .join(".agent-collab/mailbox/recipient-master.jsonl")
        .is_file());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mailbox_projection_keeps_goal_deadline_p0_only_for_internal_notification() {
    for (id, from, mtype, subject, expected_priority) in [
        ("ordinary-goal", "server", "notify", "goal:plan.md", "P1"),
        ("forged-type", "peer", "notification", "goal:plan.md", "P1"),
        (
            "forged-sender",
            "collab-server",
            "notify",
            "deadline:goal:plan.md",
            "P1",
        ),
        (
            "internal-goal",
            "collab-server",
            "notification",
            "goal:plan.md",
            "P0",
        ),
        (
            "internal-deadline",
            "collab-server",
            "notification",
            "deadline:goal:plan.md",
            "P0",
        ),
    ] {
        let message = Message {
            id: id.into(),
            from: from.into(),
            mtype: mtype.into(),
            subject: Some(subject.into()),
            ..msg(id)
        };
        let envelope = mailbox_event_envelope(&message);
        let text = notification_text(&message).unwrap();
        let action = envelope["action"].as_str().unwrap();
        assert_eq!(envelope["priority"], expected_priority);
        assert!(
            text.contains(&format!("{expected_priority} ACTION: {action}")),
            "{text}"
        );
    }
}

#[test]
fn raw_mailbox_retains_event_kind_identity_raw_entity_and_unavailable_scope_error() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-raw-retention-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mut message = msg("raw-retention");
    message.in_reply_to = Some("task-raw".into());
    backup_message(&dir, &message).unwrap();

    message.state = "delivered".into();
    backup_message(&dir, &message).unwrap();

    message.state = "read".into();
    backup_message(&dir, &message).unwrap();

    let raw_path = dir.join(".agent-collab/mailbox/recipient-master.jsonl");
    let read = read_recipient_mailbox(&raw_path, "master").unwrap();
    assert_eq!(read.records.len(), 3);
    let event_kinds = read
        .records
        .iter()
        .filter_map(|record| record["event_kind"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(event_kinds, vec!["sent", "delivered", "acked"]);
    for record in &read.records {
        assert_eq!(record["from_agent_id"], "server");
        assert_eq!(record["to_agent_id"], "master");
        assert!(record["route_scope"].is_null());
        assert!(record["binding_id"].is_null());
        assert_eq!(record["entity_key"], "thread:task-raw");
        assert!(record["exact_error"]
            .as_str()
            .is_some_and(|error| error.starts_with("MAILBOX_SCOPE_BINDING_UNAVAILABLE:")));
        assert_eq!(record["raw_body"], "body raw-retention");
        assert_eq!(record["raw_reference"], "raw-retention");
        assert_eq!(record["message"]["id"], "raw-retention");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn raw_mailbox_cross_field_inconsistency_fails_closed_and_blocks_rebuild() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-raw-reject-inconsistent-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let message = msg("raw-cross-field");
    backup_message(&dir, &message).unwrap();

    let raw_path = dir.join(".agent-collab/mailbox/recipient-master.jsonl");
    let mut records = std::fs::read_to_string(&raw_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    records[0]["raw_reference"] = serde_json::json!("different-id");
    let body = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&raw_path, format!("{body}\n")).unwrap();

    let read_error = read_recipient_mailbox(&raw_path, "master").unwrap_err();
    assert!(read_error.contains("raw_reference"), "{read_error}");
    let rebuild_error = rebuild_latest_projection(&dir, "master").unwrap_err();
    assert!(rebuild_error.contains("raw_reference"), "{rebuild_error}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn latest_projection_is_idempotent_per_projection_key() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-latest-idempotent-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mut message = msg("latest-idempotent");
    backup_message(&dir, &message).unwrap();
    message.state = "delivered".into();
    backup_message(&dir, &message).unwrap();
    message.state = "read".into();
    backup_message(&dir, &message).unwrap();
    backup_message(&dir, &message).unwrap();

    let latest_path = latest_mailbox_path(&dir, "master");
    let latest = read_latest_mailbox_projection(&latest_path, "master").unwrap();
    assert_eq!(latest.records.len(), 1);
    assert_eq!(latest.records[0]["state"], "read");
    assert_eq!(latest.records[0]["event_kind"], "acked");
    assert_eq!(
        std::fs::read_to_string(&latest_path)
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn malformed_latest_projection_tail_is_rebuilt_from_raw() {
    let dir = std::env::temp_dir().join(format!(
        "collab-mailbox-latest-repair-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let mut message = msg("latest-repair");
    backup_message(&dir, &message).unwrap();

    let latest_path = latest_mailbox_path(&dir, "master");
    let mut content = std::fs::read_to_string(&latest_path).unwrap();
    content.push_str("{\"partial\":");
    std::fs::write(&latest_path, content).unwrap();

    message.state = "delivered".into();
    backup_message(&dir, &message).unwrap();

    let latest = read_latest_mailbox_projection(&latest_path, "master").unwrap();
    assert_eq!(latest.records.len(), 1);
    assert_eq!(latest.records[0]["state"], "delivered");
    assert_eq!(latest.records[0]["event_kind"], "delivered");
    let raw = read_recipient_mailbox(
        &dir.join(".agent-collab/mailbox/recipient-master.jsonl"),
        "master",
    )
    .unwrap();
    assert_eq!(raw.records.len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mailbox_events_survive_journal_replay_with_latest_state() {
    let (server, root) = test_server();
    let id = "replay-latest";
    let message = Message {
        id: id.into(),
        from: "sender".into(),
        to: "master".into(),
        mtype: "notify".into(),
        subject: Some("progress".into()),
        body: "replay raw body".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    server.commit(&[
        Event::Sent {
            msg: message.clone(),
        },
        Event::Delivered {
            ids: vec![id.into()],
        },
        Event::Acked {
            ids: vec![id.into()],
        },
    ]);

    let latest_path = latest_mailbox_path(&root, "master");
    let latest = read_latest_mailbox_projection(&latest_path, "master").unwrap();
    assert_eq!(latest.records.len(), 1);
    assert_eq!(latest.records[0]["state"], "read");
    drop(server);

    let replayed = crate::server::replay(&root).unwrap();
    assert_eq!(replayed.msgs[id].state, "read");
    assert_eq!(replayed.msgs[id].body, "replay raw body");
    std::fs::remove_dir_all(root).unwrap();
}
