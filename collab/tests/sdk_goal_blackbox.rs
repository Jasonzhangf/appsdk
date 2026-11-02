// Real-entry blackbox for the SDK `appsdk goal` periodic subscription.
//
// This drives the public `appsdk goal subscribe|status|cancel` CLI inside an
// isolated real tmux pane against a real isolated Collab daemon. It proves,
// through public receipts only:
//   * the SDK passes an explicit interval/repeat/TTL request to the daemon and
//     reports the armed periodic occurrence returned by Collab,
//   * a real deadline notification reaches the worker and is consumed,
//   * `appsdk goal cancel` ends the goal and retires the occurrence,
//   * after cancellation no further deadline is delivered.
//
// Restart continuity is already covered by `goal_periodic_blackbox.rs` at the
// Collab entry. This test does not restate that as SDK evidence. No product
// source, production daemon, or production data is touched.

include!("support/tmux_cli_fixture.rs");

use std::fs;

fn appsdk_test_binary() -> PathBuf {
    std::env::var_os("APPSDK_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("candidate root")
                .join("rust/target/debug/appsdk")
        })
}

fn evidence_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("candidate root")
        .join(".evidence/collab-ablation/sdk-goal-blackbox-20261031")
}

fn write_bytes(evidence: &Path, name: &str, bytes: &[u8]) {
    fs::write(evidence.join(name), bytes).expect("write evidence file");
}

fn write_json(evidence: &Path, name: &str, value: &Value) {
    write_bytes(
        evidence,
        name,
        &serde_json::to_vec_pretty(value).expect("serialize evidence"),
    );
}

/// Run the real `appsdk` CLI inside the same isolated pane as the Collab CLI.
/// `PATH` leads with the candidate `collab` binary so the SDK's child
/// `Command::new("collab")` calls cannot reach a production install.
fn run_appsdk(fixture: &Fixture, pane: &Pane, args: &[&str]) -> Output {
    let binary = appsdk_test_binary();
    assert!(
        binary.is_file(),
        "appsdk binary missing: {}; build with `cargo build --manifest-path rust/Cargo.toml --bin appsdk`",
        binary.display()
    );
    let pane_py = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/pane_cli.py");
    let collab_dir = fixture.binary.parent().expect("collab binary parent");
    let mut paths: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    paths.insert(0, collab_dir.to_path_buf());

    let mut command = Command::new("python3");
    command
        .arg(&pane_py)
        .arg("client")
        .arg("--socket")
        .arg(&fixture.tmux_socket)
        .arg("--pane-id")
        .arg(&pane.pane_id)
        .arg("--root")
        .arg(&fixture.root)
        .arg("--binary")
        .arg(&binary)
        .arg("--")
        .args(args)
        .current_dir(&fixture.root)
        .env("HOME", fixture.root.join("home"))
        .env(
            "PANE_CONTROL_PATH",
            std::env::var_os("PATH").unwrap_or_default(),
        )
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("CODEX_HOME", fixture.root.join("home"))
        .env("PATH", std::env::join_paths(paths).expect("join PATH"))
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env(
            "TMUX",
            format!("{},{},0", fixture.tmux_socket.display(), pane.server_pid),
        )
        .env("TMUX_PANE", &pane.pane_id)
        .env("CODEX_SESSION_ID", &pane.session_anchor)
        .env("CODEX_THREAD_ID", &pane.thread_anchor);
    for key in [
        "COLLAB_SOCKET_PATH",
        "COLLAB_HOST_SOCKET",
        "COLLAB_LOCK_PATH",
        "COLLAB_HOST_LOCK",
        "COLLAB_APPSERVER_SOCKET",
        "CODEX_APP_SERVER_SOCKET",
        "COLLAB_APPSERVER_NAMESPACE",
        "COLLAB_WORKER",
    ] {
        command.env_remove(key);
    }
    command.output().expect("run appsdk in isolated pane")
}

fn run_appsdk_ok(
    fixture: &Fixture,
    pane: &Pane,
    args: &[&str],
    evidence: &Path,
    receipt_name: &str,
) -> Value {
    let output = run_appsdk(fixture, pane, args);
    write_bytes(evidence, &format!("{receipt_name}.stdout"), &output.stdout);
    write_bytes(evidence, &format!("{receipt_name}.stderr"), &output.stderr);
    assert!(
        output.status.success(),
        "appsdk {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("appsdk CLI emits JSON")
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

fn find_subscription<'a>(status: &'a Value, subscription_id: &str) -> &'a Value {
    status["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions")
        .iter()
        .find(|subscription| subscription["id"].as_str() == Some(subscription_id))
        .expect("subscription stays observable")
}

