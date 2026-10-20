include!("support/tmux_cli_fixture.rs");

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run_in_pane_with_tmux(
    fixture: &Fixture,
    pane: &Pane,
    label: &str,
    tmux_value: &str,
    args: &[&str],
) -> Value {
    let script_path = fixture.root.join(format!("{label}.sh"));
    let result_path = fixture.root.join(format!("{label}.json"));
    let error_path = fixture.root.join(format!("{label}.error"));
    let done_path = fixture.root.join(format!("{label}.done"));
    let command = args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "set -e\n\
         cd {root}\n\
         unset COLLAB_SOCKET_PATH COLLAB_HOST_SOCKET COLLAB_LOCK_PATH COLLAB_HOST_LOCK COLLAB_APPSERVER_SOCKET CODEX_APP_SERVER_SOCKET COLLAB_APPSERVER_NAMESPACE COLLAB_WORKER CODEX_SESSION_ID CODEX_THREAD_ID CODEX_THREAD DSH_SESSION_ID\n\
         export COLLAB_STATE_DIR={state} CODEX_HOME={home} CODEX_INTERNAL_ORIGINATOR_OVERRIDE='Codex TUI'\n\
         export TMUX={tmux_value} TMUX_PANE={pane_id}\n\
         {binary} {command} >{result} 2>{error}\n\
         printf done >{done}\n\
         sleep 600\n",
        root = shell_quote(&fixture.root.to_string_lossy()),
        state = shell_quote(&fixture.host_state.to_string_lossy()),
        home = shell_quote(&fixture.root.join("home").to_string_lossy()),
        tmux_value = shell_quote(tmux_value),
        pane_id = shell_quote(&pane.pane_id),
        binary = shell_quote(&fixture.binary.to_string_lossy()),
        result = shell_quote(&result_path.to_string_lossy()),
        error = shell_quote(&error_path.to_string_lossy()),
        done = shell_quote(&done_path.to_string_lossy()),
    );
    std::fs::write(&script_path, script).unwrap();
    tmux(
        &fixture.tmux_socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &pane.pane_id,
            &format!("sh {}", shell_quote(&script_path.to_string_lossy())),
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        done_path.exists(),
        "{label} failed: {} {}",
        std::fs::read_to_string(&error_path).unwrap_or_default(),
        std::fs::read_to_string(&result_path).unwrap_or_default()
    );
    serde_json::from_slice(&std::fs::read(result_path).unwrap()).unwrap()
}

fn context_snapshot(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .unwrap_or(value)
}

fn persisted_identity(fixture: &Fixture, worker_id: &str) -> Value {
    let path = fixture
        .host_state
        .join("identities")
        .join(worker_id)
        .join("identity.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

struct SecondaryTmux {
    socket: PathBuf,
    session: String,
}

impl Drop for SecondaryTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args([
                "-S",
                self.socket.to_str().unwrap(),
                "kill-session",
                "-t",
                &self.session,
            ])
            .output();
    }
}

