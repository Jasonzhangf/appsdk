use super::*;

#[test]
fn injected_timeout_returns_explicit_error_without_long_wait() {
    let started = Instant::now();
    let mut command = Command::new("/bin/sleep");
    command.arg("1");
    let result = run_goal_collab_command(command, Duration::from_millis(100));

    assert!(matches!(
        result,
        Err(error) if error == "GOAL_COLLAB_COMMAND_TIMEOUT"
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn injected_timeout_bounds_output_pipe_drain_without_false_success() {
    let started = Instant::now();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "/bin/sleep 5 & printf inherited-pipe"]);

    let result = run_goal_collab_command(command, Duration::from_millis(100));

    assert!(matches!(
        result,
        Err(error) if error == "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn goal_owner_context_requires_a_live_tmux_endpoint() {
    let tmux_context = serde_json::json!({
        "identity": {"transport": {
            "kind": "tmux",
            "endpoint": "/tmp/collab.sock",
            "tmux_endpoint": {
                "socket_path": "/tmp/collab.sock",
                "server_pid": 123,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 456
            }
        }},
        "liveness": {
            "live": true,
            "presence": "present",
            "transport_kind": "tmux",
            "endpoint": "/tmp/collab.sock"
        }
    });
    assert!(context_has_live_tmux_transport(&tmux_context));

    let appserver_context = serde_json::json!({
        "identity": {"transport": {"kind": "appserver", "thread_id": "thread-1"}},
        "liveness": {"live": true, "transport_kind": "appserver"}
    });
    assert!(!context_has_live_tmux_transport(&appserver_context));

    let mut unknown = tmux_context;
    unknown["liveness"]["presence"] = Value::String("unknown".into());
    assert!(!context_has_live_tmux_transport(&unknown));
}

#[test]
fn collab_tmux_init_requires_selected_endpoint_and_identity_anchor_match() {
    let response = serde_json::json!({
        "runtime": {
            "runtimeId": "runtime-1",
            "appserverId": "appserver-cli",
            "transport": "tmux",
            "tmuxEndpoint": {
                "socket_path": "/tmp/collab.sock",
                "server_pid": 123,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 456,
                "codex_session_id": "session-1",
                "codex_thread_id": "thread-1"
            },
            "projectRoot": "/repo",
            "capabilities": ["send_message_to_pane"],
            "processId": 123
        },
        "transport_selected": {
            "kind": "tmux",
            "endpoint": "/tmp/collab.sock",
            "namespace": "$1",
            "session_id": "session-1",
            "thread_id": "thread-1",
            "tmux_endpoint": {
                "socket_path": "/tmp/collab.sock",
                "server_pid": 123,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 456,
                "codex_session_id": "session-1",
                "codex_thread_id": "thread-1"
            },
            "capabilities": ["send_message_to_pane"]
        }
    });
    assert!(validate_collab_tmux_init(&response, "/repo").is_ok());

    let mut mismatched = response;
    mismatched["transport_selected"]["endpoint"] = Value::String("/tmp/other.sock".into());
    assert_eq!(
        validate_collab_tmux_init(&mismatched, "/repo").unwrap_err(),
        "COLLAB_INIT_TMUX_RUNTIME_BINDING_INVALID"
    );
}
