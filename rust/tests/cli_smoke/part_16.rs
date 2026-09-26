#[test]
fn longhorizon_show_briefs_role_fleet_and_notification_rules() {
    let root = temp_root("longhorizon-show");
    fs::create_dir_all(root.join(".appsdk-control")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","active_task":null,"endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"waiting"}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"appserver","thread_id":"thread-master-peer"}},"liveness":{"live":true,"transport_kind":"appserver"},"tasks":[],"inbox":{"unread":0}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    // Without a registered goal the briefing still teaches the contract and
    // tells the master how to register one.
    let bare = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        bare.status.success(),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    let bare_text = String::from_utf8_lossy(&bare.stdout);
    assert!(bare_text.contains("你的主要任务不是写代码"));
    assert!(bare_text.contains("goal subscribe"));

    // A typo must not be silently swallowed as a project path.
    let typo = run_in(&root, &["longhorizon", "bogus"]);
    assert!(!typo.status.success());
    assert!(String::from_utf8_lossy(&typo.stderr).contains("UNKNOWN_LONGHORIZON_SUBCOMMAND"));

    let goal_file = root.join("plan.md");
    fs::write(
        &goal_file,
        "---\ntitle: t\n---\n\n# Ship It\n\n推动目标完成。\n",
    )
    .unwrap();
    fs::write(
        root.join(".appsdk-control/long-task-goal.json"),
        format!(
            r#"{{"schema_version":1,"goal_path":"{}","interval":"10m","active":true,"registered_at":"2026-09-07T00:00:00Z"}}"#,
            goal_file.display()
        ),
    )
    .unwrap();

    let res = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        res.status.success(),
        "{}",
        String::from_utf8_lossy(&res.stderr)
    );
    let text = String::from_utf8_lossy(&res.stdout);

    // Charter, fleet rules and notification rules all travel with the wake.
    assert!(text.contains("不要空转，不要假装完成"));
    assert!(text.contains("`[subagent].max_concurrent`（默认 8）"));
    assert!(text.contains("普通 peer 不计入该额度"));
    assert!(text.contains("绝不能以 ACK、已读或一段总结结束一轮"));
    assert!(text.contains("collab worker close"));
    assert!(text.contains("appsdk subworker status"));

    // Goal objective is read out of the markdown, past the frontmatter.
    assert!(text.contains("# Ship It"));
    assert!(text.contains("推动目标完成。"));
    assert!(!text.contains("title: t"));

    let json_res = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(json_res.status.success());
    let payload: Value = serde_json::from_slice(&json_res.stdout).unwrap();
    assert_eq!(payload["role"], "master");
    assert_eq!(payload["goal"]["registered"], true);
    assert_eq!(payload["goal"]["interval"], "10m");
    assert!(payload["charter"].as_str().unwrap().contains("调度"));
    assert!(payload["charter"]
        .as_str()
        .unwrap()
        .contains("先饱和每一个 live+present 的空闲普通 peer"));
    assert!(payload["charter"]
        .as_str()
        .unwrap()
        .contains("长等待仍并发派单"));
    assert!(payload["fleet_rules"]
        .as_str()
        .unwrap()
        .contains("`[subagent].max_concurrent`（默认 8）"));
    assert!(payload["notification_rules"]
        .as_str()
        .unwrap()
        .contains("P0"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_lists_daemon_registered_pending_merges() {
    let root = temp_root("longhorizon-pending-merges");
    fs::create_dir_all(root.join(".appsdk-control")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","active_task":null,"endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"waiting"}],"tasks":[],"subagents":[],"pending_merges":[{"task_id":"task-merge-1","owner":"worker-a","requested_by":"master-peer","requested_at":"2026-09-25T00:00:00Z","status":"accepted","branch":"codex/merge-1","worktree":"playground/merge-1"}]}'
    ;;
  "*")
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let text = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("待合并"));
    assert!(stdout.contains("task-merge-1"));
    assert!(stdout.contains("codex/merge-1"));

    let json_res = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(json_res.status.success());
    let payload: Value = serde_json::from_slice(&json_res.stdout).unwrap();
    assert_eq!(payload["pending_merges"][0]["task_id"], "task-merge-1");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_never_upgrades_worker_or_unknown_to_master() {
    let root = temp_root("longhorizon-role-projection");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"worker-peer","role":"peer","active_task":"task-1","endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"working"}],"tasks":[{"id":"task-1","status":"working","owner":"worker-peer","next_step":"run tests"}],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"other-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"worker-peer","kind":"peer","transport":{"kind":"appserver","thread_id":"thread-worker-peer"}},"liveness":{"live":true,"transport_kind":"appserver"},"tasks":[{"id":"task-1"}],"inbox":{"unread":0}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let worker = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        worker.status.success(),
        "{}",
        String::from_utf8_lossy(&worker.stderr)
    );
    let payload: Value = serde_json::from_slice(&worker.stdout).unwrap();
    assert_eq!(payload["role"], "worker");
    assert!(!payload["charter"]
        .as_str()
        .unwrap()
        .contains("你是本项目的 master"));
    assert!(payload["charter"].as_str().unwrap().contains("独立 worker"));

    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[],"tasks":[],"subagents":[]}'
    ;;
  "context ")
    exit 1
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    let unknown = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        unknown.status.success(),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    let payload: Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(payload["role"], "unknown");
    assert!(payload["charter"].as_str().unwrap().contains("身份未验证"));
    assert!(!payload["charter"]
        .as_str()
        .unwrap()
        .contains("你是本项目的 master"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_requires_authoritative_master_identity() {
    let root = temp_root("longhorizon-master-authority");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${ROLE_CASE:-master}" in
      worker|tmux-master) printf '%s\n' '{"workers":[{"id":"current-peer","role":"peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      subagent) printf '%s\n' '{"workers":[],"tasks":[],"subagents":[{"peer":"current-peer","status":"working"}]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status")
    case "${ROLE_CASE:-master}" in
      master|tmux-master) printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true}}' ;;
      mismatch) printf '%s\n' '{"master":{"worker_id":"other-peer","endpoint_live":true}}' ;;
      transport-missing) printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true}}' ;;
      *) exit 44 ;;
    esac
    ;;
  "context ")
    case "${ROLE_CASE:-master}" in
      tmux-master) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","transport":{"kind":"tmux","tmux_endpoint":{"pane_id":"%1"}}},"liveness":{"live":true,"transport_kind":"tmux"}}' ;;
      transport-missing) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","transport":{"kind":"appserver"}},"liveness":{"live":true,"transport_kind":"appserver"}}' ;;
      *) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","transport":{"kind":"appserver","thread_id":"thread-current-peer"}},"liveness":{"live":true,"transport_kind":"appserver"}}' ;;
    esac
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |case_name: &str| {
        Command::new(binary())
            .args(["longhorizon", "show", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("ROLE_CASE", case_name)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };
    for (case_name, expected_role) in [
        ("master", "master"),
        ("tmux-master", "master"),
        ("worker", "unknown"),
        ("subagent", "managed-subagent"),
        ("missing", "unknown"),
        ("mismatch", "worker"),
        ("transport-missing", "unknown"),
    ] {
        let result = run(case_name);
        assert!(
            result.status.success(),
            "case={case_name} stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
        let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(payload["role"], expected_role, "case={case_name}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_rejects_invalid_worker_before_master_match() {
    let root = temp_root("longhorizon-invalid-master-worker");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-endpoint}" in
      endpoint) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":false,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      identity) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":false,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      offline) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":true}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","transport":{"kind":"appserver","thread_id":"thread-current-peer"}},"liveness":{"live":true,"transport_kind":"appserver"}}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    for variant in ["endpoint", "identity", "offline"] {
        let result = Command::new(binary())
            .args(["longhorizon", "show", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("STATUS_VARIANT", variant)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert!(result.status.success(), "variant={variant}");
        let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(payload["role"], "unknown", "variant={variant}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_never_masks_bug_read_failures() {
    let root = temp_root("longhorizon-bug-read");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[],"tasks":[],"subagents":[]}'
    ;;
  "context ")
    exit 1
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let missing = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        missing.status.success(),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    let payload: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(
        payload["open_bugs_error"]
            .as_str()
            .unwrap()
            .contains("GIT_BUG_NOT_FOUND"),
        "{}",
        payload["open_bugs_error"]
    );

    let text = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("开放缺陷读取失败"));

    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug --status")
    printf '%s\n' 'not-json'
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();

    let invalid = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        invalid.status.success(),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
    let payload: Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(
        payload["open_bugs_error"]
            .as_str()
            .unwrap()
            .contains("GIT_BUG_OPEN_JSON_INVALID"),
        "{}",
        payload["open_bugs_error"]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscription_and_master_prompt_lifecycle() {
    let root = temp_root("goal-sub");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' "$*" > notify-args
    case "$*" in
      *"--at-ms "*) ;;
      *) printf '%s\n' 'goal deadline requires --at-ms' >&2; exit 42 ;;
    esac
    case "$*" in
      *"--every-ms"*|*"--repeat-count"*) printf '%s\n' 'periodic goal deadline is invalid' >&2; exit 43 ;;
    esac
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--subject" ]; then printf '%s' "$2" > goal-subject; fi
      shift
    done
    if [ "${UNARMED_SUBSCRIBE:-}" = "1" ]; then
      printf '%s\n' '{"subscription_id":"goal-sub-unarmed","status":"pending"}'
    else
      printf '%s\n' '{"subscription_id":"goal-sub-1"}'
    fi
    ;;
  "notify status")
    subject=$(/bin/cat goal-subject 2>/dev/null || printf '%s' 'missing-subject')
    if [ "${RECONCILE_CANCEL:-}" = "1" ]; then
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-2\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    elif [ "${DUPLICATE_SUBJECT:-}" = "1" ]; then
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-1\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"},{\"id\":\"goal-sub-2\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    else
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-1\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    fi
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}'
    ;;
  "notify unsubscribe")
    if [ "${RECONCILE_CANCEL:-}" = "1" ]; then
      if [ "$3" = "goal-sub-1" ]; then exit 44; fi
      printf '%s\n' '{"subscription_id":"goal-sub-2","status":"cancelled"}'
      exit 0
    fi
    printf '%s\n' '{"subscription_id":"goal-sub-1","status":"cancelled"}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    // 1. Non-md file should fail
    let non_md = root.join("goal.txt");
    fs::write(&non_md, "some goal").unwrap();
    let res_non_md = run_in(&root, &["goal", "subscribe", "--goal", "goal.txt"]);
    assert!(!res_non_md.status.success());
    assert!(String::from_utf8_lossy(&res_non_md.stderr).contains("GOAL_PATH_MUST_BE_MD_FILE"));

    // 2. Non-existent md file should fail
    let res_not_found = run_in(&root, &["goal", "subscribe", "--goal", "missing-goal.md"]);
    assert!(!res_not_found.status.success());
    assert!(String::from_utf8_lossy(&res_not_found.stderr).contains("GOAL_FILE_NOT_FOUND"));

    // 3. Valid md goal registration with interval
    let valid_goal = root.join("long-task.md");
    fs::write(
        &valid_goal,
        "# Sample Long-Horizon Goal\nDeliver feature X.\n",
    )
    .unwrap();

    let sub_res = Command::new(binary())
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
        sub_res.status.success(),
        "{}",
        String::from_utf8_lossy(&sub_res.stderr)
    );
    let sub_json: Value = serde_json::from_slice(&sub_res.stdout).unwrap();
    assert_eq!(sub_json["active"], true);
    assert_eq!(sub_json["interval"], "5m");
    assert_eq!(sub_json["every_ms"], 300000);
    assert_eq!(sub_json["desired"], "subscribed");
    assert_eq!(sub_json["observed"], "subscribed");
    assert_eq!(sub_json["goal_id"], sub_json["goal_id"]);
    assert!(sub_json["goal_id"].as_str().unwrap().starts_with("sha256:"));
    let prompt = sub_json["master_prompt"].as_str().unwrap();
    assert!(prompt.contains("Master 专属"));
    assert!(prompt.contains("饱和"));
    assert!(prompt.contains("appsdk bug"));
    assert_eq!(sub_json["schedule"], "one-shot");
    assert_eq!(sub_json["local_schedule"], "periodic-rearm-intent");
    assert_eq!(sub_json["repeat_count"], 1);
    assert_eq!(sub_json["requested_repeat_count"], 100);
    assert!(sub_json["subject"]
        .as_str()
        .unwrap()
        .starts_with("goal:sha256:"));
    assert!(prompt.contains("每次 Collab deadline 都是单次触发"));
    let notify_args = fs::read_to_string(root.join("notify-args")).unwrap();
    assert!(notify_args.contains("--at-ms "));
    assert!(!notify_args.contains("--every-ms"));
    assert!(!notify_args.contains("--repeat-count"));

    let duplicate = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("DUPLICATE_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("GOAL_RECONCILE_SUBJECT_AMBIGUOUS"));

    let rearm = Command::new(binary())
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
        rearm.status.success(),
        "{}",
        String::from_utf8_lossy(&rearm.stderr)
    );
    let rearm_json: Value = serde_json::from_slice(&rearm.stdout).unwrap();
    assert_eq!(rearm_json["desired"], "subscribed");

    // 4. Check goal status
    let status_res = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status_res.status.success());
    let status_json: Value = serde_json::from_slice(&status_res.stdout).unwrap();
    assert_eq!(status_json["active"], true);
    assert_eq!(status_json["interval"], "5m");
    assert_eq!(status_json["desired"], "subscribed");
    assert_eq!(status_json["observed"], "subscribed");

    let record_path = root.join(".appsdk-control/long-task-goal.json");
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["subscription_id"] = Value::Null;
    record["collab_subscription"] = Value::Null;
    fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let status_reconciled = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status_reconciled.status.success());
    let status_reconciled_json: Value = serde_json::from_slice(&status_reconciled.stdout).unwrap();
    assert_eq!(status_reconciled_json["active"], true);
    assert_eq!(status_reconciled_json["subscription_id"], "goal-sub-1");

    // 5. Check standalone prompt command
    let prompt_res = Command::new(binary())
        .args([
            "goal",
            "prompt",
            "--goal",
            "long-task.md",
            "--interval",
            "10m",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(prompt_res.status.success());
    let prompt_text = String::from_utf8_lossy(&prompt_res.stdout);
    assert!(prompt_text.contains("长程任务目标文档"));
    assert!(prompt_text.contains("10m"));
    assert!(prompt_text.contains("本地重唤醒意图"));

    // 6. Cancel goal
    let cancel_res = Command::new(binary())
        .args(["goal", "cancel"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("RECONCILE_CANCEL", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(cancel_res.status.success());
    assert!(String::from_utf8_lossy(&cancel_res.stdout).contains("goal-sub-2"));

    let cancel_again = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(cancel_again.status.success());
    let cancel_again_json: Value = serde_json::from_slice(&cancel_again.stdout).unwrap();
    assert_eq!(cancel_again_json["idempotent"], true);
    assert_eq!(cancel_again_json["status"], "cancelled");
    assert!(cancel_again_json["cancel_receipt"].is_object());
    assert!(cancel_again_json["revision"].as_u64().unwrap() >= 3);

    let post_cancel_status = run_in(&root, &["goal", "status", "--json"]);
    assert!(post_cancel_status.status.success());
    let post_cancel_json: Value = serde_json::from_slice(&post_cancel_status.stdout).unwrap();
    assert_eq!(post_cancel_json["active"], false);
    assert_eq!(post_cancel_json["desired"], "unsubscribed");
    assert_eq!(post_cancel_json["observed"], "cancelled");
    assert_eq!(post_cancel_json["subscription_id"], "goal-sub-2");
    assert!(post_cancel_json["record"]["cancel_receipt"].is_object());

    let unarmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("UNARMED_SUBSCRIBE", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(unarmed.status.code(), Some(1));
    let unarmed_json: Value = serde_json::from_slice(&unarmed.stdout).unwrap();
    assert_eq!(unarmed_json["active"], false);
    assert_eq!(unarmed_json["desired"], "recovery_required");
    assert_eq!(unarmed_json["observed"], "unknown");
    assert_eq!(unarmed_json["subscription_id"], "goal-sub-unarmed");
    assert!(unarmed_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:pending"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_owner_gate_rejects_authoritative_master_mismatch() {
    let root = temp_root("goal-owner-gate-mismatch");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-live}" in
      stale) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":true}],"tasks":[],"subagents":[]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status")
    printf '%s\n' "{\"master\":{\"worker_id\":\"${MASTER_WORKER:-master-peer}\",\"endpoint_live\":${MASTER_LIVE:-true}}}"
    ;;
  "context ")
    transport_kind=${CONTEXT_TRANSPORT_KIND:-tmux}
    printf '%s\n' "{\"identity\":{\"worker_id\":\"master-peer\",\"kind\":\"peer\",\"transport\":{\"kind\":\"$transport_kind\",\"endpoint\":\"/tmp/collab.sock\",\"tmux_endpoint\":{\"socket_path\":\"/tmp/collab.sock\",\"server_pid\":123,\"tmux_session_id\":\"$1\",\"pane_id\":\"%1\",\"pane_pid\":456}}},\"liveness\":{\"live\":${CONTEXT_LIVE:-true},\"presence\":\"present\",\"transport_kind\":\"$transport_kind\",\"endpoint\":\"/tmp/collab.sock\"}}"
    ;;
  "notify status") printf '%s\n' '{"subscriptions":[]}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"owner-gate-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |extra_env: &[(&str, &str)]| {
        let mut command = Command::new(binary());
        command
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.output().unwrap()
    };

    let identity_mismatch = run(&[("MASTER_WORKER", "other-peer")]);
    assert_eq!(identity_mismatch.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&identity_mismatch.stderr).contains("GOAL_OWNER_IDENTITY_MISMATCH")
    );

    let transport_missing = run(&[("CONTEXT_TRANSPORT_KIND", "mailbox")]);
    assert_eq!(transport_missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&transport_missing.stderr)
        .contains("GOAL_OWNER_CONTEXT_TRANSPORT_NOT_LIVE"));

    let offline_master = run(&[("MASTER_LIVE", "false")]);
    assert_eq!(offline_master.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&offline_master.stderr).contains("GOAL_OWNER_MASTER_NOT_LIVE"));

    let stale_master_projection = run(&[("STATUS_VARIANT", "stale")]);
    assert!(
        stale_master_projection.status.success(),
        "{}",
        String::from_utf8_lossy(&stale_master_projection.stderr)
    );
    let stale_payload: Value = serde_json::from_slice(&stale_master_projection.stdout).unwrap();
    assert_eq!(stale_payload["active"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_owner_gate_rejects_missing_worker_liveness_fields() {
    let root = temp_root("goal-owner-gate-missing-fields");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-complete}" in
      endpoint) printf '%s\n' '{"workers":[{"id":"master-peer","identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      identity) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      offline) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true}],"tasks":[],"subagents":[]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"missing-fields-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |variant: &str| {
        Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("STATUS_VARIANT", variant)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };

    for (variant, expected) in [
        ("endpoint", "GOAL_OWNER_NOT_LIVE"),
        ("identity", "GOAL_OWNER_IDENTITY_INVALID"),
        ("offline", "GOAL_OWNER_SUSPECTED_OFFLINE"),
    ] {
        let result = run(variant);
        assert_eq!(result.status.code(), Some(1), "variant={variant}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "variant={variant} stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_drains_large_collab_output_without_timeout() {
    let root = temp_root("goal-large-collab-output");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    (
      printf '%s' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[],"padding":"'
      /bin/dd if=/dev/zero bs=4194304 count=1 2>/dev/null | /usr/bin/tr '\0' 'x'
      printf '%s\n' '"}'
    ) &
    (
      /bin/dd if=/dev/zero bs=4194304 count=1 2>/dev/null | /usr/bin/tr '\0' 'e' >&2
    ) &
    wait
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"large-output-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    // The command's read budget is deliberately long enough for a busy
    // daemon.  This regression checks that stdout/stderr are drained without
    // deadlock; a ten-second wall-clock assertion made the suite fail under
    // normal parallel load even when the command completed successfully.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(120),
        "large Collab output exceeded the declared read budget: {:?}: {}",
        started.elapsed(),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["subscription_id"], "large-output-sub");
    assert_eq!(payload["observed"], "subscribed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_allows_slow_collab_write_within_write_budget() {
    let root = temp_root("goal-slow-subscribe-write");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}}},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify subscribe")
    /bin/sleep 20
    printf '%s\n' '{"subscription_id":"slow-subscribe-write"}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_secs(19),
        "slow Collab write returned too early: {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "slow Collab write exceeded write budget: {:?}",
        started.elapsed()
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["observed"], "subscribed");
    assert_eq!(payload["subscription_id"], "slow-subscribe-write");

    fs::remove_dir_all(root).unwrap();
}

