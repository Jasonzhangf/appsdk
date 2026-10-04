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
  "notify subscribe")
    count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > subscribe-count
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--subject" ]; then printf '%s' "$2" > goal-subject; fi
      shift
    done
    if [ "$count" = "1" ]; then
      printf '%s\n' 'absolute trigger times must be in the future and before expiry' >&2
      exit 45
    fi
    printf '%s\n' '{"subscription_id":"sub-rearmed","status":"armed"}'
    ;;
  "notify status")
    printf '%s\n' '{"subscriptions":[]}'
    ;;
  "notify unsubscribe")
    printf '%s\n' 'unexpected unsubscribe' > unsubscribe-called
    exit 46
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

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
        .contains("COLLAB_SUBSCRIBE_FAILED"));

    let cancel = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(cancel.status.code(), Some(1));
    let cancel_json: Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert_eq!(cancel_json["desired"], "cancel_pending");
    assert_eq!(cancel_json["observed"], "unknown");
    assert_eq!(cancel_json["remote_state"], "no_matching_armed_subject");
    assert!(cancel_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_CANCEL_SUBJECT_NOT_FOUND"));

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
    assert_eq!(
        rearmed_json["recovery_history"][0]["previous_record"]["desired"],
        "cancel_pending"
    );
    assert_eq!(
        rearmed_json["recovery_history"][0]["previous_record"]["remote_state"],
        "no_matching_armed_subject"
    );
    assert_eq!(
        fs::read_to_string(root.join("subscribe-count")).unwrap(),
        "2"
    );
    assert!(!root.join("unsubscribe-called").exists());

    fs::remove_dir_all(root).unwrap();
}
