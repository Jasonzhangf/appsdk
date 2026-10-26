#[test]
fn live_closure_expected_inputs_match_single_and_batch_notification_payloads() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let expected = live_closure_expected_native_inputs(
        &json!({
            "id": "message-1",
            "from": "sender",
            "to": "recipient",
            "type": "notify",
            "subject": challenge,
            "body": challenge,
            "state": "pending"
        }),
        "message-1",
        challenge,
    )
    .unwrap();

    assert_eq!(expected.exact.len(), 2);
    assert!(expected.exact[0].starts_with("COLLAB_NOTIFY message-1 ["));
    assert!(expected.exact[0].contains(challenge));
    assert_eq!(
        expected.exact[0].matches("collab recv").count(),
        1,
        "a wake carries exactly one read action: {}",
        expected.exact[0]
    );
    assert!(expected.exact[1].starts_with("COLLAB_NOTIFY message-1 [notification-batch]"));
    assert!(expected.exact[1].contains("message_ids=message-1"));
    assert_eq!(
        expected.exact[1].matches("collab recv").count(),
        1,
        "a batch wake carries exactly one read action: {}",
        expected.exact[1]
    );
    assert!(live_closure_item_matches_input(
        &json!({
            "turnId": "turn-1",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                client::adapters::codex_app_server::escape_delegated_text(&expected.exact[0])
            )
        }),
        &expected,
        "message-1"
    ));
    assert!(live_closure_item_matches_input(
        &json!({
            "turnId": "turn-1",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                client::adapters::codex_app_server::escape_delegated_text(&expected.exact[1])
            )
        }),
        &expected,
        "message-1"
    ));
}

#[test]
fn live_closure_expected_inputs_accept_daemon_notification_message_type() {
    let challenge = "appsdk-collab-live:closure-1:daemon_to_peer";
    let expected = live_closure_expected_native_inputs(
        &json!({
            "id": "message-daemon",
            "from": "collab-server",
            "to": "recipient",
            "type": "notification",
            "subject": challenge,
            "body": challenge,
            "state": "pending"
        }),
        "message-daemon",
        challenge,
    )
    .unwrap();

    assert_eq!(expected.exact.len(), 2);
    assert!(expected.exact[0].starts_with("COLLAB_NOTIFY message-daemon ["));
    assert!(expected.exact[0].contains(challenge));
    assert!(expected.exact[1].contains("message_ids=message-daemon"));
}

#[test]
fn live_closure_daemon_producer_paths_are_current_project_contracts() {
    assert!(live_closure_daemon_producer_path("daemon_to_peer"));
    assert!(live_closure_daemon_producer_path("daemon_to_master"));
    assert!(live_closure_daemon_producer_path("restart_replay"));
    assert!(!live_closure_daemon_producer_path("master_to_master"));
    assert!(!live_closure_daemon_producer_path("peer_to_peer"));
}

#[test]
fn live_closure_expected_input_rejects_raw_challenge_as_native_payload() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let expected = live_closure_expected_native_inputs(
        &json!({
            "id": "message-1",
            "from": "sender",
            "to": "recipient",
            "type": "notify",
            "subject": challenge,
            "body": challenge,
            "state": "pending"
        }),
        "message-1",
        challenge,
    )
    .unwrap();
    assert!(!live_closure_item_matches_input(
        &json!({
            "turnId": "turn-1",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
            )
        }),
        &expected,
        "message-1"
    ));
}

#[test]
fn live_closure_item_correlation_accepts_multi_message_batch_entry() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let expected = live_closure_expected_native_inputs(
        &json!({
            "id": "message-target",
            "from": "sender",
            "to": "recipient",
            "type": "notify",
            "subject": challenge,
            "body": challenge,
            "state": "pending"
        }),
        "message-target",
        challenge,
    )
    .unwrap();
    let batch_input = "COLLAB_NOTIFY message-other [notification-batch] Batch wake: message_ids=message-other,message-target,message-later action_categories=other,appsdk-collab-live:closure-1:peer_to_peer,later. older_messages=2 | P1 ACTION: do the in-scope action the message asks for. Read it once with `collab recv`; that consumes this durable message.";

    let last_entry = batch_input
        .replace(",message-later", "")
        .replace(",later. older_messages=2", ".");
    assert!(live_closure_item_matches_input(
        &json!({
            "turnId": "turn-target",
            "type": "userMessage",
            "clientId": "collab-notification-message-target",
            "content": [{"type": "text", "text": last_entry}]
        }),
        &expected,
        "message-target"
    ));

    assert!(live_closure_item_matches_input(
        &json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                client::adapters::codex_app_server::escape_delegated_text(batch_input)
            )
        }),
        &expected,
        "message-target"
    ));
    assert!(live_closure_item_matches_input(
        &json!({
            "turnId": "turn-target",
            "type": "userMessage",
            "clientId": "collab-notification-message-target",
            "content": [{"type": "text", "text": batch_input}]
        }),
        &expected,
        "message-target"
    ));

    let mismatched = batch_input.replace(
        "other,appsdk-collab-live:closure-1:peer_to_peer,later",
        "other,appsdk-collab-live:other,later",
    );
    assert!(!live_closure_item_matches_input(
        &json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                client::adapters::codex_app_server::escape_delegated_text(&mismatched)
            )
        }),
        &expected,
        "message-target"
    ));
}