// Short goal intervals used to arm an absolute at-ms that Collab could reject
// as already past once the subscribe call queued behind the daemon batch
// budget. The local record was written first, so the failure stranded
// `subscription_id` at null. The interval is now rejected before any record
// mutation, so no subscribe is attempted and no goal record is created.
#[test]
fn goal_subscribe_rejects_short_interval_before_mutating_the_record() {
    let root = temp_root("goal-short-interval");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' "$*" >> notify-subscribe-calls
    printf '%s\n' '{"subscription_id":"sub-short"}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    for interval in ["10s", "1s", "119s"] {
        let rejected = Command::new(binary())
            .args([
                "goal",
                "subscribe",
                "--goal",
                "long-task.md",
                "--interval",
                interval,
                "--json",
            ])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(1), "interval {interval}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("GOAL_INTERVAL_TOO_SHORT"),
            "interval {interval}: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            !root.join(".appsdk-control/long-task-goal.json").exists(),
            "a rejected short interval must not mutate the local goal record"
        );
        assert!(
            !root.join("notify-subscribe-calls").exists(),
            "a rejected short interval must not reach Collab notify subscribe"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

// The at-ms is recomputed immediately before the subscribe call, so the
// interval is measured from just before the request instead of from before the
// local record write. The fake Collab records the at-ms it received.
#[test]
fn goal_subscribe_computes_the_trigger_immediately_before_the_collab_call() {
    let root = temp_root("goal-fresh-trigger");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
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
  "notify status") printf '%s\n' '{"subscriptions":[]}' ;;
  "notify subscribe")
    printf '%s\n' "$*" > notify-args
    printf '%s\n' '{"subscription_id":"sub-fresh","status":"armed"}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let started = goal_now_ms_for_test();
    let subscribed = Command::new(binary())
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
    assert!(
        subscribed.status.success(),
        "{}",
        String::from_utf8_lossy(&subscribed.stderr)
    );
    let payload: Value = serde_json::from_slice(&subscribed.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["observed"], "subscribed");
    assert_eq!(payload["subscription_id"], "sub-fresh");

    let notify_args = fs::read_to_string(root.join("notify-args")).unwrap();
    let at_ms: i64 = notify_args
        .split("--at-ms")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("notify subscribe must carry --at-ms")
        .parse()
        .unwrap();
    assert!(
        at_ms >= started.saturating_add(120_000),
        "the at-ms must not be earlier than the start of the command plus the interval: at_ms={at_ms} started={started}"
    );
    assert_eq!(
        payload["trigger_ms"].as_i64().unwrap(),
        at_ms,
        "the persisted record must carry the same trigger that was sent"
    );

    fs::remove_dir_all(root).unwrap();
}

