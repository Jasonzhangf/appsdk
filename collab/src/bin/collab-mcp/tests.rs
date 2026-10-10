use super::*;

#[test]
fn content_length_and_newline_frames_round_trip() {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}});
    let encoded = serde_json::to_string(&body).unwrap();
    let framed = format!("Content-Length: {}\r\n\r\n{encoded}", encoded.len());
    let (frame, req) = read_message(&mut framed.as_bytes()).unwrap().unwrap();
    assert!(matches!(frame, Frame::ContentLength));
    assert_eq!(
        handle(&req).unwrap()["result"]["protocolVersion"],
        "2025-03-26"
    );
    let line = format!("{encoded}\n");
    let (frame, req) = read_message(&mut line.as_bytes()).unwrap().unwrap();
    assert!(matches!(frame, Frame::Line));
    assert_eq!(
        handle(&req).unwrap()["result"]["serverInfo"]["name"],
        "collab"
    );
    assert_eq!(
        handle(&json!({"method":"resources/list","id":2})).unwrap()["result"]["resources"],
        json!([])
    );
}

#[test]
fn peer_lifecycle_mcp_schema_and_argv_are_closed_and_typed() {
    let definitions = tools();
    let lifecycle = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_peer_lifecycle")
        .expect("peer lifecycle tool");
    assert_eq!(lifecycle["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        lifecycle["inputSchema"]["oneOf"].as_array().unwrap().len(),
        5
    );
    let properties = lifecycle["inputSchema"]["properties"].as_object().unwrap();
    assert!(!properties.contains_key("token"));
    assert!(!properties.contains_key("query_capability"));
    assert_eq!(
            peer_lifecycle_argv(&json!({"action":"create","target_id":"ordinary-peer","cwd":"/tmp/project","model":"test-model","operation_id":"op-create"})).unwrap(),
            ["worker","create","ordinary-peer","--cwd","/tmp/project","--op","op-create","--model","test-model"]
        );
    assert!(peer_lifecycle_argv(&json!({"action":"create","target_id":"ordinary-peer","cwd":"/tmp","query_capability":"secret"})).is_err());

    assert_eq!(
        peer_lifecycle_argv(&json!({"action":"read","target_id":"peer-1"})).unwrap(),
        ["worker", "read", "peer-1"]
    );
    assert_eq!(
            peer_lifecycle_argv(&json!({"action":"update","target_id":"peer-1","cwd":"/tmp/project","operation_id":"op-1"})).unwrap(),
            ["worker", "update", "peer-1", "--cwd", "/tmp/project", "--op", "op-1"]
        );
    assert_eq!(
        peer_lifecycle_argv(&json!({"action":"close","target_id":"peer-1","reason":"finished"}))
            .unwrap(),
        ["worker", "close", "peer-1", "--reason", "finished"]
    );
    assert_eq!(
        peer_lifecycle_argv(&json!({"action":"query","operation_id":"op-1"})).unwrap(),
        ["worker", "query", "--op", "op-1"]
    );
    assert!(peer_lifecycle_argv(&json!({"action":"read","cwd":"/tmp"})).is_err());
    assert!(peer_lifecycle_argv(&json!({"action":"read","target_id":17})).is_err());
    assert!(peer_lifecycle_argv(
        &json!({"action":"update","target_id":"peer-1","cwd":"/tmp","token":"secret"})
    )
    .is_err());
    assert!(peer_lifecycle_argv(&json!({"action":"query","operation_id":true})).is_err());
}

#[test]
fn peer_lifecycle_mcp_preserves_typed_failures_and_rejects_empty_success() {
    let refused = r#"{"ok":false,"error":"PEER_LIFECYCLE_RESPONSIBILITY_CONFLICT","result":{"outcome":"refused"}}"#;
    assert_eq!(
        peer_lifecycle_cli_output(false, refused, "collab response on stderr"),
        Ok(refused.to_owned())
    );
    assert!(peer_lifecycle_cli_output(true, "", "")
        .unwrap_err()
        .contains("EMPTY_RESULT"));
    assert!(peer_lifecycle_cli_output(true, "not json", "")
        .unwrap_err()
        .contains("INVALID_RESULT"));
    assert!(peer_lifecycle_cli_output(true, r#"{"ok":true}"#, "")
        .unwrap_err()
        .contains("typed result"));
    assert!(
        peer_lifecycle_cli_output(false, r#"{"ok":true,"result":{}}"#, "daemon failed").is_err()
    );
}

#[test]
fn sendmessage_schema_requires_subject_and_body() {
    let definitions = tools();
    let send = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_sendmessage")
        .unwrap();
    assert_eq!(
        send["inputSchema"]["required"],
        json!(["to", "subject", "body"])
    );
    assert!(send["inputSchema"]["properties"]["subject"].is_object());
}

#[test]
fn collab_init_is_not_exposed_by_mcp() {
    let definitions = tools();
    assert!(
        definitions
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "collab_init"),
        "the removed CLI compatibility entry must not remain an MCP tool"
    );
    assert!(build_argv("collab_init", &json!({})).is_err());
}

#[test]
fn context_schema_exposes_only_optional_fact_supplement() {
    let definitions = tools();
    let context = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_context")
        .expect("collab_context tool");
    assert_eq!(context["inputSchema"]["required"], json!([]));
    let provide = &context["inputSchema"]["properties"]["provide"];
    assert!(provide["oneOf"].is_array(), "{provide}");
    assert!(context["description"]
        .as_str()
        .unwrap()
        .contains("required_fields"));
    assert!(
        definitions
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "collab_whoami"),
        "removed identity command must not remain in the MCP surface"
    );
}

#[test]
fn context_schema_exposes_operation_id_and_query_without_leaking_capability() {
    let definitions = tools();
    let context = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_context")
        .expect("collab_context tool");
    let properties = &context["inputSchema"]["properties"];
    assert_eq!(properties["operation_id"]["type"], json!("string"));
    assert_eq!(properties["query"]["type"], json!("boolean"));
    assert!(
        properties.get("query_capability").is_none(),
        "the raw query capability must never be an MCP argument"
    );

    let query_argv = build_argv(
        "collab_context",
        &json!({"operation_id": "ctxop-query", "query": true}),
    )
    .expect("query must build argv");
    assert_eq!(
        query_argv,
        vec!["context", "--op", "ctxop-query", "--query"]
    );
    let supplement_argv = build_argv(
        "collab_context",
        &json!({
            "operation_id": "ctxop-supplement",
            "provide": {"session_id": "session-1"}
        }),
    )
    .expect("supplement must build argv");
    assert_eq!(
        supplement_argv,
        vec![
            "context",
            "--op",
            "ctxop-supplement",
            "--provide",
            r#"{"session_id":"session-1"}"#
        ]
    );
    assert!(build_argv("collab_context", &json!({"operation_id": ""})).is_err());
}

#[test]
fn context_schema_exposes_distinct_scope_and_approval_arguments() {
    let definitions = tools();
    let context = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_context")
        .expect("collab_context tool");
    let properties = &context["inputSchema"]["properties"];
    for field in [
        "project_scope",
        "app_scope_id",
        "approve_identity",
        "approve_grant",
    ] {
        assert!(properties[field].is_object(), "{properties}");
    }

    let identity = json!({
        "decision": "approved",
        "target_identity": "worker-a",
        "expected_incumbent": {
            "binding_id": "binding-a",
            "endpoint_generation": 3
        }
    });
    let grant = json!({
        "decision": "approved",
        "target_identity": "worker-a",
        "expected_grant": {"grant_id": "grant-a", "generation": 4}
    });
    let argv = build_argv(
        "collab_context",
        &json!({
            "operation_id": "ctxop-approved",
            "project_scope": "/canonical/project",
            "app_scope_id": "appserver-cli",
            "approve_identity": identity.clone(),
            "approve_grant": grant.clone()
        }),
    )
    .expect("approved recovery must build argv");
    assert_eq!(argv[0], "context");
    assert_eq!(argv[1], "--op");
    assert_eq!(argv[2], "ctxop-approved");
    assert_eq!(argv[3], "--project");
    assert_eq!(argv[4], "/canonical/project");
    assert_eq!(argv[5], "--app-scope");
    assert_eq!(argv[6], "appserver-cli");
    assert_eq!(argv[7], "--approve-identity");
    assert_eq!(serde_json::from_str::<Value>(&argv[8]).unwrap(), identity);
    assert_eq!(argv[9], "--approve-grant");
    assert_eq!(serde_json::from_str::<Value>(&argv[10]).unwrap(), grant);

    let raw = r#"{"decision":"approved","target_identity":"worker-a"}"#;
    assert_eq!(
        build_argv("collab_context", &json!({"approve_identity": raw})).unwrap(),
        vec!["context", "--approve-identity", raw]
    );
    assert!(build_argv("collab_context", &json!({"approve_identity": 7})).is_err());
    assert!(build_argv("collab_context", &json!({"approve_grant": "not-json"})).is_err());
    assert!(build_argv(
        "collab_context",
        &json!({"query": true, "operation_id": "ctxop-q", "approve_identity": identity})
    )
    .is_ok_and(|argv| argv.contains(&"--approve-identity".to_owned())));
}

#[test]
fn context_typed_incomplete_result_is_a_tool_result_but_transport_failure_is_not() {
    static COLLAB_BIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = COLLAB_BIN_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "collab-mcp-context-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let run = |body: &str| {
        let script = dir.join("collab");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("COLLAB_BIN", &script);
        let result = call("collab_context", &json!({"operation_id": "ctxop-typed"}));
        std::env::remove_var("COLLAB_BIN");
        result
    };

    // A typed incomplete result is a valid payload: the CLI prints the
    // envelope to stdout and exits 2, and the adapter must surface it as a
    // normal tool result rather than a transport error.
    let typed = run(
            r#"printf '%s' '{"ok":false,"result":{"operation_id":"ctxop-typed","outcome":"missing_facts"}}'; exit 2"#,
        )
        .expect("typed incomplete result must be returned as a tool result");
    let payload: Value = serde_json::from_str(&typed).unwrap();
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["result"]["outcome"], "missing_facts");

    // A transport failure carries no typed envelope and stays an error.
    let failed = run("printf 'DAEMON_UNKNOWN: no socket' 1>&2; exit 1");
    assert!(failed.is_err(), "{failed:?}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn context_forwards_provide_object_and_string_unchanged() {
    let object = json!({
        "session_id": "session-1",
        "thread_id": "thread-1",
        "endpoint": "unix:///tmp/codex.sock",
        "namespace": "codex_tui"
    });
    let argv = build_argv("collab_context", &json!({"provide": object.clone()}))
        .expect("object supplement must build argv");
    assert_eq!(argv[0], "context");
    assert_eq!(argv[1], "--provide");
    assert_eq!(serde_json::from_str::<Value>(&argv[2]).unwrap(), object);

    let raw = r#"{"session_id":"session-1"}"#;
    assert_eq!(
        build_argv("collab_context", &json!({"provide": raw})).unwrap(),
        vec!["context", "--provide", raw]
    );
    assert!(build_argv("collab_context", &json!({"provide": 42})).is_err());
    assert!(build_argv("collab_context", &json!({"provide": ""})).is_err());
    assert!(build_argv(
        "collab_context",
        &json!({"provide": {"worker_id": "worker-1"}})
    )
    .is_err());
    assert!(build_argv("collab_context", &json!({"provide": {}})).is_err());
    assert!(build_argv("collab_context", &json!({"provide": {"session_id": null}})).is_err());
    assert!(build_argv("collab_context", &json!({"worker_id": "worker-1"})).is_err());
    assert!(build_argv(
        "collab_sendmessage",
        &json!({"to":"peer", "subject":"s", "body":"b", "from":"guessed-peer"})
    )
    .is_err());
    assert!(build_argv(
        "collab_recv",
        &json!({"receive_id":"r", "worker":"guessed-peer"})
    )
    .is_err());
}

