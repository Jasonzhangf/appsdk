use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

const NEXT_CARRIER: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

fn input_has_message(input: &Value, id: &str) -> bool {
    input["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .chain(input["toolOutput"]["output"].as_str())
        .any(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .any(|token| token == id)
        })
}

// Observe native submission before recv can consume the durable inbox.
fn native_deadline(
    f: &AppFixture,
    thread: &str,
    subscription: &str,
    timeout: Duration,
) -> Result<String, String> {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        let events: Vec<Value> = std::fs::read_to_string(f.name_journal())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if let Some(id) = events
            .iter()
            .find(|e| e["ev"] == "WakeBound" && e["subscription_id"] == subscription)
            .and_then(|e| e["message_id"].as_str())
        {
            let accepted = events.iter().any(|e| {
                e["ev"] == "NotificationDeliveryAccepted"
                    && e["message_id"] == id
                    && e["evidence"].is_object()
            });
            let inputs = f.state.lock().unwrap();
            if accepted
                && inputs.get(thread).is_some_and(|s| {
                    s.inputs
                        .iter()
                        .any(|input| input["threadId"] == thread && input_has_message(input, id))
                })
            {
                return Ok(id.into());
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "native deadline missing: {subscription} -> {thread}"
    ))
}

fn appsdk_goal(fixture: &AppFixture, root: &std::path::Path, thread: &str, args: &[&str]) -> Value {
    let appsdk =
        std::env::var_os("APPSDK_GOAL_CONSUMER_BINARY").expect("explicit AppSDK consumer binary");
    let mut paths = vec![PathBuf::from(binary()).parent().unwrap().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = Command::new(appsdk)
        .args(args)
        .current_dir(root)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("HOME", fixture.root.join("home"))
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("COLLAB_APPSERVER_SOCKET", &fixture.app_socket)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", thread)
        .env("CODEX_THREAD_ID", thread)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_WORKER")
        .env("CODEX_HOME", fixture.root.join("home"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "goal {args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "set APPSDK_GOAL_CONSUMER_BINARY and run explicitly"]
fn appsdk_linked_worktree_legacy_goal_is_cancelled_without_orphan() {
    let fixture = AppFixture::new();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&fixture.project)
            .status()
            .unwrap()
            .success());
    }
    let linked = fixture.root.join("linked");
    assert!(Command::new("git")
        .args(["worktree", "add", "-qb", "legacy", linked.to_str().unwrap()])
        .current_dir(&fixture.project)
        .status()
        .unwrap()
        .success());
    fixture.run_public(&["context"], THREAD_A);
    std::fs::write(linked.join("goal.md"), "# Legacy worktree goal\n").unwrap();
    let run = |args: &[&str]| appsdk_goal(&fixture, &linked, THREAD_A, args);
    let first = run(&[
        "goal",
        "subscribe",
        "--goal",
        "goal.md",
        "--interval",
        "2m",
        "--json",
    ]);
    let encoded: String = first["owner"]
        .as_str()
        .unwrap()
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect();
    let canonical = fixture
        .project
        .join(".appsdk-control/goals")
        .join(encoded)
        .join("long-task-goal.json");
    let bytes = std::fs::read(&canonical).unwrap();
    std::fs::create_dir_all(linked.join(".appsdk-control")).unwrap();
    let legacy = linked.join(".appsdk-control/long-task-goal.json");
    std::fs::write(&legacy, &bytes).unwrap();
    std::fs::remove_file(&canonical).unwrap();
    let mut foreign: Value = serde_json::from_slice(&bytes).unwrap();
    foreign["owner"] = json!("another-owner");
    let foreign_bytes = serde_json::to_vec(&foreign).unwrap();
    std::fs::write(&legacy, &foreign_bytes).unwrap();
    assert_eq!(
        run(&["goal", "status", "--json"])["error"],
        "GOAL_RECORD_NOT_FOUND"
    );
    assert!(!canonical.exists());
    assert_eq!(std::fs::read(&legacy).unwrap(), foreign_bytes);
    std::fs::write(&legacy, &bytes).unwrap();
    assert_eq!(
        run(&["longhorizon", "show", "--json"])["goal"]["owner"],
        first["owner"]
    );
    let status = run(&["goal", "status", "--json"]);
    assert_eq!(
        status["subscription_id"], first["subscription_id"],
        "{status}"
    );
    assert_eq!(status["active"], true);
    run(&["goal", "cancel", "--json"]);
    let second = run(&[
        "goal",
        "subscribe",
        "--goal",
        "goal.md",
        "--interval",
        "2m",
        "--json",
    ]);
    let subscriptions = fixture.run_public(&["notify", "status"], THREAD_A);
    let active: Vec<_> = subscriptions["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["event"] == "deadline" && s["status"] == "armed")
        .collect();
    assert_eq!(active.len(), 1, "{subscriptions}");
    assert_eq!(active[0]["id"], second["subscription_id"]);
    assert_eq!(std::fs::read(legacy).unwrap(), bytes);
    run(&["goal", "cancel", "--json"]);
}

#[test]
#[ignore = "set APPSDK_GOAL_CONSUMER_BINARY and run explicitly"]
fn appsdk_retained_historical_basename_subject_cancels_and_rearms_without_duplicate() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    std::fs::write(
        fixture.project.join("long-task.md"),
        "# Historical basename goal\n",
    )
    .unwrap();
    let first = appsdk_goal(
        &fixture,
        &fixture.project,
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    let owner = first["owner"].as_str().unwrap().to_owned();
    let goal_id = first["goal_id"].as_str().unwrap().to_owned();
    let canonical_id = first["subscription_id"].as_str().unwrap().to_owned();
    let context = fixture.run_public(&["context"], THREAD_A);
    let worker_id = context["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let encoded: String = owner.bytes().map(|b| format!("{b:02x}")).collect();
    let record_path = fixture
        .project
        .join(".appsdk-control/goals")
        .join(encoded)
        .join("long-task-goal.json");
    let mut record: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let historical_subject = "goal:long-task.md";
    let historical_id = "sub-legacy-goal";
    let mut historical = first["collab_subscription"]["subscription"].clone();
    historical["id"] = json!(historical_id);
    historical["worker_id"] = json!(worker_id);
    historical["subject"] = json!(historical_subject);
    historical["trigger_ms"] = json!(now + 600_000);
    historical["trigger_times_ms"] = json!([]);
    historical["interval_ms"] = Value::Null;
    historical["repeat_count"] = json!(1);
    historical["fired_count"] = json!(0);
    historical["expires_ms"] = json!(now + 86_400_000);
    historical["status"] = json!("armed");
    historical["status_reason"] = Value::Null;
    fixture
        .append_name_events(&[json!({"ev": "NotificationSubscribed", "subscription": historical})]);

    record["subject"] = json!(historical_subject);
    record["subscription_id"] = json!(historical_id);
    record["collab_subscription"] = json!({"subscription_id": historical_id});
    record["collab_subscribed"] = json!(true);
    record["desired"] = json!("subscribed");
    record["observed"] = json!("subscribed");
    record["active"] = json!(true);
    record["error"] = Value::Null;
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let before_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        before_cancel["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == historical_id && s["status"] == "armed"),
        "the retained historical occurrence must be armed: {before_cancel}"
    );

    let cancelled = appsdk_goal(
        &fixture,
        &fixture.project,
        THREAD_A,
        &["goal", "cancel", "--json"],
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    let retired = cancelled["record"]["cancel_receipt"]["retired_subscription_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        retired
            .iter()
            .any(|id| id.as_str() == Some(canonical_id.as_str())),
        "cancel must retire the canonical occurrence: {cancelled}"
    );
    assert!(
        retired.iter().any(|id| id.as_str() == Some(historical_id)),
        "cancel must retire the retained historical occurrence: {cancelled}"
    );
    let after_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        after_cancel["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["event"] != "deadline" || !matches!(s["status"].as_str(), Some("armed"))),
        "cancel must leave no armed goal occurrence: {after_cancel}"
    );

    // Seed an already-due retained occurrence. The next subscribe must retire
    // that exact subject and leave exactly one fresh canonical occurrence.
    let due_id = "sub-legacy-goal-due";
    let mut due = historical;
    due["id"] = json!(due_id);
    due["trigger_ms"] = json!(now - 1);
    due["status"] = json!("armed");
    due["status_reason"] = json!("deadline-master-busy-skipped");
    fixture.append_name_events(&[json!({"ev": "NotificationSubscribed", "subscription": due})]);

    record["subject"] = json!(historical_subject);
    record["subscription_id"] = json!(due_id);
    record["collab_subscription"] = json!({"subscription_id": due_id});
    record["desired"] = json!("subscribed");
    record["observed"] = json!("subscribed");
    record["active"] = json!(true);
    record["error"] = Value::Null;
    record["remote_state"] = json!("armed");
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let rearmed = appsdk_goal(
        &fixture,
        &fixture.project,
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    assert_eq!(rearmed["active"], true, "{rearmed}");
    assert_eq!(rearmed["desired"], "subscribed", "{rearmed}");
    assert_ne!(
        rearmed["subscription_id"].as_str(),
        Some(due_id),
        "{rearmed}"
    );

    fixture.restart_daemon();
    let after_rearm = fixture.run_public(&["notify", "status"], THREAD_A);
    let active_goals = after_rearm["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["event"] == "deadline" && s["status"] == "armed")
        .collect::<Vec<_>>();
    assert_eq!(active_goals.len(), 1, "{after_rearm}");
    assert_eq!(
        active_goals[0]["subject"],
        format!("goal:{goal_id}"),
        "{after_rearm}"
    );
    assert!(
        after_rearm["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == due_id && s["status"] == "cancelled"),
        "due rearm must retire the historical occurrence: {after_rearm}"
    );
}

#[test]
fn rejected_native_deadline_does_not_pass_observation() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .reject_input = true;
    let soon = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 200)
        .to_string();
    let own = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:reject",
            "--at-ms",
            &soon,
            "--ttl-seconds",
            "30",
        ],
        THREAD_A,
    );
    assert!(native_deadline(
        &fixture,
        THREAD_A,
        own["subscription"]["id"].as_str().unwrap(),
        Duration::from_secs(2)
    )
    .is_err());
    let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_A);
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["subject"] == "deadline:goal:reject"),
        "durable consumption alone would pass: {received}"
    );
}

