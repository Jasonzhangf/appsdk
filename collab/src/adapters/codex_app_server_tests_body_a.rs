    use std::net::Shutdown;
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn assert_malformed_active_turn(page: Value, expected_detail: &str) {
        match active_turn_id_from_page(&page).unwrap_err() {
            AdapterError::Unknown { operation, detail } => {
                assert_eq!(operation, "turn/steer");
                assert!(detail.contains(expected_detail), "{detail}");
            }
            error => panic!("expected AdapterError::Unknown, got {error:?}"),
        }
    }

    #[test]
    fn frame_round_trip_uses_masked_client_frames() {
        let frame = encode_frame(0x1, b"hello");
        assert_eq!(frame[0], 0x81);
        assert_eq!(frame[1] & 0x80, 0x80);
        assert_eq!(frame[1] & 0x7f, 5);
        let mask = &frame[2..6];
        let decoded = frame[6..]
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4])
            .collect::<Vec<_>>();
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn client_handshake_and_rpc_round_trip() {
        let socket = std::env::temp_dir().join(format!(
            "collab-appserver-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            let payload = read_client_frame(&mut stream);
            let request: Value = serde_json::from_slice(&payload).unwrap();
            let response = json!({"id": request["id"], "result": {"ok": true}});
            stream
                .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let value = client.call("initialize", json!({})).unwrap();
        assert_eq!(value["ok"], true);
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn client_accepts_large_appserver_rpc_response() {
        let socket = std::env::temp_dir().join(format!(
            "caslr-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            let payload = read_client_frame(&mut stream);
            let request: Value = serde_json::from_slice(&payload).unwrap();
            let body = "x".repeat(MAX_OUTGOING_FRAME_BYTES + 1024);
            let response = json!({"id": request["id"], "result": {"body": body}});
            stream
                .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let value = client.call("initialize", json!({})).unwrap();
        assert_eq!(
            value["body"].as_str().unwrap().len(),
            MAX_OUTGOING_FRAME_BYTES + 1024
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn client_rejects_oversized_appserver_rpc_response() {
        let socket = std::env::temp_dir().join(format!(
            "cosar-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            let _payload = read_client_frame(&mut stream);
            stream.write_all(&[0x81, 0x7f]).unwrap();
            stream
                .write_all(&((MAX_INCOMING_FRAME_BYTES as u64 + 1).to_be_bytes()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let error = client.call("initialize", json!({})).unwrap_err();
        match error {
            AdapterError::Unknown { operation, detail } => {
                assert_eq!(operation, "websocket frame");
                assert_eq!(detail, "native frame exceeds maximum size");
            }
            error => panic!("expected oversized websocket frame error, got {error:?}"),
        }
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_does_not_require_appserver_queue_wakeup_method() {
        let socket = std::env::temp_dir().join(format!(
            "collab-queue-required-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            loop {
                let payload = read_client_frame(&mut stream);
                let request: Value = serde_json::from_slice(&payload).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let method = request["method"].as_str().unwrap();
                let response = match method {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/loaded/list" => {
                        json!({"id": id, "result": {"data": ["thread-1"]}})
                    }
                    "thread/read" => {
                        json!({
                            "id": id,
                            "result": {
                                "thread": {
                                    "id": "thread-1",
                                    "sessionId": "session-1",
                                    "cwd": env!("CARGO_MANIFEST_DIR")
                                }
                            }
                        })
                    }
                    "thread/items/list" => {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    }
                    "turn/start" => {
                        json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                    }
                    "turn/steer" => {
                        json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                    }
                    "thread/turns/list" => {
                        json!({"id": id, "result": {"data": []}})
                    }
                    _ => unreachable!("{method}"),
                };
                stream
                    .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                    .unwrap();
                if method == "thread/turns/list" {
                    break;
                }
            }
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert!(!selected
            .capabilities
            .iter()
            .any(|capability| capability == "queue_wakeup"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_missing_appserver_thread_as_route_unavailable() {
        let socket = std::env::temp_dir().join(format!(
            "collab-missing-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            loop {
                let payload = read_client_frame(&mut stream);
                let request: Value = serde_json::from_slice(&payload).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let response = match request["method"].as_str().unwrap() {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/loaded/list" => {
                        json!({"id": id, "result": {"data": ["missing-thread"]}})
                    }
                    "thread/read" => {
                        json!({"id": id, "error": {"code": -32602, "message": "thread not found"}})
                    }
                    method => panic!("unexpected method after missing thread: {method}"),
                };
                stream
                    .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                    .unwrap();
                if request["method"] == "thread/read" {
                    break;
                }
            }
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "missing-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(matches!(error, AdapterError::RouteUnavailable { .. }));
        assert!(error.to_string().contains("ADAPTER_ROUTE_UNAVAILABLE"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_verifies_persisted_cold_thread_through_thread_read() {
        let socket = std::env::temp_dir().join(format!(
            "collab-unloaded-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            // Identity is established from `thread/read`, which serves
            // persisted threads.  A cold thread reports notLoaded status and
            // is loaded later by the native load-and-start call.
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "persisted-thread");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "persisted-thread",
                            "sessionId": "session-1",
                            "cwd": env!("CARGO_MANIFEST_DIR"),
                            "status": {"type": "notLoaded"}
                        }
                    }
                }),
            );
            let items = next_request(&mut stream);
            assert_eq!(items["method"], "thread/items/list");
            respond(
                &mut stream,
                json!({"id": items["id"], "result": {"data": []}}),
            );
            let turn_start = next_request(&mut stream);
            assert_eq!(turn_start["method"], "turn/start");
            respond(
                &mut stream,
                json!({"id": turn_start["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let steer = next_request(&mut stream);
            assert_eq!(steer["method"], "turn/steer");
            respond(
                &mut stream,
                json!({"id": steer["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let turns = next_request(&mut stream);
            assert_eq!(turns["method"], "thread/turns/list");
            respond(
                &mut stream,
                json!({"id": turns["id"], "result": {"data": []}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "persisted-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert_eq!(selected.thread_id.as_deref(), Some("persisted-thread"));
        assert_eq!(selected.session_id.as_deref(), Some("session-1"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_whose_read_reports_not_loaded() {
        let socket = std::env::temp_dir().join(format!(
            "collab-unloaded-race-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "persisted-thread");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "error": {
                        "code": -32602,
                        "message": "thread not loaded: persisted-thread"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a thread whose read reports notLoaded must not issue another method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-persisted-thread".into(),
            thread_id: "persisted-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(
            matches!(error, AdapterError::RouteUnavailable { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("persisted but not loaded"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_accepts_thread_verified_by_thread_read_identity() {
        let socket = PathBuf::from("/tmp").join(format!(
            "collab-candidate-loaded-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "thread-1");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "session-1",
                            "cwd": env!("CARGO_MANIFEST_DIR"),
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let items = next_request(&mut stream);
            assert_eq!(items["method"], "thread/items/list");
            respond(
                &mut stream,
                json!({"id": items["id"], "error": {"code": -32601, "message": "unsupported"}}),
            );
            let turn_start = next_request(&mut stream);
            assert_eq!(turn_start["method"], "turn/start");
            respond(
                &mut stream,
                json!({"id": turn_start["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let steer = next_request(&mut stream);
            assert_eq!(steer["method"], "turn/steer");
            respond(
                &mut stream,
                json!({"id": steer["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let turns = next_request(&mut stream);
            assert_eq!(turns["method"], "thread/turns/list");
            respond(
                &mut stream,
                json!({"id": turns["id"], "result": {"data": []}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert_eq!(selected.thread_id.as_deref(), Some("thread-1"));
        assert!(selected.self_check.contains("thread/read"));
        assert!(!selected.self_check.contains("thread/queue/add"));
        assert!(!selected
            .capabilities
            .iter()
            .any(|capability| capability == "queue_wakeup"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_session_mismatch() {
        let socket = PathBuf::from("/tmp").join(format!(
            "c-session-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "different-session",
                            "cwd": env!("CARGO_MANIFEST_DIR")
                        }
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(
            error.to_string().contains("thread session mismatch"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_cwd_mismatch() {
        let socket = PathBuf::from("/tmp").join(format!(
            "c-cwd-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "session-1",
                            "cwd": "/tmp"
                        }
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(error.to_string().contains("thread cwd mismatch"), "{error}");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_appserver_without_steer_or_turns_list_methods() {
        for missing_method in ["turn/steer", "thread/turns/list"] {
            let socket = std::env::temp_dir().join(format!(
                "collab-candidate-method-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let Some(listener) = bind_test_socket(&socket) else {
                return;
            };
            let missing_method = missing_method.to_string();
            let server_missing_method = missing_method.clone();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                handshake(&mut stream);
                loop {
                    let payload = read_client_frame(&mut stream);
                    let request: Value = serde_json::from_slice(&payload).unwrap();
                    let Some(id) = request.get("id").cloned() else {
                        continue;
                    };
                    let method = request["method"].as_str().unwrap();
                    let response = if method == server_missing_method {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    } else {
                        match method {
                            "initialize" => json!({"id": id, "result": {}}),
                            "thread/loaded/list" => {
                                json!({"id": id, "result": {"data": ["thread-1"]}})
                            }
                            "thread/read" => {
                                json!({
                                    "id": id,
                                    "result": {
                                        "thread": {
                                            "id": "thread-1",
                                            "sessionId": "session-1",
                                            "cwd": env!("CARGO_MANIFEST_DIR")
                                        }
                                    }
                                })
                            }
                            "thread/items/list" => {
                                json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                            }
                            "turn/start" | "turn/steer" => {
                                json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                            }
                            "thread/turns/list" => {
                                json!({"id": id, "result": {"data": []}})
                            }
                            _ => unreachable!("{method}"),
                        }
                    };
                    stream
                        .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                        .unwrap();
                    if method == server_missing_method {
                        break;
                    }
                }
                stream.shutdown(Shutdown::Both).ok();
            });

            let candidate = AppServerCandidate {
                endpoint: format!("unix://{}", socket.display()),
                namespace: "codex_tui".into(),
                session_id: "session-1".into(),
                thread_id: "thread-1".into(),
                cwd: env!("CARGO_MANIFEST_DIR").into(),
            };
            let error = verify_candidate(&candidate).unwrap_err();
            assert!(
                matches!(
                    &error,
                    AdapterError::CapabilityUnavailable {
                        operation,
                        ..
                    } if *operation == missing_method
                ),
                "{missing_method}: {error}"
            );
            server.join().unwrap();
            std::fs::remove_file(socket).ok();
        }
    }

    #[test]
    fn immediate_receipt_requires_turn_identity_and_protocol_status() {
        validate_immediate_receipt(&json!({
            "turn": {"id": "turn-1", "status": "inProgress"}
        }))
        .unwrap();

        for malformed in [
            json!({}),
            json!({"turn": {"status": "inProgress"}}),
            json!({"turn": {"id": "turn-1"}}),
            json!({"turn": {"id": "turn-1", "status": "queued"}}),
            json!({"turn": {"id": "bad turn", "status": "completed"}}),
        ] {
            assert!(
                validate_immediate_receipt(&malformed).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn steer_receipt_requires_matching_turn_identity() {
        validate_steer_receipt(&json!({"turnId": "turn-1"}), "turn-1").unwrap();

        for malformed in [
            json!({}),
            json!({"turnId": ""}),
            json!({"turnId": "turn-2"}),
            json!({"turnId": "bad turn"}),
        ] {
            assert!(
                validate_steer_receipt(&malformed, "turn-1").is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn active_turn_selection_only_accepts_in_progress_turns() {
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-active", "status": "inProgress"}]
            }))
            .unwrap()
            .as_deref(),
            Some("turn-active")
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-interrupted", "status": "interrupted"}]
            }))
            .unwrap(),
            None
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [
                    {"id": "turn-interrupted", "status": "interrupted"},
                    {"id": "turn-completed", "status": "completed"}
                ]
            }))
            .unwrap(),
            None
        );
    }

    #[test]
    fn active_turn_selection_rejects_unknown_and_missing_status() {
        for turn in [
            json!({"id": "turn-queued", "status": "queued"}),
            json!({"id": "turn-1"}),
        ] {
            let error = active_turn_id_from_page(&json!({"data": [turn]})).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("AUTO_NOTIFY_UNSUPPORTED_TURN_STATUS")
                    || error.to_string().contains("is missing status"),
                "{error}"
            );
        }
    }

    #[test]
    fn active_turn_selection_rejects_missing_id() {
        assert_malformed_active_turn(
            json!({"data": [{"status": "inProgress"}]}),
            "data[0] is missing id",
        );
    }

    #[test]
    fn active_turn_selection_rejects_non_string_id() {
        assert_malformed_active_turn(
            json!({"data": [{"id": 7, "status": "inProgress"}]}),
            "data[0] id must be a JSON string",
        );
    }

    #[test]
    fn active_turn_selection_rejects_empty_id() {
        for id in ["", "   "] {
            assert_malformed_active_turn(
                json!({"data": [{"id": id, "status": "inProgress"}]}),
                "data[0] id must be non-empty after trim",
            );
        }
    }

    #[test]
    fn active_turn_selection_rejects_whitespace_containing_id() {
        assert_malformed_active_turn(
            json!({"data": [{"id": "turn active", "status": "inProgress"}]}),
            "data[0] id must not contain whitespace",
        );
    }

    #[test]
    fn active_turn_selection_handles_valid_zero_and_multiple_turns() {
        assert_eq!(
            active_turn_id_from_page(&json!({"data": []})).unwrap(),
            None
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-active", "status": "inProgress"}]
            }))
            .unwrap()
            .as_deref(),
            Some("turn-active")
        );
        let error = active_turn_id_from_page(&json!({
            "data": [
                {"id": "turn-1", "status": "inProgress"},
                {"id": "turn-2", "status": "inProgress"}
            ]
        }))
        .unwrap_err();
        assert!(
            error.to_string().contains("STEER_ACTIVE_TURN_AMBIGUOUS"),
            "{error}"
        );
    }

    #[test]
    fn notification_action_uses_turn_start_when_active_has_no_in_progress_turn() {
        assert_eq!(
            notification_action("active", None).unwrap(),
            NotificationAction::Start
        );
        assert_eq!(
            notification_action("active", Some("turn-active".into())).unwrap(),
            NotificationAction::Steer("turn-active".into())
        );
        assert_eq!(
            notification_action("idle", None).unwrap(),
            NotificationAction::Start
        );
        // A cold thread is loaded by `turn/start`, which is the native
        // load-and-start call, so notLoaded resolves to Start.
        assert_eq!(
            notification_action("notLoaded", None).unwrap(),
            NotificationAction::Start
        );
        for status in ["systemError", "unknown", ""] {
            let error = notification_action(status, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("AUTO_NOTIFY_UNSUPPORTED_THREAD_STATUS"),
                "{error}"
            );
        }
    }
