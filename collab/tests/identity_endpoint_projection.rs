include!("support/tmux_cli_fixture.rs");

fn endpoint_live(value: &Value, worker_id: &str) -> Value {
    value["workers"]
        .as_array()
        .expect("who lists workers")
        .iter()
        .find(|worker| worker["id"].as_str() == Some(worker_id))
        .unwrap_or_else(|| panic!("who must name {worker_id}: {value}"))["endpoint_live"]
        .clone()
}

fn peer_endpoint_live(value: &Value, worker_id: &str) -> Value {
    value["peers"]
        .as_array()
        .expect("context lists peers")
        .iter()
        .find(|peer| peer["worker_id"].as_str() == Some(worker_id))
        .unwrap_or_else(|| panic!("context must name {worker_id}: {value}"))["endpoint_live"]
        .clone()
}

fn assert_not_live_consistent(fixture: &Fixture, pane: &Pane, worker_id: &str, grant_id: &Value) {
    let run = |args: &[&str]| fixture.run_ok(args, Some(pane));
    let status = run(&["master", "status"]);
    assert_eq!(status["master"]["worker_id"], worker_id, "{status}");
    assert_eq!(&status["master"]["grant_id"], grant_id, "{status}");
    let observed = status["master"]["endpoint_live"].clone();
    assert!(observed.is_null() || observed == false, "{status}");

    let context = run(&["context"]);
    let context = context
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(context);
    assert_eq!(context["master"]["worker_id"], worker_id, "{context}");
    assert_eq!(&context["master"]["grant_id"], grant_id, "{context}");
    assert_eq!(context["master"]["endpoint_live"], observed, "{context}");
    assert_eq!(
        peer_endpoint_live(&context, worker_id),
        observed,
        "{context}"
    );
    let master_peer = context["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|peer| peer["worker_id"].as_str() == Some(worker_id))
        .unwrap();
    assert_eq!(master_peer["role"], "master", "{context}");
    assert_eq!(master_peer["presence"], "present", "{context}");

    let workers = run(&["who"]);
    assert_eq!(endpoint_live(&workers, worker_id), observed, "{workers}");
    let master_row = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"].as_str() == Some(worker_id))
        .unwrap();
    assert_eq!(master_row["identity_valid"], true, "{workers}");
    assert_eq!(master_row["role"], "master", "{workers}");

    let board = run(&["board", "show"]);
    assert_eq!(board["master"]["worker_id"], worker_id, "{board}");
    assert_eq!(&board["master"]["grant_id"], grant_id, "{board}");
    assert_eq!(board["master"]["endpoint_live"], observed, "{board}");
    let member = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"].as_str() == Some(worker_id))
        .unwrap();
    assert_eq!(member["role"], "master", "{board}");
    assert_eq!(
        member["status"],
        if observed.is_null() {
            "unknown"
        } else {
            "offline"
        },
        "{board}"
    );
}

/// F3 positive: a live pane is the only case that reports `endpoint_live`
/// true, and extra pane output never changes the durable identity or role.
#[test]
fn live_pane_reports_endpoint_live_true_and_output_keeps_identity() {
    let (fixture, pane) = single_pane_fixture("collab-endpoint-live");
    let registered = fixture.registered_context(&pane, true);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("registration names the worker")
        .to_owned();
    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated endpoint projection",
        ],
        Some(&pane),
    );
    assert_eq!(promoted["master"], worker_id, "{promoted}");

    wait_for_agent_observations(&registered);

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert_eq!(status["master"]["worker_id"], worker_id, "{status}");
    assert_eq!(status["master"]["endpoint_live"], true, "{status}");
    let context = fixture.run_context(&["context"], Some(&pane));
    assert_eq!(context["identity"]["role"], "master", "{context}");
    assert_eq!(context["master"]["endpoint_live"], true, "{context}");
    assert_eq!(context["liveness"]["live"], true, "{context}");
    // The pane runner is not a confirmed agent. Endpoint reachability must
    // not fabricate agent presence, activity, or turn state.
    assert_eq!(context["liveness"]["presence"], "unknown", "{context}");
    assert_eq!(
        context["agent"]["presence_observation"]["reason"], "unsupported_foreground_process",
        "{context}"
    );
    assert_eq!(context["agent"]["thread_state"], "unknown", "{context}");
    assert_eq!(context["agent"]["activity"], "unknown", "{context}");
    assert!(
        context["agent"]["latest_turn_status"].is_null(),
        "{context}"
    );
    assert!(
        context["agent"]["can_accept_direct_input"].is_null(),
        "{context}"
    );
    let workers = fixture.run_ok(&["who"], Some(&pane));
    assert_eq!(endpoint_live(&workers, &worker_id), true, "{workers}");
    let board = fixture.run_ok(&["board", "show"], Some(&pane));
    assert_eq!(board["master"]["endpoint_live"], true, "{board}");
    let member = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"].as_str() == Some(worker_id.as_str()))
        .unwrap();
    assert_eq!(member["status"], "unknown", "{board}");
    assert_eq!(member["role"], "master", "{board}");

    // Extra pane output is activity telemetry, not identity.
    tmux(
        &fixture.tmux_socket,
        &["send-keys", "-t", &pane.pane_id, "echo still-here", "Enter"],
    );
    let after = fixture.run_context(&["context"], Some(&pane));
    assert_eq!(after["identity"]["worker_id"], worker_id, "{after}");
    assert_eq!(after["identity"]["role"], "master", "{after}");
    assert_eq!(after["master"]["endpoint_live"], true, "{after}");
}