// Cross-product acceptance is run explicitly with the candidate or installed
// AppSDK binary. Collab's standalone CI does not build the independent package.
#[test]
#[ignore = "set APPSDK_GOAL_CONSUMER_BINARY and run this cross-product acceptance explicitly"]
fn appsdk_master_and_peer_goals_use_real_daemon_and_native_delivery() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &["master", "promote", "--approval", "cross-product own goals"],
        THREAD_A,
    );
    let goal = |thread: &str, args: &[&str]| appsdk_goal(&fixture, &fixture.project, thread, args);
    std::fs::write(
        fixture.project.join("master.md"),
        "# Project orchestration\n",
    )
    .unwrap();
    std::fs::write(
        fixture.project.join("peer.md"),
        "# Delegated orchestration\n",
    )
    .unwrap();
    let master = goal(
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "master.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    let owned = goal(
        THREAD_B,
        &[
            "goal",
            "subscribe",
            "--goal",
            "peer.md",
            "--interval",
            "2m",
            "--json",
        ],
    );
    assert_ne!(master["owner"], owned["owner"]);
    assert_ne!(master["subscription_id"], owned["subscription_id"]);
    let peer_view = goal(THREAD_B, &["longhorizon", "show", "--json"]);
    assert_eq!(peer_view["goal"]["owner"], owned["owner"], "{peer_view}");
    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":std::fs::canonicalize(&fixture.project).unwrap(),
        "worker_id":peer["identity"]["worker_id"], "session_id":THREAD_B,
        "presence":"online", "reason":"fixture current carrier", "observed_ms":1
    }})]);
    let goal = |thread: &str, args: &[&str]| appsdk_goal(&fixture, &fixture.project, thread, args);
    let intent = json!({"action":"restore_name", "requested_name":owned["owner"], "authorization":"fixture same-name continuation", "replace_confirmed":true}).to_string();
    let next_peer = fixture.run_public(&["context", "--approve-grant", &intent], NEXT_CARRIER);
    let continued = goal(NEXT_CARRIER, &["goal", "status", "--json"]);
    assert_eq!(continued["active"], true, "{continued}");
    assert_eq!(continued["subscription_id"], owned["subscription_id"]);
    let remote = fixture.run_public(&["notify", "status"], NEXT_CARRIER);
    let transferred = remote["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == owned["subscription_id"])
        .unwrap();
    for field in [
        "id",
        "trigger_ms",
        "trigger_times_ms",
        "interval_ms",
        "repeat_count",
        "fired_count",
        "expires_ms",
        "created_ms",
        "status",
    ] {
        assert_eq!(
            transferred[field], owned["collab_subscription"]["subscription"][field],
            "changed {field}"
        );
    }
    assert_eq!(transferred["target"], NEXT_CARRIER);
    let old_cancel = fixture.command_public(
        &[
            "notify",
            "unsubscribe",
            owned["subscription_id"].as_str().unwrap(),
        ],
        THREAD_B,
    );
    assert!(
        !old_cancel.status.success(),
        "old carrier must not cancel transferred subscription"
    );
    let native_id = native_deadline(
        &fixture,
        NEXT_CARRIER,
        owned["subscription_id"].as_str().unwrap(),
        Duration::from_secs(130),
    )
    .unwrap();
    assert!(!fixture
        .state
        .lock()
        .unwrap()
        .get(THREAD_B)
        .unwrap()
        .inputs
        .iter()
        .any(|input| input.to_string().contains(&native_id)));
    let received = fixture.run_public(&["recv", "--timeout", "5"], NEXT_CARRIER);
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == native_id));
    let consumed = fixture.run_public(&["msg", &native_id], NEXT_CARRIER);
    assert_eq!(consumed["state"], "read", "{consumed}");
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["subject"]
                .as_str()
                .is_some_and(|s| s.starts_with("deadline:goal:"))),
        "peer goal deadline must reach the peer: {received}"
    );
    let rearmed = goal(
        NEXT_CARRIER,
        &[
            "goal",
            "subscribe",
            "--goal",
            "peer.md",
            "--interval",
            "2m",
            "--json",
        ],
    );
    assert_ne!(rearmed["subscription_id"], owned["subscription_id"]);
    goal(NEXT_CARRIER, &["goal", "cancel", "--json"]);
    let master_status = goal(THREAD_A, &["goal", "status", "--json"]);
    assert_eq!(master_status["active"], true, "{master_status}");
    assert_eq!(master_status["subscription_id"], master["subscription_id"]);
    let peer_id = next_peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &[
            "send",
            "--to",
            peer_id,
            "--subject",
            "cross-product-next-goal",
            "next goal",
        ],
        THREAD_A,
    );
    let next = fixture.run_public(&["recv", "--timeout", "5"], NEXT_CARRIER);
    assert!(
        next["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["subject"] == "cross-product-next-goal"),
        "direct-message receive must survive goal cancellation: {next}"
    );
}