fn goal_now_ms_for_test() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// A goal deadline that Collab skipped while the master was busy stays `armed`
// with its one-shot trigger in the past, so `goal subscribe` must not report
// that already-due deadline as the retained, idempotent subscription: the
// patrol loop would silently stop while the local record still reads
// `subscribed`. The fake Collab answers `notify status` from the same record
// the daemon exposes, and drops cancelled subscriptions out of that view like
// the daemon reducer does.
#[test]
fn goal_subscribe_rearms_a_due_one_shot_deadline_instead_of_retaining_it() {
    let root = temp_root("goal-due-retain");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
past="${GOAL_FAKE_PAST_MS:?}"
future="${GOAL_FAKE_FUTURE_MS:?}"
subscription() {
  printf '{"id":"sub-retained","status":"armed","event":"deadline","subject":"goal:long-task.md","trigger_times_ms":[%s],"fired_count":0,"status_reason":"%s","expires_ms":9999999999999}' "$1" "$2"
}
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true}}' ;;
  "context "*|"context") printf '%s\n' '{"identity":{"worker_id":"master-peer","kind":"peer","transport":{"kind":"tmux","endpoint":"/tmp/collab.sock","tmux_endpoint":{"socket_path":"/tmp/collab.sock","server_pid":123,"tmux_session_id":"$1","pane_id":"%1","pane_pid":456}},"role":"master"},"liveness":{"live":true,"presence":"present","transport_kind":"tmux","endpoint":"/tmp/collab.sock"}}' ;;
  "notify status")
    if [ -f cancelled.marker ]; then
      printf '%s\n' '{"subscriptions":[]}'
    elif [ "${GOAL_FAKE_DUE:-}" = "1" ]; then
      printf '{"subscriptions":[%s]}\n' "$(subscription "$past" deadline-master-busy-skipped)"
    else
      printf '{"subscriptions":[%s]}\n' "$(subscription "$future" "")"
    fi
    ;;
  "notify unsubscribe")
    printf '%s\n' "$*" >> unsubscribe-calls
    : > cancelled.marker
    printf '%s\n' '{"subscription_id":"sub-retained","status":"cancelled"}'
    ;;
  "notify subscribe")
    printf '%s\n' "$*" >> subscribe-calls
    count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
    printf '%s' "$count" > subscribe-count
    at=""
    previous=""
    for argument in "$@"; do
      if [ "$previous" = "--at-ms" ]; then
        at="$argument"
      fi
      previous="$argument"
    done
    printf '{"subscription_id":"sub-arm-%s","status":"armed","event":"deadline","subject":"goal:long-task.md","trigger_times_ms":[%s],"fired_count":0}\n' "$count" "$at"
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let subscribed = |due: bool| {
        Command::new(binary())
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
            .env(
                "GOAL_FAKE_PAST_MS",
                (goal_now_ms_for_test() - 600_000).to_string(),
            )
            .env(
                "GOAL_FAKE_FUTURE_MS",
                (goal_now_ms_for_test() + 600_000).to_string(),
            )
            .env("GOAL_FAKE_DUE", if due { "1" } else { "0" })
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };

    let first = subscribed(false);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_payload: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_payload["subscription_id"], "sub-arm-1");

    // Condition 2: a one-shot deadline that is still in the future keeps the
    // existing idempotent retain behavior and does not reach Collab again.
    let retained = subscribed(false);
    assert!(
        retained.status.success(),
        "{}",
        String::from_utf8_lossy(&retained.stderr)
    );
    let retained_payload: Value = serde_json::from_slice(&retained.stdout).unwrap();
    assert_eq!(retained_payload["subscription_id"], "sub-retained");
    assert_eq!(retained_payload["idempotent"], true);
    let subscribe_calls = fs::read_to_string(root.join("subscribe-calls")).unwrap();
    assert_eq!(subscribe_calls.lines().count(), 1);
    assert!(!root.join("unsubscribe-calls").exists());

    // Condition 1: once the retained one-shot deadline is due, the subscribe
    // must cancel it and arm a fresh future trigger instead of returning it.
    let started = goal_now_ms_for_test();
    let rearmed = subscribed(true);
    assert!(
        rearmed.status.success(),
        "{}",
        String::from_utf8_lossy(&rearmed.stderr)
    );
    let rearmed_payload: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
    assert_eq!(rearmed_payload["subscription_id"], "sub-arm-2");
    assert_ne!(rearmed_payload["idempotent"], true);
    assert_eq!(
        rearmed_payload["collab_subscription"]["subscription_id"],
        "sub-arm-2"
    );
    let rearmed_trigger = rearmed_payload["collab_subscription"]["trigger_times_ms"][0]
        .as_i64()
        .expect("the re-armed subscription must carry a trigger");
    assert!(
        rearmed_trigger > started,
        "the re-armed trigger must be in the future: trigger={rearmed_trigger} started={started}"
    );
    assert!(
        fs::read_to_string(root.join("unsubscribe-calls"))
            .unwrap()
            .contains("sub-retained"),
        "the due retained subscription must be cancelled before re-arming"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_failure_does_not_report_active() {
    let root = temp_root("goal-sub-fail");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        fake_bin.join("collab"),
"#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{\"workers\":[{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}],\"tasks\":[],\"subagents\":[]}' ;;\n  \"master status\") printf '%s\\n' '{\"master\":{\"worker_id\":\"master-peer\",\"endpoint_live\":true}}' ;;\n  \"context \") printf '%s\\n' '{\"identity\":{\"worker_id\":\"master-peer\",\"kind\":\"peer\",\"transport\":{\"kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\",\"tmux_endpoint\":{\"socket_path\":\"/tmp/collab.sock\",\"server_pid\":123,\"tmux_session_id\":\"$1\",\"pane_id\":\"%1\",\"pane_pid\":456}}},\"liveness\":{\"live\":true,\"presence\":\"present\",\"transport_kind\":\"tmux\",\"endpoint\":\"/tmp/collab.sock\"}}' ;;\n  *) printf '%s\\n' 'daemon stopped' >&2; exit 44 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&fake_bin.join("collab"), fs::Permissions::from_mode(0o755)).unwrap();

    let sub = Command::new(binary())
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
    assert_eq!(sub.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&sub.stderr).contains("COLLAB_SUBSCRIBE_FAILED"));
    let sub_json: Value = serde_json::from_slice(&sub.stdout).unwrap();
    assert_eq!(sub_json["active"], false);
    assert_eq!(sub_json["desired"], "recovery_required");
    assert_eq!(sub_json["observed"], "unknown");

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], false);
    assert_eq!(status_json["observed"], "unknown");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_lifecycle_rejects_invalid_subscription_and_preserves_pending_cancel() {
    let root = temp_root("goal-invalid-response");
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
    printf '%s\n' 'not-json'
    ;;
  "notify status")
    printf '%s\n' 'also-not-json'
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
    assert_eq!(subscribed.status.code(), Some(1));
    let subscribe_json: Value = serde_json::from_slice(&subscribed.stdout).unwrap();
    assert_eq!(subscribe_json["active"], false);
    assert_eq!(subscribe_json["desired"], "subscribed");
    assert_eq!(subscribe_json["observed"], "unknown");
    assert!(subscribe_json["error"]
        .as_str()
        .unwrap()
        .contains("COLLAB_SUBSCRIBE_RESPONSE_INVALID"));

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], false);
    assert_eq!(status_json["observed"], "unknown");

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
    assert!(root.join(".appsdk-control/long-task-goal.json").is_file());

    fs::remove_dir_all(root).unwrap();
}
