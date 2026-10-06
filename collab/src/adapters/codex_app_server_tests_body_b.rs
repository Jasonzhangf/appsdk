    #[test]
    fn active_interrupted_only_and_in_progress_turn_actions_are_explicit() {
        let interrupted_only = active_turn_id_from_page(&json!({
            "data": [{"id": "turn-interrupted", "status": "interrupted"}]
        }))
        .unwrap();
        assert_eq!(
            notification_action("active", interrupted_only).unwrap(),
            NotificationAction::Start
        );

        let active = active_turn_id_from_page(&json!({
            "data": [{"id": "turn-active", "status": "inProgress"}]
        }))
        .unwrap();
        assert_eq!(
            notification_action("active", active).unwrap(),
            NotificationAction::Steer("turn-active".into())
        );
    }

    #[test]
    fn immediate_notify_routes_active_thread_to_steer() {
        let socket = temp_socket("notify-active");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-active", "status": "inProgress", "items": []}
                        ]
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/steer");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(request["params"]["expectedTurnId"], "turn-active");
            assert_eq!(request["params"]["clientUserMessageId"], "message-active");
            respond(
                &mut stream,
                json!({"id": request["id"], "result": {"turnId": "turn-active"}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-active",
        )
        .unwrap();
        assert_eq!(receipt["turnId"], "turn-active");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_starts_active_thread_with_only_interrupted_turn() {
        let socket = temp_socket("notify-interrupted");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-interrupted", "status": "interrupted", "items": []}
                        ]
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(
                request["params"]["clientUserMessageId"],
                "message-interrupted"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-interrupted",
        )
        .unwrap();
        assert_eq!(receipt["turn"]["id"], "turn-started");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_rejects_multiple_in_progress_turns() {
        let socket = temp_socket("notify-multiple-active");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-1", "status": "inProgress", "items": []},
                            {"id": "turn-2", "status": "inProgress", "items": []}
                        ]
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-multiple-active",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("STEER_ACTIVE_TURN_AMBIGUOUS"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_routes_idle_thread_to_turn_start() {
        let socket = temp_socket("notify-start-idle");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(request["params"]["input"], json!([]));
            assert_eq!(
                request["params"]["toolOutput"]["name"],
                "send_message_to_thread"
            );
            assert_eq!(request["params"]["toolOutput"]["namespace"], "codex_tui");
            assert_eq!(
                request["params"]["toolOutput"]["output"],
                "<codex_delegation>\n  <source_thread_id>sender-thread</source_thread_id>\n  <client_message_id>message-start</client_message_id>\n  <input>notify body</input>\n</codex_delegation>"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_uses_registered_codex_app_namespace() {
        let socket = temp_socket("notify-start-desktop");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["toolOutput"]["namespace"], "codex_app");
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut transport = selected_transport(&socket);
        transport.namespace = Some("codex_app".into());
        immediate_notify(
            &transport,
            Some("sender-thread"),
            "notify body",
            "message-desktop-start",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_attributes_turn_start_to_sender_not_recipient() {
        let socket = temp_socket("notify-source-thread");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "recipient-thread",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "recipient-thread");
            assert_eq!(
                request["params"]["toolOutput"]["output"],
                "<codex_delegation>\n  <source_thread_id>sender-thread</source_thread_id>\n  <client_message_id>message-source-thread</client_message_id>\n  <input>notify body</input>\n</codex_delegation>"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut transport = selected_transport(&socket);
        transport.thread_id = Some("recipient-thread".into());
        immediate_notify(
            &transport,
            Some("sender-thread"),
            "notify body",
            "message-source-thread",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_loads_not_loaded_thread_through_turn_start() {
        let socket = temp_socket("notify-not-loaded-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "notLoaded"}
                        }
                    }
                }),
            );
            // A cold thread is loaded by the immediate notification itself:
            // `turn/start` is the native load-and-start call.
            let start = next_request(&mut stream);
            assert_eq!(start["method"], "turn/start");
            assert_eq!(start["params"]["threadId"], "thread-1");
            respond(
                &mut stream,
                json!({
                    "id": start["id"],
                    "result": {
                        "turn": {"id": "turn-loaded", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_materializes_missing_rollout_through_turn_start() {
        let socket = temp_socket("notify-missing-rollout-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "missingRollout");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let start = next_request(&mut stream);
            assert_eq!(start["method"], "turn/start");
            assert_eq!(start["params"]["threadId"], "thread-1");
            assert_eq!(start["params"]["clientUserMessageId"], "message-rollout");
            respond(
                &mut stream,
                json!({
                    "id": start["id"],
                    "result": {
                        "turn": {"id": "turn-materialized", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-rollout",
        )
        .unwrap();
        assert_eq!(receipt["turn"]["id"], "turn-materialized");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_refuses_when_resumed_thread_still_reports_not_loaded() {
        let socket = temp_socket("notify-read-not-loaded-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "error": {
                        "code": -32602,
                        "message": "thread not loaded: thread-1"
                    }
                }),
            );
            // A thread that this connection just resumed must not still report
            // not-loaded; delivery refuses instead of starting a turn.
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a resumed-but-still-cold thread must not issue turn/start"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("still reports not loaded"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_rejects_missing_thread_without_turn_start() {
        for response in [
            json!({
                "error": {
                    "code": -32602,
                    "message": "thread not found: thread-1"
                }
            }),
            json!({
                "error": {
                    "code": -32602,
                    "message": "thread not found"
                }
            }),
        ] {
            let socket = temp_socket("notify-not-loaded-reject");
            let Some(listener) = bind_test_socket(&socket) else {
                return;
            };
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                handshake(&mut stream);
                initialize(&mut stream);
                prepare_recipient_thread(&mut stream, "ok");
                let read_id = next_request_id(&mut stream);
                let mut response = response;
                response["id"] = json!(read_id);
                respond(&mut stream, response);
                let mut byte = [0_u8; 1];
                assert_eq!(
                    stream.read(&mut byte).unwrap(),
                    0,
                    "not-loaded admission must not issue another App Server method"
                );
                stream.shutdown(Shutdown::Both).ok();
            });

            let error = immediate_notify(
                &selected_transport(&socket),
                Some("sender-thread"),
                "notify body",
                "message-start",
            )
            .unwrap_err();
            assert!(
                matches!(error, AdapterError::RouteUnavailable { .. }),
                "{error}"
            );
            server.join().unwrap();
            std::fs::remove_file(socket).ok();
        }
    }

    #[test]
    fn immediate_notify_rejects_wrapped_missing_thread_without_turn_start() {
        let socket = temp_socket("notify-wrapped-missing-thread");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "error": {
                        "code": -32602,
                        "message": "rpc unknown: thread not found"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "missing route must not issue another App Server method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        assert!(
            matches!(error, AdapterError::RouteUnavailable { .. }),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "ADAPTER_ROUTE_UNAVAILABLE: rpc unknown: thread not found"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_reports_thread_writer_conflict_as_terminal() {
        let socket = temp_socket("notify-writer-conflict");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "thread/resume");
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": "thread thread-1 already has an active writer"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a writer conflict must not issue another App Server method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        // The recipient is owned by another live App Server process, so this
        // endpoint must report a terminal conflict instead of an opaque rpc
        // failure or a retryable route error.
        assert!(
            matches!(error, AdapterError::ThreadWriterConflict { .. }),
            "{error}"
        );
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("APPSERVER_THREAD_WRITER_CONFLICT: "),
            "{rendered}"
        );
        assert!(
            rendered.contains("thread thread-1 already has an active writer"),
            "{rendered}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn thread_writer_conflict_detection_matches_only_the_writer_refusal() {
        assert!(is_thread_writer_conflict(
            "thread 01a0a530-cca3-76e3-9364-699d66ded6f0 already has an active writer"
        ));
        assert!(is_thread_writer_conflict(
            "thread-store conflict: thread thread-1 already has an active writer"
        ));
        for detail in [
            "thread not found: thread-1",
            "thread not loaded: thread-1",
            "thread thread-1 is closing; retry thread/resume after the thread is closed",
            "",
        ] {
            assert!(!is_thread_writer_conflict(detail), "{detail}");
        }
    }

    #[test]
    fn wrapped_missing_thread_error_is_route_unavailable() {
        for detail in [
            "thread not found: thread-1",
            "rpc unknown: thread not found",
            "rpc unknown: thread not found: thread-1",
        ] {
            assert!(is_thread_not_found_error("rpc", detail, "thread-1"));
        }
        for detail in [
            "thread not found: other-thread",
            "rpc unknown: thread not found: other-thread",
            "thread not loaded: thread-1",
            "rpc unknown: thread not loaded: thread-1",
        ] {
            assert!(!is_thread_not_found_error("rpc", detail, "thread-1"));
        }
        assert!(!is_thread_not_found_error(
            "thread/read",
            "rpc unknown: thread not found: thread-1",
            "thread-1"
        ));
    }

    #[test]
    fn delegated_prompt_escapes_xml_special_characters() {
        assert_eq!(
            delegated_prompt(Some("thread-1"), "message-1", "a & b < c > d"),
            "<codex_delegation>\n  <source_thread_id>thread-1</source_thread_id>\n  <client_message_id>message-1</client_message_id>\n  <input>a &amp; b &lt; c &gt; d</input>\n</codex_delegation>"
        );
        assert_eq!(
            delegated_prompt(None, "message-1", "automatic"),
            "<codex_delegation>\n  <client_message_id>message-1</client_message_id>\n  <input>automatic</input>\n</codex_delegation>"
        );
    }

    #[test]
    #[ignore]
    fn live_immediate_notify_accepts_loaded_thread() {
        let Some(candidate) = candidate_from_env().unwrap() else {
            panic!("CODEX_THREAD_ID is required");
        };
        let transport = verify_candidate(&candidate).expect("live App Server candidate");
        let receipt = immediate_notify(
            &transport,
            Some(candidate.thread_id.as_str()),
            "Reply with exactly COLLAB_START_PROBE_OK and do not run tools.",
            "collab-start-probe",
        )
        .expect("live immediate notify");
        assert_eq!(receipt["turn"]["status"], "inProgress");
    }

    #[test]
    fn immediate_notify_rejects_queued_submission_as_success() {
        let socket = temp_socket("notify-queued");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "queuedSubmission": {"id": "queue-1"}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        assert!(immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-queued"
        )
        .is_err());
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn queued_notify_routes_active_thread_to_queue_add() {
        let socket = temp_socket("queued-notify-active");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active"}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [{"id": "turn-active", "status": "inProgress"}]
                    }
                }),
            );
            let queue = next_request(&mut stream);
            assert_eq!(queue["method"], "thread/queue/add");
            assert_eq!(queue["params"]["threadId"], "thread-1");
            assert_eq!(
                queue["params"]["clientUserMessageId"],
                "message-queued-active"
            );
            respond(
                &mut stream,
                json!({
                    "id": queue["id"],
                    "result": {
                        "queuedSubmission": {"id": "queue-queued-active"}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = queued_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-queued-active",
        )
        .unwrap();
        assert_eq!(receipt["queuedSubmission"]["id"], "queue-queued-active");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn queued_notify_starts_idle_thread_with_turn_start() {
        let socket = temp_socket("queued-notify-idle");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let start = next_request(&mut stream);
            assert_eq!(start["method"], "turn/start");
            assert_eq!(start["params"]["threadId"], "thread-1");
            assert_eq!(start["params"]["toolOutput"]["namespace"], "codex_app");
            assert_eq!(
                start["params"]["clientUserMessageId"],
                "message-queued-idle"
            );
            respond(
                &mut stream,
                json!({
                    "id": start["id"],
                    "result": {
                        "turn": {"id": "turn-queued-idle", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut transport = selected_transport(&socket);
        transport.namespace = Some("codex_app".into());
        let receipt = queued_notify(
            &transport,
            None,
            "notify body",
            "message-queued-idle",
        )
        .unwrap();
        assert_eq!(receipt["turn"]["id"], "turn-queued-idle");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    fn selected_transport(socket: &Path) -> SelectedTransport {
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(format!("unix://{}", socket.display())),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test".into(),
        }
    }

    fn temp_socket(tag: &str) -> PathBuf {
        PathBuf::from(format!(
            "collab-{tag}-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn bind_test_socket(socket: &Path) -> Option<UnixListener> {
        match UnixListener::bind(socket) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "SKIP socket integration assertion: sandbox denied unix socket bind at {}",
                    socket.display()
                );
                None
            }
            Err(error) => panic!("bind {}: {error}", socket.display()),
        }
    }

    fn handshake(stream: &mut UnixStream) {
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            )
            .unwrap();
    }

    fn initialize(stream: &mut UnixStream) {
        let request = next_request(stream);
        assert_eq!(request["method"], "initialize");
        respond(stream, json!({"id": request["id"], "result": {}}));
        let initialized = read_client_frame(stream);
        let initialized: Value = serde_json::from_slice(&initialized).unwrap();
        assert_eq!(initialized["method"], "initialized");
    }

    /// Answer the per-connection `thread/resume` that immediate delivery issues
    /// before it can start a turn.  Test fixtures that model a usable thread
    /// answer success; the ones modelling a missing thread answer not-found,
    /// which delivery must surface unchanged.
    fn prepare_recipient_thread(stream: &mut UnixStream, outcome: &str) {
        let request = next_request(stream);
        assert_eq!(request["method"], "thread/resume");
        match outcome {
            "ok" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "result": {"thread": {"id": request["params"]["threadId"]}}
                }),
            ),
            "notFound" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "error": {"code": -32600, "message": "thread not found"}
                }),
            ),
            "missingRollout" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "error": {
                        "code": -32602,
                        "message": format!("no rollout found for thread id {}", request["params"]["threadId"].as_str().unwrap())
                    }
                }),
            ),
            other => panic!("unsupported recipient thread outcome {other}"),
        }
    }

    fn next_request(stream: &mut UnixStream) -> Value {
        serde_json::from_slice(&read_client_frame(stream)).unwrap()
    }

    fn next_request_id(stream: &mut UnixStream) -> Value {
        next_request(stream)["id"].clone()
    }

    fn respond(stream: &mut UnixStream, value: Value) {
        stream
            .write_all(&encode_frame(0x1, &serde_json::to_vec(&value).unwrap()))
            .unwrap();
    }

    fn read_client_frame(stream: &mut UnixStream) -> Vec<u8> {
        try_read_client_frame(stream).unwrap()
    }

    fn try_read_client_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header)?;
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as usize;
        if length == 126 {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes)?;
            length = u16::from_be_bytes(bytes) as usize;
        }
        let mut mask = [0_u8; 4];
        if masked {
            stream.read_exact(&mut mask)?;
        }
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload)?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        Ok(payload)
    }
