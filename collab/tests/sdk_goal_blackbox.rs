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
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("sdk-goal-blackbox")
}

fn crud_evidence_dir() -> PathBuf {
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "sdk-goal-crud-blackbox-{}-{run_id}",
        std::process::id()
    ))
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

fn relay_request(endpoint: &Path, request: &str) -> String {
    use std::io::{BufRead, BufReader, Write};
    let mut daemon = std::os::unix::net::UnixStream::connect(endpoint).unwrap();
    daemon
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    daemon.write_all(request.as_bytes()).unwrap();
    let mut reply = String::new();
    BufReader::new(daemon).read_line(&mut reply).unwrap();
    reply
}

fn intercept_goal_reply(
    fixture: &Fixture,
    pane: &Pane,
    args: &[&str],
    active_snapshot_id: Option<String>,
    before: impl FnOnce(&Path, Value) -> bool + Send + 'static,
) -> (Output, Value) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let endpoint = fixture.host_state.join("server.sock");
    let daemon = fixture.host_state.join("edit.sock");
    fs::rename(&endpoint, &daemon).unwrap();
    let relay = UnixListener::bind(&endpoint).unwrap();
    relay.set_nonblocking(true).unwrap();
    let upstream = daemon.clone();
    let proof = std::thread::spawn(move || -> Option<Value> {
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut observed_armed_batch = false;
        while Instant::now() < deadline {
            let (mut client, _) = match relay.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept edit test connection: {error}"),
            };
            client.set_nonblocking(false).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            client
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            if BufReader::new(&client).read_line(&mut request).unwrap() == 0 {
                continue;
            }
            if request.contains("\"GoalScheduleReconcile\"") {
                assert!(active_snapshot_id.is_none() || observed_armed_batch);
                let drop_reply = before(&upstream, serde_json::from_str(&request).unwrap());
                let reply = relay_request(&upstream, &request);
                if !drop_reply {
                    client.write_all(reply.as_bytes()).unwrap();
                }
                return Some(serde_json::from_str(&reply).expect("daemon reply"));
            }
            let reply = relay_request(&upstream, &request);
            if let Some(id) = active_snapshot_id
                .as_deref()
                .filter(|_| request.contains("\"NotificationStatus\""))
            {
                let snapshot: Value = serde_json::from_str(&reply).unwrap();
                let selected = find_subscription(&snapshot, id);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                assert_eq!(selected["status"], "armed");
                assert!(selected["expires_ms"].as_u64().unwrap() > now);
                observed_armed_batch = true;
            }
            client.write_all(reply.as_bytes()).unwrap();
        }
        None
    });
    let output = run_appsdk(fixture, pane, args);
    let proof = proof.join();
    fs::remove_file(&endpoint).unwrap();
    fs::rename(&daemon, &endpoint).unwrap();
    (
        output,
        proof.unwrap().expect("intercepted the edit response"),
    )
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

