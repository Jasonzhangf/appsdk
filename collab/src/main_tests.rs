use super::*;
use crate::proto::{SelectedTransport, TransportKind};
use std::io::BufRead;
use std::time::{Duration, Instant};

fn test_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cm-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn identity_with_runtime(runtime: Option<RuntimeIdentity>) -> Identity {
    Identity {
        worker_id: "worker-1".into(),
        token: "token-1".into(),
        project_scope: None,
        runtime,
        transport: None,
    }
}

#[test]
fn init_accepts_explicit_worker_id_for_pane_free_registration() {
    let cli = Cli::try_parse_from([
        "collab",
        "init",
        "--worker-id",
        "codex-thread-6465736b746f702d746872656164",
    ])
    .unwrap();
    assert!(matches!(
        cli.cmd,
        Cmd::Init { worker_id: Some(worker_id) }
            if worker_id == "codex-thread-6465736b746f702d746872656164"
    ));
}

#[test]
fn cli_rejects_managed_subagent_start_before_resolving_collab_context() {
    let error = run(Cmd::Subagent {
        command: subagent::Action::Start {
            id: Some("child-test".into()),
            runtime: None,
        },
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("MANAGED_SUBAGENT_UNSUPPORTED:"),
        "unexpected start result: {error:#}"
    );
}

/// Bind the process environment to one explicit App Server address while
/// holding the shared env lock.  Registration reuse now compares the
/// persisted thread/session against the live ones, so a test that builds
/// a thread-backed fixture must state the address it means instead of
/// inheriting whatever the developer's shell exported.
fn set_current_session_thread(thread_id: &str, session_id: &str) {
    std::env::set_var("CODEX_THREAD_ID", thread_id);
    std::env::set_var("CODEX_SESSION_ID", session_id);
}

fn clear_current_session_thread() {
    std::env::remove_var("CODEX_THREAD_ID");
    std::env::remove_var("CODEX_SESSION_ID");
}

#[test]
fn runtime_for_request_rejects_an_unregistered_identity() {
    let identity = identity_with_runtime(None);
    let error = runtime_for_request(&identity).unwrap_err();
    assert!(error
        .to_string()
        .contains("identity has no registered runtime binding"));
}

#[test]
fn runtime_for_request_accepts_a_persisted_tui_binding() {
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-tui").unwrap(),
        appserver_id: identity::AppServerId::new("tui-default").unwrap(),
        endpoint_generation: 3,
        binding_id: identity::BindingId::new("binding-tui").unwrap(),
        session_id: None,
        native_thread_id: None,
    };
    let identity = identity_with_runtime(Some(runtime.clone()));
    assert_eq!(runtime_for_request(&identity).unwrap(), &runtime);
}

#[test]
fn live_closure_item_turn_id_is_required_for_native_correlation() {
    assert_eq!(
        live_closure_item_turn_id(&json!({"text": "challenge message"})),
        None
    );
    assert_eq!(
        live_closure_item_turn_id(&json!({"turnId": "turn-1"})),
        Some("turn-1")
    );
    assert_eq!(
        live_closure_item_turn_id(&json!({"turn_id": "turn-2"})),
        Some("turn-2")
    );
    assert_eq!(live_closure_item_turn_id(&json!({"turnId": "  "})), None);
}

#[test]
fn live_closure_item_message_id_accepts_native_notification_client_id() {
    assert_eq!(
        live_closure_item_message_id(&json!({
            "clientId": "collab-notification-message-1"
        })),
        Some("message-1")
    );
    assert_eq!(
        live_closure_item_message_id(&json!({
            "clientUserMessageId": "message-2"
        })),
        Some("message-2")
    );
    assert_eq!(live_closure_item_message_id(&json!({})), None);
}

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
    assert!(expected.exact[0].contains("READ IS NOT DONE"));
    assert!(expected.exact[1].starts_with("COLLAB_NOTIFY message-1 [notification-batch]"));
    assert!(expected.exact[1].contains("message_ids=message-1"));
    assert!(expected.exact[1].contains("READ IS NOT DONE"));
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
    let batch_input = "COLLAB_NOTIFY message-other [notification-batch] Batch wake: message_ids=message-other,message-target,message-later task_ids=none action_categories=other,appsdk-collab-live:closure-1:peer_to_peer,later. Read full durable details from collab inbox; execute the actions, do not ACK-only. older_messages=2; run collab inbox | P1 ACTION: do the in-scope action the message asks for. Details: collab msg message-other. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work.";

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

