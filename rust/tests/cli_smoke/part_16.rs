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

    install_goal_test_context(&fake_collab, &root);
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
            r#"{{"schema_version":1,"owner":"master-peer","goal_path":"{}","interval":"10m","active":true,"registered_at":"2026-09-07T00:00:00Z"}}"#,
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
    assert!(text.contains("Peer-first"));
    assert!(text.contains("不以 peer 忙闲为前提"));
    assert!(!text.contains("`[subagent].max_concurrent`（默认 8）"));
    assert!(text.contains("绝不能以 ACK、已读或一段总结结束一轮"));
    assert!(text.contains("collab subagent"));
    assert!(text.contains("旧状态诊断"));
    assert!(text.contains("PROTECTED_HOLD"));
    assert!(text.contains("重新武装"));

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
        .contains("Peer-first"));
    assert!(payload["charter"].as_str().unwrap().contains("Peer-first"));
    assert!(payload["charter"]
        .as_str()
        .unwrap()
        .contains("不以 peer 忙闲为前提"));
    assert!(payload["notification_rules"]
        .as_str()
        .unwrap()
        .contains("重新武装"));
    assert!(payload["fleet_rules"]
        .as_str()
        .unwrap()
        .contains("PROTECTED_HOLD"));
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
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","role":"master","kind":"peer"}}'
    ;;
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
fn longhorizon_peer_hides_project_lists_except_own_delivery_dependencies() {
    let root = temp_root("longhorizon-peer-scope");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    let project_scope = root.canonicalize().unwrap();
    fs::write(
        &fake_collab,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "context ")
    if [ "$ROLE_CASE" = "master" ]; then
      printf '%s\n' '{{"name":"master-peer","identity":{{"worker_id":"master-peer","role":"master","kind":"peer"}},"binding":{{"project_scope":"{}"}}}}'
    else
      printf '%s\n' '{{"name":"peer-a","identity":{{"worker_id":"peer-a","role":"peer","kind":"peer"}},"binding":{{"project_scope":"{}"}}}}'
    fi
    ;;
  "status --all")
    printf '%s\n' '{{"workers":[{{"id":"peer-a","active_task":"task-own","endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"working"}},{{"id":"peer-b","active_task":null,"endpoint_live":false,"identity_valid":false,"suspected_offline":true,"agent_state":"unknown"}}],"tasks":[{{"id":"task-own","status":"working","owner":"peer-a","next_step":"verify"}},{{"id":"task-foreign","status":"working","owner":"peer-b","next_step":"verify"}}],"pending_merges":[{{"task_id":"merge-own","owner":"peer-a","requested_by":"master-peer","status":"accepted","branch":"codex/own","worktree":"playground/own"}},{{"task_id":"merge-foreign","owner":"peer-b","requested_by":"master-peer","status":"accepted","branch":"codex/foreign","worktree":"playground/foreign"}}],"subagents":[]}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
            project_scope.display(),
            project_scope.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        "#!/bin/sh\nprintf '%s\\n' '[{\"human_id\":\"bug-framework\",\"labels\":[\"P0\",\"cli\"],\"title\":\"SDK verify block\"}]'\n",
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let peer = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env_remove("ROLE_CASE")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        peer.status.success(),
        "{}",
        String::from_utf8_lossy(&peer.stderr)
    );
    let peer_payload: Value = serde_json::from_slice(&peer.stdout).unwrap();
    assert_eq!(peer_payload["role"], "peer");
    assert_eq!(
        peer_payload["pending_merges"].as_array().unwrap().len(),
        1
    );
    assert_eq!(peer_payload["pending_merges"][0]["task_id"], "merge-own");
    assert_eq!(
        peer_payload["workers_needing_intervention"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(peer_payload["open_bugs"].as_array().unwrap().is_empty());

    let master = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("ROLE_CASE", "master")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        master.status.success(),
        "{}",
        String::from_utf8_lossy(&master.stderr)
    );
    let master_payload: Value = serde_json::from_slice(&master.stdout).unwrap();
    assert_eq!(master_payload["role"], "master");
    assert_eq!(
        master_payload["pending_merges"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        master_payload["workers_needing_intervention"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(master_payload["open_bugs"].as_array().unwrap().len(), 1);

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
    printf '%s\n' '{"workers":[{"id":"worker-peer","role":"peer","active_task":"task-1","endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"working"}],"tasks":[{"id":"task-1","status":"working","owner":"worker-peer","next_step":"run tests"},{"id":"other-goal","status":"working","owner":"another-peer"},{"id":"foreign-blocker","status":"blocked","owner":"another-peer"}],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"other-peer","endpoint_live":true}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"worker-peer","kind":"peer","role":"peer","transport":{"kind":"appserver","thread_id":"thread-worker-peer"}},"liveness":{"live":true,"transport_kind":"appserver"},"tasks":[{"id":"task-1"}],"inbox":{"unread":0}}'
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
    assert_eq!(payload["role"], "peer");
    assert_eq!(payload["assigned"].as_array().unwrap().len(), 1);
    assert_eq!(payload["assigned"][0]["id"], "task-1");
    assert!(payload["blocked"].as_array().unwrap().is_empty());
    assert!(!payload["charter"]
        .as_str()
        .unwrap()
        .contains("你是本项目的 master"));
    assert!(payload["charter"].as_str().unwrap().contains("peer 不是执行 worker"));

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
fn longhorizon_show_uses_current_context_role_independent_of_transport() {
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
      tmux-master) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","role":"master","transport":{"kind":"tmux","tmux_endpoint":{"pane_id":"%1"}}},"liveness":{"live":true,"transport_kind":"tmux"}}' ;;
      transport-missing) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","role":"master","transport":{"kind":"appserver"}},"liveness":{"live":false,"transport_kind":"appserver"}}' ;;
      worker|mismatch) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","role":"peer"}}' ;;
      subagent) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer"},"role_brief":{"derivation":{"kind":"managed-subagent"}}}' ;;
      missing) printf '%s\n' '{"identity":null}' ;;
      *) printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","role":"master","transport":{"kind":"appserver","thread_id":"thread-current-peer"}},"liveness":{"live":true,"transport_kind":"appserver"}}' ;;
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
        ("worker", "peer"),
        ("subagent", "managed-subagent"),
        ("missing", "unknown"),
        ("mismatch", "peer"),
        ("transport-missing", "master"),
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
fn longhorizon_role_uses_one_context_without_secondary_master_probe() {
    let root = temp_root("longhorizon-context-owner");
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let context = serde_json::json!({"registered":true,"name":"Jenny",
        "binding":{"project_scope":root.canonicalize().unwrap(),"session_id":"current-session"},
        "identity":{"worker_id":"current-master","kind":"peer","role":"master"},
        "role_brief":{"role":"master"},"liveness":{"live":false}});
    let collab = fake_bin.join("collab");
    fs::write(&collab, format!(r#"#!/bin/sh
case "$1 $2" in
  "context ") printf 'context\n' >> context-calls; printf '%s\n' '{context}' ;;
  "status --all") printf '%s\n' '{{"workers":[],"tasks":[],"subagents":[]}}' ;;
  "master status") printf 'forbidden\n' > secondary-master-call; exit 73 ;;
  *) exit 64 ;;
