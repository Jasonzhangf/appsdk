fn install_project_patrol_fake(project: &Path, role: &str) -> (PathBuf, PathBuf) {
    let fake_bin = project.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let collab = fake_bin.join("collab");
    fs::write(
        &collab,
        r#"#!/bin/sh
case "$1 $2" in
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    if [ "$desired" = "ended" ]; then
      printf '%s\n' "$*" >> ended-calls
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"desired\":\"ended\",\"status\":\"ended\",\"subscription\":null}"
      exit 0
    fi
    printf '%s' "goal:$goal_id" > goal-subject
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"master-peer\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"sub-goal\",\"worker_id\":\"master-peer\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"goal:$goal_id\",\"trigger_times_ms\":[9999999999999],\"fired_count\":0}}"
    ;;
  "notify status")
    printf 'called\n' >> notify-status-calls
    patrol=""
    patrol_subject="${PATROL_STATUS_SUBJECT:-project-patrol}"
    patrol_repeat="${PATROL_STATUS_REPEAT:-100}"
    if [ -f patrol-id ]; then
      patrol_id=$(/bin/cat patrol-id)
      if [ "${PATROL_STATUS_MODE:-armed}" = "consumed" ]; then
        patrol="{\"id\":\"$patrol_id\",\"worker_id\":\"master-peer\",\"event\":\"master-idle\",\"subject\":\"$patrol_subject\",\"interval_ms\":900000,\"repeat_count\":$patrol_repeat,\"fired_count\":100,\"expires_ms\":9999999999999,\"status\":\"consumed\",\"created_ms\":2}"
      else
        patrol="{\"id\":\"$patrol_id\",\"worker_id\":\"master-peer\",\"event\":\"master-idle\",\"subject\":\"$patrol_subject\",\"interval_ms\":900000,\"repeat_count\":$patrol_repeat,\"fired_count\":0,\"expires_ms\":9999999999999,\"status\":\"armed\",\"created_ms\":2}"
      fi
    fi
    goal=""
    if [ -f goal-subject ]; then
      goal_subject=$(/bin/cat goal-subject)
      goal="{\"id\":\"sub-goal\",\"worker_id\":\"master-peer\",\"event\":\"deadline\",\"subject\":\"$goal_subject\",\"trigger_times_ms\":[9999999999999],\"repeat_count\":1,\"fired_count\":0,\"expires_ms\":9999999999999,\"status\":\"armed\"}"
    fi
    if [ -n "$goal" ] && [ -n "$patrol" ]; then
      printf '{"subscriptions":[%s,%s]}\n' "$goal" "$patrol"
    elif [ -n "$goal" ]; then
      printf '{"subscriptions":[%s]}\n' "$goal"
    elif [ -n "$patrol" ]; then
      printf '{"subscriptions":[%s]}\n' "$patrol"
    else
      printf '%s\n' '{"subscriptions":[]}'
    fi
    ;;
  "notify subscribe")
    printf '%s\n' "$*" >> notify-subscribe-calls
    if [ "${PATROL_SUBSCRIBE_FAIL:-0}" = "1" ]; then
      printf '%s\n' 'patrol subscribe rejected by fixture' >&2
      exit 45
    fi
    count=$((`/bin/cat patrol-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > patrol-count
    printf '%s' "sub-patrol-$count" > patrol-id
    printf '{"subscription":{"id":"sub-patrol-%s","worker_id":"master-peer","event":"master-idle","subject":"project-patrol","interval_ms":900000,"repeat_count":100,"fired_count":0,"expires_ms":9999999999999,"status":"armed","created_ms":2}}\n' "$count"
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","active_task":null,"endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"waiting"}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  *)
    printf '%s\n' "$*" >> unexpected-collab-calls
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&collab, fs::Permissions::from_mode(0o755)).unwrap();

    let delegate = fake_bin.join("collab.delegate");
    fs::rename(&collab, &delegate).unwrap();
    let context = serde_json::json!({
        "result": {"snapshot": {
            "name": "master-peer",
            "session_id": "test-session",
            "binding": {"project_scope": project.canonicalize().unwrap()},
            "identity": {
                "worker_id": "master-peer",
                "role": role,
                "kind": "peer",
                "transport": {"kind": "appserver", "thread_id": "test-thread"}
            },
            "liveness": {"live": true, "transport_kind": "appserver"}
        }}
    })
    .to_string();
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    fs::write(
        &collab,
        format!(
            "#!/bin/sh\nif [ \"$1\" = context ]; then printf '%s\\n' {}; else exec {} \"$@\"; fi\n",
            quote(&context),
            quote(&delegate.to_string_lossy())
        ),
    )
    .unwrap();
    fs::set_permissions(&collab, fs::Permissions::from_mode(0o755)).unwrap();
    (fake_bin, collab)
}

fn subscribe_project_goal(root: &Path, fake_bin: &Path) -> std::process::Output {
    Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(root)
        .env("PATH", fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

#[test]
fn goal_project_patrol_master_arms_persists_and_reuses_schedule() {
    let root = temp_root("goal-project-patrol-master");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "master");

    let first = subscribe_project_goal(&root, &fake_bin);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let receipt: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(receipt["patrol_armed"], true);
    assert_eq!(receipt["patrol_subscription_id"], "sub-patrol-1");
    assert_eq!(receipt["patrol_source"], "armed");
    assert_eq!(receipt["patrol_every_ms"], 1_500_000);
    let calls = fs::read_to_string(root.join("notify-subscribe-calls")).unwrap();
    assert_eq!(calls.lines().count(), 1);
    assert!(calls.contains("--event master-idle"));
    assert!(calls.contains("--subject project-patrol"));
    assert!(calls.contains("--every-ms 1500000"));
    let persisted: Value =
        serde_json::from_slice(&fs::read(goal_test_record_path(&root)).unwrap()).unwrap();
    assert_eq!(persisted["patrol_armed"], true);
    assert_eq!(persisted["patrol_subscription_id"], "sub-patrol-1");

    let reused = subscribe_project_goal(&root, &fake_bin);
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    let reused_receipt: Value = serde_json::from_slice(&reused.stdout).unwrap();
    assert_eq!(reused_receipt["patrol_armed"], true);
    assert_eq!(reused_receipt["patrol_subscription_id"], "sub-patrol-1");
    assert_eq!(reused_receipt["patrol_source"], "reused");
    assert_eq!(
        fs::read_to_string(root.join("notify-subscribe-calls"))
            .unwrap()
            .lines()
            .count(),
        1,
        "a valid armed patrol schedule must be reused, not duplicated"
    );
    assert!(!root.join("ended-calls").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_project_patrol_rearms_consumed_finite_schedule() {
    let root = temp_root("goal-project-patrol-consumed");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "master");

    let first = subscribe_project_goal(&root, &fake_bin);
    assert!(first.status.success());
    let first_receipt: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_receipt["patrol_subscription_id"], "sub-patrol-1");

    let rearmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("PATROL_STATUS_MODE", "consumed")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        rearmed.status.success(),
        "{}",
        String::from_utf8_lossy(&rearmed.stderr)
    );
    let rearmed_receipt: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
    assert_eq!(rearmed_receipt["patrol_armed"], true);
    assert_eq!(rearmed_receipt["patrol_subscription_id"], "sub-patrol-2");
    assert_eq!(rearmed_receipt["patrol_source"], "armed");
    assert_eq!(
        fs::read_to_string(root.join("notify-subscribe-calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_project_patrol_peer_does_not_arm() {
    let root = temp_root("goal-project-patrol-peer");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "peer");

    let receipt = subscribe_project_goal(&root, &fake_bin);
    assert!(
        receipt.status.success(),
        "{}",
        String::from_utf8_lossy(&receipt.stderr)
    );
    let payload: Value = serde_json::from_slice(&receipt.stdout).unwrap();
    assert_eq!(payload["patrol_applicable"], false);
    assert_eq!(payload["patrol_armed"], false);
    assert!(payload["patrol_subscription_id"].is_null());
    assert!(!root.join("notify-status-calls").exists());
    assert!(!root.join("notify-subscribe-calls").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_project_patrol_does_not_reuse_legacy_master_idle_subject() {
    let root = temp_root("goal-project-patrol-legacy-subject");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "master");
    fs::write(root.join("patrol-id"), "legacy-master-idle").unwrap();

    let receipt = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("PATROL_STATUS_SUBJECT", "legacy-master-idle")
        .env("PATROL_STATUS_REPEAT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        receipt.status.success(),
        "{}",
        String::from_utf8_lossy(&receipt.stderr)
    );
    let payload: Value = serde_json::from_slice(&receipt.stdout).unwrap();
    assert_eq!(payload["patrol_armed"], true);
    assert_eq!(payload["patrol_subscription_id"], "sub-patrol-1");
    assert_eq!(
        payload["patrol_source"], "armed",
        "a legacy finite master-idle timer with another subject is not the standing project timer"
    );
    let calls = fs::read_to_string(root.join("notify-subscribe-calls")).unwrap();
    assert_eq!(calls.lines().count(), 1);
    assert!(calls.contains("--subject project-patrol"));
    assert!(
        !root.join("unexpected-collab-calls").exists(),
        "the legacy subscription must not be retired or modified"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_project_patrol_arm_failure_is_explicit_and_keeps_goal() {
    let root = temp_root("goal-project-patrol-failure");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "master");

    let receipt = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("PATROL_SUBSCRIBE_FAIL", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        receipt.status.success(),
        "{}",
        String::from_utf8_lossy(&receipt.stderr)
    );
    let payload: Value = serde_json::from_slice(&receipt.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["subscription_id"], "sub-goal");
    assert_eq!(payload["patrol_armed"], false);
    assert_eq!(
        payload["patrol_source"], "failed",
        "a rejected patrol arm must not pretend to be armed"
    );
    assert!(payload["patrol_error"]
        .as_str()
        .unwrap()
        .contains("GOAL_PATROL_SUBSCRIBE_FAILED"));
    assert!(!root.join("ended-calls").exists());
    let persisted: Value =
        serde_json::from_slice(&fs::read(goal_test_record_path(&root)).unwrap()).unwrap();
    assert_eq!(persisted["active"], true);
    assert_eq!(persisted["patrol_armed"], false);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_projects_patrol_without_subscription_side_effect() {
    let root = temp_root("longhorizon-project-patrol-projection");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let (fake_bin, _) = install_project_patrol_fake(&root, "master");

    let subscribed = subscribe_project_goal(&root, &fake_bin);
    assert!(
        subscribed.status.success(),
        "{}",
        String::from_utf8_lossy(&subscribed.stderr)
    );
    if root.join("notify-subscribe-calls").exists() {
        fs::remove_file(root.join("notify-subscribe-calls")).unwrap();
    }

    let show = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let payload: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(payload["goal"]["patrol_armed"], true);
    assert_eq!(payload["goal"]["patrol_subscription_id"], "sub-patrol-1");
    assert!(
        !root.join("notify-subscribe-calls").exists(),
        "longhorizon show must never create a patrol subscription"
    );

    fs::remove_dir_all(root).unwrap();
}
