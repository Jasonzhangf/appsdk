// Real-entry blackbox for the periodic goal reminder lifecycle.
//
// This test drives the public `collab goal reconcile` CLI against a real
// daemon in an isolated tmux fixture (the same harness that
// `tmux_recv_e2e.rs` uses). It proves, through public receipts only:
//   * an explicit periodic goal declaration arms a sustained occurrence,
//   * at least three distinct occurrences fire and are really consumed via
//     `collab recv`,
//   * the schedule keeps firing across an isolated daemon restart,
//   * ending the goal retires the occurrence and no further wake fires,
//   * an occurrence without an explicit periodic declaration stays one-shot.
//
// No product source is touched, no production daemon or data is used, and no
// consumption or clock is synthesized: every fact comes from the CLI receipts
// or the owned journal.

include!("support/tmux_cli_fixture.rs");

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as i64
}

fn deadline_subject(goal_id: &str) -> String {
    format!("deadline:goal:{goal_id}")
}

fn deadline_message_ids(receipt: &Value, goal_id: &str) -> Vec<String> {
    let subject = deadline_subject(goal_id);
    receipt["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message["subject"].as_str() == Some(subject.as_str()))
                .filter_map(|message| message["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Long-poll `collab recv` until `needed` distinct deadline occurrences for
/// `goal_id` have been received and confirmed read. Each receipt is the real
/// consumption: `recv` commits a read and advances the periodic cursor.
fn collect_deadline_occurrences(
    fixture: &Fixture,
    pane: &Pane,
    goal_id: &str,
    needed: usize,
    label: &str,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut attempt = 0usize;
    while collected.len() < needed {
        assert!(
            Instant::now() < deadline,
            "{label}: timed out waiting for {needed} deadline occurrences; collected={collected:?}"
        );
        attempt += 1;
        let received = fixture.run_ok(
            &[
                "recv",
                "--timeout",
                "0",
                "--receive-id",
                &format!("{label}-{attempt}"),
            ],
            Some(pane),
        );
        for id in deadline_message_ids(&received, goal_id) {
            if collected.contains(&id) {
                continue;
            }
            let status = fixture.run_ok(&["msg", &id], Some(pane));
            assert_eq!(
                status["state"], "read",
                "{label}: recv must consume occurrence {id}: {status}"
            );
            collected.push(id);
        }
        if collected.len() < needed {
            thread::sleep(Duration::from_millis(100));
        }
    }
    collected
}

fn owned_journal(fixture: &Fixture) -> PathBuf {
    let project = std::fs::canonicalize(&fixture.root).unwrap();
    let record: Value = std::fs::read_to_string(fixture.host_state.join("routes.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["project_scope"] == project.to_string_lossy().as_ref())
        .expect("owned project route record");
    PathBuf::from(record["storage_root"].as_str().unwrap())
        .join(".agent-collab/server/journal.jsonl")
}

/// Count committed wake/send events for this occurrence. Used to prove the
/// ended occurrence emits nothing further.
fn count_deadline_events(
    fixture: &Fixture,
    subscription_id: &str,
    goal_id: &str,
) -> (usize, usize) {
    let journal = std::fs::read_to_string(owned_journal(fixture)).unwrap_or_default();
    let subject = format!("\"subject\":\"{}\"", deadline_subject(goal_id));
    let wakes = journal
        .lines()
        .filter(|line| line.contains("\"ev\":\"WakeBound\""))
        .filter(|line| line.contains(subscription_id))
        .count();
    let sends = journal
        .lines()
        .filter(|line| line.contains("\"ev\":\"Sent\""))
        .filter(|line| line.contains(&subject))
        .count();
    (wakes, sends)
}

fn find_subscription<'a>(status: &'a Value, subscription_id: &str) -> &'a Value {
    status["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions")
        .iter()
        .find(|subscription| subscription["id"].as_str() == Some(subscription_id))
        .expect("subscription stays observable")
}

#[test]
fn explicit_periodic_goal_deadline_consumes_three_restarts_and_stops_on_end() {
    let (mut fixture, pane) = single_pane_fixture("goal-periodic-blackbox");
    let registered = fixture.registered_context(&pane, true);
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the worker")
        .to_owned();
    let owner = registered["name"]
        .as_str()
        .expect("context receipt names the occupancy")
        .to_owned();

    let goal_id = "goal-periodic-blackbox";
    let interval_ms: i64 = 2_000;
    let repeat: u32 = 5;
    let trigger_ms = now_epoch_ms() + interval_ms;
    let armed = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            &owner,
            "--desired",
            "active",
            "--at-ms",
            &trigger_ms.to_string(),
            "--every-ms",
            &interval_ms.to_string(),
            "--repeat",
            &repeat.to_string(),
            "--ttl-seconds",
            "300",
        ],
        Some(&pane),
    );
    assert_eq!(armed["status"], "armed", "{armed}");
    assert_eq!(
        armed["one_shot"], false,
        "explicit period must not be one-shot: {armed}"
    );
    let subscription_id = armed["subscription"]["id"]
        .as_str()
        .expect("armed occurrence names its subscription")
        .to_owned();
    assert_eq!(
        armed["subscription"]["periodic_declared"], true,
        "explicit period must be declared on the occurrence: {armed}"
    );
    assert_eq!(armed["subscription"]["interval_ms"], interval_ms, "{armed}");
    assert_eq!(armed["subscription"]["repeat_count"], repeat, "{armed}");

    let first = collect_deadline_occurrences(&fixture, &pane, goal_id, 3, "periodic-first");
    assert_eq!(first.len(), 3);
    let mut distinct = first.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "three distinct occurrences required: {first:?}"
    );

    // Isolated daemon restart: the armed periodic occurrence must survive.
    fixture.restart_daemon();
    let restored = fixture.run_context(&["context"], Some(&pane));
    assert_eq!(
        restored["identity"]["worker_id"], worker,
        "same pane must recover the same worker after restart: {restored}"
    );
    let after_restart =
        collect_deadline_occurrences(&fixture, &pane, goal_id, 1, "periodic-after-restart");
    assert_eq!(after_restart.len(), 1);
    assert!(
        !first.contains(&after_restart[0]),
        "post-restart occurrence must be distinct from the pre-restart ones"
    );

    let before_end = fixture.run_ok(&["notify", "status"], Some(&pane));
    let armed_occurrence = find_subscription(&before_end, &subscription_id);
    assert_eq!(armed_occurrence["status"], "armed", "{armed_occurrence}");
    assert_eq!(armed_occurrence["fired_count"], 4, "{armed_occurrence}");

    // End the goal while the occurrence is still armed (repeat budget 5, four
    // consumed). The schedule must be retired, not naturally exhausted.
    let ended = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            &owner,
            "--desired",
            "ended",
            "--reason",
            "isolated blackbox goal completed",
        ],
        Some(&pane),
    );
    assert_eq!(ended["status"], "ended", "{ended}");
    assert!(
        ended["retired_subscription_ids"]
            .as_array()
            .is_some_and(|ids| ids
                .iter()
                .any(|id| id.as_str() == Some(subscription_id.as_str()))),
        "ending must retire the armed occurrence: {ended}"
    );
    let after_end_status = fixture.run_ok(&["notify", "status"], Some(&pane));
    assert_eq!(
        find_subscription(&after_end_status, &subscription_id)["status"],
        "cancelled",
        "{after_end_status}"
    );

    // Wait past a full interval and prove the ended occurrence emits nothing.
    let before = count_deadline_events(&fixture, &subscription_id, goal_id);
    let crossed = Instant::now() + Duration::from_secs(6);
    while Instant::now() < crossed {
        thread::sleep(Duration::from_millis(100));
    }
    let after = count_deadline_events(&fixture, &subscription_id, goal_id);
    assert_eq!(
        after, before,
        "an ended periodic goal must not emit another wake: before={before:?} after={after:?}"
    );
    let drained = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "periodic-after-end",
        ],
        Some(&pane),
    );
    assert!(
        deadline_message_ids(&drained, goal_id).is_empty(),
        "no further deadline may reach the worker after end: {drained}"
    );
}