#[test]
fn live_closure_item_correlation_accepts_native_item_envelopes() {
    let item = json!({
        "turnId": "turn-envelope",
        "item": {
            "type": "userMessage",
            "id": "item-envelope",
            "clientId": "collab-notification-message-envelope"
        }
    });
    assert_eq!(live_closure_item_turn_id(&item), Some("turn-envelope"));
    assert_eq!(
        live_closure_item_message_id(&item),
        Some("message-envelope")
    );
    assert_eq!(live_closure_item_payload(&item)["id"], "item-envelope");
}

#[test]
fn live_closure_item_correlation_accepts_send_message_function_call_output() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let item = json!({
        "type": "functionCallOutput",
        "id": "fco_01a0bee4-83d0-7f40-9e5e-2f8d9b9c564f",
        "name": "send_message_to_thread",
        "namespace": "codex_tui",
        "output": format!(
            "<codex_delegation>\n  <source_thread_id>01a0b92e-bc55-75e1-8078-8c55e59cfd1d</source_thread_id>\n  <client_message_id>collab-notification-message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        )
    });
    let turns = vec![json!({
        "id": "turn-target",
        "status": "completed",
        "items": [item, {
            "type": "agentMessage",
            "id": "result-target",
            "text": "Target peer remains ready."
        }]
    })];

    let items = live_closure_turn_items(&turns).unwrap();
    let expected = LiveClosureExpectedNativeInputs {
        exact: vec![challenge.to_owned()],
        batch_category: "closure".to_owned(),
    };
    assert!(live_closure_item_matches_input(
        &items[0],
        &expected,
        "message-target"
    ));
}

#[test]
fn live_closure_item_correlation_accepts_daemon_function_output_without_source_thread() {
    let challenge = "appsdk-collab-live:closure-1:daemon_to_peer";
    let item = json!({
        "turnId": "turn-target",
        "type": "functionCallOutput",
        "id": "fco_01a0c851-6d67-7872-abca-7266849ef9a8",
        "name": "send_message_to_thread",
        "namespace": "codex_tui",
        "output": format!(
            "<codex_delegation>\n  <client_message_id>collab-notification-message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        )
    });

    let expected = LiveClosureExpectedNativeInputs {
        exact: vec![challenge.to_owned()],
        batch_category: "closure".to_owned(),
    };
    assert!(live_closure_item_matches_input(
        &item,
        &expected,
        "message-target"
    ));
}

#[test]
fn live_closure_item_correlation_rejects_mismatched_prefixed_function_call_output() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let item = json!({
        "turnId": "turn-target",
        "type": "functionCallOutput",
        "name": "send_message_to_thread",
        "output": format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        )
    });

    let expected = LiveClosureExpectedNativeInputs {
        exact: vec![challenge.to_owned()],
        batch_category: "closure".to_owned(),
    };
    assert!(!live_closure_item_matches_input(
        &item,
        &expected,
        "message-target"
    ));
}

#[test]
fn live_closure_item_correlation_accepts_xml_escaped_function_call_output() {
    let challenge = "appsdk-collab-live:a & b < c > d";
    let item = json!({
        "turnId": "turn-target",
        "type": "functionCallOutput",
        "name": "send_message_to_thread",
        "output": format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-target</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
            client::adapters::codex_app_server::escape_delegated_text(challenge)
        )
    });

    let expected = LiveClosureExpectedNativeInputs {
        exact: vec![challenge.to_owned()],
        batch_category: "closure".to_owned(),
    };
    assert!(live_closure_item_matches_input(
        &item,
        &expected,
        "message-target"
    ));
}

#[test]
fn live_closure_item_correlation_rejects_malformed_or_mismatched_function_outputs() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let valid_output = format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        );
    for item in [
        json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input>{challenge}-other</input>\n</codex_delegation>"
            )
        }),
        json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "other_tool",
            "output": valid_output
        }),
        json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!("prefix\n{valid_output}")
        }),
        json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input>{challenge}</input>\n</codex_delegation>\nsuffix"
            )
        }),
        json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input></input>\n</codex_delegation>"
        }),
        json!({
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": valid_output
        }),
    ] {
        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(!live_closure_item_matches_input(
            &item,
            &expected,
            "message-target"
        ));
    }

    let stale_output = format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-old</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        );
    let expected = LiveClosureExpectedNativeInputs {
        exact: vec![challenge.to_owned()],
        batch_category: "closure".to_owned(),
    };
    assert!(!live_closure_item_matches_input(
        &json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": stale_output,
        }),
        &expected,
        "message-target"
    ));
}