#[test]
fn current_pane_placement_ignores_tmux_value_and_preserves_session_peer() {
    let (mut fixture, first) = single_pane_fixture("identity-current-placement");
    std::fs::write(
        fixture.host_state.join("tmux-endpoints.json"),
        serde_json::to_vec(&json!({
            "endpoints": [fixture.tmux_socket.to_string_lossy()]
        }))
        .unwrap(),
    )
    .unwrap();

    let malformed = run_in_pane_with_tmux(
        &fixture,
        &first,
        "placement-malformed",
        "not-a-tmux-value",
        &["context", "--op", "placement-malformed"],
    );
    assert_eq!(malformed["ok"], true, "{malformed}");
    assert_eq!(
        context_snapshot(&malformed)["identity"]["transport"]["kind"],
        "tmux",
        "{malformed}"
    );
    let worker_id = context_snapshot(&malformed)["identity"]["worker_id"]
        .as_str()
        .expect("context snapshot names the worker")
        .to_owned();
    let token_before = persisted_identity(&fixture, &worker_id)["token"]
        .as_str()
        .expect("persisted identity carries the private token")
        .to_owned();

    let promoted = run_in_pane_with_tmux(
        &fixture,
        &first,
        "placement-promote",
        "/tmp/stale-collab-tmux,999999,0",
        &[
            "master",
            "promote",
            "--approval",
            "user assigned current session master",
        ],
    );
    assert_eq!(promoted["master"], worker_id, "{promoted}");

    let at_ms = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 120_000)
        .to_string();
    let subscribed = run_in_pane_with_tmux(
        &fixture,
        &first,
        "placement-patrol-subscribe",
        "invalid",
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "placement:patrol",
            "--at-ms",
            &at_ms,
            "--ttl-seconds",
            "300",
        ],
    );
    let subscription_before = subscribed["subscription"].clone();
    let subscription_id = subscription_before["id"].as_str().unwrap().to_owned();

    let split = tmux(
        &fixture.tmux_socket,
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &first.pane_id,
            "sleep 600",
        ],
    );
    let moved = Pane {
        server_pid: first.server_pid,
        pane_id: String::from_utf8(split.stdout).unwrap().trim().to_owned(),
        session_anchor: "ignored-session".into(),
        thread_anchor: "ignored-thread".into(),
    };
    let moved_context = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-moved",
        "/tmp/stale-collab-tmux,999999,0",
        &["context", "--op", "placement-moved"],
    );
    assert_eq!(moved_context["ok"], true, "{moved_context}");
    let moved_snapshot = context_snapshot(&moved_context);
    assert_eq!(moved_snapshot["identity"]["worker_id"], worker_id);
    assert_eq!(moved_snapshot["identity"]["role"], "master");
    assert_eq!(
        moved_snapshot["binding"]["tmux_endpoint"]["pane_id"],
        moved.pane_id
    );
    assert_eq!(
        persisted_identity(&fixture, &worker_id)["token"],
        token_before
    );
    let notification_status = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-patrol-status",
        "invalid",
        &["notify", "status"],
    );
    let subscription_after = notification_status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["id"] == subscription_id)
        .unwrap();
    assert_eq!(subscription_after["target"], moved.pane_id);
    assert_eq!(subscription_after["status"], "armed");
    for field in [
        "created_ms",
        "expires_ms",
        "trigger_ms",
        "repeat_count",
        "fired_count",
    ] {
        assert_eq!(
            subscription_after[field], subscription_before[field],
            "schedule changed: {field}"
        );
    }
    let retried_from_new_process = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-retry",
        "not-a-tmux-value",
        &["context", "--op", "placement-malformed"],
    );
    assert_eq!(retried_from_new_process["ok"], true);
    assert_eq!(
        context_snapshot(&retried_from_new_process)["identity"]["worker_id"],
        worker_id
    );
    let recovery_args = [
        "context",
        "--op",
        "placement-restore-master",
        "--approve-grant",
        "{\"action\":\"restore_master\",\"authorization\":\"user approved current recovery\"}",
    ];
    let recovered = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-recovery",
        "not-a-tmux-value",
        &recovery_args,
    );
    assert_eq!(recovered["ok"], true, "{recovered}");
    assert_eq!(
        context_snapshot(&recovered)["identity"]["role"],
        "master",
        "{recovered}"
    );
    let recovery_replay = run_in_pane_with_tmux(
        &fixture,
        &first,
        "placement-recovery-replay",
        "not-a-tmux-value",
        &recovery_args,
    );
    assert_eq!(recovery_replay["ok"], true, "{recovery_replay}");
    assert_eq!(
        context_snapshot(&recovery_replay)["identity"]["token"],
        Value::Null,
        "public readback must not expose the credential"
    );
    assert_eq!(
        persisted_identity(&fixture, &worker_id)["token"],
        token_before
    );

    let sent = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-send",
        "not-a-tmux-value",
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "current placement",
            "--delivery",
            "queued",
            "body after placement move",
        ],
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("send returns a durable message id")
        .to_owned();
    let received = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-recv",
        "not-a-tmux-value",
        &["recv", "--timeout", "0", "--receive-id", "placement-recv"],
    );
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == message_id),
        "{received}"
    );
    let reply = run_in_pane_with_tmux(
        &fixture,
        &first,
        "placement-reply",
        "not-a-tmux-value",
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "reply",
            "--delivery",
            "queued",
            "reply from superseded placement",
        ],
    );
    let reply_id = reply["msg_id"]
        .as_str()
        .or_else(|| reply["message_id"].as_str())
        .expect("reply returns a durable message id");
    let reply_received = run_in_pane_with_tmux(
        &fixture,
        &moved,
        "placement-reply-recv",
        "not-a-tmux-value",
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "placement-reply-recv",
        ],
    );
    assert!(
        reply_received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == reply_id),
        "{reply_received}"
    );
}

#[test]
fn overlapping_pane_ids_resolve_the_descendant_callers_server() {
    let (fixture, first) = single_pane_fixture("identity-overlap-caller");
    let secondary = SecondaryTmux {
        socket: fixture.root.join("secondary.sock"),
        session: "identity-overlap-secondary".into(),
    };
    tmux(
        &secondary.socket,
        &["new-session", "-d", "-s", &secondary.session, "sleep 600"],
    );
    let secondary_pane =
        String::from_utf8(tmux(&secondary.socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    assert_eq!(
        secondary_pane, first.pane_id,
        "the test requires overlapping pane ids"
    );
    std::fs::write(
        fixture.host_state.join("tmux-endpoints.json"),
        serde_json::to_vec(&json!({
            "endpoints": [
                fixture.tmux_socket.to_string_lossy(),
                secondary.socket.to_string_lossy(),
                "/tmp/collab-dead-unrelated-tmux.sock"
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let registered = run_in_pane_with_tmux(
        &fixture,
        &first,
        "overlap-context",
        "not-a-tmux-value",
        &["context", "--op", "overlap-context"],
    );
    assert_eq!(registered["ok"], true, "{registered}");
    let route = run_in_pane_with_tmux(
        &fixture,
        &first,
        "overlap-route",
        "/tmp/stale-collab-tmux,999999,0",
        &["route", "resolve", "--pane-id", &first.pane_id],
    );
    assert_eq!(
        route["tmux_endpoint"]["socket_path"],
        fixture.tmux_socket.to_string_lossy().as_ref(),
        "{route}"
    );
    assert_ne!(
        route["tmux_endpoint"]["socket_path"],
        secondary.socket.to_string_lossy().as_ref(),
        "{route}"
    );
}