#[test]
fn subagent_dispatch_schema_exposes_stable_request_and_task_fields() {
    let definitions = tools();
    let subagent = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_subagent")
        .unwrap();
    assert!(subagent["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .contains(&json!("dispatch")));
    let properties = subagent["inputSchema"]["properties"].as_object().unwrap();
    for field in ["request_id", "subject", "body", "feature_id", "priority"] {
        assert!(properties.contains_key(field), "missing MCP field {field}");
    }
}

#[test]
fn task_accept_schema_requires_owner_task_id() {
    let definitions = tools();
    let accept = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_task_accept")
        .unwrap();
    assert_eq!(accept["inputSchema"]["required"], json!(["id"]));
}

#[test]
fn recv_schema_requires_caller_supplied_receive_id() {
    let definitions = tools();
    let recv = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_recv")
        .expect("collab_recv tool");
    let properties = recv["inputSchema"]["properties"].as_object().unwrap();
    assert!(
        properties.contains_key("receive_id"),
        "MCP recv must expose the durable receive identity for replay"
    );
    assert_eq!(
        recv["inputSchema"]["required"],
        json!(["receive_id"]),
        "MCP recv must require a caller-owned receive identity"
    );
}

#[test]
fn recv_rejects_absent_or_invalid_receive_id_before_spawn() {
    for args in [
        json!({}),
        json!({"timeout": 30}),
        json!({"receive_id": null}),
        json!({"receive_id": 42}),
        json!({"receive_id": ""}),
        json!({"receive_id": "   "}),
    ] {
        let error = build_argv("collab_recv", &args)
            .expect_err(&format!("collab_recv must reject {args} before spawn"));
        assert!(
            error.contains("receive_id"),
            "unexpected error for {args}: {error}"
        );
    }
}

#[test]
fn recv_forwards_caller_receive_id_unchanged() {
    let argv = build_argv(
        "collab_recv",
        &json!({"timeout": 30, "receive_id": "recv-0922-abc"}),
    )
    .expect("valid receive_id must build argv");
    assert_eq!(
        argv,
        vec!["recv", "--timeout", "30", "--receive-id", "recv-0922-abc"]
    );
    let repeated = build_argv("collab_recv", &json!({"receive_id": "recv-0922-abc"}))
        .expect("repeat call must build argv");
    assert_eq!(repeated, vec!["recv", "--receive-id", "recv-0922-abc"]);
}

#[test]
fn master_idle_event_is_public_in_mcp_schema() {
    let definitions = tools();
    let subscribe = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_notify_subscribe")
        .unwrap();
    let events = subscribe["inputSchema"]["properties"]["event"]["enum"]
        .as_array()
        .unwrap();
    assert!(events.contains(&json!("master-idle")));
    assert!(events.contains(&json!("direct-message")));
    assert!(events.contains(&json!("resource-released")));
    assert!(events.contains(&json!("deadline")));
    assert!(!events.contains(&json!("async-result")));
}

#[test]
fn notification_schedule_fields_match_mcp_call_arguments() {
    let definitions = tools();
    let subscribe = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_notify_subscribe")
        .unwrap();
    let properties = subscribe["inputSchema"]["properties"].as_object().unwrap();
    for field in [
        "at_ms",
        "every_ms",
        "trigger_ms",
        "repeat_count",
        "ttl_seconds",
    ] {
        assert!(properties.contains_key(field), "missing MCP field {field}");
    }
}

#[test]
fn master_tool_exposes_status_promote_clear_and_delegate() {
    let definitions = tools();
    let master = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "collab_master")
        .unwrap();
    assert_eq!(master["inputSchema"]["required"], json!(["action"]));
    assert_eq!(
        master["inputSchema"]["properties"]["action"]["enum"],
        json!(["status", "promote", "clear", "delegate"])
    );
}

#[test]
fn lifecycle_review_and_integration_tools_are_exposed() {
    let definitions = tools();
    for (name, required) in [
        ("collab_task_review", json!(["id", "evidence"])),
        (
            "collab_task_integrated",
            json!(["id", "commit", "evidence"]),
        ),
    ] {
        let tool = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing MCP tool {name}"));
        assert_eq!(tool["inputSchema"]["required"], required);
    }
}