#[test]
fn live_closure_full_turn_items_preserve_exact_message_turn_result_correlation() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let turns = vec![
        json!({
            "id": "turn-other",
            "status": "completed",
            "items": [{
                "type": "userMessage",
                "clientId": "collab-notification-message-other",
                "content": [{"type": "text", "text": challenge}]
            }, {
                "type": "agentMessage",
                "id": "result-other"
            }]
        }),
        json!({
            "id": "turn-target",
            "status": "completed",
            "items": [{
                "type": "userMessage",
                "clientId": "collab-notification-message-target",
                "content": [{"type": "text", "text": challenge}]
            }, {
                "type": "agentMessage",
                "id": "result-target"
            }]
        }),
    ];
    let items = live_closure_turn_items(&turns).unwrap();
    let input = items
        .iter()
        .find(|item| {
            live_closure_item_message_id(item) == Some("message-target")
                && live_closure_item_contains_challenge(item, challenge)
        })
        .unwrap();
    let input_turn_id = live_closure_item_turn_id(input).unwrap();
    assert_eq!(input_turn_id, "turn-target");
    let completed_turn = turns
        .iter()
        .find(|turn| turn["status"] == "completed" && turn["id"].as_str() == Some(input_turn_id))
        .unwrap();
    assert_eq!(completed_turn["id"], "turn-target");
    let result = items
        .iter()
        .rev()
        .find(|item| {
            live_closure_item_payload(item)["id"] == "result-target"
                && live_closure_item_turn_id(item) == Some("turn-target")
        })
        .unwrap();
    assert_eq!(live_closure_item_payload(result)["id"], "result-target");
}

#[test]
fn live_closure_full_turn_items_reject_malformed_history() {
    for malformed in [
        json!([{"status": "completed", "items": []}]),
        json!([{"id": "turn-1", "status": "completed"}]),
        json!([{
            "id": "turn-1",
            "status": "completed",
            "items": ["not-an-item"]
        }]),
        json!([{
            "id": "turn-1",
            "status": "completed",
            "items": [{"turnId": 7}]
        }]),
        json!([{
            "id": "turn-1",
            "status": "completed",
            "items": [{"item": "not-an-envelope"}]
        }]),
    ] {
        assert!(
            live_closure_turn_items(malformed.as_array().unwrap()).is_err(),
            "{malformed}"
        );
    }
}

#[test]
fn live_closure_full_turn_items_reject_mismatched_history() {
    for mismatched in [
        json!({
            "turnId": "turn-other",
            "type": "userMessage",
            "clientId": "collab-notification-message-target",
            "content": [{"type": "text", "text": "challenge"}]
        }),
        json!({
            "turnId": "turn-target",
            "item": {
                "turnId": "turn-other",
                "type": "userMessage",
                "clientId": "collab-notification-message-target",
                "content": [{"type": "text", "text": "challenge"}]
            }
        }),
    ] {
        let error = live_closure_turn_items(&[json!({
            "id": "turn-target",
            "status": "completed",
            "items": [mismatched]
        })])
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_MISMATCH"));
    }
}

#[test]
fn live_closure_fresh_thread_materialization_is_bounded_pending() {
    let attempts = std::cell::Cell::new(0);
    let execution = wait_live_closure_fresh_thread_materialization(
        || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                return Err(live_closure_turns_read_error(
                    client::adapters::AdapterError::Unknown {
                        operation: "rpc",
                        detail: "list_turns is not supported yet".into(),
                    },
                ));
            }
            Ok(json!({"status": "completed"}))
        },
        Instant::now() + Duration::from_millis(500),
        Duration::from_millis(1),
    )
    .unwrap();

    assert_eq!(execution, json!({"status": "completed"}));
    assert_eq!(attempts.get(), 2);
}

#[test]
fn live_closure_permanent_unsupported_turns_read_fails_closed() {
    let attempts = std::cell::Cell::new(0);
    let error = wait_live_closure_fresh_thread_materialization(
        || {
            attempts.set(attempts.get() + 1);
            Err(live_closure_turns_read_error(
                client::adapters::AdapterError::Unknown {
                    operation: "rpc",
                    detail: "thread/turns/list is not supported yet".into(),
                },
            ))
        },
        Instant::now() + Duration::from_millis(500),
        Duration::from_millis(1),
    )
    .unwrap_err();

    assert_eq!(attempts.get(), 1);
    assert!(error
        .to_string()
        .starts_with("COLLAB_LIVE_CLOSURE_TARGET_TURNS_READ:"));
    assert!(error
        .to_string()
        .contains("thread/turns/list is not supported yet"));
}

#[test]
fn live_closure_item_correlation_requires_the_exact_native_challenge_body() {
    let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
    let item = json!({
        "turnId": "turn-envelope",
        "item": {
            "type": "userMessage",
            "clientId": "collab-notification-message-envelope",
            "content": [{"type": "text", "text": challenge }]
        }
    });
    assert!(live_closure_item_contains_challenge(&item, challenge));

    let wrapped = json!({
        "turnId": "turn-envelope",
        "item": {
            "type": "userMessage",
            "clientId": "collab-notification-message-envelope",
            "content": [{"type": "text", "text": format!("{challenge} | READ IS NOT DONE") }]
        }
    });
    assert!(!live_closure_item_contains_challenge(&wrapped, challenge));

    let mismatched = json!({
        "turnId": "turn-envelope",
        "item": {
            "type": "userMessage",
            "clientId": "collab-notification-message-envelope",
            "content": [{"type": "text", "text": "appsdk-collab-live:closure-1:other-path" }]
        }
    });
    assert!(!live_closure_item_contains_challenge(
        &mismatched,
        challenge
    ));
}