#[test]
#[ignore = "set APPSDK_GOAL_CONSUMER_BINARY and run explicitly"]
fn appsdk_goals_arm_only_master_project_patrol() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "own-goal project patrol arm",
        ],
        THREAD_A,
    );
    std::fs::write(
        fixture.project.join("master.md"),
        "# Project orchestration\n",
    )
    .unwrap();
    std::fs::write(
        fixture.project.join("peer.md"),
        "# Delegated orchestration\n",
    )
    .unwrap();
    let goal = |thread: &str, args: &[&str]| appsdk_goal(&fixture, &fixture.project, thread, args);
    // Only the standing master-idle/project-patrol/-900000 record counts; the
    // caller's own goal deadline and the daemon direct-message lease do not.
    let project_patrols = |status: &Value| -> Vec<Value> {
        status["subscriptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|subscription| {
                subscription["event"] == "master-idle"
                    && subscription["subject"] == "project-patrol"
                    && subscription["interval_ms"] == 900_000
            })
            .cloned()
            .collect()
    };

    // The real AppSDK goal init must arm the master's standing project patrol.
    let master = goal(
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "master.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    assert_eq!(master["patrol_source"], "armed", "{master}");
    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    let armed = project_patrols(&master_status);
    assert_eq!(
        armed.len(),
        1,
        "exactly one armed project-patrol subscription: {master_status}"
    );
    assert_eq!(armed[0]["status"], "armed", "{master_status}");
    let patrol_id = armed[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        master["patrol_subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{master}"
    );
    let deadline_id = master["subscription_id"].as_str().unwrap().to_owned();

    // A peer goal must not arm a project patrol of its own.
    let peer = goal(
        THREAD_B,
        &[
            "goal",
            "subscribe",
            "--goal",
            "peer.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    assert_ne!(master["owner"], peer["owner"]);
    assert_ne!(peer["patrol_armed"], true, "{peer}");
    let peer_status = fixture.run_public(&["notify", "status"], THREAD_B);
    assert!(
        project_patrols(&peer_status).is_empty(),
        "peer notify status must not contain project-patrol: {peer_status}"
    );

    // Restoring the existing master goal keeps the same standing resource.
    let restored = goal(
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "master.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    assert_eq!(
        restored["patrol_subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{restored}"
    );
    let restored_status = fixture.run_public(&["notify", "status"], THREAD_A);
    let restored_patrols = project_patrols(&restored_status);
    assert_eq!(
        restored_patrols.len(),
        1,
        "one valid project-patrol resource after restore: {restored_status}"
    );
    assert_eq!(restored_patrols[0]["id"].as_str(), Some(patrol_id.as_str()));

    // Cancelling the goal's finite deadline retires that deadline but must keep
    // the standing patrol and the daemon direct-message lease armed.
    let cancelled = goal(THREAD_A, &["goal", "cancel", "--json"]);
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    let after_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    let deadline_retired = after_cancel["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["id"] == deadline_id.as_str())
        .is_none_or(|subscription| subscription["status"] != "armed");
    assert!(
        deadline_retired,
        "goal cancel must retire the finite deadline: {after_cancel}"
    );
    let standing = project_patrols(&after_cancel);
    assert_eq!(
        standing.len(),
        1,
        "standing project patrol retained after goal cancel: {after_cancel}"
    );
    assert_eq!(standing[0]["id"].as_str(), Some(patrol_id.as_str()));
    assert_eq!(standing[0]["status"], "armed", "{after_cancel}");
    assert!(
        has_direct_message_subscription(&after_cancel),
        "direct-message lease retained after goal cancel: {after_cancel}"
    );

    // Cancelling the peer goal must not touch the master's standing resource.
    let peer_cancel = goal(THREAD_B, &["goal", "cancel", "--json"]);
    assert_eq!(peer_cancel["status"], "cancelled", "{peer_cancel}");
    let after_peer_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    let still_standing = project_patrols(&after_peer_cancel);
    assert_eq!(
        still_standing.len(),
        1,
        "master standing patrol unaffected by peer goal cancel: {after_peer_cancel}"
    );
    assert_eq!(still_standing[0]["id"].as_str(), Some(patrol_id.as_str()));
    assert_eq!(still_standing[0]["status"], "armed", "{after_peer_cancel}");

    // A cancelled standing project patrol is a durable decision, not a missing
    // resource. The real AppSDK consumer must report the cancelled source and
    // must not rebuild an armed patrol behind the master's back.
    let promoted = fixture.run_public(
        &[
            "ceo",
            "promote",
            "--approval",
            "fixture user records durable project progress",
        ],
        THREAD_A,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let progress = fixture.run_public(
        &[
            "ceo",
            "reply",
            "--subject",
            "appsdk-goal-patrol-progress",
            "--urgent",
            "no actionable project work remains",
        ],
        THREAD_A,
    );
    assert_eq!(progress["recorded"], true, "{progress}");
    assert!(
        progress["revision"]
            .as_u64()
            .is_some_and(|revision| revision >= 1),
        "the CEO reply must record a durable project revision: {progress}"
    );
    let cancelled_patrol = fixture.run_public(
        &[
            "notify",
            "cancel",
            "--event",
            "master-idle",
            "--reason",
            "fixture no actionable project work",
            "--resume-condition",
            "new project work or a durable wait release",
        ],
        THREAD_A,
    );
    assert_eq!(
        cancelled_patrol["status"], "cancelled",
        "{cancelled_patrol}"
    );
    assert_eq!(cancelled_patrol["changed"], true, "{cancelled_patrol}");
    assert_eq!(
        cancelled_patrol["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled_patrol}"
    );

    let cancelled_goal = goal(
        THREAD_A,
        &[
            "goal",
            "subscribe",
            "--goal",
            "master.md",
            "--interval",
            "5m",
            "--json",
        ],
    );
    assert_eq!(
        cancelled_goal["patrol_source"], "cancelled",
        "{cancelled_goal}"
    );
    assert_eq!(
        cancelled_goal["patrol_status"], "cancelled",
        "{cancelled_goal}"
    );
    assert_eq!(cancelled_goal["patrol_armed"], false, "{cancelled_goal}");
    assert_eq!(
        cancelled_goal["patrol_subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled_goal}"
    );
    let cancelled_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        cancelled_status["project_patrol"]["status"], "cancelled",
        "{cancelled_status}"
    );
    assert_eq!(
        cancelled_status["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled_status}"
    );
    let cancelled_records = project_patrols(&cancelled_status);
    assert_eq!(
        cancelled_records.len(),
        1,
        "the consumer must not duplicate the cancelled patrol: {cancelled_status}"
    );
    assert_eq!(
        cancelled_records[0]["status"], "cancelled",
        "{cancelled_status}"
    );

    // The same durable record is restored by an explicit exact subscribe.
    let restored_patrol = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "master-idle",
            "--subject",
            "project-patrol",
            "--every-ms",
            "900000",
            "--repeat-count",
            "100",
            "--ttl-seconds",
            "604800",
        ],
        THREAD_A,
    );
    assert_eq!(restored_patrol["status"], "rearmed", "{restored_patrol}");
    assert_eq!(
        restored_patrol["subscription"]["id"].as_str(),
        Some(patrol_id.as_str()),
        "{restored_patrol}"
    );
}

#[test]
fn peer_goal_deadline_is_delivered_consumed_and_cancelled_without_master_proxy() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "own-goal consumer"],
        THREAD_A,
    );
    let later = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000)
        .to_string();
    let master = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:project",
            "--at-ms",
            &later,
            "--ttl-seconds",
            "120",
        ],
        THREAD_A,
    );
    let master_sub = master["subscription"]["id"].as_str().unwrap();
    let soon = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 200)
        .to_string();
    let own = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:peer-owned",
            "--at-ms",
            &soon,
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );
    let peer_sub = own["subscription"]["id"].as_str().unwrap();
    assert_ne!(master_sub, peer_sub);
    assert_eq!(own["subscription"]["worker_id"], peer_id);
    let native_id = native_deadline(&fixture, THREAD_B, peer_sub, Duration::from_secs(5)).unwrap();
    let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_B);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == "deadline:goal:peer-owned")
        .expect("peer must receive its own goal deadline");
    assert_eq!(message["to"], peer_id);
    assert_eq!(message["id"], native_id);
    let status = fixture.run_public(&["notify", "status"], THREAD_B);
    let subscription = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == peer_sub)
        .unwrap();
    assert_eq!(subscription["status"], "consumed");
    assert_eq!(subscription["fired_count"], 1);
    let removed_close = fixture.command_public(&["notify", "unsubscribe", peer_sub], THREAD_B);
    assert!(
        !removed_close.status.success(),
        "public subscription-closing command must be removed"
    );
    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        master_status["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == master_sub)
            .unwrap()["status"],
        "armed"
    );
    fixture.run_public(
        &[
            "send",
            "--to",
            peer_id,
            "--subject",
            "after-own-goal-cancel",
            "new assignment",
        ],
        THREAD_A,
    );
    let after = fixture.run_public(&["recv", "--timeout", "5"], THREAD_B);
    assert!(
        after["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["subject"] == "after-own-goal-cancel"),
        "goal cleanup must preserve direct-message reception: {after}"
    );
}

