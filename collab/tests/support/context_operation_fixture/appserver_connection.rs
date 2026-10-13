fn serve_appserver_connection(
    mut stream: UnixStream,
    root: &Path,
    session_id: &str,
    thread_id: &str,
    state: &Arc<Mutex<TestAppServerState>>,
    gate: &Arc<ArchiveGate>,
) {
    // macOS carries O_NONBLOCK from a nonblocking listener to accepted
    // sockets. Keep only accept() nonblocking; frame reads need to wait for
    // the peer's next request after the HTTP upgrade.
    stream
        .set_nonblocking(false)
        .expect("set accepted AppServer stream blocking");
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if stream.read_exact(&mut byte).is_err() {
            return;
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .is_err()
    {
        return;
    }
    loop {
        let payload = match read_appserver_frame(&mut stream) {
            Ok(Some(payload)) => payload,
            Ok(None) => return,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return;
            }
            Err(error) => {
                eprintln!("test AppServer websocket read failed: {error}");
                return;
            }
        };
        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
            eprintln!("test AppServer received invalid JSON frame");
            return;
        };
        eprintln!(
            "test AppServer request method={:?} id={:?}",
            request.get("method"),
            request.get("id")
        );
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str);
        let response = match method {
            Some("thread/start") => {
                let mut state = state.lock().unwrap();
                state.start_requests += 1;
                let created = format!("created-thread-{}", state.start_requests);
                state.created_thread = Some(created.clone());
                state.created_cwd = request
                    .pointer("/params/cwd")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_owned();
                if state.effect == "create_response_loss" {
                    return;
                }
                // A notification with another id must never be accepted as the start response.
                let unrelated = json!({"id":999999,"result":{"thread":{"id":"unrelated-thread"}}});
                write_appserver_frame(&mut stream, &serde_json::to_vec(&unrelated).unwrap())
                    .unwrap();
                json!({"id":id,"result":{"thread":{"id":created}}})
            }
            Some("thread/resume") => {
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or(thread_id);
                json!({"id":id,"result":{"thread":{"id":requested,"status":{"type":"idle"}}}})
            }
            Some("turn/start") => {
                let mut state = state.lock().unwrap();
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                // `immediate_notify` carries the body inside the delegated
                // tool output for a fresh turn and inside `input` when
                // steering; accept either carrier.
                let input_text = request
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_output = request
                    .pointer("/params/toolOutput/output")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let prompt = format!("{input_text}\n{tool_output}");
                let marker_file = challenge_marker_from_prompt(&prompt);
                let client_message_id = request
                    .pointer("/params/clientUserMessageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_create_readiness = client_message_id.starts_with("create-ready-");
                let is_update_challenge = client_message_id.starts_with("update-challenge-");
                let now = now_secs();
                let effect = state.effect.clone();
                if is_create_readiness {
                    state.readiness_requests += 1;
                    let turn_id = "create-readiness-turn";
                    let cwd = state.created_cwd.clone();
                    let marker = marker_file
                        .as_deref()
                        .and_then(|file| read_challenge_marker(&cwd, file));
                    match effect.as_str() {
                        "create_ready" | "ready_and_update_ok" | "reject_generic_turn_start" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-challenge", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "create_ready_delayed" => {
                            state.challenge_marker = marker_file.clone();
                            state.challenge_reads = 0;
                            state.created_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        // Producer-side correlation failures: the turn is
                        // dispatched, but the history cannot prove the exact
                        // marker, turn, or thread, so it must never finalize.
                        "create_ready_wrong_marker" => {
                            if marker.is_some() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-marker", "type": "agentMessage",
                                        "text": format!("post-update:{cwd}:wrong-marker"),
                                    }],
                                }));
                            }
                        }
                        "create_ready_wrong_turn" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": "wrong-create-turn", "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-turn", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "create_ready_wrong_thread" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-thread", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        _ => {}
                    }
                    json!({"id": id, "result": {"turn": {"id": turn_id, "status": "inProgress"}}})
                } else if is_update_challenge {
                    state.challenge_requests += 1;
                    let turn_id = "update-challenge-turn";
                    let cwd = state.cwd.clone();
                    let marker = marker_file
                        .as_deref()
                        .and_then(|file| read_challenge_marker(&cwd, file));
                    let mut new_turns: Vec<Value> = Vec::new();
                    match effect.as_str() {
                        "completed_turn" | "update_challenge_ok" | "ready_and_update_ok" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-challenge", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_delayed" => {
                            state.challenge_marker = marker_file.clone();
                            state.challenge_reads = 0;
                            new_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        "update_challenge_quoted" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-quoted", "type": "agentMessage",
                                        "text": format!("\"{}\"", challenge_envelope(&cwd, marker)),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_failed" => {
                            new_turns.push(json!({
                                "id": turn_id, "status": "completed", "completedAt": now,
                                "items": [{
                                    "id": "update-failed", "type": "agentMessage",
                                    "text": "execution failed: cat: no such file or directory",
                                }],
                            }));
                        }
                        "update_challenge_prefix" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-prefix", "type": "agentMessage",
                                        "text": format!("post-update:{cwd}-longer:{marker}"),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_stale" => {
                            if let Some(marker) = marker.as_deref() {
                                // A stale turn completed in the same second but
                                // is not the dispatched challenge turn.
                                new_turns.push(json!({
                                    "id": "stale-preupdate-turn", "status": "completed",
                                    "completedAt": now,
                                    "items": [{
                                        "id": "update-stale", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                            new_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        "update_challenge_wrong_thread" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-wrong-thread", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        _ => {}
                    }
                    if requested.as_deref() == state.created_thread.as_deref() {
                        state.created_turns.extend(new_turns);
                    } else {
                        state.turns.extend(new_turns);
                    }
                    json!({"id": id, "result": {"turn": {"id": turn_id, "status": "inProgress"}}})
                } else {
                    let register_marker = prompt.lines().find_map(|line| {
                        let prefix = "COLLAB_REGISTER_CHALLENGE ";
                        let start = line.find(prefix)?;
                        let challenge = line[start + prefix.len()..].split("</input>").next()?;
                        let challenge: Value = serde_json::from_str(challenge).ok()?;
                        challenge["challenge"].as_str().map(str::to_owned)
                    });
                    if let Some(marker) = register_marker.as_deref() {
                        let turn_id = "registration-challenge-turn";
                        if state.effect != "register_no_reply" {
                            let text = if state.effect == "register_wrong_reply" {
                                "wrong-reply"
                            } else {
                                marker
                            };
                            let turn = json!({"id":turn_id, "status":"completed", "completedAt":now,
                                "items":[{"type":"userMessage", "clientMessageId":client_message_id},
                                    {"type":"agentMessage", "text":text}]});
                            if requested.as_deref() == state.created_thread.as_deref() {
                                state.created_turns.push(turn);
                            } else {
                                state.turns.push(turn);
                            }
                        }
                        json!({"id":id,"result":{"turn":{"id":turn_id,"status":"completed"}}})
                    } else if state.effect == "reject_generic_turn_start" {
                        json!({
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": "APPSERVER_NOTIFICATION_REJECTED: fixture refused generic notification",
                                "data": {
                                    "notification_error": "fixture refused generic notification",
                                    "repair_required": true
                                }
                            }
                        })
                    } else {
                        json!({"id": id, "result": {"turn": {"id": "unrelated-turn", "status": "completed"}}})
                    }
                }
            }
            Some("thread/read") => {
                let include_turns = request
                    .pointer("/params/includeTurns")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let effect = state.lock().unwrap().effect.clone();
                let archived = state.lock().unwrap().archived;
                if effect == "archive_hold_readback" && archived {
                    let mut inner = gate.inner.lock().unwrap();
                    inner.readback_requests += 1;
                    while !inner.release_readback {
                        inner = gate.changed.wait(inner).unwrap();
                    }
                }
                let mut state = state.lock().unwrap();
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or(thread_id);
                let created = state.created_thread.as_deref() == Some(requested);
                if created && state.effect == "create_register_failure" {
                    // The first exact read establishes the session; formal Register then fails its probe.
                    if !include_turns {
                        return;
                    }
                }
                if include_turns {
                    complete_delayed_challenge(&mut state, created);
                }
                let mut thread = json!({
                    "id": if created { requested } else { thread_id },
                    "sessionId": if created {
                        if state.start_requests == 1 {
                            "created-session".to_owned()
                        } else {
                            format!("created-session-{}", state.start_requests)
                        }
                    } else {
                        session_id.to_owned()
                    },
                    "cwd": if created { &state.created_cwd } else { &state.cwd },
                    "status": {"type": if state.archived { "notLoaded" } else { "idle" }}
                });
                // Simulate a completed challenge that landed on a different
                // thread: the exact-thread correlation must reject it. The
                // post-start read must still see the exact created thread so
                // registration can complete; only post-challenge reads are
                // redirected.
                if created
                    && include_turns
                    && state.effect == "create_ready_wrong_thread"
                    && state.readiness_requests > 0
                {
                    thread["id"] = json!("wrong-thread");
                }
                if !created
                    && include_turns
                    && state.effect == "update_challenge_wrong_thread"
                    && state.challenge_requests > 0
                {
                    thread["id"] = json!("wrong-thread");
                }
                if include_turns {
                    thread["turns"] = if created {
                        json!(state.created_turns)
                    } else {
                        json!(state.turns)
                    };
                }
                json!({"id": id, "result": {"thread": thread}})
            }
            Some("thread/archive") => {
                let effect = state.lock().unwrap().effect.clone();
                {
                    let mut inner = gate.inner.lock().unwrap();
                    inner.archive_requests += 1;
                    if effect == "archive_hold_readback" {
                        while !inner.release_archive {
                            inner = gate.changed.wait(inner).unwrap();
                        }
                    }
                }
                match effect.as_str() {
                    "archive_unknown" => return,
                    "archive_missing" => {
                        json!({"id": id, "error": {"code": -32000, "message": format!("thread {thread_id} not found")}})
                    }
                    "archive_refused" => {
                        json!({"id": id, "error": {"code": -32000, "message": "archive refused"}})
                    }
                    _ => {
                        state.lock().unwrap().archived = true;
                        json!({"id": id, "result": {"archived": true}})
                    }
                }
            }
            Some("thread/settings/update") => {
                let effect = state.lock().unwrap().effect.clone();
                if effect == "unknown" {
                    return;
                }
                if effect == "refused" {
                    json!({"id": id, "error": {"code": -32000, "message": "settings refused"}})
                } else {
                    let cwd = request
                        .pointer("/params/cwd")
                        .and_then(Value::as_str)
                        .unwrap_or(root.to_string_lossy().as_ref())
                        .to_owned();
                    let mut state = state.lock().unwrap();
                    state.settings_requests += 1;
                    state.cwd = cwd.clone();
                    json!({"id": id, "result": {"thread": {"id": thread_id, "cwd": cwd}}})
                }
            }
            _ => json!({"id": id, "result": {}}),
        };
        let Ok(encoded) = serde_json::to_vec(&response) else {
            return;
        };
        if let Err(error) = write_appserver_frame(&mut stream, &encoded) {
            eprintln!("test AppServer websocket write failed: {error}");
            return;
        }
    }
}
