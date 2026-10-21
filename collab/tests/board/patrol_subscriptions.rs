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
    fixture.run_public(&["notify", "unsubscribe", peer_sub], THREAD_B);
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