#[test]
fn undeclared_goal_deadline_stays_one_shot() {
    let (fixture, pane) = single_pane_fixture("goal-one-shot-blackbox");
    let registered = fixture.registered_context(&pane, true);
    let owner = registered["name"]
        .as_str()
        .expect("context receipt names the occupancy")
        .to_owned();

    let goal_id = "goal-one-shot-blackbox";
    let trigger_ms = now_epoch_ms() + 1_500;
    let armed = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            &owner,
            "--desired",
            "active",
            "--at-ms",
            &trigger_ms.to_string(),
            "--ttl-seconds",
            "300",
        ],
        Some(&pane),
    );
    assert_eq!(armed["status"], "armed", "{armed}");
    assert_eq!(
        armed["one_shot"], true,
        "no period declaration must stay one-shot: {armed}"
    );
    let subscription_id = armed["subscription"]["id"]
        .as_str()
        .expect("armed occurrence names its subscription")
        .to_owned();
    assert_eq!(
        armed["subscription"]["periodic_declared"], false,
        "a request without an explicit period must not declare one: {armed}"
    );

    let occurred = collect_deadline_occurrences(&fixture, &pane, goal_id, 1, "one-shot-single");
    assert_eq!(occurred.len(), 1);

    // Past the original trigger window the one-shot must stay consumed.
    let crossed = Instant::now() + Duration::from_secs(4);
    while Instant::now() < crossed {
        thread::sleep(Duration::from_millis(100));
    }
    let extra = fixture.run_ok(
        &["recv", "--timeout", "0", "--receive-id", "one-shot-after"],
        Some(&pane),
    );
    assert!(
        deadline_message_ids(&extra, goal_id).is_empty(),
        "a one-shot goal must not fire again: {extra}"
    );
    let status = fixture.run_ok(&["notify", "status"], Some(&pane));
    let occurrence = find_subscription(&status, &subscription_id);
    assert_eq!(occurrence["status"], "consumed", "{occurrence}");
    assert_eq!(
        occurrence["status_reason"], "goal-deadline-one-shot-delivered",
        "a one-shot goal must record its single-delivery reason: {occurrence}"
    );
}