/// F3 negative: when a registered master's own pane disappears, or its tmux
/// socket is gone, identity, role and grant stay durable and every projection
/// agrees on `false` or `null` (probe undecided), never `true`.
#[test]
fn missing_pane_and_lost_socket_report_not_live() {
    let label = "collab-endpoint-missing";
    let (mut fixture, master_pane) = single_pane_fixture(label);
    let master = fixture.registered_context(&master_pane, true);
    let master_id = master["identity"]["worker_id"]
        .as_str()
        .expect("master registration names the worker")
        .to_owned();
    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated endpoint projection",
        ],
        Some(&master_pane),
    );
    assert_eq!(promoted["master"], master_id, "{promoted}");
    let before = fixture.run_ok(&["master", "status"], Some(&master_pane));
    let grant_id = before["master"]["grant_id"].clone();

    // A second live pane keeps the tmux server alive after the master pane is
    // removed, and lets `context` authenticate from a live caller.
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            &format!("{label}-peer"),
            "sleep 600",
        ],
    );
    let peer_pane_id = String::from_utf8(created.stdout)
        .expect("peer pane id is utf8")
        .trim()
        .to_owned();
    let peer_pane = Pane {
        server_pid: master_pane.server_pid,
        pane_id: peer_pane_id.clone(),
        session_anchor: format!("session-{peer_pane_id}"),
        thread_anchor: format!("thread-{peer_pane_id}"),
    };
    fixture.registered_context(&peer_pane, true);

    // Remove the master pane but keep the server alive. Tmux can return an
    // error for a missing target, so false and unknown are both truthful.
    tmux(
        &fixture.tmux_socket,
        &["kill-pane", "-t", &master_pane.pane_id],
    );

    assert_not_live_consistent(&fixture, &peer_pane, &master_id, &grant_id);

    // Closing the last test-owned session removes the socket, so the probe can
    // no longer decide and the endpoint must report `null`, never `true`.
    tmux(
        &fixture.tmux_socket,
        &["kill-session", "-t", &format!("{label}-peer")],
    );
    // Observe the lost endpoint from a real independent caller, rather than
    // impersonating a process in the session that was just closed.
    let lost_socket = fixture.tmux_socket.clone();
    let observer_socket = fixture.root.join("observer.sock");
    let observer_session = format!("{label}-observer");
    let observer_seed = format!("{label}-observer-seed");
    tmux(
        &observer_socket,
        &["new-session", "-d", "-s", &observer_seed, "sleep 600"],
    );
    tmux(
        &observer_socket,
        &["new-session", "-d", "-s", &observer_session, "sleep 600"],
    );
    tmux(&observer_socket, &["kill-session", "-t", &observer_seed]);
    fixture.owned_sessions.push(observer_session);
    std::fs::write(
        fixture.host_state.join("tmux-endpoints.json"),
        serde_json::to_vec(&json!({
            "endpoints": [lost_socket.to_string_lossy(), observer_socket.to_string_lossy()]
        }))
        .unwrap(),
    )
    .unwrap();
    fixture.tmux_socket = observer_socket;
    let observer = Pane {
        server_pid: String::from_utf8(
            tmux(&fixture.tmux_socket, &["display-message", "-p", "#{pid}"]).stdout,
        )
        .unwrap()
        .trim()
        .parse()
        .unwrap(),
        pane_id: String::from_utf8(
            tmux(
                &fixture.tmux_socket,
                &["display-message", "-p", "#{pane_id}"],
            )
            .stdout,
        )
        .unwrap()
        .trim()
        .to_owned(),
        session_anchor: "observer-session".into(),
        thread_anchor: "observer-thread".into(),
    };
    fixture.registered_context(&observer, true);
    assert_not_live_consistent(&fixture, &observer, &master_id, &grant_id);

    // An actual non-socket endpoint produces a connection/probe error. No
    // mocked presence, transport substitution, or journal mutation is used.
    // Tmux may leave its unserved socket node after the official shutdown.
    match std::fs::remove_file(&lost_socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("remove owned stopped tmux socket: {error}"),
    }
    std::fs::write(&lost_socket, b"not a tmux socket").unwrap();
    assert_not_live_consistent(&fixture, &observer, &master_id, &grant_id);
}
