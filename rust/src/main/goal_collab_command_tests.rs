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
fn goal_owner_context_reads_the_nested_collab_snapshot_envelope() {
    // `collab context` nests the live binding under `result.snapshot`. The
    // long-horizon role and owner gates must read that envelope, not a
    // top-level `identity` that no longer exists.
    let nested = serde_json::json!({
        "ok": true,
        "result": {"snapshot": {
            "identity": {
                "worker_id": "codex-%1",
                "transport": {
                    "kind": "tmux",
                    "endpoint": "/tmp/collab.sock",
                    "tmux_endpoint": {
                        "socket_path": "/tmp/collab.sock",
                        "server_pid": 123,
                        "tmux_session_id": "$1",
                        "pane_id": "%1",
                        "pane_pid": 456
                    }
                }
            },
            "liveness": {
                "live": true,
                "presence": "present",
                "transport_kind": "tmux",
                "endpoint": "/tmp/collab.sock"
            }
        }}
    });
    assert_eq!(
        crate::long_horizon_role::normalized_collab_context(&nested)["identity"]["worker_id"],
        "codex-%1"
    );
    assert!(context_has_live_tmux_transport(&nested));
    let mut conflicting = nested.clone();
    conflicting["identity"] = serde_json::json!({"worker_id": "top-level"});
    assert_eq!(
        crate::long_horizon_role::normalized_collab_context(&conflicting),
        nested["result"]["snapshot"],
        "the object-valued nested snapshot must be selected without merging top-level fields"
    );

    // A legacy top-level context still normalizes to itself.
    let legacy = serde_json::json!({
        "identity": {"worker_id": "legacy"},
        "liveness": {"live": true}
    });
    assert_eq!(
        crate::long_horizon_role::normalized_collab_context(&legacy),
        legacy,
        "legacy top-level input must be preserved as-is"
    );
    let scalar_snapshot = serde_json::json!({
        "result": {"snapshot": "not-an-object"},
        "identity": {"worker_id": "legacy"}
    });
    assert_eq!(
        crate::long_horizon_role::normalized_collab_context(&scalar_snapshot),
        scalar_snapshot,
        "a non-object snapshot must fall back to the original input"
    );
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