esac
"#)).unwrap();
    fs::set_permissions(&collab, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(binary()).args(["longhorizon","show","--json"])
        .current_dir(&root).env("PATH", &fake_bin).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["role"], "master", "current context owns role despite an unreachable endpoint: {payload}");
    assert!(!root.join("secondary-master-call").exists());
    assert_eq!(fs::read_to_string(root.join("context-calls")).unwrap().lines().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_separates_stale_identity_from_unreachable_worker() {
    let root = temp_root("longhorizon-stale-identity-worker");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-stale_identity}" in
      endpoint) printf '%s\n' '{"workers":[{"id":"current-peer","role":"peer","endpoint_live":false,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      stale_identity) printf '%s\n' '{"workers":[{"id":"current-peer","role":"peer","active_task":"stale-route-invite","endpoint_live":true,"identity_valid":false,"suspected_offline":false}],"tasks":[{"id":"stale-route-invite","status":"working","owner":"current-peer","next_step":"continue orchestration"}],"subagents":[]}' ;;
      offline) printf '%s\n' '{"workers":[{"id":"current-peer","role":"peer","endpoint_live":true,"identity_valid":true,"suspected_offline":true}],"tasks":[],"subagents":[]}' ;;
      missing_worker) printf '%s\n' '{"workers":[],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"other-master","endpoint_live":true}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"current-peer","kind":"peer","role":"worker","transport":{"kind":"appserver","thread_id":"thread-current-peer"}},"liveness":{"live":true,"transport_kind":"appserver"}}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    for (variant, expected_role) in [
        ("endpoint", "peer"),
        ("stale_identity", "peer"),
        ("offline", "peer"),
        ("missing_worker", "peer"),
    ] {
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
        assert_eq!(payload["role"], expected_role, "variant={variant}");
        if variant == "stale_identity" {
            assert_eq!(payload["assigned"][0]["id"], "stale-route-invite");
            assert!(payload["charter"].as_str().unwrap().contains("peer 不是执行 worker"));
        }
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
fn goal_subscription_and_goal_prompt_lifecycle() {
    let root = temp_root("goal-sub");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "goal reconcile")
    printf '%s\n' "$*" > reconcile-args
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
        --at-ms) at="$2"; shift ;;
      esac
      shift
    done
    case "$*" in
      *"--every-ms"*|*"--repeat-count"*) printf '%s\n' 'periodic goal deadline is invalid' >&2; exit 43 ;;
    esac
    if [ "$desired" = "ended" ]; then
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"ended\",\"status\":\"ended\",\"subscription\":null}"
    else
      subject="goal:$goal_id"
      printf '%s' "$subject" > goal-subject
      if [ "${UNARMED_SUBSCRIBE:-}" = "1" ]; then
        printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"pending\",\"subscription\":{\"id\":\"goal-sub-unarmed\",\"status\":\"pending\",\"event\":\"deadline\",\"subject\":\"$subject\"}}"
      else
        printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"goal-sub-1\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}}"
      fi
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
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
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
    let prompt = sub_json["goal_prompt"].as_str().unwrap();
    assert!(prompt.contains("Master / Peer"));
    assert!(prompt.contains("peer 不是执行 worker"));
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
    let reconcile_args = fs::read_to_string(root.join("reconcile-args")).unwrap();
    assert!(reconcile_args.contains("--desired active"));
    assert!(reconcile_args.contains("--at-ms "));
    assert!(!reconcile_args.contains("--every-ms"));
    assert!(!reconcile_args.contains("--repeat-count"));

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

    let record_path = goal_test_record_path(&root);
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
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(cancel_res.status.success());
    assert!(String::from_utf8_lossy(&cancel_res.stdout).contains("goal-sub-1"));

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

    let post_cancel_status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(post_cancel_status.status.success());
    let post_cancel_json: Value = serde_json::from_slice(&post_cancel_status.stdout).unwrap();
    assert_eq!(post_cancel_json["active"], false);
    assert_eq!(post_cancel_json["desired"], "unsubscribed");
    assert_eq!(post_cancel_json["observed"], "cancelled");
    assert_eq!(post_cancel_json["subscription_id"], "goal-sub-1");
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
    assert!(unarmed_json["subscription_id"].is_null());
    assert!(unarmed_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_RECONCILE_ACTIVE_NOT_ARMED"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_owner_gate_does_not_require_master_or_tmux() {
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
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    if [ "$desired" = "ended" ]; then
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"ended\",\"status\":\"ended\",\"subscription\":null}"
    else
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"owner-gate-sub\",\"status\":\"armed\"}}"
    fi
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
    let run = |extra_env: &[(&str, &str)]| {
        if goal_test_record_path(&root).exists() {
            fs::remove_file(goal_test_record_path(&root)).unwrap();
        }
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
    assert!(identity_mismatch.status.success());

    let transport_missing = run(&[("CONTEXT_TRANSPORT_KIND", "mailbox")]);
    assert!(transport_missing.status.success());

    let offline_master = run(&[("MASTER_LIVE", "false")]);
    assert!(offline_master.status.success());

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
fn goal_owner_gate_uses_context_without_status_liveness_fields() {
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
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"missing-fields-sub\",\"status\":\"armed\"}}"
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
    let run = |variant: &str| {
        if goal_test_record_path(&root).exists() {
            fs::remove_file(goal_test_record_path(&root)).unwrap();
        }
        Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("STATUS_VARIANT", variant)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };

    for variant in ["endpoint", "identity", "offline"] {
        let result = run(variant);
        assert!(
            result.status.success(),
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
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    (
      printf '%s' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"large-output-sub\",\"status\":\"armed\"},\"padding\":\""
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
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
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
  "goal reconcile")
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    /bin/sleep 20
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"slow-subscribe-write\",\"status\":\"armed\"}}"
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
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
  "goal reconcile")
    printf '%s\n' "$*" >> goal-reconcile-calls
    printf '%s\n' '{"goal_id":"unexpected","desired":"active","status":"armed","subscription":{"id":"sub-short","status":"armed"}}'
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
            !root.join("goal-reconcile-calls").exists(),
            "a rejected short interval must not reach Collab goal reconciliation"
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
  "goal reconcile")
    printf '%s\n' "$*" > reconcile-args
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
      esac
      shift
    done
    printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"sub-fresh\",\"status\":\"armed\"}}"
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
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

    let reconcile_args = fs::read_to_string(root.join("reconcile-args")).unwrap();
    let at_ms: i64 = reconcile_args
        .split("--at-ms")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("goal reconcile must carry --at-ms")
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
  "goal reconcile")
    reconcile_args="$*"
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --goal-id) goal_id="$2"; shift ;;
        --owner) goal_owner="$2"; shift ;;
        --desired) desired="$2"; shift ;;
        --at-ms) at="$2"; shift ;;
      esac
      shift
    done
    if [ "$desired" = "ended" ]; then
      printf '%s\n' "$reconcile_args" >> ended-calls
      : > cancelled.marker
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"ended\",\"status\":\"ended\",\"subscription\":null}"
    else
      printf '%s\n' "$reconcile_args" >> subscribe-calls
      count=$((`/bin/cat subscribe-count 2>/dev/null || printf '0'` + 1))
      printf '%s' "$count" > subscribe-count
      printf '%s\n' "{\"goal_id\":\"$goal_id\",\"goal_owner\":\"$goal_owner\",\"desired\":\"active\",\"status\":\"armed\",\"subscription\":{\"id\":\"sub-arm-$count\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"goal:$goal_id\",\"trigger_times_ms\":[$at],\"fired_count\":0}}"
    fi
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    install_goal_test_context(&fake_collab, &root);
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
    assert!(!root.join("ended-calls").exists());

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
        rearmed_payload["collab_subscription"]["subscription"]["id"],
        "sub-arm-2"
    );
    let rearmed_trigger = rearmed_payload["collab_subscription"]["subscription"]
        ["trigger_times_ms"][0]
        .as_i64()
        .expect("the re-armed subscription must carry a trigger");
    assert!(
        rearmed_trigger > started,
        "the re-armed trigger must be in the future: trigger={rearmed_trigger} started={started}"
    );
    assert!(
        fs::read_to_string(root.join("ended-calls"))
            .unwrap()
            .contains("--desired ended"),
        "the due retained goal occurrence must be ended before re-arming"
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

    install_goal_test_context(&fake_bin.join("collab"), &root);

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
    assert!(
        String::from_utf8_lossy(&sub.stderr).contains("GOAL_RECONCILE_ACTIVE_FAILED")
    );
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
  "goal reconcile")
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

    install_goal_test_context(&fake_collab, &root);
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
    assert_eq!(subscribe_json["desired"], "recovery_required");
    assert_eq!(subscribe_json["observed"], "unknown");
    assert!(subscribe_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_RECONCILE_RESPONSE_INVALID"));

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
    assert!(goal_test_record_path(&root).is_file());

    fs::remove_dir_all(root).unwrap();
}
#[test]
fn goal_owner_scoped_peer_native_and_master_lifecycle() {
    let root = temp_root("goal-owner-scoped");
    fs::create_dir_all(root.join("fake-bin")).unwrap();
    fs::write(root.join("goal.md"), "# Shared document\nDeliver own goal").unwrap();
    let script = root.join("fake-bin/collab");
    fs::write(&script, r#"#!/bin/sh
case "$1 $2" in
 "context ") printf '{"result":{"snapshot":{"binding":{"project_scope":"%s"},"name":"%s","session_id":"%s","identity":{"worker_id":"carrier-%s","role":"%s","kind":"peer","transport":{"kind":"appserver","thread_id":"native-thread"}},"role_brief":{"derivation":{"kind":"%s"}}}}}\n' "$PROJECT" "$OWNER" "${SESSION:-first}" "${CARRIER:-$OWNER}" "${ROLE:-peer}" "${DERIVATION:-peer}" ;;
 "status --all") printf '{"workers":[{"id":"%s","role":"peer","endpoint_live":true,"suspected_offline":false}],"tasks":[]}' "$OWNER" ;;
 "master status") printf '{"master":{"worker_id":"Jenny","endpoint_live":true}}' ;;
 "goal reconcile")
   while [ "$#" -gt 0 ]; do
     case "$1" in
       --goal-id) goal_id="$2"; shift ;;
       --owner) goal_owner="$2"; shift ;;
       --desired) desired="$2"; shift ;;
     esac
     shift
   done
   subject="goal:$goal_id"
   printf '%s' "$subject" > "$PROJECT/$OWNER.subject"
   if [ "$desired" = "ended" ]; then
     printf cancelled > "$PROJECT/$OWNER.state"
     printf '{"goal_id":"%s","goal_owner":"%s","desired":"ended","status":"ended","subscription":null}' "$goal_id" "$goal_owner"
   else
     printf 'armed' > "$PROJECT/$OWNER.state"
     printf '{"goal_id":"%s","goal_owner":"%s","desired":"active","status":"armed","subscription":{"id":"%s-sub","status":"armed","event":"deadline","subject":"%s"}}' "$goal_id" "$goal_owner" "$OWNER" "$subject"
   fi
   ;;
 "notify status")
   [ "${STATUS_FAIL:-}" = 1 ] && exit 42
   subject=$(/bin/cat "$PROJECT/$OWNER.subject" 2>/dev/null)
   state=$(/bin/cat "$PROJECT/$OWNER.state" 2>/dev/null)
   printf '{"subscriptions":[{"id":"%s-sub","owner":"%s","status":"%s","subject":"%s","event":"deadline"}]}' "$OWNER" "${REMOTE_OWNER:-$OWNER}" "${state:-armed}" "$subject" ;;
 *) exit 64 ;;
