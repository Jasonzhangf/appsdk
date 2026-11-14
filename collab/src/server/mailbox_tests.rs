//! Tests for the mailbox projection and batching helpers.
use super::*;
use crate::server::current_state;
use crate::server::state::{now_ms, Event, Message};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[test]
fn wake_text_keeps_location_action_and_one_read_action_only() {
    let text = notification_text(&msg("minimal-wake")).unwrap();
    assert!(
        text.starts_with(
            "COLLAB_NOTIFY minimal-wake [worker-idle:minimal-wake] body minimal-wake | P1 ACTION: "
        ),
        "{text}"
    );
    assert_eq!(
        text.matches("collab recv").count(),
        1,
        "a plain wake carries exactly one read action: {text}"
    );
    for repeated_rule in [
        "READ IS NOT DONE",
        "resume your current task",
        "not routinely to the user",
        "Do not reply to system reminders",
        "appsdk longhorizon show",
    ] {
        assert!(
            !text.contains(repeated_rule),
            "a plain wake must not repeat the generic execution protocol: {repeated_rule} in {text}"
        );
    }
}

#[test]
fn batch_wake_keeps_entries_and_one_read_action_only() {
    let entries = vec![
        (
            0_i64,
            "msg-a".to_string(),
            String::new(),
            "direct-message".to_string(),
            notification_text(&msg("a")).unwrap(),
        ),
        (
            1_i64,
            "msg-b".to_string(),
            String::new(),
            "notify".to_string(),
            compose_notification("msg-b", "user-question", "用户问了一个需要区分的具体问题"),
        ),
    ];
    let batch = batch_notification_text(&entries, 2);
    assert!(
        batch.starts_with(
            "Batch wake: entries=msg-a:worker-idle:a=body a;msg-b:user-question=用户问了一个需要区分的具体问题."
        ),
        "{batch}"
    );
    assert!(
        !batch.contains("message_ids="),
        "a batch wake must not re-list ids that each entry already carries: {batch}"
    );
    assert!(
        !batch.contains(" last="),
        "a batch wake must not repeat the last entry title: {batch}"
    );
    assert!(
        batch.contains("msg-b:user-question=用户问了一个需要区分的具体问题"),
        "each batch entry must carry its own title and a faithful preview: {batch}"
    );
    assert!(batch.contains("older_messages=2"), "{batch}");
    for removed in ["task_ids", "collab inbox", "ACK", "resume", "Reply to"] {
        assert!(
            !batch.contains(removed),
            "a batch wake must not repeat execution rules: {removed} in {batch}"
        );
    }
    let composed = compose_notification("msg-a", "notification-batch", &batch);
    assert_eq!(
        composed.matches("collab recv").count(),
        1,
        "the composed batch wake carries exactly one read action: {composed}"
    );
}

#[test]
fn content_first_title_is_derived_from_the_first_non_empty_body_line() {
    assert_eq!(
        notification_subject_from_body("\n  第一行标题  \n第二行正文").as_deref(),
        Some("第一行标题")
    );
    assert_eq!(
        notification_subject_from_body(&"x".repeat(80))
            .unwrap()
            .chars()
            .count(),
        48
    );
    assert_eq!(notification_subject_from_body(" \n\t "), None);
}

fn test_server() -> (crate::server::Server, PathBuf) {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-mailbox-projection-{}-{sequence}",
        std::process::id()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let state_path = server_dir.join("state.json");
    current_state::save(&state_path, &State::default()).unwrap();
    (
        crate::server::Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            state_path,
            host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
            register_gate: Arc::new(Mutex::new(())),
            state: Mutex::new(crate::server::state::State::default()),
            appserver_candidate_check: crate::server::default_appserver_candidate_check(),
            appserver_notification_sink: crate::server::default_appserver_notification_sink(),
            appserver_thread_status: crate::server::default_appserver_thread_status(),
            appserver_thread_archive: crate::server::default_appserver_thread_archive(),
            rex_monitor: Default::default(),
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