#[test]
fn role_patrol_peer_deadline_is_finite_owned_and_consumable() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "private patrol test"],
        THREAD_A,
    );
    let subscribed = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "patrol:assigned-work",
            "--every-ms",
            "100",
            "--repeat-count",
            "2",
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );
    let subscription_id = subscribed["subscription"]["id"].as_str().unwrap();
    assert_eq!(subscribed["subscription"]["worker_id"], peer_id);
    assert_eq!(subscribed["subscription"]["repeat_count"], 2);
    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(!master_status.to_string().contains(subscription_id));
    for _ in 0..2 {
        let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_B);
        let messages = received["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{received}");
        assert_eq!(messages[0]["subject"], "deadline:patrol:assigned-work");
        assert_eq!(messages[0]["to"], peer_id);
    }
    let status = fixture.run_public(&["notify", "status"], THREAD_B);
    let subscription = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == subscription_id)
        .unwrap();
    assert_eq!(subscription["fired_count"], 2);
    assert_eq!(subscription["status"], "consumed");
    let no_more = fixture.run_public(&["recv", "--timeout", "2"], THREAD_B);
    assert_eq!(no_more["count"], 0, "{no_more}");

    for args in [
        vec![
            "notify",
            "subscribe",
            "--event",
            "master-idle",
            "--subject",
            "patrol",
            "--ttl-seconds",
            "30",
        ],
        vec![
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:project",
            "--every-ms",
            "100",
            "--ttl-seconds",
            "30",
        ],
        vec![
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "patrol:invalid",
            "--every-ms",
            "0",
            "--ttl-seconds",
            "30",
        ],
    ] {
        let output = fixture.command_public(&args, THREAD_B);
        assert!(
            !output.status.success(),
            "unexpected success: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn accepted_master_goal_deadline_clears_goal_due_before_recv() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &["master", "promote", "--approval", "public-governance-latch"],
        THREAD_A,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let master_at = (now + 200).to_string();
    let master = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:accepted-latch",
            "--at-ms",
            &master_at,
            "--ttl-seconds",
            "60",
        ],
        THREAD_A,
    );
    let master_sub = master["subscription"]["id"].as_str().unwrap();
    native_deadline(&fixture, THREAD_A, master_sub, Duration::from_secs(10)).unwrap();

    let context = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        context["master_wake"]["goal_due"], false,
        "an accepted but unconsumed master goal deadline must clear goal_due: {context}"
    );

    fixture.run_public(&["context"], THREAD_B);
    let peer_at = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 250)
        .to_string();
    let peer = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:peer-latch",
            "--at-ms",
            &peer_at,
            "--ttl-seconds",
            "60",
        ],
        THREAD_B,
    );
    let peer_sub = peer["subscription"]["id"].as_str().unwrap();
    native_deadline(&fixture, THREAD_B, peer_sub, Duration::from_secs(10)).unwrap();
    let context = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        context["master_wake"]["goal_due"], false,
        "a peer-owned goal deadline must not pollute master goal_due: {context}"
    );

    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .reject_input = true;
    let later_at = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 200)
        .to_string();
    let later = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "goal:later-latch",
            "--at-ms",
            &later_at,
            "--ttl-seconds",
            "60",
        ],
        THREAD_A,
    );
    let later_sub = later["subscription"]["id"].as_str().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let later_message = loop {
        let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let bound = events
            .iter()
            .find(|e| e["ev"] == "WakeBound" && e["subscription_id"] == later_sub)
            .and_then(|e| e["message_id"].as_str());
        if let Some(message_id) = bound {
            if events
                .iter()
                .any(|e| e["ev"] == "NotificationDeliveryFailed" && e["message_id"] == message_id)
            {
                break message_id.to_owned();
            }
        }
        assert!(
            Instant::now() < until,
            "later master deadline never produced a rejected delivery for {later_sub}"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        !events.iter().any(|e| {
            e["ev"] == "NotificationDeliveryAccepted" && e["message_id"] == later_message
        }),
        "a rejected delivery must not record adaptor acceptance"
    );
    let context = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        context["master_wake"]["goal_due"], true,
        "a later master deadline must re-raise goal_due until accepted: {context}"
    );
}

