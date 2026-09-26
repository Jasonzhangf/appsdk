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

#[test]
fn goal_cancel_failure_keeps_exact_subscription_record() {
    let root = temp_root("goal-cancel-failure");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' '{"subscription":{"id":"sub-exact","status":"armed"}}'
    ;;
  "notify status")
    subject="${STATUS_SUBJECT:-goal:long-task.md}"
    printf '%s\n' "{\"subscriptions\":[{\"id\":\"sub-exact\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}'
    ;;
  "notify unsubscribe")
    printf '%s\n' 'unsubscribe failed' >&2
    exit 44
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let subscribed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(subscribed.status.success());
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
    assert_eq!(cancel_json["subscription_id"], "sub-exact");
    assert!(root.join(".appsdk-control/long-task-goal.json").is_file());

    let resubscribe = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(resubscribe.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&resubscribe.stderr)
        .contains("GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED"));
    let resubscribe_json: Value = serde_json::from_slice(&resubscribe.stdout).unwrap();
    assert_eq!(resubscribe_json["desired"], "cancel_pending");

    let drifted_resubscribe = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("STATUS_SUBJECT", "goal:drifted-subject")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(drifted_resubscribe.status.code(), Some(1));
    let drifted_stderr = String::from_utf8_lossy(&drifted_resubscribe.stderr);
    assert!(
        drifted_stderr.contains("GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED"),
        "{}",
        drifted_stderr
    );
    assert!(
        drifted_stderr.contains("sub-exact remains armed under subject goal:drifted-subject"),
        "{}",
        drifted_stderr
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_lifecycle_reconciles_legacy_subject_and_exposes_one_shot_recovery() {
    let root = temp_root("goal-legacy-periodic-recovery");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"sub-periodic"}' ;;
  "notify status")
    if [ "${STATUS_EXPIRED:-}" = "1" ]; then
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"expired","event":"deadline","subject":"goal:sha256:legacy"}]}'
    elif [ "${DEDUPE_SUBJECT:-}" = "1" ]; then
      count=$((`/bin/cat status-count 2>/dev/null || printf '0'` + 1))
      printf '%s' "$count" > status-count
      if [ "$count" = "1" ]; then
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:legacy-retained"}]}'
      elif [ "$count" = "2" ]; then
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:long-task.md"}]}'
      else
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:sha256:current"}]}'
      fi
    elif [ "${LEGACY_SUBJECT:-}" = "1" ]; then
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:long-task.md"}]}'
    else
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:sha256:current"}]}'
    fi
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let initial = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
        ])
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
    let initial_text = String::from_utf8_lossy(&initial.stdout);
    assert!(initial_text.contains(
        "One-shot deadline: first trigger after 5m (local rearm intent: every 5m, up to 100 deliveries; TTL 604800 seconds)"
    ));
    assert!(initial_text.contains("one-shot armed; rearm is explicit and verifiable"));

    let record_path = root.join(".appsdk-control/long-task-goal.json");
    let mut legacy: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    legacy["subject"] = Value::String("goal:long-task.md".into());
    legacy["subscription_id"] = Value::Null;
    legacy["collab_subscription"] = Value::Null;
    fs::write(&record_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let reconciled = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("LEGACY_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        reconciled.status.success(),
        "{}",
        String::from_utf8_lossy(&reconciled.stderr)
    );
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).unwrap();
    assert_eq!(reconciled["subject"], "goal:long-task.md");
    assert_eq!(reconciled["subscription_id"], "sub-periodic");

    let mut dedupe_record: Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    dedupe_record["subject"] = Value::String("goal:legacy-retained".into());
    dedupe_record["subscription_id"] = Value::Null;
    dedupe_record["collab_subscription"] = Value::Null;
    fs::write(
        &record_path,
        serde_json::to_vec_pretty(&dedupe_record).unwrap(),
    )
    .unwrap();
    let dedupe = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("DEDUPE_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        dedupe.status.success(),
        "{}",
        String::from_utf8_lossy(&dedupe.stderr)
    );
    let dedupe: Value = serde_json::from_slice(&dedupe.stdout).unwrap();
    assert_eq!(dedupe["subscription_id"], "sub-periodic");
    assert_eq!(dedupe["subject"], "goal:legacy-retained");
    let status_calls: u32 = fs::read_to_string(root.join("status-count"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(status_calls >= 3, "subject candidates were not all queried");

    let expired = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("STATUS_EXPIRED", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        expired.status.success(),
        "{}",
        String::from_utf8_lossy(&expired.stderr)
    );
    let expired: Value = serde_json::from_slice(&expired.stdout).unwrap();
    assert_eq!(expired["desired"], "recovery_required");
    assert_eq!(expired["observed"], "expired");
    assert!(expired["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:expired"));
    assert!(
        expired["record"]["recovery"]
            .as_str()
            .unwrap()
            .contains("appsdk goal subscribe"),
        "recovery={:?}",
        expired["record"]["recovery"]
    );

    let rearmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
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
    let rearmed: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
    assert_eq!(rearmed["desired"], "subscribed");
    assert!(rearmed["recovery_history"].is_array());

    for (flag, expected) in [
        ("--repeat", "GOAL_REPEAT_COUNT_INVALID: 'oops'"),
        ("--ttl-seconds", "GOAL_TTL_INVALID: 'oops'"),
        ("--ttl-seconds", "GOAL_TTL_INVALID: '0'"),
    ] {
        let value = if expected.contains("'0'") {
            "0"
        } else {
            "oops"
        };
        let invalid = Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", flag, value])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&invalid.stderr).contains(expected));
    }

    let overflow = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "18446744073709551615s",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(overflow.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&overflow.stderr)
        .contains("GOAL_DURATION_OVERFLOW:18446744073709551615s"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_rearms_consumed_subscription_without_cancel() {
    let root = temp_root("goal-consumed-rearm");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify subscribe")
    count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > subscribe-count
    if [ "$count" = "1" ]; then
      printf '%s\n' '{"subscription_id":"sub-consumed","status":"armed"}'
    else
      printf '%s\n' '{"subscription_id":"sub-rearmed","status":"armed"}'
    fi
    ;;
  "notify status")
    printf '%s\n' '{"subscriptions":[{"id":"sub-consumed","status":"consumed","event":"deadline","subject":"goal:sha256:consumed-goal"}]}'
    ;;
  "notify unsubscribe")
    printf '%s\n' 'unexpected cancel' > cancel-called
    printf '%s\n' '{"subscription_id":"sub-consumed","status":"cancelled"}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

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
    let goal_id = initial_json["goal_id"].as_str().unwrap().to_string();
    assert_eq!(initial_json["subscription_id"], "sub-consumed");
    assert_eq!(initial_json["subject"], format!("goal:{}", goal_id));

    let rearmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
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
    assert_eq!(rearmed_json["subscription_id"], "sub-rearmed");
    assert_eq!(rearmed_json["subject"], format!("goal:{}", goal_id));
    let previous = &rearmed_json["recovery_history"][0]["previous_record"];
    assert_eq!(previous["subscription_id"], "sub-consumed");
    assert_eq!(previous["remote_state"], "consumed");
    assert_eq!(previous["observed"], "consumed");
    assert_eq!(previous["active"], false);
    assert_eq!(previous["collab_subscribed"], false);
    assert_eq!(previous["collab_subscription"]["id"], "sub-consumed");
    let persisted: Value = serde_json::from_slice(
        &fs::read(root.join(".appsdk-control/long-task-goal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["subscription_id"], "sub-rearmed");
    assert_eq!(
        persisted["recovery_history"][0]["previous_record"]["subscription_id"],
        "sub-consumed"
    );
    assert!(!root.join("cancel-called").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_status_reconciles_armed_subscription_after_recovery_required() {
    let root = temp_root("goal-status-recovery-reconciliation");
    fs::create_dir_all(root.join(".appsdk-control")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify status")
    printf '%s\n' '{"subscriptions":[{"id":"sub-recovered","worker_id":"master-peer","event":"deadline","subject":"goal:retained-subject","status":"armed"}]}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join(".appsdk-control/long-task-goal.json"),
        serde_json::json!({
            "schema_version": 1,
            "goal_id": "sha256:current-goal",
            "goal_path": "long-task.md",
            "subject": "goal:retained-subject",
            "desired": "recovery_required",
            "observed": "unknown",
            "active": false,
            "collab_subscribed": true,
            "collab_subscription": null,
            "subscription_id": null,
            "remote_state": "unknown",
            "error": "GOAL_STATUS_SUBSCRIPTION_ID_MISSING",
            "revision": 3
        })
        .to_string(),
    )
    .unwrap();

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], true);
    assert_eq!(status_json["desired"], "subscribed");
    assert_eq!(status_json["observed"], "subscribed");
    assert_eq!(status_json["subscription_id"], "sub-recovered");
    assert_eq!(status_json["record"]["subject"], "goal:retained-subject");
    assert!(status_json["record"]["error"].is_null());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_persistence_failure_retains_reconciliation_state() {
    let root = temp_root("goal-persistence-failure");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let marker = root.join("remote-called");
    let fake_touch = fake_bin.join("touch");
    fs::write(
        &fake_touch,
        "#!/bin/sh\n/bin/chmod a-w .appsdk-control\n/usr/bin/touch \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_touch, fs::Permissions::from_mode(0o755)).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{{\"workers\":[{{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}}],\"tasks\":[],\"subagents\":[]}}' ;;\n  \"master status\") printf '%s\\n' '{{\"master\":{{\"worker_id\":\"master-peer\",\"endpoint_live\":true}}}}' ;;\n  \"context \") printf '%s\\n' '{{\"identity\":{{\"worker_id\":\"master-peer\",\"kind\":\"peer\",\"transport\":{{\"kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\",\"tmux_endpoint\":{{\"socket_path\":\"/tmp/collab.sock\",\"server_pid\":123,\"tmux_session_id\":\"$1\",\"pane_id\":\"%1\",\"pane_pid\":456}}}}}},\"liveness\":{{\"live\":true,\"presence\":\"present\",\"transport_kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\"}}}}' ;;\n  *) touch '{}' ; printf '%s\\n' '{{\"subscription_id\":\"orphan\"}}' ;;\nesac\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], false);
    assert_eq!(payload["desired"], "subscribed");
    assert_eq!(payload["observed"], "unknown");
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_RECORD_WRITE_FAILED"));
    assert_eq!(payload["subscription_id"], "orphan");
    assert!(marker.exists());

    fs::set_permissions(&control_dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_stale_lock_is_recovered_after_owner_exit() {
    let root = temp_root("goal-stale-lock");
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let mut exited = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let stale_pid = exited.id();
    assert!(exited.wait().unwrap().success());
    let lock_path = control_dir.join("long-task-goal.lock");
    fs::write(
        &lock_path,
        format!("pid={} owner=crashed-goal-worker\n", stale_pid),
    )
    .unwrap();

    let status = run_in(&root, &["goal", "status", "--json"]);
    assert!(status.status.success());
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());

    fs::write(&lock_path, "").unwrap();
    let empty_lock = run_in(&root, &["goal", "status", "--json"]);
    assert!(empty_lock.status.success());
    let empty_payload: Value = serde_json::from_slice(&empty_lock.stdout).unwrap();
    assert_eq!(empty_payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());
    let recovery_receipt = fs::read_dir(&control_dir)
        .unwrap()
        .flatten()
        .find_map(|entry| {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("long-task-goal.lock.recovery.")
            {
                return None;
            }
            let receipt: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            (receipt["reason"] == "empty metadata").then_some(entry)
        })
        .expect("empty lock recovery receipt");
    let recovery_receipt: Value =
        serde_json::from_slice(&fs::read(recovery_receipt.path()).unwrap()).unwrap();
    assert_eq!(recovery_receipt["status"], "recovered");
    assert_eq!(recovery_receipt["original_metadata"], "");
    assert_eq!(recovery_receipt["reason"], "empty metadata");

    fs::write(&lock_path, "pid=").unwrap();
    let truncated_lock = run_in(&root, &["goal", "status", "--json"]);
    assert!(truncated_lock.status.success());
    let truncated_payload: Value = serde_json::from_slice(&truncated_lock.stdout).unwrap();
    assert_eq!(truncated_payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());

    let mut live_owner = Command::new("/bin/sh")
        .args(["-c", "sleep 2"])
        .spawn()
        .unwrap();
    fs::write(
        &lock_path,
        format!("pid={} owner=live-owner\n", live_owner.id()),
    )
    .unwrap();
    let reused_pid = run_in(&root, &["goal", "status", "--json"]);
    assert!(reused_pid.status.success());
    let reused_payload: Value = serde_json::from_slice(&reused_pid.stdout).unwrap();
    assert_eq!(reused_payload["error"], "GOAL_RECORD_NOT_FOUND");

    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    fs::write(
        &lock_path,
        format!("pid={} owner=live-owner\n", live_owner.id()),
    )
    .unwrap();
    hold_advisory_lock(&held);
    let busy = run_in(&root, &["goal", "status", "--json"]);
    assert_eq!(busy.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&busy.stderr).contains("GOAL_LOCK_BUSY"));
    drop(held);
    live_owner.kill().unwrap();
    live_owner.wait().unwrap();
    let recovered = run_in(&root, &["goal", "status", "--json"]);
    assert!(recovered.status.success());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_timeout_failure_remains_explicit() {
    let root = temp_root("goal-sub-timeout");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        fake_bin.join("collab"),
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{\"workers\":[{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}],\"tasks\":[],\"subagents\":[]}' ;;\n  \"master status\") printf '%s\\n' '{\"master\":{\"worker_id\":\"master-peer\",\"endpoint_live\":true}}' ;;\n  \"context \") printf '%s\\n' '{\"identity\":{\"worker_id\":\"master-peer\",\"kind\":\"peer\",\"transport\":{\"kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\",\"tmux_endpoint\":{\"socket_path\":\"/tmp/collab.sock\",\"server_pid\":123,\"tmux_session_id\":\"$1\",\"pane_id\":\"%1\",\"pane_pid\":456}}},\"liveness\":{\"live\":true,\"presence\":\"present\",\"transport_kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\"}}' ;;\n  \"notify subscribe\") printf '%s\\n' 'collab request timed out' >&2; exit 124 ;;\n  *) printf '%s\\n' 'unexpected collab command' >&2; exit 64 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&fake_bin.join("collab"), fs::Permissions::from_mode(0o755)).unwrap();

    let result = Command::new(binary())
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

    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("COLLAB_SUBSCRIBE_FAILED"), "{stderr}");
    assert!(stderr.contains("exit=124"), "{stderr}");
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], false);
    assert_eq!(payload["desired"], "recovery_required");
    assert_eq!(payload["observed"], "unknown");
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("COLLAB_SUBSCRIBE_FAILED:exit=124"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_bug_permission_failure_remains_explicit() {
    let root = temp_root("longhorizon-bug-permission");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"workers\":[],\"tasks\":[],\"subagents\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        "#!/bin/sh\nprintf '%s\\n' 'permission denied' >&2\nexit 126\n",
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let result = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert!(result.status.success());
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("GIT_BUG_OPEN_READ_FAILED"));
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("permission denied"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_bug_read_does_not_fall_back_to_upstream_after_local_failure() {
    let root = temp_root("longhorizon-bug-upstream-fallback");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    let upstream = home.join("Documents/github/appsdk");
    fs::create_dir_all(&upstream).unwrap();
    fs::write(upstream.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream);

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"workers\":[],\"tasks\":[],\"subagents\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let fake_git_bug = fake_bin.join("git-bug");
    let fake_git_bug_script = "#!/bin/sh\nprintf '%s\\n' 'permission denied' >&2\nexit 126\n";
    fs::write(&fake_git_bug, fake_git_bug_script).unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let result = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(result.status.success());
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(payload["open_bugs"].as_array().unwrap().is_empty());
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("GIT_BUG_OPEN_READ_FAILED"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_block_governance_reminder_lifecycle() {
    let root = temp_root("task-block");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
if [ "$1" = "task" ] && [ "$2" = "block" ] && [ "$3" = "task-404" ]; then
  printf '%s\n' '{"ok":true,"task_id":"task-404","status":"blocked"}'
  exit 0
fi
if [ "$1" = "task" ] && [ "$2" = "block" ] && [ "$3" = "task-405" ]; then
  printf '%s\n' '{"ok":true,"task_id":"task-405","status":"blocked"}'
  exit 0
fi
printf '%s\n' 'task not found'
exit 44
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let block_res = Command::new(binary())
        .args([
            "task",
            "block",
            "task-404",
            "--reason",
            "Waiting on upstream SDK bug fix",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(block_res.status.success());
    let block_json: Value = serde_json::from_slice(&block_res.stdout).unwrap();
    assert_eq!(block_json["status"], "blocked");
    assert_eq!(block_json["reminders_stopped"], false);
    assert_eq!(block_json["task_id"], "task-404");
    let rule = block_json["rule"].as_str().unwrap();
    assert!(rule.contains("cause, owner, unblock condition"));

    // Also check human text output contains the prominent reminder
    let human_res = Command::new(binary())
        .args(["task", "block", "task-405"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(human_res.status.success());
    let human_text = String::from_utf8_lossy(&human_res.stdout);
    assert!(human_text.contains("通知策略以 Collab 响应为准"));
    assert!(human_text.contains("合法等待必须写清原因"));

    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' 'task not found' >&2\nexit 44\n",
    )
    .unwrap();
    let failed = Command::new(binary())
        .args(["task", "block", "task-404", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(44));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("COLLAB_TASK_BLOCK_FAILED"));
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("\"ok\":true"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn canonical_function_map_required_gates_resolve_exactly_once() {
    let maps_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/maps");
    let function_map: Value =
        serde_json::from_slice(&fs::read(maps_root.join("function-map.json")).unwrap()).unwrap();
    let verification_map: Value =
        serde_json::from_slice(&fs::read(maps_root.join("verification-map.json")).unwrap())
            .unwrap();

    let gate_ids = verification_map["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gate| gate["gate_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let unique_gate_ids = gate_ids.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        gate_ids.len(),
        unique_gate_ids.len(),
        "canonical verification-map contains duplicate gate_id values"
    );

    for function in function_map["functions"].as_array().unwrap() {
        let function_id = function["function_id"].as_str().unwrap();
        for gate_id in function["required_gates"].as_array().unwrap() {
            let gate_id = gate_id.as_str().unwrap();
            assert_eq!(
                gate_ids.iter().filter(|candidate| candidate.as_str() == gate_id).count(),
                1,
                "function {function_id} requires gate {gate_id}, but it does not resolve exactly once"
            );
        }
    }
}

#[test]
fn canonical_zone_transition_and_promotion_schema_require_parallel_live_closure() {
    let contracts_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts");
    let template_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/minimal/contracts");

    for (label, root) in [("canonical", &contracts_root), ("minimal", &template_root)] {
        let manifest: Value = serde_json::from_slice(
            &fs::read(root.join("transitions/zone-transition.manifest.json")).unwrap(),
        )
        .unwrap();
        let promotion_transition = manifest["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|transition| {
                transition["from"].as_str() == Some("playground")
                    && transition["to"].as_str() == Some("active")
            })
            .unwrap_or_else(|| panic!("{label} manifest must declare playground -> active"));
        let parallel_required = promotion_transition["record_required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert!(
            parallel_required.contains(&"CollabLiveClosureRecordWhenParallel"),
            "{label} playground -> active transition must declare the parallel live closure record: {parallel_required:?}"
        );

        let promotion_schema: Value = serde_json::from_slice(
            &fs::read(root.join("records/promotion-record.schema.json")).unwrap(),
        )
        .unwrap();
        let conditional = promotion_schema["allOf"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("{label} promotion schema must declare a conditional requirement")
            })
            .iter()
            .any(|clause| {
                clause["if"]["required"].as_array().is_some_and(|required| {
                    required
                        .iter()
                        .any(|field| field.as_str() == Some("collaboration_record_id"))
                }) && clause["then"]["required"]
                    .as_array()
                    .is_some_and(|required| {
                        required
                            .iter()
                            .any(|field| field.as_str() == Some("collab_live_closure_record_id"))
                    })
            });
        assert!(
            conditional,
            "{label} promotion schema must conditionally require collab_live_closure_record_id for collaboration promotion"
        );
    }
}

#[test]
fn dagpipe_fix_lifecycle_runs_the_declared_graph_and_state_machine() {
    let root = temp_root("dagpipe-fix-lifecycle");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    assert!(!artifact_hash.is_empty());

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["graph"]["id"], "appsdk-fix-lifecycle");
    assert_eq!(result["graph"]["version"], "0.1.0");
    assert_eq!(result["authority"], "advisory_projection");
    assert_eq!(result["state"], "promoted");
    assert_eq!(
        result["single_source_single_sink"],
        serde_json::json!({"inputs": 1, "outputs": 1})
    );
    assert_eq!(
        result["node_order"],
        serde_json::json!([
            "admission_claim",
            "admission_candidate",
            "admission_review",
            "admission_effectiveness",
            "admission_remote",
            "emit_promotion",
        ])
    );
    let transitions = result["state_journal"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "state_changed")
        .map(|event| {
            (
                event["from"].as_str().unwrap().to_owned(),
                event["event"].as_str().unwrap().to_owned(),
                event["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transitions,
        vec![
            ("open".to_owned(), "claim".to_owned(), "claimed".to_owned()),
            (
                "claimed".to_owned(),
                "candidate".to_owned(),
                "candidate_verified".to_owned()
            ),
            (
                "candidate_verified".to_owned(),
                "review_pass".to_owned(),
                "architecture_reviewed".to_owned()
            ),
            (
                "architecture_reviewed".to_owned(),
                "effectiveness_replay".to_owned(),
                "effectiveness_verified".to_owned()
            ),
            (
                "effectiveness_verified".to_owned(),
                "remote_receipt".to_owned(),
                "remote_verified".to_owned()
            ),
            (
                "remote_verified".to_owned(),
                "promote".to_owned(),
                "promoted".to_owned()
            ),
        ]
    );
    assert_eq!(result["result"]["state"], "promoted");
    assert_eq!(result["result"]["promotion"]["promotion_id"], "promotion-1");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_lifecycle_fails_closed_on_invalid_record_chain() {
    let root = temp_root("dagpipe-fix-lifecycle-invalid");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let review_path = root.join(".appsdk/records/review-record-app-core.json");
    let mut review: Value = serde_json::from_slice(&fs::read(&review_path).unwrap()).unwrap();
    review["verdict"] = Value::String("fail".into());
    fs::write(
        &review_path,
        serde_json::to_string_pretty(&review).unwrap() + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DAGPIPE_EXECUTION_FAILED")
            && stderr.contains("ARCHITECTURE_REVIEW_INPUT_MISMATCH"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_rejects_worktree_record_module_mismatch() {
    let root = temp_root("dagpipe-fix-worktree-module-mismatch");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let worktree_path = root.join(".appsdk/records/worktree-record-app-core.json");
    let mut worktree: Value = serde_json::from_slice(&fs::read(&worktree_path).unwrap()).unwrap();
    worktree["module_id"] = Value::String("other-module".into());
    fs::write(
        &worktree_path,
        serde_json::to_string_pretty(&worktree).unwrap() + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DAGPIPE_EXECUTION_FAILED")
            && stderr.contains("FIX_WORKTREE_MODULE_MISMATCH"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_rejects_candidate_record_module_mismatch() {
    let root = temp_root("dagpipe-fix-candidate-module-mismatch");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let candidate_path = root.join(".appsdk/records/fix-candidate-record-app-core.json");
    let mut candidate: Value = serde_json::from_slice(&fs::read(&candidate_path).unwrap()).unwrap();
    candidate["module_id"] = Value::String("other-module".into());
    fs::write(
        &candidate_path,
        serde_json::to_string_pretty(&candidate).unwrap() + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DAGPIPE_EXECUTION_FAILED")
            && stderr.contains("FIX_CANDIDATE_MODULE_MISMATCH"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_rejects_promotion_gate_result_not_pass() {
    let root = temp_root("dagpipe-fix-promotion-gate-fail");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let promotion_path = root.join(".appsdk/records/promotion-record-app-core.json");
    let mut promotion: Value = serde_json::from_slice(&fs::read(&promotion_path).unwrap()).unwrap();
    promotion["required_gate_results"][0]["result"] = Value::String("fail".into());
    fs::write(
        &promotion_path,
        serde_json::to_string_pretty(&promotion).unwrap() + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DAGPIPE_EXECUTION_FAILED")
            && stderr.contains("PROMOTION_GATE_RESULT_NOT_PASS"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_rejects_path_traversal_module_identifier() {
    let root = temp_root("dagpipe-fix-lifecycle-module-id");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);

    let output = run(&["dagpipe", "fix", root_text, "--module", "../escape"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DAGPIPE_MODULE_IDENTIFIER_INVALID"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_fix_is_advisory_and_authoritative_verify_still_fails_on_missing_evidence() {
    let root = temp_root("dagpipe-fix-lifecycle-advisory");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let missing_evidence = root.join(".appsdk/records/evidence/app-core/baseline-1.json");
    fs::remove_file(&missing_evidence).unwrap();

    let output = run(&["dagpipe", "fix", root_text, "--module", "app-core"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["authority"], "advisory_projection");
    assert_eq!(result["authoritative_gate"], "appsdk verify <project>");

    let promote = run(&["promote", root_text, "--to", "architecture_stable"]);
    assert!(
        !promote.status.success(),
        "dagpipe advisory output must not be treated as authoritative lifecycle acceptance; stdout={} stderr={}",
        String::from_utf8_lossy(&promote.stdout),
        String::from_utf8_lossy(&promote.stderr)
    );
    assert!(
        String::from_utf8_lossy(&promote.stderr).contains("MISSING_RECORD"),
        "promote to architecture_stable must fail on missing evidence; stderr={}",
        String::from_utf8_lossy(&promote.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_reports_every_embedded_graph_as_single_source_single_sink() {
    let output = run(&["dagpipe", "validate"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["single_source_single_sink"], Value::Bool(true));
    let ids = result["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|graph| graph["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"appsdk-fix-lifecycle"));
    assert!(ids.contains(&"appsdk-notification-object"));
}

fn dagpipe_address(session: &str) -> Value {
    serde_json::json!({"scopeId": "scope-1", "sessionId": session})
}

fn dagpipe_message_record(message_id: &str) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "messageId": message_id,
        "conversationId": "conversation-1",
        "from": dagpipe_address("sender"),
        "to": dagpipe_address("recipient"),
        "title": "title",
        "priority": "p1",
        "body": "body",
        "deliveryMode": "direct",
        "coalesceKey": null,
        "issueId": null,
        "adapterId": "adapter-1",
        "deliveryAttemptRequired": true,
        "createdAt": "2026-01-01T00:00:00Z",
        "state": "created",
        "evidence": [],
        "route": {
            "mode": "local",
            "sameAppserver": true,
            "sameProject": true,
            "sourceRole": "peer",
            "targetRole": "peer"
        },
        "lastError": null
    })
}

fn dagpipe_message_state(message_id: &str, state: &str, at: &str) -> Value {
    serde_json::json!({
        "messageId": message_id,
        "state": state,
        "evidence": {"state": state, "at": at, "details": {"transport": "appsdk-internal"}}
    })
}

fn dagpipe_notification_record(notification_id: &str, message_id: &str, generation: u64) -> Value {
    serde_json::json!({
        "notificationId": notification_id,
        "messageId": message_id,
        "generation": generation,
        "recipient": dagpipe_address("recipient"),
        "title": "title",
        "priority": "p1",
        "issueId": null,
        "coalesceKey": null,
        "body": "body",
        "createdAt": "2026-01-01T00:00:00Z",
        "availableAt": "2026-01-01T00:00:00Z",
        "status": "pending",
        "emittedAt": null,
        "adapterId": "adapter-1",
        "transportReceipt": null,
        "lastError": null
    })
}

fn dagpipe_notification_summary(notification_id: &str, message_id: &str, generation: u64) -> Value {
    serde_json::json!({
        "notificationId": notification_id,
        "messageId": message_id,
        "generation": generation,
        "title": "title",
        "priority": "p1",
        "issueId": null,
        "coalesceKey": null,
        "createdAt": "2026-01-01T00:00:00Z"
    })
}

fn dagpipe_attempt(attempt_id: &str, operation: &str) -> Value {
    let mut attempt = serde_json::json!({
        "attemptId": attempt_id,
        "operation": operation,
        "adapterId": "adapter-1",
        "startedAt": "2026-01-01T00:00:02Z"
    });
    if operation == "notification.batch_emitted" {
        attempt["batchId"] = Value::String(format!("batch-{attempt_id}"));
    }
    attempt
}

fn dagpipe_message_created_event(event_id: &str, at: &str, message_id: &str) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "message.created",
        "data": dagpipe_message_record(message_id)
    })
}

fn dagpipe_message_state_event(event_id: &str, at: &str, message_id: &str, state: &str) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "message.state",
        "data": dagpipe_message_state(message_id, state, at)
    })
}

fn dagpipe_notification_queued(
    event_id: &str,
    at: &str,
    key: &str,
    notification_id: &str,
    message_id: &str,
    generation: u64,
) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "notification.queued",
        "data": {
            "key": key,
            "notification": dagpipe_notification_record(notification_id, message_id, generation)
        }
    })
}

fn dagpipe_notification_attempt_event(
    event_id: &str,
    at: &str,
    attempt_id: &str,
    keys: &[&str],
    operation: &str,
) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "notification.delivery_attempt",
        "data": {
            "attemptId": attempt_id,
            "keys": keys,
            "attempt": dagpipe_attempt(attempt_id, operation)
        }
    })
}

fn dagpipe_notification_emitted_event(
    event_id: &str,
    at: &str,
    attempt_id: &str,
    keys: &[&str],
) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "notification.emitted",
        "data": {"attemptId": attempt_id, "keys": keys, "at": at}
    })
}

fn dagpipe_system_message_created_event(event_id: &str, at: &str, message_id: &str) -> Value {
    let mut record = dagpipe_message_record(message_id);
    record["state"] = Value::String("accepted".into());
    record["evidence"] = serde_json::json!([{
        "state": "accepted",
        "at": at,
        "details": {"transport": "appsdk-internal", "source": "daemon"}
    }]);
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "message.created",
        "data": record
    })
}