fn goal_record_file(root: &Path, owner: &str, goal_id: &str) -> PathBuf {
    let owner_component = owner
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let goal_component = goal_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    root.join(".appsdk-control")
        .join("goals")
        .join(owner_component)
        .join("records")
        .join(format!("{goal_component}.json"))
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
    let repeat = 100u32;
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
            let message = received["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["id"] == id)
                .expect("received deadline occurrence");
            assert!(
                message["body"]
                    .as_str()
                    .unwrap()
                    .contains("sdk-goal-blackbox.md"),
                "the full generated goal prompt must reach the peer: {message}"
            );
            fixture.assert_message_consumed(&id, &pane);
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

#[test]
fn sdk_goal_crud_isolated_to_selected_goal_on_real_daemon() {
    let (fixture, pane) = single_pane_fixture("sdk-goal-crud-blackbox");
    let evidence = crud_evidence_dir();
    fs::create_dir_all(&evidence).expect("create evidence dir");
    let registered = fixture.registered_context(&pane, true);
    write_json(&evidence, "registered-context.json", &registered);
    let owner = registered["name"]
        .as_str()
        .expect("context receipt names the stable owner")
        .to_owned();
    let subscribe_goal = |name: &str, body: &str, repeat: &str| {
        let path = fixture.root.join(name);
        fs::write(&path, body).expect("write isolated goal");
        let receipt = run_appsdk_ok(
            &fixture,
            &pane,
            &[
                "goal",
                "subscribe",
                "--goal",
                path.to_str().unwrap(),
                "--interval",
                "120s",
                "--repeat",
                repeat,
                "--json",
            ],
            &evidence,
            &format!("subscribe-{}", name.trim_end_matches(".md")),
        );
        write_json(
            &evidence,
            &format!("subscribe-{}.json", name.trim_end_matches(".md")),
            &receipt,
        );
        assert_eq!(receipt["active"], true, "{receipt}");
        assert_eq!(receipt["remote_state"], "armed", "{receipt}");
        assert_eq!(receipt["every_ms"], 120_000, "{receipt}");
        let goal_id = receipt["goal_id"]
            .as_str()
            .expect("subscribe receipt names exact goal")
            .to_owned();
        let subscription_id = receipt["subscription_id"]
            .as_str()
            .expect("subscribe receipt names exact subscription")
            .to_owned();
        (goal_id, subscription_id)
    };

    let (goal_a, subscription_a) = subscribe_goal("goal-a.md", "# Goal A\n\nDeliver A.\n", "5");
    let (goal_b, subscription_b) = subscribe_goal("goal-b.md", "# Goal B\n\nDeliver B.\n", "6");

    // Registration owns one direct-message lease. The daemon permits three
    // active subscriptions per agent, so retire the first goal before arming
    // the third while retaining all three goal records for CRUD isolation.
    let cancelled_for_capacity = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "cancel", "--goal-id", &goal_a, "--json"],
        &evidence,
        "cancel-goal-a-capacity",
    );
    write_json(
        &evidence,
        "cancel-goal-a-capacity.json",
        &cancelled_for_capacity,
    );
    assert_eq!(cancelled_for_capacity["cancel_receipt"]["goal_id"], goal_a);
    assert_eq!(cancelled_for_capacity["subscription_id"], subscription_a);
    assert_eq!(cancelled_for_capacity["status"], "cancelled");

    let (goal_c, subscription_c) = subscribe_goal("goal-c.md", "# Goal C\n\nDeliver C.\n", "7");
    let goal_ids = vec![goal_a, goal_b, goal_c];
    let subscription_ids = vec![subscription_a, subscription_b, subscription_c];
    assert_eq!(
        goal_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        subscription_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );

    let record_paths = goal_ids
        .iter()
        .map(|goal_id| goal_record_file(&fixture.root, &owner, goal_id))
        .collect::<Vec<_>>();
    let bytes_before_read = record_paths
        .iter()
        .map(|path| fs::read(path).expect("read retained goal record"))
        .collect::<Vec<_>>();
    let listed = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "list", "--json"],
        &evidence,
        "list-before-edit",
    );
    write_json(&evidence, "list-before-edit.json", &listed);
    assert_eq!(listed["count"], 3, "{listed}");
    for goal_id in &goal_ids {
        assert!(
            listed["goals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|goal| goal["goal_id"].as_str() == Some(goal_id.as_str())),
            "list must retain {goal_id}: {listed}"
        );
    }
    for (path, before) in record_paths.iter().zip(&bytes_before_read) {
        assert_eq!(fs::read(path).unwrap(), *before, "list must be read-only");
    }
    for (index, goal_id) in goal_ids.iter().enumerate() {
        let status = run_appsdk_ok(
            &fixture,
            &pane,
            &["goal", "status", "--goal-id", goal_id, "--json"],
            &evidence,
            &format!("status-{}", index + 1),
        );
        write_json(&evidence, &format!("status-{}.json", index + 1), &status);
        assert_eq!(
            status["goal_id"].as_str(),
            Some(goal_id.as_str()),
            "{status}"
        );
        assert_eq!(
            status["subscription_id"].as_str(),
            Some(subscription_ids[index].as_str()),
            "{status}"
        );
        assert_eq!(status["active"], index == 1 || index == 2, "{status}");
        for (path, before) in record_paths.iter().zip(&bytes_before_read) {
            assert_eq!(
                fs::read(path).unwrap(),
                *before,
                "status must not mutate any retained record"
            );
        }
    }
    let ambiguous = run_appsdk(&fixture, &pane, &["goal", "status", "--json"]);
    write_bytes(&evidence, "status-ambiguous.stdout", &ambiguous.stdout);
    write_bytes(&evidence, "status-ambiguous.stderr", &ambiguous.stderr);
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("GOAL_RECORD_SELECTOR_REQUIRED"),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
    let missing = run_appsdk(
        &fixture,
        &pane,
        &[
            "goal",
            "edit",
            "--goal-id",
            "sha256:missing-real-blackbox",
            "--repeat",
            "4",
            "--json",
        ],
    );
    write_bytes(&evidence, "edit-missing.stdout", &missing.stdout);
    write_bytes(&evidence, "edit-missing.stderr", &missing.stderr);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("GOAL_RECORD_NOT_FOUND"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let initial_remote = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(&evidence, "notify-status-initial.json", &initial_remote);
    let initial_remote_subscriptions = subscription_ids
        .iter()
        .map(|id| find_subscription(&initial_remote, id).clone())
        .collect::<Vec<_>>();
    assert_eq!(initial_remote_subscriptions[0]["status"], "cancelled");
    assert_eq!(initial_remote_subscriptions[1]["status"], "armed");
    assert_eq!(initial_remote_subscriptions[2]["status"], "armed");

    let edited = run_appsdk_ok(
        &fixture,
        &pane,
        &[
            "goal",
            "edit",
            "--goal-id",
            &goal_ids[1],
            "--interval",
            "180s",
            "--repeat",
            "8",
            "--json",
        ],
        &evidence,
        "edit-goal-b",
    );
    write_json(&evidence, "edit-goal-b.json", &edited);
    assert_eq!(
        edited["goal_id"].as_str(),
        Some(goal_ids[1].as_str()),
        "{edited}"
    );
    assert_eq!(
        edited["subscription_id"].as_str(),
        Some(subscription_ids[1].as_str()),
        "{edited}"
    );
    assert_eq!(edited["record"]["interval"], "180s", "{edited}");
    assert_eq!(edited["record"]["repeat_count"], 8, "{edited}");
    assert_eq!(fs::read(&record_paths[0]).unwrap(), bytes_before_read[0]);
    assert_eq!(fs::read(&record_paths[2]).unwrap(), bytes_before_read[2]);
    let after_edit = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(&evidence, "notify-status-after-edit.json", &after_edit);
    assert_eq!(
        find_subscription(&after_edit, &subscription_ids[1])["interval_ms"],
        180_000
    );
    assert_eq!(
        find_subscription(&after_edit, &subscription_ids[1])["repeat_count"],
        8
    );
    assert_eq!(
        find_subscription(&after_edit, &subscription_ids[0]),
        &initial_remote_subscriptions[0]
    );
    assert_eq!(
        find_subscription(&after_edit, &subscription_ids[2]),
        &initial_remote_subscriptions[2]
    );
    let bytes_after_edit = record_paths
        .iter()
        .map(|path| fs::read(path).expect("read retained goal record after edit"))
        .collect::<Vec<_>>();

    let updated = run_appsdk_ok(
        &fixture,
        &pane,
        &[
            "goal",
            "update",
            "--goal-id",
            &goal_ids[2],
            "--interval",
            "240s",
            "--repeat",
            "9",
            "--json",
        ],
        &evidence,
        "update-goal-c",
    );
    write_json(&evidence, "update-goal-c.json", &updated);
    assert_eq!(
        updated["goal_id"].as_str(),
        Some(goal_ids[2].as_str()),
        "{updated}"
    );
    assert_eq!(
        updated["subscription_id"].as_str(),
        Some(subscription_ids[2].as_str()),
        "{updated}"
    );
    assert_eq!(updated["record"]["interval"], "240s", "{updated}");
    assert_eq!(updated["record"]["repeat_count"], 9, "{updated}");
    assert_eq!(fs::read(&record_paths[0]).unwrap(), bytes_after_edit[0]);
    assert_eq!(fs::read(&record_paths[1]).unwrap(), bytes_after_edit[1]);
    let after_update = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(&evidence, "notify-status-after-update.json", &after_update);
    assert_eq!(
        find_subscription(&after_update, &subscription_ids[0]),
        &initial_remote_subscriptions[0]
    );
    assert_eq!(
        find_subscription(&after_update, &subscription_ids[1])["interval_ms"],
        180_000
    );
    assert_eq!(
        find_subscription(&after_update, &subscription_ids[1])["repeat_count"],
        8
    );
    assert_eq!(
        find_subscription(&after_update, &subscription_ids[2])["interval_ms"],
        240_000
    );
    assert_eq!(
        find_subscription(&after_update, &subscription_ids[2])["repeat_count"],
        9
    );

    let cancelled = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "cancel", "--goal-id", &goal_ids[2], "--json"],
        &evidence,
        "cancel-goal-c",
    );
    write_json(&evidence, "cancel-goal-c.json", &cancelled);
    assert_eq!(
        cancelled["cancel_receipt"]["goal_id"].as_str(),
        Some(goal_ids[2].as_str()),
        "{cancelled}"
    );
    assert_eq!(
        cancelled["subscription_id"].as_str(),
        Some(subscription_ids[2].as_str()),
        "{cancelled}"
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    let after_cancel = fixture.run_ok(&["notify", "status"], Some(&pane));
    write_json(&evidence, "notify-status-after-cancel.json", &after_cancel);
    assert_eq!(
        find_subscription(&after_cancel, &subscription_ids[2])["status"],
        "cancelled"
    );
    assert_eq!(
        find_subscription(&after_cancel, &subscription_ids[0]),
        find_subscription(&after_update, &subscription_ids[0])
    );
    assert_eq!(
        find_subscription(&after_cancel, &subscription_ids[1]),
        find_subscription(&after_update, &subscription_ids[1])
    );
    let final_list = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "list", "--json"],
        &evidence,
        "list-after-cancel",
    );
    write_json(&evidence, "list-after-cancel.json", &final_list);
    assert_eq!(final_list["count"], 3, "{final_list}");
    for goal_id in &goal_ids {
        assert!(
            final_list["goals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|goal| goal["goal_id"].as_str() == Some(goal_id.as_str())),
            "cancel must retain observable record {goal_id}: {final_list}"
        );
    }

    let goal_c = &goal_ids[2];
    let renewed = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_c,
            "--owner",
            &owner,
            "--desired",
            "active",
            "--every-ms",
            "120000",
            "--repeat",
            "3",
            "--ttl-seconds",
            "600",
            "--content",
            "Renew outside AppSDK",
        ],
        Some(&pane),
    );
    let renewed_id = renewed["subscription"]["id"].as_str().unwrap();
    assert_ne!(renewed_id, subscription_ids[2]);
    let ended = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "cancel", "--goal-id", goal_c, "--json"],
        &evidence,
        "cancel-externally-renewed-goal",
    );
    assert_eq!(ended["subscription_id"], renewed_id, "{ended}");
    assert_eq!(ended["status"], "cancelled", "{ended}");

    let last_batch = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_c,
            "--owner",
            &owner,
            "--desired",
            "active",
            "--every-ms",
            "1000",
            "--repeat",
            "3",
            "--ttl-seconds",
            "10",
            "--content",
            "Expire without a local cancellation",
        ],
        Some(&pane),
    );
    let last_batch_id = last_batch["subscription"]["id"].as_str().unwrap();
    let expires = last_batch["subscription"]["expires_ms"].as_u64().unwrap();
    let (ended_output, end_receipt) = intercept_goal_reply(
        &fixture,
        &pane,
        &["goal", "cancel", "--goal-id", goal_c, "--json"],
        Some(last_batch_id.to_owned()),
        move |_, request| {
            assert_eq!(request["desired"], "ended");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            std::thread::sleep(Duration::from_millis(expires.saturating_sub(now) + 100));
            false
        },
    );
    assert!(ended_output.status.success(), "{ended_output:?}");
    assert_eq!(end_receipt["ended_subscription"]["status"], "expired");
    let ended_again: Value = serde_json::from_slice(&ended_output.stdout).unwrap();
    write_json(
        &evidence,
        "observe-expiry-between-snapshot-and-end.json",
        &ended_again,
    );
    assert_eq!(
        ended_again["subscription_id"], last_batch_id,
        "{ended_again}"
    );
    assert_eq!(ended_again["status"], "expired", "{ended_again}");

    // Simulate a missing local projection with retained daemon history.
    fs::remove_file(&record_paths[2]).unwrap();
    let remote_only = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "status", "--goal-id", goal_c, "--json"],
        &evidence,
        "remote-only-exact-status",
    );
    assert_eq!(
        remote_only["subscription_id"], last_batch_id,
        "{remote_only}"
    );
    assert_eq!(remote_only["observed"], "expired", "{remote_only}");
    assert_eq!(remote_only["error"], "GOAL_PERIODIC_BATCH_ENDED:expired");
    let list = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "list", "--json"],
        &evidence,
        "list-remote-history",
    );
    assert_eq!(list["count"], 3, "{list}");
    assert!(
        !record_paths[2].exists(),
        "read back must not recreate a local record"
    );
    let edited_remote = run_appsdk_ok(
        &fixture,
        &pane,
        &[
            "goal",
            "edit",
            "--goal-id",
            goal_c,
            "--repeat",
            "4",
            "--json",
        ],
        &evidence,
        "edit-remote-only-terminal-goal",
    );
    assert_eq!(
        edited_remote["subscription_id"], last_batch_id,
        "{edited_remote}"
    );
    assert_eq!(edited_remote["status"], "expired", "{edited_remote}");
    let final_subscriptions = fixture.run_ok(&["notify", "status"], Some(&pane));
    assert_eq!(
        find_subscription(&final_subscriptions, &subscription_ids[1]),
        find_subscription(&after_cancel, &subscription_ids[1])
    );

    let future = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000)
        .to_string();
    let one_shot = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            "convert-once",
            "--owner",
            &owner,
            "--desired",
            "active",
            "--at-ms",
            &future,
            "--ttl-seconds",
            "600",
        ],
        Some(&pane),
    );
    assert_eq!(one_shot["one_shot"], true);
    let converted = run_appsdk_ok(
        &fixture,
        &pane,
        &[
            "goal",
            "edit",
            "--goal-id",
            "convert-once",
            "--interval",
            "120s",
            "--repeat",
            "4",
            "--json",
        ],
        &evidence,
        "convert-one-shot-to-periodic-by-exact-edit",
    );
    assert_eq!(converted["subscription_id"], one_shot["subscription"]["id"]);
    assert_eq!(converted["record"]["periodic_declared"], true);
    assert_eq!(converted["record"]["schedule"], "periodic");

    let new_document = fixture.root.join("goal-b-revised.md");
    fs::write(
        &new_document,
        "# Revised Goal B\n\nConfirm a recovered edit.\n",
    )
    .unwrap();
    let edit = [
        "goal",
        "edit",
        "--goal-id",
        &goal_ids[1],
        "--goal",
        new_document.to_str().unwrap(),
        "--interval",
        "240s",
        "--json",
    ];
    let (lost, committed) = intercept_goal_reply(&fixture, &pane, &edit, None, |_, _| true);
    assert_eq!(committed["ok"], true, "{committed}");
    assert_eq!(lost.status.code(), Some(1));
    let pending: Value = serde_json::from_slice(&lost.stdout).unwrap();
    assert!(pending["edit_reconciliation"]["original_error"]
        .as_str()
        .unwrap()
        .contains("DAEMON_UNKNOWN"));
    let pending_bytes = fs::read(&record_paths[1]).unwrap();
    let observed = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "status", "--goal-id", &goal_ids[1], "--json"],
        &evidence,
        "observe-committed-edit-without-reply",
    );
    assert_eq!(
        observed["goal_path"],
        new_document.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(observed["record"]["every_ms"], 240_000);
    assert!(
        observed["record"]["edit_reconciliation"].is_null(),
        "{observed}"
    );
    assert_eq!(fs::read(&record_paths[1]).unwrap(), pending_bytes);
    let retried = run_appsdk_ok(
        &fixture,
        &pane,
        &edit,
        &evidence,
        "retry-edit-without-reply",
    );
    assert_eq!(retried["receipt"]["deduplicated"], true, "{retried}");
    assert_eq!(retried["subscription_id"], subscription_ids[1]);

    let next_document = fixture.root.join("goal-b-stale-edit.md");
    fs::write(&next_document, "# Must not edit a replacement batch\n").unwrap();
    let competing = [
        "goal",
        "edit",
        "--goal-id",
        &goal_ids[1],
        "--goal",
        next_document.to_str().unwrap(),
        "--interval",
        "180s",
        "--json",
    ];
    let (lost_after_renewal, rejected) =
        intercept_goal_reply(&fixture, &pane, &competing, None, |upstream, edit| {
            let mut end = edit.clone();
            for field in [
                "operation",
                "expected_subscription_id",
                "interval_ms",
                "content",
            ] {
                end.as_object_mut().unwrap().remove(field);
            }
            end["desired"] = Value::String("ended".into());
            let invoke = |request: &Value| -> Value {
                let response = relay_request(upstream, &format!("{request}\n"));
                serde_json::from_str(&response).unwrap()
            };
            assert_eq!(invoke(&end)["ok"], true);
            end["desired"] = Value::String("active".into());
            end["interval_ms"] = Value::from(120_000);
            end["repeat_count"] = Value::from(3);
            end["ttl_seconds"] = Value::from(600);
            end["content"] = Value::String("A separately authorized replacement batch".into());
            assert_eq!(invoke(&end)["ok"], true);
            true
        });
    assert_eq!(lost_after_renewal.status.code(), Some(1));
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(rejected["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_SCHEDULE_SELECTION_CHANGED"));
    let recovered = run_appsdk_ok(
        &fixture,
        &pane,
        &["goal", "status", "--goal-id", &goal_ids[1], "--json"],
        &evidence,
        "no-edit-on-renewed-batch-after-lost-reply",
    );
    assert_ne!(recovered["subscription_id"], subscription_ids[1]);
    assert_eq!(recovered["record"]["every_ms"], 120_000);
    assert_eq!(
        recovered["record"]["edit_reconciliation"]["status"],
        "pending"
    );
    assert_eq!(
        recovered["record"]["collab_subscription"]["goal_content"]["source"]["content"],
        "A separately authorized replacement batch"
    );

    write_bytes(
        &evidence,
        "crud-facts.txt",
        format!(
            "Real daemon goal CRUD acceptance\nowner={owner}\ngoal_ids={}\nsubscription_ids={}\nintervals=120s,180s,240s\ncommands=subscribe x3,cancel goal-a for capacity,list,status --goal-id x3,edit goal-b,update goal-c,cancel goal-c\nambiguous_status=GOAL_RECORD_SELECTOR_REQUIRED\nmissing_edit=GOAL_RECORD_NOT_FOUND\n",
            goal_ids.join(","),
            subscription_ids.join(","),
        )
        .as_bytes(),
    );
}