fn board_patrol_ids(status: &Value, field: &str) -> Vec<String> {
    status["board_patrol"][field]
        .as_array()
        .unwrap_or_else(|| panic!("board_patrol.{field} must be an array: {status}"))
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

fn has_direct_message_subscription(status: &Value) -> bool {
    status["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|subscription| subscription["event"] == "direct-message")
}

#[test]
fn board_idle_patrol_control_cancel_persists_and_rearm_keeps_inbox() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(alice["name"], "Alice");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);

    let before = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(before["board_patrol"]["interval_ms"], 900_000);
    assert_eq!(before["board_patrol"]["periodic_fallback"], true);
    assert_eq!(before["board_patrol"]["realtime_connected"], false);
    assert_eq!(board_patrol_ids(&before, "eligible_task_ids"), vec!["T1"]);
    assert!(board_patrol_ids(&before, "cancelled_tasks").is_empty());
    assert!(
        has_direct_message_subscription(&before),
        "context must retain the daemon-owned direct-message lease: {before}"
    );

    let cancelled = fixture.run_public(
        &[
            "notify",
            "cancel",
            "--task-id",
            "T1",
            "--reason",
            "checkpoint",
            "--resume-condition",
            "after review",
        ],
        THREAD_A,
    );
    assert_eq!(cancelled["cancelled_task_ids"], json!(["T1"]));

    let after = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(after["board_patrol"]["cancelled_tasks"], json!(["T1"]));
    assert!(board_patrol_ids(&after, "eligible_task_ids").is_empty());
    assert!(has_direct_message_subscription(&after));
    assert_eq!(
        fixture.run_public(&["task", "status", "T1"], THREAD_A)["id"],
        "T1"
    );

    fixture.restart_daemon();
    let resumed = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(resumed["name"], "Alice");
    let persisted = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(persisted["board_patrol"]["cancelled_tasks"], json!(["T1"]));

    let rearmed = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "board-idle",
            "--task-id",
            "T1",
        ],
        THREAD_A,
    );
    assert_eq!(rearmed["event"], "board-idle");
    assert_eq!(rearmed["status"], "armed");
    assert_eq!(rearmed["rearmed_task_ids"], json!(["T1"]));

    let final_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(board_patrol_ids(&final_status, "cancelled_tasks").is_empty());
    assert_eq!(
        board_patrol_ids(&final_status, "eligible_task_ids"),
        vec!["T1"]
    );
}