esac
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let run = |cwd: &Path, owner: &str, args: &[&str]| {
        let out = Command::new(binary()).args(args).current_dir(cwd)
            .env("PATH", root.join("fake-bin")).env("PROJECT", &root)
            .env("OWNER", owner).env("SESSION", "continued-session").output().unwrap();
        assert!(out.status.success(), "{} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let peer = run(&root, "Alice", &["goal", "subscribe", "--goal", "goal.md", "--json"]);
    assert_eq!(peer["owner"], "Alice");
    let master = run(&root, "Jenny", &["goal", "subscribe", "--goal", "goal.md", "--json"]);
    assert_eq!(master["owner"], "Jenny");
    let scoped_path = |owner: &str| root.join(".appsdk-control/goals")
        .join(owner.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
        .join("long-task-goal.json");
    let master_bytes = fs::read(scoped_path("Jenny")).unwrap();
    let failed_status = Command::new(binary()).args(["goal", "status", "--json"])
        .current_dir(&root).env("PATH", root.join("fake-bin"))
        .env("PROJECT", &root).env("OWNER", "Alice").env("STATUS_FAIL", "1").output().unwrap();
    let failed: Value = serde_json::from_slice(&failed_status.stdout).unwrap();
    assert_eq!(failed["desired"], "recovery_required");
    assert_eq!(fs::read(scoped_path("Jenny")).unwrap(), master_bytes);
    let nested = root.join("worktree");
    fs::create_dir_all(&nested).unwrap();
    let continued = run(&nested, "Alice", &["goal", "status", "--json"]);
    assert_eq!(continued["record"]["owner"], "Alice");
    assert_eq!(continued["subscription_id"], "Alice-sub");
    run(&nested, "Alice", &["goal", "cancel", "--json"]);
    let untouched = run(&root, "Jenny", &["goal", "status", "--json"]);
    assert_eq!(untouched["active"], true);
    assert_eq!(untouched["subscription_id"], "Jenny-sub");
    run(&root, "Alice", &["goal", "subscribe", "--goal", "goal.md", "--json"]);
    assert_eq!(run(&root, "Alice", &["goal", "status", "--json"])["active"], true);
    // Own-name continuation uses the current carrier but retains the goal.
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(scoped_path("Alice")).unwrap()).unwrap()["session_id"], "continued-session");
    // A foreign legacy singleton never becomes this peer's goal.
    let legacy_path = root.join(".appsdk-control/long-task-goal.json");
    let mut legacy_record: Value = serde_json::from_slice(&master_bytes).unwrap();
    legacy_record["owner"] = serde_json::json!("carrier-Jenny");
    let legacy_bytes = serde_json::to_vec(&legacy_record).unwrap();
    fs::write(&legacy_path, &legacy_bytes).unwrap();
    assert_eq!(run(&root, "Bob", &["goal", "status", "--json"])["error"], "GOAL_RECORD_NOT_FOUND");
    assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
    // Matching legacy reads are projections only; an explicit cancel promotes
    // the retained subscription to the scoped record and preserves the source.
    fs::remove_file(scoped_path("Jenny")).unwrap();
    let legacy_status = run(&root, "Jenny", &["goal", "status", "--json"]);
    assert_eq!(legacy_status["record"]["record_source"], "legacy-read-only");
    assert_eq!(legacy_status["record"]["owner"], "Jenny");
    assert!(!scoped_path("Jenny").exists());
    run(&root, "Jenny", &["goal", "cancel", "--json"]);
    assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(scoped_path("Jenny")).unwrap()).unwrap()["observed"], "cancelled");
    // Scoped owner mismatches fail before reconciliation or remote writes.
    fs::write(scoped_path("Alice"), &master_bytes).unwrap();
    let mismatch = Command::new(binary()).args(["goal", "status", "--json"])
        .current_dir(&root).env("PATH", root.join("fake-bin"))
        .env("PROJECT", &root).env("OWNER", "Alice").output().unwrap();
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("GOAL_RECORD_OWNER_MISMATCH"));
    assert_eq!(fs::read(scoped_path("Alice")).unwrap(), master_bytes);
    // The owner's held lock blocks only that owner. Independent names can
    // register concurrently, even when they use the same goal document.
    let held = OpenOptions::new().create(true).read(true).write(true)
        .open(scoped_path("Alice").with_file_name("long-task-goal.lock")).unwrap();
    hold_advisory_lock(&held);
    let command = |owner: &str, args: &[&str]| {
        let mut cmd = Command::new(binary());
        cmd.args(args).current_dir(&root).env("PATH", root.join("fake-bin"))
            .env("PROJECT", &root).env("OWNER", owner)
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        cmd
    };
    let bob = command("Bob", &["goal", "subscribe", "--goal", "goal.md", "--json"]).spawn().unwrap();
    let carol = command("Carol", &["goal", "subscribe", "--goal", "goal.md", "--json"]).spawn().unwrap();
    let busy = command("Alice", &["goal", "status", "--json"]).output().unwrap();
    assert!(String::from_utf8_lossy(&busy.stderr).contains("GOAL_LOCK_BUSY"));
    assert!(bob.wait_with_output().unwrap().status.success());
    assert!(carol.wait_with_output().unwrap().status.success());
    drop(held);
    let continued = command("Bob", &["goal", "status", "--json"])
        .env("SESSION", "second-session").env("CARRIER", "new-worker").output().unwrap();
    assert!(continued.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&continued.stdout).unwrap()["record"]["session_id"], "second-session");
    let native_brief = run(&root, "Bob", &["longhorizon", "show", "--json"]);
    assert_eq!(native_brief["role"], "peer");
    assert_eq!(native_brief["goal"]["owner"], "Bob");
    assert!(native_brief["charter"].as_str().unwrap().contains("peer 不是执行 worker"));
    let foreign_remote = command("Bob", &["goal", "status", "--json"])
        .env("REMOTE_OWNER", "Jenny").output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&foreign_remote.stdout).unwrap()["active"], false);
    assert_eq!(fs::read(root.join("Bob.state")).unwrap(), b"armed");
    // Unknown / managed callers cannot mutate, including via prompt.
    for args in [vec!["goal", "subscribe", "--goal", "goal.md", "--json"],
        vec!["goal", "status", "--json"], vec!["goal", "cancel", "--json"],
        vec!["goal", "prompt", "--goal", "goal.md"]] {
        let denied = command("Dave", &args).env("ROLE", "managed-subagent").output().unwrap();
        assert!(!denied.status.success());
        assert!(String::from_utf8_lossy(&denied.stderr).contains("GOAL_OWNER_MANAGED_SUBAGENT"));
        let derived = command("Dave", &args).env("DERIVATION", "managed-subagent").output().unwrap();
        assert!(!derived.status.success());
        assert!(String::from_utf8_lossy(&derived.stderr).contains("GOAL_OWNER_MANAGED_SUBAGENT"));
        let unknown = command("", &args).output().unwrap();
        assert!(!unknown.status.success());
    }
    assert!(!scoped_path("Dave").exists());
    let managed_brief = command("Dave", &["longhorizon", "show", "--json"])
        .env("DERIVATION", "managed-subagent").output().unwrap();
    assert!(managed_brief.status.success());
    let managed_brief: Value = serde_json::from_slice(&managed_brief.stdout).unwrap();
    assert_eq!(managed_brief["role"], "managed-subagent");
    assert_eq!(managed_brief["goal"]["registered"], false);
    // Unowned/malformed legacy data cannot be imported, but it is not Dave's
    // goal and must not block his explicitly requested independent goal.
    for legacy in [br#"{"active":true,"goal_id":"old"}"#.as_slice(), b"{broken".as_slice()] {
        fs::write(&legacy_path, legacy).unwrap();
        let absent = run(&root, "Dave", &["goal", "status", "--json"]);
        assert_eq!(absent["active"], false);
        let own = run(&root, "Dave", &["goal", "subscribe", "--goal", "goal.md", "--json"]);
        assert_eq!(own["owner"], "Dave");
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy);
        run(&root, "Dave", &["goal", "cancel", "--json"]);
        fs::remove_file(scoped_path("Dave")).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

// Lifecycle fixtures use the current authenticated context contract. Their
// notify responses remain local to each failure-mode test.
fn install_goal_test_context(collab: &Path, project: &Path) {
    let delegate = collab.with_extension("delegate");
    fs::rename(collab, &delegate).unwrap();
    let context = serde_json::json!({
        "result": {"snapshot": {
            "name": "master-peer", "session_id": "test-session",
            "binding": {"project_scope": project.canonicalize().unwrap()},
            "identity": {"worker_id": "master-peer", "role": "master", "kind": "peer",
                "transport": {"kind": "appserver", "thread_id": "test-thread"}}
        }}
    }).to_string();
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    fs::write(collab, format!("#!/bin/sh\nif [ \"$1\" = context ]; then printf '%s\\n' {}; else exec {} \"$@\"; fi\n",
        quote(&context), quote(&delegate.to_string_lossy()))).unwrap();
    fs::set_permissions(collab, fs::Permissions::from_mode(0o755)).unwrap();
}

fn goal_test_record_path(root: &Path) -> PathBuf {
    root.join(".appsdk-control/goals/6d61737465722d70656572/long-task-goal.json")
}
