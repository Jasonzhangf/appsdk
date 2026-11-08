#[path = "support/upgrade_health.rs"]
mod support;

use serde_json::Value;
use std::os::unix::net::UnixListener;
use std::time::Duration;

use support::*;

#[test]
fn status_check_is_a_public_read_only_entry() {
    let _test = test_guard();
    let root = temp_root("public-entry");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let state_before = snapshot(&state_root);
    let responder = std::thread::spawn(move || {
        let mut stream = connect(&listener, Duration::from_secs(3));
        assert_eq!(request(&mut stream)["op"], "Ping");
        std::io::Write::write_all(
            &mut stream,
            &ping_response(Some(VERSION), Some("capability-readiness-v1")),
        )
        .unwrap();
    });

    let output = run_check(&cli, &project_root, &state_root, false);
    if let Err(error) = responder.join() {
        panic!(
            "controlled listener failed: {error:?}; child output: {}",
            text(&output)
        );
    }
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "pending_user_activation", "{value}");
    assert_eq!(
        value["required_action"], "caller_context_required",
        "{value}"
    );
    assert_eq!(value["checks"]["daemon"]["status"], "passed", "{value}");
    assert_eq!(
        snapshot(&state_root),
        state_before,
        "status --check mutated host state"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn status_check_local_rejects_missing_sibling_mcp() {
    let _test = test_guard();
    let root = temp_root("local-missing");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = root.join("bin").join("collab");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_collab"), &cli).unwrap();

    let output = run_check(&cli, &project_root, &state_root, true);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("COLLAB_HEALTH_LOCAL_MCP_MISSING"),
        "{}",
        text(&output)
    );
    assert!(!state_root.join("server.sock").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn local_check_rejects_wrong_mcp_version() {
    let _test = test_guard();
    let root = temp_root("local-version");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    scripted_mcp(
        &cli,
        r#"#!/bin/sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","serverInfo":{"name":"collab","version":"0.2.9999"}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'
"#,
    );
    let output = run_check(&cli, &project_root, &state_root, true);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("COLLAB_HEALTH_LOCAL_MCP_VERSION_DIFFERS"),
        "{}",
        text(&output)
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn local_check_rejects_missing_tool() {
    let _test = test_guard();
    let root = temp_root("local-tool");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    scripted_mcp(
        &cli,
        r#"#!/bin/sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","serverInfo":{"name":"collab","version":"0.2.0000"}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'
"#,
    );
    let output = run_check(&cli, &project_root, &state_root, true);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("COLLAB_HEALTH_LOCAL_MCP_TOOL_MISSING"),
        "{}",
        text(&output)
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn local_check_rejects_malformed_frame() {
    let _test = test_guard();
    let root = temp_root("local-frame");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    scripted_mcp(&cli, "#!/bin/sh\nprintf 'this is not json\\n'\n");
    let output = run_check(&cli, &project_root, &state_root, true);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("COLLAB_HEALTH_LOCAL_MCP_FRAME_INVALID"),
        "{}",
        text(&output)
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn full_check_rejects_wrong_daemon_version() {
    let _test = test_guard();
    let root = temp_root("daemon-version");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let responder = std::thread::spawn(move || {
        let mut stream = connect(&listener, Duration::from_secs(3));
        assert_eq!(request(&mut stream)["op"], "Ping");
        std::io::Write::write_all(
            &mut stream,
            &ping_response(Some("0.2.9999"), Some("capability-readiness-v1")),
        )
        .unwrap();
    });
    let output = run_check(&cli, &project_root, &state_root, false);
    if let Err(error) = responder.join() {
        panic!(
            "controlled listener failed: {error:?}; child output: {}",
            text(&output)
        );
    }
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["required_action"], "daemon_version_differs",
        "{value}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn full_check_rejects_unsupported_daemon() {
    let _test = test_guard();
    let root = temp_root("daemon-unsupported");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let responder = std::thread::spawn(move || {
        let mut stream = connect(&listener, Duration::from_secs(3));
        assert_eq!(request(&mut stream)["op"], "Ping");
        std::io::Write::write_all(&mut stream, &ping_response(Some(VERSION), None)).unwrap();
    });
    let output = run_check(&cli, &project_root, &state_root, false);
    if let Err(error) = responder.join() {
        panic!(
            "controlled listener failed: {error:?}; child output: {}",
            text(&output)
        );
    }
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["required_action"], "daemon_readiness_unsupported",
        "{value}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn full_check_reports_daemon_absent() {
    let _test = test_guard();
    let root = temp_root("daemon-absent");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    let output = run_check(&cli, &project_root, &state_root, false);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["required_action"], "daemon_absent", "{value}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn full_check_reports_malformed_daemon_response_as_error() {
    let _test = test_guard();
    let root = temp_root("daemon-malformed");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let cli = copy_pair(&root);
    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let responder = std::thread::spawn(move || {
        let mut stream = connect(&listener, Duration::from_secs(3));
        assert_eq!(request(&mut stream)["op"], "Ping");
        std::io::Write::write_all(&mut stream, b"not json\n").unwrap();
    });
    let output = run_check(&cli, &project_root, &state_root, false);
    if let Err(error) = responder.join() {
        panic!(
            "controlled listener failed: {error:?}; child output: {}",
            text(&output)
        );
    }
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert_ne!(output.status.code(), Some(3), "{}", text(&output));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn full_check_with_retained_dsh_caller_is_success_and_read_only() {
    let _test = test_guard();
    let root = temp_root("read-only-success");
    let project_root = create_project(&root);
    let state_root = root.join("host-state");
    std::fs::create_dir_all(&state_root).unwrap();
    retained_dsh_identity(&project_root, &state_root);
    let cli = copy_pair(&root);
    let socket = state_root.join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let state_before = snapshot(&state_root);
    let responder = std::thread::spawn(move || {
        let mut stream = connect(&listener, Duration::from_secs(3));
        assert_eq!(request(&mut stream)["op"], "Ping");
        std::io::Write::write_all(
            &mut stream,
            &ping_response(Some(VERSION), Some("capability-readiness-v1")),
        )
        .unwrap();
        let mut readiness_stream = connect(&listener, Duration::from_secs(3));
        let request = request(&mut readiness_stream);
        assert_eq!(request["op"], "CapabilityReadiness", "{request}");
        assert_eq!(
            request["project_context"]["app_scope_id"], "appserver-cli",
            "{request}"
        );
        std::io::Write::write_all(&mut readiness_stream, &readiness_response()).unwrap();
    });
    let output = run_check_with_dsh_session(
        &cli,
        &project_root,
        &state_root,
        false,
        Some("readiness-session"),
    );
    if let Err(error) = responder.join() {
        panic!(
            "controlled listener failed: {error:?}; child output: {}",
            text(&output)
        );
    }
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "success", "{value}");
    assert_eq!(
        value["checks"]["capabilities"]["status"], "ready",
        "{value}"
    );
    assert_eq!(
        value["checks"]["capabilities"]["receive"]["owner"], "gateway",
        "{value}"
    );
    assert_eq!(
        value["checks"]["capabilities"]["transport"]["receive_owner"], "gateway",
        "{value}"
    );
    assert_eq!(
        snapshot(&state_root),
        state_before,
        "status --check mutated host state"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