#[test]
fn board_idle_patrol_control_cancel_all_is_atomic_and_new_task_stays_eligible() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    fixture.run_public(&["task", "register", "T2"], THREAD_A);
    fixture.run_public(&["task", "register", "U1"], THREAD_B);

    let mixed = fixture.command_public(
        &["notify", "cancel", "--task-id", "T1", "--task-id", "U1"],
        THREAD_A,
    );
    assert!(!mixed.status.success(), "mixed owner cancel must fail");
    let mixed_output = format!(
        "{}{}",
        String::from_utf8_lossy(&mixed.stdout),
        String::from_utf8_lossy(&mixed.stderr)
    );
    assert!(
        mixed_output.contains("BOARD_PATROL_TASK_OWNER_REQUIRED"),
        "unexpected mixed-owner failure: {mixed_output}"
    );
    let unchanged = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(board_patrol_ids(&unchanged, "cancelled_tasks").is_empty());
    assert_eq!(
        board_patrol_ids(&unchanged, "eligible_task_ids"),
        vec!["T1", "T2"]
    );

    let cancelled = fixture.run_public(&["notify", "cancel", "--reason", "all"], THREAD_A);
    assert_eq!(cancelled["cancelled_task_ids"], json!(["T1", "T2"]));

    fixture.run_public(&["task", "register", "T3"], THREAD_A);
    let with_new_task = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        with_new_task["board_patrol"]["cancelled_tasks"],
        json!(["T1", "T2"])
    );
    assert_eq!(
        board_patrol_ids(&with_new_task, "eligible_task_ids"),
        vec!["T3"]
    );

    fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "board-idle",
            "--task-id",
            "T1",
        ],
        THREAD_A,
    );
    let rearmed = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(rearmed["board_patrol"]["cancelled_tasks"], json!(["T2"]));
    assert_eq!(
        board_patrol_ids(&rearmed, "eligible_task_ids"),
        vec!["T1", "T3"]
    );
}

