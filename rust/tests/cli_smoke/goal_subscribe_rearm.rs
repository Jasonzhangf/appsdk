#[test]
fn goal_subscribe_rearms_after_failed_subscribe_and_absent_remote_cancel() {
    let root = temp_root("goal-failed-subscribe-rearm");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}'
    ;;
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    if [ "$desired" != "active" ]; then
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$owner\",\"desired\":\"$desired\",\"status\":\"ended\",\"cancelled\":true}"
      exit 0
    fi
    count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > subscribe-count
    if [ "$count" = "1" ]; then
      printf '%s\n' 'absolute trigger times must be in the future and before expiry' >&2
      exit 45
    fi
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$owner\",\"desired\":\"$desired\",\"status\":\"armed\",\"subscription\":{\"id\":\"sub-rearmed\",\"status\":\"armed\"}}"
    ;;
  "notify status")
    printf '%s\n' '{"subscriptions":[]}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    install_goal_test_context(&fake_collab, &root);

    let failed_subscribe = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "2m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(failed_subscribe.status.code(), Some(1));
    let failed_json: Value = serde_json::from_slice(&failed_subscribe.stdout).unwrap();
    assert_eq!(failed_json["active"], false);
    assert_eq!(failed_json["desired"], "recovery_required");
    assert!(failed_json["subscription_id"].is_null());
    assert!(failed_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_RECONCILE_ACTIVE_FAILED"));

    let cancel = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancel_json: Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert_eq!(cancel_json["status"], "cancelled");
    assert_eq!(cancel_json["record"]["desired"], "unsubscribed");
    assert_eq!(cancel_json["record"]["observed"], "cancelled");

    let rearmed = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        rearmed.status.success(),
        "{}",
        String::from_utf8_lossy(&rearmed.stderr)
    );
    let rearmed_json: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
    assert_eq!(rearmed_json["active"], true);
    assert_eq!(rearmed_json["desired"], "subscribed");
    assert_eq!(rearmed_json["observed"], "subscribed");
    assert_eq!(rearmed_json["interval"], "5m");
    assert_eq!(rearmed_json["subscription_id"], "sub-rearmed");
    assert!(rearmed_json.get("recovery_history").is_none(), "{rearmed_json}");
    assert_eq!(
        fs::read_to_string(root.join("subscribe-count")).unwrap(),
        "2"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_rearm_keeps_only_latest_nonrecursive_recovery_record() {
    let root = temp_root("goal-nonrecursive-rearm-history");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}'
    ;;
  "goal reconcile")
    count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > subscribe-count
    printf '%s' "sub-$count" > current-sub
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$owner\",\"desired\":\"$desired\",\"status\":\"armed\",\"subscription\":{\"id\":\"sub-$count\",\"status\":\"armed\"}}"
    ;;
  "notify status")
    id=`/bin/cat current-sub 2>/dev/null || printf 'none'`
    subject=`/bin/cat goal-subject 2>/dev/null || printf 'goal:unknown'`
    printf '%s\n' "{\"subscriptions\":[{\"id\":\"$id\",\"status\":\"consumed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    install_goal_test_context(&fake_collab, &root);

    let initial = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial_json: Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_eq!(initial_json["active"], true);
    assert_eq!(initial_json["subscription_id"], "sub-1");

    for round in 2..=7 {
        let expected_previous = format!("sub-{}", round - 1);
        let expected_current = format!("sub-{}", round);
        let rearmed = Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert!(
            rearmed.status.success(),
            "round {}: {}",
            round,
            String::from_utf8_lossy(&rearmed.stderr)
        );
        let rearmed_json: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
        assert_eq!(rearmed_json["active"], true);
        assert_eq!(rearmed_json["desired"], "subscribed");
        assert_eq!(rearmed_json["observed"], "subscribed");
        assert_eq!(rearmed_json["subscription_id"], expected_current);

        let persisted: Value =
            serde_json::from_slice(&fs::read(goal_test_record_path(&root)).unwrap()).unwrap();
        let history = persisted["recovery_history"].as_array().unwrap();
        assert_eq!(
            history.len(),
            1,
            "round {} persisted recovery_history: {}",
            round,
            persisted["recovery_history"]
        );
        let previous = &history[0]["previous_record"];
        assert!(
            previous.get("recovery_history").is_none(),
            "round {} embedded previous_record: {}",
            round,
            previous
        );
        assert_eq!(
            previous["subscription_id"],
            Value::String(expected_previous.clone())
        );
        assert_eq!(previous["remote_state"], "consumed");
        assert_eq!(previous["observed"], "consumed");
        assert_eq!(previous["active"], false);
        assert_eq!(
            previous["collab_subscription"]["id"],
            Value::String(expected_previous)
        );
    }

    assert_eq!(
        fs::read_to_string(root.join("subscribe-count")).unwrap(),
        "7"
    );
    fs::remove_dir_all(root).unwrap();
}
