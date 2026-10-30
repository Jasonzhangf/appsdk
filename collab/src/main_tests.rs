use super::*;
use crate::proto::{SelectedTransport, TransportKind};
use std::io::BufRead;
use std::time::{Duration, Instant};

#[test]
fn supervised_up_rejects_a_launchd_accepted_but_unready_daemon() {
    let plist = std::path::Path::new("/tmp/com.appsdk.collab.e2e.plist");
    let sock = std::path::Path::new("/tmp/com.appsdk.collab.e2e.sock");
    let error = supervised_up_started(plist, sock, false, false).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("SUPERVISED_DAEMON_NOT_READY:"),
        "{error:#}"
    );
    assert!(error.to_string().contains(plist.to_str().unwrap()));
    assert!(error.to_string().contains(sock.to_str().unwrap()));

    assert!(
        !supervised_up_started(plist, sock, true, true).expect("ready daemon must start"),
        "a ready daemon that was already running must report started=false"
    );
    assert!(
        supervised_up_started(plist, sock, false, true).expect("ready daemon must start"),
        "a ready daemon that was not running must report started=true"
    );
}

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
fn removed_identity_selectors_are_rejected() {
    for args in [
        vec!["collab", "init", "--worker-id", "worker-1"],
        vec!["collab", "whoami"],
        vec!["collab", "worker", "recover"],
        vec!["collab", "master", "recover"],
        vec!["collab", "context", "--worker", "worker-1"],
        vec!["collab", "recv", "--worker", "worker-1"],
        vec!["collab", "inbox", "--worker", "worker-1"],
        vec!["collab", "ack", "message-1", "--worker", "worker-1"],
        vec![
            "collab", "send", "--from", "worker-1", "--to", "worker-2", "hello",
        ],
    ] {
        assert!(
            Cli::try_parse_from(args.clone()).is_err(),
            "removed identity selector must be rejected: {args:?}"
        );
    }

    let cli =
        Cli::try_parse_from(["collab", "context", "--provide", r#"{"session_id":"s"}"#]).unwrap();
    assert!(matches!(
        cli.cmd,
        Cmd::Context {
            provide: Some(provide),
            ..
        } if provide == r#"{"session_id":"s"}"#
    ));
}

#[test]
fn sendmessage_positional_address_before_content_parses_exact_target_and_body() {
    let cli = Cli::try_parse_from(["collab", "sendmessage", "Bob", "--content", "hello"])
        .expect("documented sendmessage form must parse");
    let Cmd::Send {
        to,
        content,
        subject,
        body,
        ..
    } = cli.cmd
    else {
        panic!("expected the send command");
    };
    assert_eq!(to, None);
    assert_eq!(content.as_deref(), Some("hello"));
    assert_eq!(subject, None);
    assert_eq!(body, vec!["Bob".to_owned()]);
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

include!("main_tests_live_closure.rs");

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
    let independent_first = new_receive_id();
    let independent_second = new_receive_id();
    assert_ne!(
        independent_first, independent_second,
        "each default receive owns a distinct identity"
    );
    assert!(
        crate::identity::validate_id_for_protocol(&independent_first).is_ok(),
        "the generated identity must satisfy protocol identifier rules"
    );
    assert!(
        crate::identity::validate_id_for_protocol(&independent_second).is_ok(),
        "the generated identity must satisfy protocol identifier rules"
    );

    let same_nonce = 1_729_977_600_000_000_000;
    let first_nonced = new_receive_id_from_nonce(same_nonce);
    let second_nonced = new_receive_id_from_nonce(same_nonce);
    assert_ne!(
        first_nonced, second_nonced,
        "each default receive owns a distinct identity"
    );
    assert!(
        crate::identity::validate_id_for_protocol(&first_nonced).is_ok(),
        "the generated identity must satisfy protocol identifier rules"
    );
    assert!(
        crate::identity::validate_id_for_protocol(&second_nonced).is_ok(),
        "the generated identity must satisfy protocol identifier rules"
    );
    let retained = format!("receive-{}-{same_nonce}", std::process::id());
    assert!(
        first_nonced.starts_with(&retained) && second_nonced.starts_with(&retained),
        "the stable pid and nonce information must remain in each identity"
    );
    let banner = receive_recovery_banner(&first_nonced);
    assert!(banner.contains(&format!("receive_id={first_nonced}")));
    assert!(
        banner.contains(&format!("collab recv --receive-id {first_nonced}")),
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

#[test]
fn context_root_resolution_accepts_first_registration_in_non_git_workspace() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-nongit");
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let (scope, resolution) = resolve_bootstrap_root(&host_paths, &root).unwrap();
    std::env::set_current_dir(previous).unwrap();
    assert_eq!(resolution, "cwd");
    assert_eq!(scope.root, root.canonicalize().unwrap());
    assert!(!root.join(".agent-collab").exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_root_resolution_keeps_registered_only_callers_closed_in_non_git_workspace() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-nongit-query");
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let error = resolve_context_root(&host_paths, &root)
        .err()
        .expect("a non-Git workspace without route or baseline must stay closed for query");
    std::env::set_current_dir(previous).unwrap();
    assert!(
        error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED:"),
        "{error:#}"
    );
    assert!(!root.join(".agent-collab").exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_root_resolution_rejects_git_subdirectory_without_baseline() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-git-subdir");
    let subdir = root.join("nested");
    std::fs::create_dir_all(&subdir).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
    // A Git subdirectory is an ordinary directory to Collab. Without a route
    // or a local baseline it must fail closed like any other unscoped cwd.
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&subdir).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let error = resolve_context_root(&host_paths, &subdir)
        .err()
        .expect("a Git subdirectory without a route or baseline must fail closed");
    std::env::set_current_dir(previous).unwrap();
    assert!(
        error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED:"),
        "{error:#}"
    );
    assert!(
        !error.to_string().contains("not its root"),
        "{}",
        "{error:#}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_root_resolution_rejects_playground_without_route_or_baseline() {
    // A plain directory under any name is unscoped to Collab. With no route
    // and no local baseline it fails closed with the generic unresolved error.
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let parent = test_root("context-root-playground");
    let root = parent.join("playground").join("task");
    std::fs::create_dir_all(&root).unwrap();
    let host_paths = scope::HostPaths::resolve().unwrap();
    let error = resolve_context_root(&host_paths, &root)
        .err()
        .expect("an unregistered unscoped directory must fail closed");
    assert!(
        error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED:"),
        "{error:#}"
    );
    assert!(
        !error.to_string().contains("playground worktree"),
        "{error:#}"
    );
    std::fs::remove_dir_all(parent).ok();
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
fn context_root_resolution_ignores_a_historical_pane_route_for_current_cwd() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-root-current-cwd");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let previous = std::env::current_dir().unwrap();
    let previous_tmux = std::env::var_os("TMUX");
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::set_current_dir(&root).unwrap();
    std::env::set_var("TMUX", "/tmp/historical-pane.sock,4242,0");
    std::env::set_var("TMUX_PANE", "%99");
    let host_paths = scope::HostPaths::resolve().unwrap();
    let resolved = resolve_context_root(&host_paths, &root);
    std::env::set_current_dir(previous).unwrap();
    match previous_tmux {
        Some(value) => std::env::set_var("TMUX", value),
        None => std::env::remove_var("TMUX"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    let (scope, resolution) = resolved.expect("current cwd must win over historical pane state");
    assert_eq!(resolution, "cwd");
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
/// `context_env_view` runs on every `collab context` response, including the
/// identity terminal, so a non-Unicode entry in the process environment must not
/// abort the single bootstrap entry. `std::env::vars()` panics on one; `vars_os()`
/// plus `to_str()` skips it.
#[cfg(unix)]
#[test]
fn context_env_view_skips_a_non_unicode_environment_entry() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = "COLLAB_TEST_NON_UTF8";
    let previous = std::env::var_os(key);
    // 0x80 is not a valid UTF-8 byte, and the key is under a selected prefix, so
    // the only reason it can be absent from the view is the encoding guard.
    std::env::set_var(key, OsString::from_vec(vec![0x66, 0x6f, 0x80]));

    let view = crate::main_context::context_env_view();

    match previous {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
    assert!(
        view.get(key).is_none(),
        "a non-Unicode value must be skipped, not abort the snapshot: {view}"
    );
}

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

#[test]
fn identity_supplement_rejects_unknown_duplicate_and_invalid_fields_before_observation() {
    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for raw in [
        r#"{"worker_id":"worker-1"}"#,
        r#"{"session_id":"one","session_id":"two"}"#,
        r#"{"session_id":""}"#,
        r#"{"session_id":null}"#,
        r#"{"namespace":"   "}"#,
        r#"{"thread_id":"thread\none"}"#,
        r#"{"session_id":5}"#,
        r#"{}"#,
        r#"not-json"#,
    ] {
        let error = crate::main_context::collect_identity_facts(Some(raw))
            .expect_err("invalid supplement must fail before observation");
        assert!(
            error.to_string().starts_with("IDENTITY_FACT_INVALID:"),
            "unexpected error for {raw}: {error:#}"
        );
    }
}

#[test]
fn identity_facts_preserve_explicit_endpoint_when_namespace_is_missing() {
    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let keys = [
        "COLLAB_APPSERVER_SOCKET",
        "CODEX_APP_SERVER_SOCKET",
        "COLLAB_APPSERVER_NAMESPACE",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "TMUX_PANE",
        "CODEX_SESSION_ID",
        "CODEX_THREAD_ID",
    ];
    let previous: Vec<_> = keys
        .iter()
        .map(|key| (*key, std::env::var_os(key)))
        .collect();
    for key in keys {
        std::env::remove_var(key);
    }
    std::env::set_var("COLLAB_APPSERVER_SOCKET", "/tmp/identity-facts.sock");

    let facts = crate::client::adapters::codex_app_server::identity_facts_from_env()
        .expect("an explicit endpoint is a partial identity fact");

    for (key, value) in previous {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    assert_eq!(
        facts.endpoint.as_deref(),
        Some("unix:///tmp/identity-facts.sock")
    );
    assert!(facts.namespace.is_none());
}

#[test]
fn identity_supplement_conflict_never_overrides_observed_fact() {
    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    std::env::set_var("CODEX_SESSION_ID", "observed-session");

    let error =
        crate::main_context::collect_identity_facts(Some(r#"{"session_id":"supplied-session"}"#))
            .expect_err("a supplied fact must not override an observed fact");

    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    assert!(
        error.to_string().starts_with("IDENTITY_FACT_CONFLICT:"),
        "{error:#}"
    );
}

#[test]
fn identity_context_required_forwards_the_daemon_update_exactly() {
    let update = json!({
        "required": true,
        "reason": "IDENTITY_INFORMATION_REQUIRED",
        "required_fields": ["session_id", "thread_id"],
        "field_descriptions": {
            "session_id": "Current runtime session identifier",
            "thread_id": "Current native thread identifier"
        },
        "action": "collab context --provide '<JSON containing required_fields>'",
        "requires_approval": false
    });
    let snapshot =
        json!({"registered": false, "identity": null, "requires_identity_update": update});
    let error = crate::main_context::identity_context_required(&snapshot);
    let response = &error
        .downcast_ref::<client::ServerResponseError>()
        .unwrap()
        .response;
    assert_eq!(
        response.data["requires_identity_update"],
        snapshot["requires_identity_update"]
    );
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

#[path = "main_tests_part2.rs"]
mod part2;

mod qqbot_lifecycle_tests {
    use super::*;
    use crate::server::conversation_default::{
        conversation_default_key, lookup_default, ConversationDefault,
    };
    use crate::server::state::{Event, State};

    fn new_state() -> State {
        State::default()
    }

    fn default_for(
        state: &mut State,
        channel: &str,
        conversation: &str,
        target_address: &str,
    ) -> u64 {
        let channel_instance = "qqbot@host";
        let key = conversation_default_key(channel, channel_instance, conversation);
        let revision = state
            .conversation_defaults
            .get(&key)
            .map(|default| default.revision)
            .unwrap_or(0);
        let default = ConversationDefault {
            channel: channel.to_owned(),
            channel_instance: channel_instance.to_owned(),
            conversation: conversation.to_owned(),
            project_scope: "/project".into(),
            target_address: target_address.to_owned(),
            revision: revision + 1,
            updated_ms: 0,
        };
        state.apply(&Event::ConversationDefaultSet { default });
        revision + 1
    }

    #[test]
    fn per_conversation_default_is_isolated_and_switched_atomically() {
        let mut state = new_state();
        // Two QQ conversations for two external users start with no default.
        assert!(lookup_default(&state, "qqbot", "qqbot@host", "u1:session").is_none());
        assert!(lookup_default(&state, "qqbot", "qqbot@host", "u2:session").is_none());

        // User 1 sets its default to Alice; user 2's default must be unaffected.
        default_for(&mut state, "qqbot", "u1:session", "project.Alice");
        default_for(&mut state, "qqbot", "u2:session", "project.Bob");
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .target_address,
            "project.Alice"
        );
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u2:session")
                .unwrap()
                .target_address,
            "project.Bob"
        );

        // Switching user 1 to Jenny must not disturb user 2.
        default_for(&mut state, "qqbot", "u1:session", "project.Jenny");
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .target_address,
            "project.Jenny"
        );
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u2:session")
                .unwrap()
                .target_address,
            "project.Bob",
            "switching one conversation must not move another"
        );
        // Each accepted switch advances its own revision monotonically.
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .revision,
            2
        );
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@host", "u2:session")
                .unwrap()
                .revision,
            1
        );
    }

    #[test]
    fn inbound_duplicate_platform_event_is_one_durable_message() {
        use crate::server::channel_envelope::{platform_event_key, ChannelEnvelope};
        let mut state = new_state();
        let key = platform_event_key("qqbot", "u1:session", "platform-msg-1");
        let envelope = ChannelEnvelope {
            source_channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            external_user: "u1".into(),
            external_conversation: "u1:session".into(),
            platform_message_id: "platform-msg-1".into(),
            reply_route: "qqbot://u1/session".into(),
            recipient_address: "project.Bob".into(),
            attachments: Vec::new(),
        };
        state.apply(&Event::ChannelEnvelopeRecorded {
            message_id: "collab-msg-1".into(),
            idempotency_key: key.clone(),
            envelope: envelope.clone(),
        });
        // A duplicate platform event (even with a different Collab id) must not
        // create a second message or a second envelope.
        state.apply(&Event::ChannelEnvelopeRecorded {
            message_id: "collab-msg-2".into(),
            idempotency_key: key.clone(),
            envelope,
        });
        assert_eq!(
            state.platform_event_index.len(),
            1,
            "{:?}",
            state.platform_event_index
        );
        assert_eq!(state.channel_envelopes.len(), 1);
        assert_eq!(
            state.platform_event_index.get(&key).map(String::as_str),
            Some("collab-msg-1"),
            "the first durable message must win for the platform event"
        );
        assert!(state.channel_envelopes.contains_key("collab-msg-1"));
        assert!(!state.channel_envelopes.contains_key("collab-msg-2"));
    }

    #[test]
    fn outbound_attachment_path_must_stay_inside_authorized_root() {
        use crate::server::channel_envelope::authorized_attachment_path;
        let base = std::env::temp_dir().join(format!(
            "att-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("authorized");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("报告 file.pdf");
        std::fs::write(&inside, b"payload").unwrap();
        let outside = base.join("secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        let dir = root.join("subdir");
        std::fs::create_dir_all(&dir).unwrap();

        // In-root regular file is accepted (Chinese and space in the name).
        let resolved = authorized_attachment_path(&root, &inside).expect("in-root file");
        assert_eq!(resolved, inside.canonicalize().unwrap());
        // Out-of-root path is rejected.
        let err = authorized_attachment_path(&root, &outside).unwrap_err();
        assert!(err.contains("ATTACHMENT_PATH_OUT_OF_ROOT"), "{err}");
        // Missing path is reported, not fabricated.
        let missing = root.join("nope.bin");
        let err = authorized_attachment_path(&root, &missing).unwrap_err();
        assert!(err.contains("ATTACHMENT_PATH_UNAVAILABLE"), "{err}");
        // A directory is not a sendable attachment.
        let err = authorized_attachment_path(&root, &dir).unwrap_err();
        assert!(err.contains("ATTACHMENT_PATH_NOT_FILE"), "{err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn conversation_default_and_channel_envelope_survive_snapshot_replay() {
        use crate::server::channel_envelope::{platform_event_key, ChannelEnvelope};
        let mut state = new_state();
        default_for(&mut state, "qqbot", "u1:session", "project.Bob");
        let key = platform_event_key("qqbot@host", "u1:session", "platform-msg-1");
        state.apply(&Event::ChannelEnvelopeRecorded {
            message_id: "collab-msg-1".into(),
            idempotency_key: key.clone(),
            envelope: ChannelEnvelope {
                source_channel: "qqbot".into(),
                channel_instance: "qqbot@host".into(),
                external_user: "u1".into(),
                external_conversation: "u1:session".into(),
                platform_message_id: "platform-msg-1".into(),
                reply_route: "qqbot://u1/session".into(),
                recipient_address: "project.Bob".into(),
                attachments: Vec::new(),
            },
        });
        // Compacted journal baselines must restore the typed default and the
        // platform idempotency index, not just the message rows.
        let lines: Vec<String> = state
            .snapshot_events()
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect();
        let mut replayed = new_state();
        for line in lines {
            let event: Event = serde_json::from_str(&line).unwrap();
            replayed.apply(&event);
        }
        assert_eq!(
            lookup_default(&replayed, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .target_address,
            "project.Bob"
        );
        assert_eq!(
            replayed.platform_event_index.get(&key).map(String::as_str),
            Some("collab-msg-1")
        );
        assert!(replayed.channel_envelopes.contains_key("collab-msg-1"));
    }
}