#[test]
fn board_idle_patrol_control_rearm_is_scoped_to_the_caller() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    fixture.run_public(&["task", "register", "U1"], THREAD_B);
    fixture.run_public(&["notify", "cancel", "--task-id", "T1"], THREAD_A);
    fixture.run_public(&["notify", "cancel", "--task-id", "U1"], THREAD_B);

    let rearmed = fixture.run_public(&["notify", "subscribe", "--event", "board-idle"], THREAD_A);
    assert_eq!(rearmed["rearmed_task_ids"], json!(["T1"]));

    let alice = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(board_patrol_ids(&alice, "cancelled_tasks").is_empty());
    assert_eq!(board_patrol_ids(&alice, "eligible_task_ids"), vec!["T1"]);

    let bob = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(bob["board_patrol"]["cancelled_tasks"], json!(["U1"]));
    assert!(board_patrol_ids(&bob, "eligible_task_ids").is_empty());
}

#[test]
fn board_idle_patrol_control_lifecycle_uses_stable_owner_key() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let alice_name = alice["name"].as_str().unwrap().to_owned();
    let bob_name = bob["name"].as_str().unwrap().to_owned();
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    fixture.run_public(&["notify", "cancel", "--task-id", "T1"], THREAD_A);

    fixture.append_name_events(&[json!({
        "ev": "TaskAssignmentUpdated",
        "assignment": {
            "task_id": "T1",
            "name": alice_name,
            "session_id": THREAD_B,
            "assignment_revision": 1,
            "updated_ms": 1
        }
    })]);
    let same_name = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        same_name["board_patrol"]["cancelled_tasks"],
        json!(["T1"]),
        "same stable name with a new session carrier must retain cancellation: {same_name}"
    );

    fixture.append_name_events(&[
        json!({
            "ev": "TaskAssignmentUpdated",
            "assignment": {
                "task_id": "T1",
                "name": bob_name,
                "session_id": THREAD_B,
                "assignment_revision": 2,
                "updated_ms": 2
            }
        }),
        json!({
            "ev": "TaskAssignmentUpdated",
            "assignment": {
                "task_id": "T1",
                "name": alice_name,
                "session_id": THREAD_B,
                "assignment_revision": 3,
                "updated_ms": 3
            }
        }),
    ]);
    let transferred_back = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        board_patrol_ids(&transferred_back, "cancelled_tasks").is_empty(),
        "a true owner transfer must clear the old cancellation: {transferred_back}"
    );
    assert_eq!(
        board_patrol_ids(&transferred_back, "eligible_task_ids"),
        vec!["T1"]
    );
}