#[test]
fn live_closure_pages_follow_backwards_cursor_to_find_over_window_challenge() {
    let requested_cursors = std::cell::RefCell::new(Vec::new());
    let pages = [
        json!({
            "data": [{"id": "recent-item"}],
            "backwardsCursor": "older-page"
        }),
        json!({
            "data": [{"id": "exact-challenge-item"}],
            "backwardsCursor": null
        }),
    ];
    let values = read_live_closure_pages(|cursor| {
        requested_cursors
            .borrow_mut()
            .push(cursor.map(str::to_owned));
        Ok(pages[requested_cursors.borrow().len() - 1].clone())
    })
    .unwrap();

    assert_eq!(
        requested_cursors.into_inner(),
        vec![None, Some("older-page".into())]
    );
    assert_eq!(
        values
            .iter()
            .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["recent-item", "exact-challenge-item"]
    );
}

#[test]
fn live_closure_pages_use_next_cursor_and_bound_repeated_cursors() {
    let requested_cursors = std::cell::RefCell::new(Vec::new());
    let pages = [
        json!({"data": [{"id": "turn-1"}], "nextCursor": "page-2"}),
        json!({"data": [{"id": "turn-2"}], "nextCursor": "page-2"}),
    ];
    let values = read_live_closure_pages(|cursor| {
        requested_cursors
            .borrow_mut()
            .push(cursor.map(str::to_owned));
        Ok(pages[requested_cursors.borrow().len() - 1].clone())
    })
    .unwrap();
    assert_eq!(
        values,
        vec![json!({"id": "turn-1"}), json!({"id": "turn-2"})]
    );
    assert_eq!(
        requested_cursors.into_inner(),
        vec![None, Some("page-2".into())]
    );
}