#[test]
fn sdk_goal_subscribe_consumes_a_real_deadline_and_cancel_stops_it() {
    let (fixture, pane) = single_pane_fixture("sdk-goal-blackbox");
    let evidence = evidence_dir();
    fs::create_dir_all(&evidence).expect("create evidence dir");

    let registered = fixture.registered_context(&pane, true);
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the worker")
        .to_owned();
    let owner = registered["name"]
        .as_str()
        .expect("context receipt names the occupancy")
        .to_owned();
    let project_scope = registered["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project")
        .to_owned();
    write_json(&evidence, "registered-context.json", &registered);

    let goal_file = fixture.root.join("sdk-goal-blackbox.md");
    fs::write(
        &goal_file,
        "# SDK goal periodic blackbox\n\nVerify the real appsdk goal entry consumes a periodic deadline and stops on cancel.\n",
    )
    .expect("write isolated goal file");

    let interval_ms = 120_000i64;
    let repeat = 5u32;
    let ttl_seconds = 600u64;
    let subscribed = run_appsdk_ok(
        &fixture,
        &pane,
        &[
            "goal",
            "subscribe",
            "--goal",
            goal_file.to_str().unwrap(),
            "--interval",
            "120s",
            "--repeat",
            &repeat.to_string(),
            "--ttl",
            &ttl_seconds.to_string(),
            "--json",
        ],
        &evidence,
        "subscribe",
    );
    write_json(&evidence, "subscribe.json", &subscribed);

    assert_eq!(subscribed["active"], true, "{subscribed}");
    assert_eq!(subscribed["desired"], "subscribed", "{subscribed}");
    assert_eq!(subscribed["observed"], "subscribed", "{subscribed}");
    assert_eq!(subscribed["remote_state"], "armed", "{subscribed}");
    assert_eq!(subscribed["periodic_declared"], true, "{subscribed}");
    assert_eq!(subscribed["every_ms"], interval_ms, "{subscribed}");
    assert_eq!(subscribed["repeat_count"], repeat, "{subscribed}");
    assert_eq!(subscribed["ttl_seconds"], ttl_seconds, "{subscribed}");
    let subscription_id = subscribed["subscription_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .expect("subscribe receipt names the subscription")
        .to_owned();
    assert_eq!(
        subscribed["collab_subscription"]["status"], "armed",
        "{subscribed}"
    );
    assert_eq!(
        subscribed["collab_subscription"]["one_shot"], false,
        "{subscribed}"
    );
    assert_eq!(
        subscribed["collab_subscription"]["subscription"]["id"], subscription_id,
        "{subscribed}"
    );
    assert_eq!(
        subscribed["collab_subscription"]["subscription"]["periodic_declared"], true,
        "{subscribed}"
    );
    assert_eq!(
        subscribed["collab_subscription"]["subscription"]["interval_ms"], interval_ms,
        "{subscribed}"
    );
    assert_eq!(
        subscribed["collab_subscription"]["subscription"]["repeat_count"], repeat,
        "{subscribed}"
    );
    let goal_id = subscribed["goal_id"]
        .as_str()
        .expect("subscribe receipt names the goal")
        .to_owned();

    write_bytes(
        &evidence,
        "subscribe-assertions.txt",
        format!(
            "worker={worker}\nowner={owner}\nproject_scope={project_scope}\ngoal_id={goal_id}\nsubscription_id={subscription_id}\ninterval_ms={interval_ms}\nrepeat={repeat}\nttl_seconds={ttl_seconds}\n"
        )
        .as_bytes(),
    );

    // Consume the first real deadline occurrence generated by the SDK-declared
    // schedule. The daemon's minimum SDK interval is 120 s, so allow a margin.
    let mut consumed: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut attempt = 0usize;
    while consumed.is_empty() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the SDK goal deadline; subscription={subscription_id}"
        );
        attempt += 1;
        let received = fixture.run_ok(
            &[
                "recv",
                "--timeout",
                "0",
                "--receive-id",
                &format!("sdk-goal-deadline-{attempt}"),
            ],
            Some(&pane),
        );
        let ids = deadline_message_ids(&received, &goal_id);
        if !ids.is_empty() {
            write_json(&evidence, "recv-deadline.json", &received);
        }
        for id in ids {
            let status = fixture.run_ok(&["msg", &id], Some(&pane));
            write_json(&evidence, &format!("msg-{id}.json"), &status);
            assert_eq!(status["state"], "read", "recv must consume {id}: {status}");
            consumed.push(id);
        }
        if consumed.is_empty() {
            thread::sleep(Duration::from_millis(100));
        }
    }
    write_bytes(
        &evidence,
        "consumed-occurrences.txt",
        consumed.join("\n").as_bytes(),
    );

    let after_fire = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "status", "--json"],
        &evidence,
        "status-after-fire",
    );
    write_json(&evidence, "status-after-fire.json", &after_fire);
    assert_eq!(after_fire["active"], true, "{after_fire}");
    assert_eq!(after_fire["desired"], "subscribed", "{after_fire}");
    assert_eq!(after_fire["observed"], "subscribed", "{after_fire}");
    assert_eq!(
        after_fire["record"]["collab_subscription"]["id"], subscription_id,
        "{after_fire}"
    );
    assert_eq!(
        after_fire["record"]["collab_subscription"]["status"], "armed",
        "{after_fire}"
    );
    let fired_count_after_fire = after_fire["record"]["collab_subscription"]["fired_count"]
        .as_u64()
        .expect("notify status reports fired_count");
    assert!(
        fired_count_after_fire >= 1,
        "the consumed occurrence must be reflected in fired_count: {after_fire}"
    );

    // End the goal through the SDK entry. This must call the daemon's ended
    // reconcile and retire the still-armed periodic occurrence.
    let cancelled = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "cancel", "--json"],
        &evidence,
        "cancel",
    );
    write_json(&evidence, "cancel.json", &cancelled);
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["subscription_id"], subscription_id, "{cancelled}");
    assert_eq!(
        cancelled["cancel_receipt"]["status"], "ended",
        "{cancelled}"
    );
    assert!(
        cancelled["cancel_receipt"]["retired_subscription_ids"]
            .as_array()
            .is_some_and(|ids| ids
                .iter()
                .any(|id| id.as_str() == Some(subscription_id.as_str()))),
        "cancel must retire the SDK subscription: {cancelled}"
    );
    assert_eq!(
        cancelled["record"]["desired"], "unsubscribed",
        "{cancelled}"
    );
    assert_eq!(cancelled["record"]["observed"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["record"]["active"], false, "{cancelled}");

    let after_cancel = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "status", "--json"],
        &evidence,
        "status-after-cancel",
    );
    write_json(&evidence, "status-after-cancel.json", &after_cancel);
    assert_eq!(after_cancel["active"], false, "{after_cancel}");
    assert_eq!(after_cancel["desired"], "unsubscribed", "{after_cancel}");
    assert_eq!(after_cancel["observed"], "cancelled", "{after_cancel}");

    let cancelled_status = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(
        &evidence,
        "notify-status-after-cancel.json",
        &cancelled_status,
    );
    let cancelled_occurrence = find_subscription(&cancelled_status, &subscription_id);
    assert_eq!(
        cancelled_occurrence["status"], "cancelled",
        "{cancelled_occurrence}"
    );
    assert_eq!(
        cancelled_occurrence["fired_count"], fired_count_after_fire,
        "cancel must stop at the observed count: {cancelled_occurrence}"
    );

    // Wait past one full interval so the next scheduled occurrence would have
    // fired if cancellation had not removed it from the schedule.
    let crossed = Instant::now() + Duration::from_secs(125);
    while Instant::now() < crossed {
        thread::sleep(Duration::from_millis(200));
    }
    let drained = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "sdk-goal-after-cancel",
        ],
        Some(&pane),
    );
    write_json(&evidence, "recv-after-cancel.json", &drained);
    assert!(
        deadline_message_ids(&drained, &goal_id).is_empty(),
        "no further SDK goal deadline may reach the worker after cancel: {drained}"
    );
    let final_status = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(&evidence, "notify-status-final.json", &final_status);
    let final_occurrence = find_subscription(&final_status, &subscription_id);
    assert_eq!(
        final_occurrence["status"], "cancelled",
        "{final_occurrence}"
    );
    assert_eq!(
        final_occurrence["fired_count"], fired_count_after_fire,
        "a cancelled occurrence must not fire after a full interval: {final_occurrence}"
    );

    write_bytes(
        &evidence,
        "facts.txt",
        format!(
            "SDK goal periodic blackbox facts\n\
             date: {}\n\
             candidate: {}\n\
             worker: {worker}\n\
             owner: {owner}\n\
             project_scope: {project_scope}\n\
             goal_file: {}\n\
             goal_id: {goal_id}\n\
             subscription_id: {subscription_id}\n\
             requested: interval=120s repeat={repeat} ttl={ttl_seconds}s\n\
             recv_poll_attempts: {attempt}\n\
             consumed_occurrences: {}\n\
             fired_count_after_fire: {fired_count_after_fire}\n\
             fired_count_final: {}\n\
             sdk_entry: appsdk goal subscribe/status/cancel\n\
             daemon: isolated collab serve (test-owned)\n",
            chrono::Utc::now().to_rfc3339(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("candidate root")
                .display(),
            goal_file.display(),
            consumed.len(),
            final_occurrence["fired_count"],
        )
        .as_bytes(),
    );
}