#[test]
fn master_notify_status_projects_peer_board_patrol_cancellations() {
    let fixture = AppFixture::new();
    let master = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user promotes Jenny for patrol projection"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(master["name"], "Jenny");
    let master_id = master["identity"]["worker_id"].as_str().unwrap().to_owned();

    let peer = fixture.run_public(&["context"], THREAD_B);
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap().to_owned();
    let peer_name = peer["name"].as_str().unwrap().to_owned();
    fixture.run_public(&["task", "register", "T1"], THREAD_B);
    let cancelled = fixture.run_public(
        &[
            "notify",
            "cancel",
            "--task-id",
            "T1",
            "--reason",
            "peer checkpoint",
            "--resume-condition",
            "after peer review",
        ],
        THREAD_B,
    );
    assert_eq!(cancelled["cancelled_task_ids"], json!(["T1"]));

    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    let project = master_status["project_board_patrol"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("master notify status must project board patrol: {master_status}")
        });
    let peer_record = project
        .iter()
        .find(|record| record["worker_id"] == peer_id)
        .unwrap_or_else(|| {
            panic!("peer {peer_id} missing from project projection: {master_status}")
        });
    assert_eq!(peer_record["owner_key"], peer_name);
    assert_eq!(peer_record["eligible_task_ids"], json!([]));
    let peer_cancelled = peer_record["cancelled_tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("peer cancellations must be an array: {master_status}"));
    assert_eq!(peer_cancelled.len(), 1, "{master_status}");
    assert_eq!(peer_cancelled[0]["task_id"], "T1");
    assert_eq!(peer_cancelled[0]["reason"], "peer checkpoint");
    assert_eq!(peer_cancelled[0]["resume_condition"], "after peer review");
    assert!(
        project
            .iter()
            .any(|record| { record["worker_id"] == master_id && record["owner_key"] == "Jenny" }),
        "project projection must include the current master carrier: {master_status}"
    );

    let peer_status = fixture.run_public(&["notify", "status"], THREAD_B);
    assert!(
        peer_status.get("project_board_patrol").is_none(),
        "ordinary peer must not receive project-wide projection: {peer_status}"
    );
    assert_eq!(
        peer_status["board_patrol"]["cancelled_tasks"],
        json!(["T1"])
    );
    assert!(board_patrol_ids(&peer_status, "eligible_task_ids").is_empty());

    let other_project = AppFixture::new();
    let other_peer = other_project.run_public(&["context"], NEXT_CARRIER);
    let other_peer_id = other_peer["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(other_peer_id, peer_id);
    let refreshed = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        !refreshed["project_board_patrol"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["worker_id"] == other_peer_id),
        "project projection must not leak another project's worker: {refreshed}"
    );
}
