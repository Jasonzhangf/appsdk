#[test]
fn authorized_subscription_and_goal_admission_survive_missing_transport() {
    let (mut fixture, host, identity) = lifecycle_native_fixture("offline-subscription-crud");
    let worker = identity["worker_id"].as_str().unwrap();
    fixture.stop_daemon();
    let native_starts = host.start_request_count();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let slot = map["slots"].as_array_mut().unwrap().iter_mut().find(|slot| {
        slot["identity_status"] == "active" && slot["current"]["binding"]["agent_id"] == worker
    }).unwrap();
    let owner = slot["identity_key"]["name"].as_str().unwrap().to_owned();
    slot["current"].as_object_mut().unwrap().remove("transport");
    fs::write(&map_path, serde_json::to_vec_pretty(&map).unwrap()).unwrap();
    fixture.restart_after_external_stop();

    let run = |args: &[&str]| -> Value {
        let output = fixture.configured_command(args, None)
            .env("CODEX_SESSION_ID", "lifecycle-session")
            .env("CODEX_THREAD_ID", "lifecycle-thread")
            .output().unwrap();
        assert!(output.status.success(), "collab {args:?} failed: {}", text(&output));
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let subscription = run(&[
        "notify", "subscribe", "--event", "deadline", "--subject", "offline-subscription",
        "--every-ms", "1000", "--repeat-count", "2", "--ttl-seconds", "600",
    ]);
    let created = run(&[
        "goal", "reconcile", "--goal-id", "offline-goal", "--owner", &owner, "--desired", "active",
        "--content", "Continue the offline goal", "--every-ms", "120000", "--repeat", "2",
        "--ttl-seconds", "600",
    ]);
    let subscription_id = subscription["subscription"]["id"].as_str().unwrap();
    assert_eq!(subscription["subscription"]["status"], "armed");
    assert_eq!(subscription["subscription"]["method"], "");
    assert_eq!(subscription["subscription"]["target"], "");
    let goal = &created["subscription"];
    let goal_id = goal["id"].as_str().unwrap();
    assert_eq!(goal["method"], "");
    assert_eq!(goal["target"], "");

    let denied = lifecycle_wire(&fixture, &identity, json!({
        "op":"GoalScheduleReconcile","worker_id":worker,"token":"wrong-token",
        "goal_id":"offline-goal","goal_owner":owner,"desired":"ended"
    }));
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(fixture.current_state()["notification_subscriptions"][goal_id], *goal);

    let edited = run(&[
        "goal", "reconcile", "--goal-id", "offline-goal", "--owner", &owner, "--desired", "active",
        "--operation", "edit", "--content", "Updated content while offline",
    ]);
    for field in ["id","trigger_ms","expires_ms","repeat_count","fired_count"] {
        assert_eq!(edited["subscription"][field], goal[field], "{edited}");
    }
    let ended = run(&[
        "goal", "reconcile", "--goal-id", "offline-goal", "--owner", &owner, "--desired", "ended",
    ]);
    assert_eq!(ended["status"], "ended", "{ended}");
    let retired_goal = run(&["notify", "status"]);
    assert!(retired_goal["subscriptions"].as_array().unwrap().iter().any(|entry| {
        entry["id"] == goal_id && entry["status"] == "cancelled"
    }));
    let pending_deadline = |state: &Value| -> Option<String> {
        state["wake_bindings"].as_object().unwrap().iter()
            .find(|(_, bound)| *bound == subscription_id)
            .map(|(id, _)| id.clone())
    };
    assert!(wait_until(Duration::from_secs(15), || {
        pending_deadline(&fixture.current_state()).is_some()
    }), "the offline subscription should produce a durable deadline");
    let state = fixture.current_state();
    let message_id = pending_deadline(&state).unwrap();
    assert_eq!(state["msgs"][&message_id]["state"], "pending");
    assert_eq!(state["msgs"][&message_id]["wake_attempt_count"], 0);
    assert!(state["notification_delivery_accepted"][&message_id].is_null());
    assert_eq!(state["notification_subscriptions"][goal_id]["status"], "cancelled");
    assert_eq!(
        state["notification_subscriptions"][subscription_id],
        subscription["subscription"]
    );
    assert_eq!(host.start_request_count(), native_starts);

    let restored = fixture.context_provide(
        "offline-subscription-crud-restore",
        &json!({
            "session_id":"lifecycle-session","thread_id":"lifecycle-thread",
            "endpoint":host.endpoint(),"namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    assert_eq!(restored["result"]["snapshot"]["identity"]["worker_id"], worker);
    let restored_state = fixture.current_state();
    let retargeted = &restored_state["notification_subscriptions"][subscription_id];
    assert_eq!(retargeted["method"], "appserver");
    assert_eq!(retargeted["target"], "lifecycle-thread");
    for field in ["id","trigger_ms","expires_ms","repeat_count","fired_count","status"] {
        assert_eq!(retargeted[field], subscription["subscription"][field], "{retargeted}");
    }
    assert_eq!(restored_state["notification_subscriptions"][goal_id]["status"], "cancelled");
    let received = run(&["recv", "--timeout", "2"]);
    assert!(received["messages"].as_array().unwrap().iter().any(|message| {
        message["id"] == message_id && message["subject"] == "deadline:offline-subscription"
    }), "{received}");
}

fn unmapped_worker_row(response: &Value, worker_id: &str) -> Value {
    response["workers"]
        .as_array()
        .unwrap_or_else(|| panic!("worker response has an array: {response}"))
        .iter()
        .find(|worker| worker["id"] == worker_id)
        .cloned()
        .unwrap_or_else(|| panic!("worker {worker_id} is missing from {response}"))
}

fn assert_unmapped_worker_read(row: &Value) {
    assert_eq!(row["display_name"], "", "{row}");
    assert_eq!(row["work_roles"], json!([]), "{row}");
    assert!(row["role"].is_null(), "{row}");
    assert!(row["role_brief"].is_null(), "{row}");
    assert!(row["transport"].is_null(), "{row}");
    assert_eq!(row["status"], "identity-mismatch", "{row}");
    assert_eq!(row["presence"], "unknown", "{row}");
    assert!(row["endpoint_live"].is_null(), "{row}");
    assert_eq!(row["identity_valid"], false, "{row}");
    assert_eq!(row["agent_state"], "unknown", "{row}");
    assert!(row["route_match"].is_null(), "{row}");
    assert!(row["transport_view"].is_null(), "{row}");
    assert!(row["active_task"].is_null(), "{row}");
    assert!(row["active_status"].is_null(), "{row}");
    assert!(
        row["diagnostic"]
            .as_str()
            .is_some_and(|diagnostic| diagnostic.contains("without a current identity mapping")),
        "{row}"
    );
}

fn read_effect_snapshot(fixture: &Fixture) -> Value {
    let state = fixture.current_state();
    json!({
        "tasks": state["tasks"],
        "msgs": state["msgs"],
        "receive_receipts": state["receive_receipts"],
        "notification_subscriptions": state["notification_subscriptions"],
        "wake_bindings": state["wake_bindings"],
        "master_wake": state["master_wake"],
    })
}

#[test]
fn unmapped_worker_status_reads_preserve_retained_records_without_side_effects() {
    let mut fixture = Fixture::without_tmux("unmapped-worker-status-read");
    defer_background_timer(&mut fixture);
    let (fixture, old_host, next_host, old, next) =
        seed_lifecycle_native_pair(fixture, "ack_only");
    let old_worker = old["worker_id"].as_str().unwrap().to_owned();
    let next_worker = next["worker_id"].as_str().unwrap().to_owned();
    let child_socket = next_host
        .endpoint()
        .strip_prefix("unix://")
        .expect("test AppServer endpoint uses unix://")
        .to_owned();

    let takeover = fixture
        .configured_command(
            &[
                "context",
                "--approve-grant",
                r#"{"action":"restore_master","authorization":"fixture successor replaces Jenny","replace_confirmed":true}"#,
            ],
            None,
        )
        .env("CODEX_APP_SERVER_SOCKET", child_socket)
        .env("CODEX_SESSION_ID", "lifecycle-child-session")
        .env("CODEX_THREAD_ID", "lifecycle-child-thread")
        .output()
        .unwrap();
    assert!(
        takeover.status.success(),
        "R6 takeover failed: stdout={} stderr={}",
        String::from_utf8_lossy(&takeover.stdout),
        String::from_utf8_lossy(&takeover.stderr)
    );
    let takeover: Value = serde_json::from_slice(&takeover.stdout).unwrap();
    assert_eq!(takeover["result"]["snapshot"]["name"], "Jenny", "{takeover}");
    assert_eq!(
        takeover["result"]["snapshot"]["identity"]["worker_id"],
        next_worker.as_str(),
        "{takeover}"
    );

    let task = lifecycle_wire(&fixture, &next, json!({
        "op":"TaskRegister", "worker_id":next_worker, "token":next["token"],
        "task_id":"unmapped-retained-work", "owner":null, "feature_id":null,
        "worktree_path":null, "branch":null, "base_commit":null,
        "next_step":"retain this work during status reads"
    }));
    assert_eq!(task["ok"], true, "{task}");
    let subscription = lifecycle_wire(&fixture, &next, json!({
        "op":"NotificationSubscribe", "worker_id":next_worker, "token":next["token"],
        "event":"deadline", "subject":"unmapped-retained-subscription",
        "trigger_ms":null, "interval_ms":600_000, "repeat_count":3, "ttl_seconds":3600
    }));
    assert_eq!(subscription["ok"], true, "{subscription}");

    let map_path = fixture.host_state.join("identity-map.json");
    let map_before = fs::read(&map_path).unwrap();
    let map: Value = serde_json::from_slice(&map_before).unwrap();
    assert!(
        map["slots"].as_array().unwrap().iter().all(|slot| {
            !(slot["identity_status"] == "active"
                && slot["current"]["binding"]["agent_id"] == old_worker)
        }),
        "R6 must leave the displaced worker without a current mapping: {map}"
    );
    let effects_before = read_effect_snapshot(&fixture);

    // Read admission must not authorize the displaced holder's active calls.
    for request in [
        json!({"op":"Context", "worker_id":old_worker, "token":old["token"]}),
        json!({"op":"TaskRegister", "worker_id":old_worker, "token":old["token"],
            "task_id":"unmapped-mutation-must-not-exist", "owner":null,
            "feature_id":null, "worktree_path":null, "branch":null,
            "base_commit":null, "next_step":null}),
    ] {
        let denied = lifecycle_wire(&fixture, &old, request);
        assert_eq!(denied["ok"], false, "{denied}");
        assert!(!denied["error"].as_str().unwrap().contains("decode"), "{denied}");
    }

    let workers = lifecycle_wire(&fixture, &next, json!({"op":"Workers"}));
    let unmapped_status = lifecycle_wire(
        &fixture, &next, json!({"op":"WorkerStatus", "worker_id":old_worker}),
    );
    let current_status = lifecycle_wire(
        &fixture, &next, json!({"op":"WorkerStatus", "worker_id":next_worker}),
    );
    let outcomes: Vec<_> = [&workers, &unmapped_status, &current_status].iter()
        .map(|response| json!({"ok":response["ok"], "error":response["error"]})).collect();
    assert!(outcomes.iter().all(|outcome| outcome["ok"] == true),
        "Workers / selected old / selected current outcomes: {outcomes:?}");
    assert_eq!(workers["ok"], true, "{workers}");
    let old_row = unmapped_worker_row(&workers, &old_worker);
    assert_unmapped_worker_read(&old_row);
    let current_row = unmapped_worker_row(&workers, &next_worker);
    assert_eq!(current_row["display_name"], "Jenny", "{current_row}");
    assert_eq!(current_row["identity_valid"], true, "{current_row}");
    assert_eq!(current_row["role_brief"]["role"], "master", "{current_row}");

    let statuses = lifecycle_wire(&fixture, &next, json!({"op":"WorkerStatus"}));
    assert_eq!(statuses["ok"], true, "{statuses}");
    assert_unmapped_worker_read(&unmapped_worker_row(&statuses, &old_worker));

    assert_eq!(unmapped_status["ok"], true, "{unmapped_status}");
    assert_eq!(unmapped_status["count"], 1, "{unmapped_status}");
    assert_unmapped_worker_read(&unmapped_worker_row(&unmapped_status, &old_worker));

    assert_eq!(current_status["ok"], true, "{current_status}");
    assert_eq!(current_status["count"], 1, "{current_status}");
    let current_status = unmapped_worker_row(&current_status, &next_worker);
    assert_eq!(current_status["display_name"], "Jenny", "{current_status}");
    assert_eq!(
        current_status["role_brief"]["role"], "master",
        "{current_status}"
    );

    let status_all = lifecycle_wire(&fixture, &next, json!({"op":"StatusAll"}));
    assert_eq!(status_all["ok"], true, "{status_all}");
    assert_unmapped_worker_read(&unmapped_worker_row(&status_all, &old_worker));
    assert_eq!(
        unmapped_worker_row(&status_all, &next_worker)["role_brief"]["role"],
        "master",
        "{status_all}"
    );

    assert_eq!(
        fs::read(&map_path).unwrap(),
        map_before,
        "public worker-status reads must not rewrite the identity map"
    );
    assert_eq!(
        read_effect_snapshot(&fixture),
        effects_before,
        "public worker-status reads must not change tasks, mailbox, wake or subscription state"
    );

    let returned = fixture.context_provide(
        "unmapped-worker-status-read-old-return",
        &json!({
            "session_id":"lifecycle-master-session",
            "thread_id":"lifecycle-master-thread",
            "endpoint":old_host.endpoint(),
            "namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
    let returned_name = returned["result"]["snapshot"]["name"].as_str().unwrap();
    let returned_worker = returned["result"]["snapshot"]["identity"]["worker_id"]
        .as_str().unwrap();
    assert_ne!(
        returned_name, "Jenny",
        "ordinary return must not reclaim the replacement name: {returned}"
    );
    let returned_workers = lifecycle_wire(&fixture, &next, json!({"op":"Workers"}));
    assert_eq!(returned_workers["ok"], true, "{returned_workers}");
    assert_eq!(
        unmapped_worker_row(&returned_workers, &next_worker)["role_brief"]["role"],
        "master",
        "{returned_workers}"
    );
    assert_eq!(
        unmapped_worker_row(&returned_workers, returned_worker)["display_name"],
        returned_name,
        "{returned_workers}"
    );
    if returned_worker != old_worker {
        assert_unmapped_worker_read(&unmapped_worker_row(&returned_workers, &old_worker));
    }
}

#[test]
fn unmapped_worker_status_read_reports_identity_map_parse_errors() {
    let (fixture, _old_host, _next_host, _old, next) =
        lifecycle_native_pair_fixture("unmapped-worker-status-map-error", "ack_only");
    let map_path = fixture.host_state.join("identity-map.json");
    fs::write(&map_path, b"{not-valid-json").unwrap();

    for request in [
        json!({"op":"Workers"}),
        json!({"op":"WorkerStatus"}),
        json!({"op":"WorkerStatus", "worker_id":next["worker_id"]}),
        json!({"op":"StatusAll"}),
    ] {
        let response = lifecycle_wire(&fixture, &next, request);
        assert_eq!(response["ok"], false, "{response}");
        assert!(response["error"].as_str().unwrap().contains("IDENTITY_MAP_DECODE_FAILED"),
            "map parse failure must stay visible: {response}");
    }
}

#[test]
fn unmapped_worker_status_read_reports_current_ambiguity() {
    let (fixture, _old_host, _next_host, _old, next) =
        lifecycle_native_pair_fixture("unmapped-worker-status-ambiguous", "ack_only");
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let mut duplicate = map["slots"].as_array().unwrap().iter()
        .find(|slot| slot["current"]["binding"]["agent_id"] == next["worker_id"])
        .unwrap().clone();
    duplicate["identity_key"]["name"] = json!("peer-999");
    map["slots"].as_array_mut().unwrap().push(duplicate);
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let before = fs::read(&map_path).unwrap();
    for request in [
        json!({"op":"Workers"}),
        json!({"op":"WorkerStatus"}),
        json!({"op":"WorkerStatus", "worker_id":next["worker_id"]}),
        json!({"op":"StatusAll"}),
    ] {
        let response = lifecycle_wire(&fixture, &next, request);
        assert_eq!(response["ok"], false, "{response}");
        assert!(response["error"].as_str().unwrap().contains("IDENTITY_MAP_AMBIGUOUS"),
            "current ambiguity must stay visible: {response}");
    }
    assert_eq!(fs::read(map_path).unwrap(), before);
}