#[test]
fn live_closure_timeout_is_bounded_and_configurable() {
    assert_eq!(
        live_closure_timeout_from_value(None).unwrap(),
        Duration::from_millis(DEFAULT_LIVE_CLOSURE_TIMEOUT_MS)
    );
    assert_eq!(
        live_closure_timeout_from_value(Some("240000")).unwrap(),
        Duration::from_millis(240_000)
    );
    assert_eq!(
        live_closure_timeout_from_value(Some("3600000")).unwrap(),
        Duration::from_millis(MAX_LIVE_CLOSURE_TIMEOUT_MS)
    );
    assert!(live_closure_timeout_from_value(Some("0"))
        .unwrap_err()
        .to_string()
        .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
    assert!(live_closure_timeout_from_value(Some("3600001"))
        .unwrap_err()
        .to_string()
        .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
    assert!(live_closure_timeout_from_value(Some("not-a-duration"))
        .unwrap_err()
        .to_string()
        .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
}

#[test]
fn live_closure_receipt_wait_rechecks_until_consume_before_deadline() {
    let attempts = std::cell::Cell::new(0);
    let receipt = wait_live_closure_receipt(
        || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            Ok(if attempt == 0 {
                json!({"id": "message-1", "state": "pending", "body": "challenge"})
            } else {
                json!({"id": "message-1", "state": "read", "body": "challenge"})
            })
        },
        Instant::now() + Duration::from_millis(500),
        "message-1",
        "challenge",
    )
    .unwrap();
    assert_eq!(receipt["state"], "read");
    assert_eq!(attempts.get(), 2);
}

#[test]
fn cli_project_context_uses_the_cli_app_and_exact_root() {
    let root = test_root("project-context");
    let context = cli_project_context(&root).unwrap();
    let canonical = root.canonicalize().unwrap();
    assert_eq!(context.app_scope_id.as_str(), identity::CLI_APP_SERVER_ID);
    assert_eq!(context.canonical_root, canonical.to_string_lossy());
    assert_eq!(context.project_scope.as_str(), canonical.to_string_lossy());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn default_recv_publishes_a_replayable_receive_identity_before_consuming() {
    let first = new_receive_id();
    let second = new_receive_id();
    assert_ne!(
        first, second,
        "each default receive owns a distinct identity"
    );
    assert!(
        crate::identity::validate_id_for_protocol(&first).is_ok(),
        "the generated identity must satisfy protocol identifier rules"
    );
    let banner = receive_recovery_banner(&first);
    assert!(banner.contains(&format!("receive_id={first}")));
    assert!(
        banner.contains(&format!("collab recv --receive-id {first}")),
        "the default path must publish the exact replay command: {banner}"
    );
}

#[test]
fn default_recv_aborts_before_dispatch_when_identity_publication_fails() {
    struct FailingStderr;
    impl std::io::Write for FailingStderr {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stderr is not writable",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dispatched = std::cell::Cell::new(false);
    let error = recv_with_published_identity(&mut FailingStderr, "receive-publish-fail-1", |_| {
        dispatched.set(true);
        Ok(json!({"consumed": true}))
    })
    .unwrap_err();
    assert!(
        !dispatched.get(),
        "an unpublished receive identity must abort before Poll dispatch"
    );
    assert!(
        error
            .to_string()
            .starts_with("RECEIVE_IDENTITY_PUBLISH_FAILED"),
        "the publication failure must be the first reported error: {error}"
    );
}

#[test]
fn default_recv_publishes_the_replay_identity_before_dispatch() {
    struct RecordingStderr {
        published: std::rc::Rc<std::cell::RefCell<String>>,
        flushed: std::rc::Rc<std::cell::Cell<bool>>,
    }
    impl std::io::Write for RecordingStderr {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.published
                .borrow_mut()
                .push_str(&String::from_utf8_lossy(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushed.set(true);
            Ok(())
        }
    }
    let published = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let flushed = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut stderr = RecordingStderr {
        published: published.clone(),
        flushed: flushed.clone(),
    };
    let observed = published.clone();
    let observed_flushed = flushed.clone();
    let error =
        recv_with_published_identity(&mut stderr, "receive-publish-order-1", move |receive_id| {
            let visible = observed.borrow().clone();
            assert!(
                observed_flushed.get(),
                "the replay identity must be flushed before the request is dispatched"
            );
            assert!(
                visible.contains(&format!("receive_id={receive_id}")),
                "the replay identity must be visible before dispatch: {visible}"
            );
            Err(anyhow::anyhow!("ADAPTER_TIMEOUT: original transport error"))
        })
        .unwrap_err();
    assert!(
        error.to_string().starts_with("ADAPTER_TIMEOUT"),
        "the original dispatch error must be the first reported error: {error}"
    );
}

/// The cross-project retire request is carried by exactly one thread-local
/// cell. If the getter and the setter each declare their own `thread_local!`,
/// they read and write different cells, `context_registration_requested()`
/// always returns `false`, and every `Req::Register` is sent with
/// `retire_cross_project_anchor: false` — so an unnamed `collab context` in a
/// foreign scope fails closed forever instead of retiring a provably dead
/// cross-project anchor and minting.
#[test]
fn context_registration_requested_reads_the_cell_the_setter_writes() {
    assert!(!context_registration_requested());
    set_context_registration_requested(true);
    assert!(
        context_registration_requested(),
        "the setter and the getter must share one thread-local cell"
    );
    set_context_registration_requested(false);
    assert!(!context_registration_requested());
}

/// The retire request is the adjudication channel, so it belongs to the explicit
/// `--worker` override only. If the implicit `collab context` path also asked for
/// it, an unnamed peer could retire another peer's live anchor on a foreign
/// project scope instead of failing closed with the error that points the user
/// at `--worker`.
#[test]
fn only_the_named_override_may_retire_a_foreign_anchor() {
    assert!(!crate::main_context::context_may_retire_foreign_anchor(None));
    assert!(crate::main_context::context_may_retire_foreign_anchor(Some(
        "codex-%3"
    )));
}

#[test]
fn context_root_resolution_fails_closed_without_route_or_baseline() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-unresolved");
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let error = resolve_context_root(&host_paths, &root)
        .err()
        .expect("no route and no baseline must fail closed");
    std::env::set_current_dir(previous).unwrap();
    assert!(
        error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED:"),
        "{error:#}"
    );
    assert!(error.to_string().contains("`collab context`"));
    assert!(!root.join(".agent-collab").exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_root_resolution_prefers_cwd_baseline_for_local_project() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-cwd");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let (scope, resolution) = resolve_context_root(&host_paths, &root).unwrap();
    std::env::set_current_dir(previous).unwrap();
    assert!(
        matches!(resolution, "route" | "cwd"),
        "expected route or cwd, got {resolution}"
    );
    assert_eq!(scope.root, root.canonicalize().unwrap());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_daemon_down_marker_fails_closed_and_preserves_marker() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-down-marker");
    let state_root = std::env::temp_dir().join(format!(
        "cs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::fs::write(state_root.join("DOWN"), "explicit down\n").unwrap();
    let previous_state = std::env::var_os(crate::scope::COLLAB_STATE_DIR_ENV);
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);

    let scope = Scope { root: root.clone() };
    let error = client::ensure_server(&scope.sock_path()).unwrap_err();
    assert!(
        error.to_string().starts_with("DAEMON_UNAVAILABLE:"),
        "{error:#}"
    );
    assert!(state_root.join("DOWN").is_file());

    match previous_state {
        Some(value) => std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, value),
        None => std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV),
    }
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// `collab context` is the single bootstrap read, so the identity-relevant
/// variables an agent previously grepped out of its own shell must be part of
/// the snapshot. Unrelated values must not leak: this projection is the one
/// place the caller's environment enters a Collab response.
#[test]
fn context_env_view_selects_only_collab_identity_variables() {
    // A pre-existing environment-sensitive test can panic while holding this
    // lock; take the poisoned guard instead of turning that into a new failure.
    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = [
        (
            "COLLAB_TEST_ENV_MARKER",
            std::env::var_os("COLLAB_TEST_ENV_MARKER"),
        ),
        (
            "CODEX_TEST_ENV_MARKER",
            std::env::var_os("CODEX_TEST_ENV_MARKER"),
        ),
        (
            "APPSDK_TEST_ENV_MARKER",
            std::env::var_os("APPSDK_TEST_ENV_MARKER"),
        ),
        (
            "AWS_SECRET_ACCESS_KEY",
            std::env::var_os("AWS_SECRET_ACCESS_KEY"),
        ),
        ("CODEX_API_KEY", std::env::var_os("CODEX_API_KEY")),
        ("COLLAB_TOKEN", std::env::var_os("COLLAB_TOKEN")),
    ];
    std::env::set_var("COLLAB_TEST_ENV_MARKER", "collab-value");
    std::env::set_var("CODEX_TEST_ENV_MARKER", "codex-value");
    std::env::set_var("APPSDK_TEST_ENV_MARKER", "appsdk-value");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "must-not-leak");
    std::env::set_var("CODEX_API_KEY", "must-not-leak-either");
    std::env::set_var("COLLAB_TOKEN", "must-not-leak-either");

    let view = crate::main_context::context_env_view();

    for (key, value) in previous {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    assert_eq!(view["COLLAB_TEST_ENV_MARKER"], "collab-value");
    assert_eq!(view["CODEX_TEST_ENV_MARKER"], "codex-value");
    assert_eq!(view["APPSDK_TEST_ENV_MARKER"], "appsdk-value");
    assert!(
        view.get("AWS_SECRET_ACCESS_KEY").is_none(),
        "unrelated secrets must stay out of the context snapshot: {view}"
    );
    // A selected prefix is not enough: `CODEX_API_KEY` and `COLLAB_TOKEN` are
    // real credential shapes under those prefixes, and this snapshot is written
    // on every bootstrap.
    assert!(
        view.get("CODEX_API_KEY").is_none(),
        "a credential under a selected prefix must not be emitted: {view}"
    );
    assert!(
        view.get("COLLAB_TOKEN").is_none(),
        "a credential under a selected prefix must not be emitted: {view}"
    );
    assert!(
        view.get("HOME").is_some(),
        "the home directory is part of the identity probe: {view}"
    );
}

/// Only the closed set of identity failures enters the identity terminal. A
/// route, runtime-binding, or transport failure is a different problem and must
/// keep failing closed rather than being reported as an identity request.
#[test]
fn only_classified_identity_failures_enter_the_identity_terminal() {
    use crate::main_context::IdentityFailure;

    // The real daemon strings, not fabricated uppercase variants.
    assert_eq!(
        IdentityFailure::classify("token mismatch: identity does not own this worker_id"),
        Some(IdentityFailure::TokenMismatch)
    );
    assert_eq!(
        IdentityFailure::classify(
            "IDENTITY_REBIND_UNPROVEN: no current anchor; pass --worker to explicitly recover a durable identity"
        ),
        Some(IdentityFailure::RebindUnproven)
    );
    assert_eq!(
        IdentityFailure::classify("IDENTITY_RESTORE_CROSS_PROJECT: refused"),
        Some(IdentityFailure::CrossProjectRestore)
    );
    assert_eq!(
        IdentityFailure::classify("COLLAB_IDENTITY_ANCHOR_MISSING: identity requires a tmux pane"),
        Some(IdentityFailure::AnchorMissing)
    );

    for unrelated in [
        "DAEMON_UNKNOWN: failed to send request to /tmp/server.sock",
        "DAEMON_UNAVAILABLE: no daemon at /tmp/server.sock",
        "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker has no registered runtime binding",
        "RUNTIME_BINDING_REJECTED: tmux identity anchor is already bound to worker codex-%8",
        "RECOVERY_RECONCILE_REQUIRED: host route is not at project generation 22",
    ] {
        assert_eq!(
            IdentityFailure::classify(unrelated),
            None,
            "{unrelated} is not an identity failure and must propagate"
        );
    }
}

/// The identity terminal must name the exact failure and the one action that
/// can restore the identity. A bare prose error would put the agent back in the
/// status-hunt this consolidation removes.
#[test]
fn identity_update_view_names_the_reason_and_the_recovery_action() {
    use crate::main_context::IdentityFailure;

    let error = anyhow::anyhow!(
        "IDENTITY_REBIND_UNPROVEN: worker codex-%3 has a reachable anchor that cannot be proven"
    );
    let view = crate::main_context::identity_update_view(
        &error,
        IdentityFailure::RebindUnproven,
        Some("codex-%3"),
    );
    assert_eq!(view["required"], true);
    assert_eq!(view["reason"], "IDENTITY_REBIND_UNPROVEN");
    assert_eq!(view["worker_id"], "codex-%3");
    // `worker_id` reports what the caller asked for; the action still asks the
    // operator to name a durable identity, because re-running the invocation
    // that just failed is the loop this field exists to prevent.
    assert_eq!(view["action"], "collab context --worker <worker_id>");
    assert_eq!(view["requires_approval"], true);
    assert!(
        view["exact_error"]
            .as_str()
            .unwrap()
            .contains("IDENTITY_REBIND_UNPROVEN"),
        "{view}"
    );

    let unnamed =
        crate::main_context::identity_update_view(&error, IdentityFailure::RebindUnproven, None);
    assert!(unnamed["worker_id"].is_null());
    // The failing invocation had no `--worker`, and repeating it verbatim would
    // loop, so the action must name the argument the identity layer documents as
    // the recovery.
    assert_eq!(unnamed["action"], "collab context --worker <worker_id>");
    assert_eq!(unnamed["requires_approval"], true);

    // Cross-project adjudication is the one repair that requires an operator to
    // declare the identity, so the action must name `--worker` even when the
    // caller supplied none, and approval must be explicit.
    let cross = crate::main_context::identity_update_view(
        &anyhow::anyhow!("IDENTITY_RESTORE_CROSS_PROJECT: refused without an operator declaration"),
        IdentityFailure::CrossProjectRestore,
        None,
    );
    assert_eq!(cross["requires_approval"], true);
    assert_eq!(cross["action"], "collab context --worker <worker_id>");

    // The real daemon rejection is lowercase prose; the reason must still be the
    // stable code the skill documents, and a rejected token cannot be re-run
    // into validity, so it escalates instead of repeating `collab context`.
    let mismatch = crate::main_context::identity_update_view(
        &anyhow::anyhow!("token mismatch: identity does not own this worker_id"),
        IdentityFailure::TokenMismatch,
        None,
    );
    assert_eq!(mismatch["reason"], "TOKEN_MISMATCH");
    assert_eq!(mismatch["requires_approval"], false);
    assert!(
        mismatch["action"].as_str().unwrap().contains("sendmessage"),
        "{mismatch}"
    );
}

/// Every classified failure must offer an action that is not the invocation
/// that produced it; otherwise an agent that follows the field loops forever.
#[test]
fn no_classified_failure_repeats_the_command_that_just_failed() {
    use crate::main_context::IdentityFailure;

    for (failure, produced_by) in [
        (IdentityFailure::TokenMismatch, "collab context"),
        (IdentityFailure::RebindUnproven, "collab context"),
        (IdentityFailure::CrossProjectRestore, "collab context"),
        (IdentityFailure::AnchorMissing, "collab context"),
    ] {
        let action = failure.action(None);
        assert_ne!(
            action,
            produced_by,
            "{} must not re-emit the failing invocation",
            failure.code()
        );
        assert!(
            action.contains("--worker") || action.contains("sendmessage"),
            "{} must name an executable next step, got {action}",
            failure.code()
        );
    }
}

/// The wiring contract of the identity terminal: a classified identity failure
/// becomes a snapshot, and anything else keeps its original error so the CLI
/// still exits non-zero instead of reporting an identity request.
#[test]
fn identity_terminal_classifies_and_otherwise_propagates() {
    let root = test_root("context-identity-terminal-wiring");
    let bootstrap = crate::main_context::ContextBootstrap {
        scope: Scope { root: root.clone() },
        project_root_resolution: "cwd",
        baseline_created: false,
        daemon_started: false,
    };

    let classified = crate::main_context::identity_terminal(
        &bootstrap,
        anyhow::anyhow!("token mismatch: identity does not own this worker_id"),
        None,
    )
    .expect("a classified identity failure becomes the terminal snapshot");
    assert_eq!(classified["registered"], false);
    assert_eq!(
        classified["requires_identity_update"]["reason"],
        "TOKEN_MISMATCH"
    );

    for unrelated in [
        "DAEMON_UNKNOWN: failed to send request to /tmp/server.sock",
        "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker has no registered runtime binding",
        "RECOVERY_RECONCILE_REQUIRED: host route is not at project generation 22",
    ] {
        let error = crate::main_context::identity_terminal(
            &bootstrap,
            anyhow::anyhow!("{unrelated}"),
            None,
        )
        .expect_err("a non-identity failure must keep failing closed");
        assert_eq!(
            error.to_string(),
            unrelated,
            "the original error must survive unchanged"
        );
    }

    std::fs::remove_dir_all(root).ok();
}

/// The identity terminal is an explicit snapshot, never a fabricated success:
/// `registered` stays false, `identity` stays null, and the recovery request is
/// present so one call still answers "what is the project state" and "what must
/// I fix about my identity".
#[test]
fn identity_update_snapshot_is_explicit_and_not_a_registered_snapshot() {
    let root = test_root("context-identity-terminal");
    let bootstrap = crate::main_context::ContextBootstrap {
        scope: Scope { root: root.clone() },
        project_root_resolution: "cwd",
        baseline_created: false,
        daemon_started: false,
    };
    let error = anyhow::anyhow!("token mismatch: identity does not own this worker_id");
    let snapshot = crate::main_context::identity_update_snapshot(
        &bootstrap,
        &error,
        crate::main_context::IdentityFailure::TokenMismatch,
        Some("codex-%9"),
    );

    assert_eq!(snapshot["registered"], false);
    assert!(snapshot["identity"].is_null());
    assert_eq!(snapshot["requires_identity_update"]["required"], true);
    assert_eq!(
        snapshot["requires_identity_update"]["reason"],
        "TOKEN_MISMATCH"
    );
    assert_eq!(snapshot["bootstrap"]["identity"], "unresolved");
    assert_eq!(snapshot["bootstrap"]["registered"], false);
    assert!(
        snapshot.get("env").is_some(),
        "the identity terminal still carries the environment probe: {snapshot}"
    );
    assert!(
        snapshot["requires_identity_update"]["next"]
            .as_str()
            .unwrap()
            .contains("live master"),
        "{snapshot}"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn cli_error_decorates_route_resolve_not_found_from_current_and_legacy_daemons() {
    for error in [
            "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread thread-old-daemon",
        ] {
            let formatted = format_cli_error(error);
            assert!(formatted.starts_with(error), "{formatted}");
            for expected in [
                "`collab context`",
            ] {
                assert!(
                    formatted.contains(expected),
                    "missing {expected}: {formatted}"
                );
            }
            for forbidden in [
                "`collab down`",
                "`collab up` once",
                "`appsdk init .`",
                "`collab route resolve --pane-id <pane-id>`",
                "`collab master status`",
            ] {
                assert!(
                    !formatted.contains(forbidden),
                    "forbidden recovery step {forbidden}: {formatted}"
                );
            }
            assert_eq!(
                format_cli_error(&formatted),
                formatted,
                "recovery guidance must not be duplicated"
            );
        }

    let current = format!(
            "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread thread-current-daemon; {}",
            crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY
        );
    assert_eq!(format_cli_error(&current), current);
    assert_eq!(
        current
            .matches(crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY)
            .count(),
        1,
        "{current}"
    );
}

#[test]
fn cli_error_decorates_identity_rebind_and_cross_project_with_manual_steps() {
    for error in [
        "IDENTITY_REBIND_UNPROVEN: existing peer codex-%4 does not match the current pane",
        "IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project",
    ] {
        let formatted = format_cli_error(error);
        assert!(formatted.starts_with(error), "{formatted}");
        for expected in [
            "`collab context`",
            "canonical project main checkout",
            "live master",
            "out-of-band",
        ] {
            assert!(
                formatted.contains(expected),
                "missing {expected}: {formatted}"
            );
        }
        if error.starts_with("IDENTITY_REBIND_UNPROVEN") {
            for expected in ["COLLAB_WORKER=", "--subject blocker \"<exact error;"] {
                assert!(
                    formatted.contains(expected),
                    "missing {expected}: {formatted}"
                );
            }
        }
        if error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT") {
            assert!(
                formatted.contains(
                    "do not try `collab sendmessage` through the same failing identity path"
                ),
                "cross-project must not reuse the failing sendmessage path: {formatted}"
            );
        }
        for forbidden in ["do not edit routes", "copy tokens", "start a second daemon"] {
            assert!(
                formatted.contains(forbidden),
                "missing forbidden recovery step {forbidden}: {formatted}"
            );
        }
        assert_eq!(
            format_cli_error(&formatted),
            formatted,
            "recovery guidance must not be duplicated"
        );
    }
}

#[path = "main_tests_part2.rs"]
mod part2;
