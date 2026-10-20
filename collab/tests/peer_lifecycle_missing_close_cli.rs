include!("support/context_operation_fixture.rs");

use std::thread;

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("collab CLI emits JSON")
}

fn create_session(fixture: &Fixture, label: &str) -> Pane {
    let output = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "new-session",
            "-d",
            "-s",
            label,
            "sleep 600",
        ])
        .output()
        .expect("create owned tmux session");
    assert!(
        output.status.success(),
        "create owned tmux session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pane_id = String::from_utf8(
        Command::new("tmux")
            .args([
                "-S",
                fixture.tmux_socket.to_str().unwrap(),
                "display-message",
                "-p",
                "-t",
                label,
                "#{pane_id}",
            ])
            .output()
            .expect("read owned pane id")
            .stdout,
    )
    .expect("pane id is UTF-8")
    .trim()
    .to_owned();
    let server_pid = String::from_utf8(
        Command::new("tmux")
            .args([
                "-S",
                fixture.tmux_socket.to_str().unwrap(),
                "display-message",
                "-p",
                "#{pid}",
            ])
            .output()
            .expect("read owned tmux server pid")
            .stdout,
    )
    .expect("tmux server pid is UTF-8")
    .trim()
    .parse::<u32>()
    .expect("tmux server pid is numeric");
    Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    }
}

#[test]
fn close_retires_a_no_session_tmux_peer_after_its_exact_pane_is_missing() {
    let (mut fixture, master) =
        Fixture::isolated_with_pane_command("stale-peer-close", "sleep 600");

    let master_context = fixture.run_ok(&["context"], Some(&master));
    let master_id = master_context["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("master worker id")
        .to_owned();
    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated stale peer close acceptance",
        ],
        Some(&master),
    );
    assert_eq!(promoted["master"], master_id, "{promoted}");

    let stale = create_session(&fixture, "stale-peer");
    fixture.track_tmux_session("stale-peer");
    let stale_result = fixture.root.join("stale-context.json");
    let stale_error = fixture.root.join("stale-context.err");
    let stale_done = fixture.root.join("stale-context.done");
    let script = format!(
        "set -e\nunset CODEX_SESSION_ID CODEX_THREAD_ID CODEX_THREAD\n{} context --op stale-peer-bootstrap >{} 2>{}\nprintf done >{}\nsleep 600\n",
        shell_quote(&fixture.binary.to_string_lossy()),
        shell_quote(&stale_result.to_string_lossy()),
        shell_quote(&stale_error.to_string_lossy()),
        shell_quote(&stale_done.to_string_lossy())
    );
    fixture.run_in_pane(&stale, "stale-peer-register", &script);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !stale_done.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        stale_done.exists(),
        "stale peer registration failed: {}",
        std::fs::read_to_string(&stale_error).unwrap_or_default()
    );
    let stale_context = parse_json(&std::fs::read(&stale_result).expect("stale context result"));
    assert_eq!(
        stale_context["result"]["outcome"], "completed",
        "{stale_context}"
    );
    let stale_id = stale_context["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("stale worker id")
        .to_owned();
    assert_eq!(
        stale_context["result"]["snapshot"]["identity"]["transport"]["session_id"],
        Value::Null
    );
    assert_eq!(
        stale_context["result"]["snapshot"]["identity"]["transport"]["thread_id"],
        Value::Null
    );

    let killed = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "kill-session",
            "-t",
            "stale-peer",
        ])
        .output()
        .expect("close owned stale session");
    assert!(
        killed.status.success(),
        "close owned stale session failed: {}",
        String::from_utf8_lossy(&killed.stderr)
    );

    let read = fixture
        .configured_command(&["worker", "read", &stale_id], Some(&master))
        .output()
        .expect("run exact worker read");
    let read_json = parse_json(&read.stdout);
    assert_eq!(
        read_json["result"]["target"]["worker_id"], stale_id,
        "read must expose the exact target: {read_json}"
    );

    let close = fixture
        .configured_command(
            &[
                "worker",
                "close",
                &stale_id,
                "--reason",
                "remove stale no-session binding causing tombstone poison",
            ],
            Some(&master),
        )
        .output()
        .expect("run exact worker close");
    let close_json = parse_json(&close.stdout);
    assert!(
        close.status.success() && close_json["ok"] == Value::Bool(true),
        "close failed: {close_json}"
    );
    assert_eq!(close_json["result"]["action"], "close", "{close_json}");
    assert_eq!(close_json["result"]["outcome"], "complete", "{close_json}");
    assert_eq!(close_json["result"]["phase"], "complete", "{close_json}");
    assert_eq!(
        close_json["result"]["target"]["binding_id"],
        stale_context["result"]["snapshot"]["binding"]["binding_id"],
        "{close_json}"
    );
    assert_eq!(
        close_json["result"]["close"]["runtime_archive"]["state"], "missing",
        "{close_json}"
    );
    assert_eq!(
        close_json["result"]["close"]["worker_retirement"]["state"], "verified",
        "{close_json}"
    );
    assert_eq!(
        close_json["result"]["close"]["binding_retirement"]["state"], "verified",
        "{close_json}"
    );
    assert_eq!(
        close_json["result"]["close"]["route_retirement"]["state"], "not-applicable",
        "{close_json}"
    );
    assert_eq!(
        close_json["result"]["close"]["subscription_retirement"]["state"], "verified",
        "{close_json}"
    );

    let after = fixture.run_ok(&["worker", "status"], Some(&master));
    let remaining = after["workers"].as_array().expect("workers array");
    assert!(
        remaining
            .iter()
            .all(|worker| worker["id"].as_str() != Some(stale_id.as_str())),
        "retired worker still listed: {after}"
    );
}
