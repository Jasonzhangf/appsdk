use super::*;
use crate::proto::TransportKind;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn fixture_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_FIXTURE_LOCK.lock().await
}

fn endpoint() -> RexEndpoint {
    RexEndpoint {
        server: "unix:///tmp/rex-fixture.sock".into(),
        session_id: "session:11111111-1111-1111-1111-111111111111".into(),
        block_id: "block:22222222-2222-2222-2222-222222222222".into(),
        host_id: Some("host-fixture".into()),
        instance_id: "instance-fixture".into(),
        cwd: "/tmp/rex-fixture-cwd".into(),
    }
}

fn candidate() -> RexCandidate {
    RexCandidate {
        server: endpoint().server,
        session_id: endpoint().session_id,
        block_id: endpoint().block_id,
        cwd: endpoint().cwd,
    }
}

fn selected_transport() -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some("agent-session".into()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(endpoint()),
        capabilities: vec!["send_message_to_block".into()],
        self_check: "fixture".into(),
    }
}

struct TestEnv {
    dir: PathBuf,
}

impl TestEnv {
    fn new(scenario: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rex-primitives-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create Rex fixture log dir");
        let fixture = dir.join("rex-cli");
        let source = if matches!(scenario, "hang" | "no_stdin") {
            include_str!("fixture_stalled_rex.sh")
        } else {
            include_str!("fixture_rex.py")
        };
        fs::write(&fixture, source).expect("write embedded CLI fixture");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
        set_test_cli(Some(fixture));
        set_test_timeout(Some(Duration::from_secs(5)));
        std::env::set_var("REXPRIMITIVE_LOG_DIR", &dir);
        std::env::set_var("REXPRIMITIVE_SCENARIO", scenario);
        Self { dir }
    }

    fn calls(&self) -> Vec<Value> {
        let path = self.dir.join("invocations.jsonl");
        let Ok(text) = fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).expect("fixture invocation JSON"))
            .collect()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        set_test_cli(None);
        set_test_timeout(None);
        std::env::remove_var("REXPRIMITIVE_LOG_DIR");
        std::env::remove_var("REXPRIMITIVE_SCENARIO");
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn method(call: &Value) -> String {
    let argv = call["argv"].as_array().expect("fixture argv array");
    for candidate in [
        "server.status",
        "session.list",
        "session.list_blocks",
        "com.superlogical.terminal.write",
        "com.superlogical.terminal.process",
        "com.superlogical.terminal.pwd",
        "com.superlogical.terminal.program_status",
        "capture",
        "send-key",
    ] {
        if argv.iter().any(|value| value.as_str() == Some(candidate)) {
            return candidate.into();
        }
    }
    String::new()
}

#[tokio::test]
async fn candidate_deadline_is_shared_by_all_verification_steps() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("slow_all");
    set_test_timeout(Some(Duration::from_secs(3)));
    let started = Instant::now();
    let result = verify_candidate(&candidate(), "agent-session").await;
    assert!(
        result.is_err(),
        "verification must stop at its total deadline"
    );
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(!env
        .calls()
        .iter()
        .any(|call| method(call) == "session.list_blocks"));
}

#[tokio::test]
async fn notify_writes_body_then_sends_a_real_enter_key() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("normal");
    let text = "期限正文\n第二行";
    let receipt = notify(&selected_transport(), text, Some("immediate"))
        .await
        .expect("body write plus a separate Enter submits the notification");
    let value: Value = serde_json::from_str(&receipt).expect("receipt JSON");
    assert_eq!(value["transport"], "rex");
    assert_eq!(value["bytes_written"], text.len() as u64);
    assert_eq!(value["text_queued"], true);
    assert_eq!(value["enter_accepted"], true);
    assert_eq!(value["submission_queued"], true);
    assert_eq!(value["consumed"], false);
    let calls = env.calls();
    let writes: Vec<&Value> = calls
        .iter()
        .filter(|call| method(call) == "com.superlogical.terminal.write")
        .collect();
    assert_eq!(writes.len(), 1, "notification must not retry the write");
    let request: Value =
        serde_json::from_str(writes[0]["stdin"].as_str().unwrap()).expect("write JSON");
    assert_eq!(request["args"]["data"], text);
    let enter = calls
        .iter()
        .find(|call| method(call) == "send-key")
        .expect("notification must submit through the real key path");
    let argv: Vec<&str> = enter["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    let send_key_at = argv
        .iter()
        .position(|arg| *arg == "send-key")
        .expect("send-key argv");
    assert_eq!(
        &argv[send_key_at..],
        [
            "send-key",
            "-s",
            endpoint().session_id.as_str(),
            "-b",
            endpoint().block_id.as_str(),
            "enter",
        ]
        .as_slice(),
        "Enter must target the selected session and block"
    );
}

#[tokio::test]
async fn typed_enter_rejection_keeps_the_queued_stage_and_native_code() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("enter_typed_reject");
    let text = "abc";
    let error = notify(&selected_transport(), text, Some("immediate"))
        .await
        .expect_err("a typed Enter rejection must not become success");
    assert_eq!(error.class, NotificationDeliveryClass::Unknown);
    assert_eq!(error.native_code.as_deref(), Some("conflict"));
    assert_eq!(
        error.stage,
        RexInputStage::WriteQueued {
            bytes_written: text.len(),
        }
    );
    assert_eq!(
        env.calls()
            .iter()
            .filter(|call| method(call) == "send-key")
            .count(),
        1,
        "a rejected Enter must not be retried"
    );
}

#[tokio::test]
async fn failed_enter_preserves_the_queued_write_stage_without_resending() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("enter_fail");
    let text = "已排队正文";
    let error = notify(&selected_transport(), text, Some("immediate"))
        .await
        .expect_err("a failed Enter after the body write must remain visible");
    assert_eq!(error.class, NotificationDeliveryClass::Unknown);
    assert_eq!(
        error.stage,
        RexInputStage::WriteQueued {
            bytes_written: text.len(),
        }
    );
    assert_eq!(
        env.calls()
            .iter()
            .filter(|call| method(call) == "com.superlogical.terminal.write")
            .count(),
        1,
        "the body must not be resent after Enter fails"
    );
}

#[tokio::test]
async fn snapshot_reads_official_capture_and_returns_latest_lines() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("capture_lines");
    let value = snapshot(&endpoint(), 3).await.expect("capture snapshot");
    assert_eq!(value["transport"], "rex");
    assert_eq!(value["session_id"], endpoint().session_id);
    assert_eq!(value["block_id"], endpoint().block_id);
    assert_eq!(value["lines"], 3);
    assert_eq!(value["durable"], false);
    assert_eq!(value["output"], "line-8\nline-9\nline-10");
    let calls = env.calls();
    let capture = calls
        .iter()
        .find(|call| method(call) == "capture")
        .expect("capture call");
    let argv: Vec<&str> = capture["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(argv.contains(&"--unwrap"));
    assert!(argv.contains(&"--trim"));
    assert!(argv.contains(&"--format=text"));
    assert!(
        !calls.iter().any(|call| {
            matches!(
                method(call).as_str(),
                "com.superlogical.terminal.write" | "send-key"
            )
        }),
        "snapshot must not write terminal input"
    );
}

#[tokio::test]
async fn snapshot_empty_output_succeeds_and_failure_is_not_empty_success() {
    let _guard = fixture_guard().await;
    {
        let _env = TestEnv::new("capture_empty");
        let value = snapshot(&endpoint(), 5).await.expect("empty capture");
        assert_eq!(value["output"], "");
    }
    {
        let _env = TestEnv::new("capture_fail");
        let error = snapshot(&endpoint(), 5)
            .await
            .expect_err("capture failure must not become empty success");
        assert_eq!(error.class, NotificationDeliveryClass::Unknown);
    }
}

#[tokio::test]
async fn snapshot_line_bounds_are_rejected_before_any_cli_call() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("normal");
    for lines in [0usize, 201usize] {
        let error = snapshot(&endpoint(), lines)
            .await
            .expect_err("lines outside 1..=200 must be rejected");
        assert_eq!(error.class, NotificationDeliveryClass::KnownNotDelivered);
    }
    assert!(env.calls().is_empty());
}

#[tokio::test]
async fn observation_deadline_preserves_completed_sources_and_skips_later_cli() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("observe_slow_second");
    set_test_timeout(Some(Duration::from_secs(2)));
    let observation = observe_current(&endpoint()).await.unwrap();
    assert!(observation.process.is_some());
    assert!(observation.gap_possible);
    assert!(!observation.source_errors.is_empty());
    assert!(!env
        .calls()
        .iter()
        .any(|call| method(call) == "com.superlogical.terminal.program_status"));
}

#[tokio::test]
async fn a_replaced_service_cannot_receive_input_or_authoritative_observations() {
    let _guard = fixture_guard().await;
    let env = TestEnv::new("success");
    let mut selected = selected_transport();
    selected.rex_endpoint.as_mut().unwrap().instance_id = "retired-instance".into();
    let target = selected.rex_endpoint.as_ref().unwrap();
    assert!(write_text(target, "must not be submitted").await.is_err());
    assert!(observe_current(target).await.is_err());
    assert!(
        notify(&selected, "must not be submitted", Some("immediate"))
            .await
            .is_err()
    );
    assert!(env
        .calls()
        .iter()
        .all(|call| method(call) == "server.status"));
}

#[tokio::test]
async fn argv_stdin_and_environment_are_exact() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    std::env::set_var("REX_SHOULD_NOT_LEAK", "1");
    let transport = verify_candidate(&candidate(), "agent-session")
        .await
        .expect("verify Rex candidate");
    assert_eq!(transport.kind, TransportKind::Rex);
    assert_eq!(transport.session_id.as_deref(), Some("agent-session"));
    assert_eq!(
        transport.rex_endpoint.as_ref().unwrap().host_id.as_deref(),
        Some("host-fixture")
    );
    assert_eq!(
        transport.rex_endpoint.as_ref().unwrap().instance_id,
        "instance-fixture"
    );

    let calls = env.calls();
    assert!(!calls.is_empty());
    for call in &calls {
        let argv: Vec<&str> = call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let server = argv
            .iter()
            .position(|value| *value == "--server")
            .expect("--server present");
        assert_eq!(argv[server + 1], endpoint().server);
        assert!(argv.contains(&"--autostart=false"));
        let timeout = argv
            .iter()
            .position(|value| *value == "--timeout")
            .expect("--timeout present");
        let seconds: f64 = argv[timeout + 1]
            .trim_end_matches('s')
            .parse()
            .expect("finite timeout seconds");
        assert!(seconds.is_finite() && seconds > 0.0);
        assert_eq!(call["rex_env"].as_array().unwrap().len(), 0);
    }

    let list = calls
        .iter()
        .find(|call| method(call) == "session.list")
        .expect("session.list call");
    assert_eq!(list["stdin"], "{}");
    let blocks = calls
        .iter()
        .find(|call| method(call) == "session.list_blocks")
        .expect("session.list_blocks call");
    assert_eq!(
        blocks["stdin"],
        json!({"session_id": endpoint().session_id}).to_string()
    );
    std::env::remove_var("REX_SHOULD_NOT_LEAK");
}

#[tokio::test]
async fn old_rex_targets_are_rejected_before_write() {
    let _fixture = fixture_guard().await;
    {
        let env = TestEnv::new("non_terminal_block");
        let error = verify_candidate(&candidate(), "agent-session")
            .await
            .expect_err("non-terminal creator must be rejected");
        assert_eq!(
            error.class,
            NotificationDeliveryClass::KnownNotDelivered,
            "{error}"
        );
        assert!(env
            .calls()
            .iter()
            .all(|call| method(call) != "com.superlogical.terminal.write"));
    }
    {
        let env = TestEnv::new("session_missing");
        let error = verify_candidate(&candidate(), "agent-session")
            .await
            .expect_err("missing session must be rejected");
        assert_eq!(
            error.class,
            NotificationDeliveryClass::KnownNotDelivered,
            "{error}"
        );
        assert_eq!(env.calls().len(), 3);
    }
}

#[tokio::test]
async fn utf8_bytes_are_counted_and_preserved() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    let text = "你好，world 🌍";
    let receipt = write_text(&endpoint(), text)
        .await
        .expect("write UTF-8 text");
    assert_eq!(receipt.bytes_written, text.as_bytes().len());
    let calls = env.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(method(&calls[0]), "server.status");
    let request: Value = serde_json::from_str(calls[1]["stdin"].as_str().unwrap()).unwrap();
    assert_eq!(request["args"]["data"], text);
}

#[tokio::test]
async fn write_unknown_and_short_write_never_enter() {
    let _fixture = fixture_guard().await;
    for (scenario, stage) in [
        ("write_unknown", RexInputStage::BeforeWrite),
        (
            "write_short",
            RexInputStage::WriteQueued { bytes_written: 1 },
        ),
    ] {
        let env = TestEnv::new(scenario);
        let error = notify(&selected_transport(), "a complete body", Some("immediate"))
            .await
            .expect_err("write outcome must be unknown");
        assert_eq!(
            error.class,
            NotificationDeliveryClass::Unknown,
            "scenario={scenario}, error={error:?}, calls={:?}",
            env.calls()
        );
        assert_eq!(error.stage, stage);
        let calls = env.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(method(&calls[0]), "server.status");
        assert_eq!(method(&calls[1]), "com.superlogical.terminal.write");
    }
}

#[tokio::test]
async fn typed_native_rejections_keep_exact_code() {
    let _fixture = fixture_guard().await;
    {
        let env = TestEnv::new("write_typed_reject");
        let error = notify(&selected_transport(), "abc", Some("immediate"))
            .await
            .expect_err("typed write rejection");
        assert_eq!(error.class, NotificationDeliveryClass::KnownNotDelivered);
        assert_eq!(error.native_code.as_deref(), Some("session_not_found"));
        assert_eq!(error.stage, RexInputStage::BeforeWrite);
        assert_eq!(env.calls().len(), 2);
    }
}

#[tokio::test]
async fn queued_mode_is_rejected_before_any_spawn() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    let error = notify(&selected_transport(), "body", Some("queued"))
        .await
        .expect_err("queued mode must be rejected");
    assert_eq!(error.class, NotificationDeliveryClass::KnownNotDelivered);
    assert_eq!(error.stage, RexInputStage::BeforeWrite);
    assert!(env.calls().is_empty());
}

#[tokio::test]
async fn allowed_modes_return_rex_receipt_json() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    for mode in [
        None,
        Some("immediate"),
        Some("explicit-notification"),
        Some("daemon-live-closure"),
        Some("restart-replay-pending"),
    ] {
        let receipt = notify(&selected_transport(), "hi", mode)
            .await
            .unwrap_or_else(|error| panic!("allowed mode {mode:?} failed: {error}"));
        let value: Value = serde_json::from_str(&receipt).expect("receipt JSON");
        assert_eq!(value["transport"], "rex");
        assert_eq!(value["bytes_written"], 2);
        assert_eq!(value["text_queued"], true);
        assert_eq!(value["enter_accepted"], true);
        assert_eq!(value["submission_queued"], true);
        assert_eq!(value["consumed"], false);
        assert!(value.get("queued").is_none());
    }
    assert_eq!(env.calls().len(), 20);
}

#[tokio::test]
async fn finite_timeout_kills_and_reaps_owned_child() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("hang");
    let started = Instant::now();
    let target = endpoint();
    let error = run_cli(
        "write_text",
        Path::new(&official_cli_path()),
        &target.server,
        tokio::time::Instant::now() + Duration::from_millis(150),
        &terminal_write_argv(&target),
        Some(
            json!({"session_id":target.session_id,"block_id":target.block_id,"args":{"data":"x"}})
                .to_string(),
        ),
    )
    .await
    .expect_err("the shared write process runner must time out");
    assert_eq!(
        error.class,
        NotificationDeliveryClass::Unknown,
        "{error}; calls={:?}",
        env.calls()
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "parent timeout must be enforced"
    );
    let calls = env.calls();
    assert!(calls.len() <= 1, "timeout must not retry the write command");
}

#[test]
fn non_finite_and_non_positive_timeouts_are_rejected() {
    for value in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        let error = validate_timeout_secs(value).expect_err("timeout must be rejected");
        assert_eq!(error.class, NotificationDeliveryClass::KnownNotDelivered);
    }
    assert!(validate_timeout_secs(0.001).is_ok());
}

#[test]
fn event_lines_keep_source_exact_target_and_mark_gap() {
    let endpoint = endpoint();
    let good = json!({
        "type": "session_event",
        "payload": {
            "session_id": endpoint.session_id,
            "event": {
                "type": "block_event",
                "payload": {
                    "block_id": endpoint.block_id,
                    "creator_name": TERMINAL_CREATOR,
                    "name": "program_status_changed",
                    "payload": {
                        "id": "agent",
                        "record": {"state": "working", "updated_at": "2026-11-08T00:00:00Z"}
                    }
                }
            }
        }
    })
    .to_string();

    match parse_event_line(&endpoint, &good) {
        RexEventLine::Matched(event) => {
            assert_eq!(event.session_id, endpoint.session_id);
            assert_eq!(event.block_id, endpoint.block_id);
            assert_eq!(event.creator_name, TERMINAL_CREATOR);
            assert_eq!(event.name, "program_status_changed");
            assert_eq!(event.payload["record"]["state"], "working");
            assert!(event.source.starts_with("rex.events:"));
            assert_eq!(event.raw["type"], "session_event");
        }
        other => panic!("expected matching event, got {other:?}"),
    }

    let wrong = json!({
        "type": "session_event",
        "payload": {
            "session_id": "session:wrong",
            "event": {
                "type": "block_event",
                "payload": {
                    "block_id": endpoint.block_id,
                    "creator_name": TERMINAL_CREATOR,
                    "name": "program_status_changed",
                    "payload": {}
                }
            }
        }
    })
    .to_string();
    assert!(matches!(
        parse_event_line(&endpoint, &wrong),
        RexEventLine::Ignored
    ));
    assert!(matches!(
        parse_event_line(&endpoint, "{not json}"),
        RexEventLine::Malformed { .. }
    ));

    let batch = parse_event_lines(&endpoint, vec![good, "{not json}".to_string(), wrong]);
    assert_eq!(batch.events.len(), 1);
    assert!(batch.gap_possible);
    assert!(batch.detail.is_some());
}

#[tokio::test]
async fn observe_current_preserves_sources_without_inferring_idle() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    let observation = observe_current(&endpoint())
        .await
        .expect("observe current Rex target");
    assert_eq!(observation.process.as_ref().unwrap().pid, Some(4242));
    assert_eq!(
        observation.process.as_ref().unwrap().cwd.as_deref(),
        Some("/tmp/rex-fixture-cwd")
    );
    assert_eq!(
        observation.pwd.as_deref(),
        Some("file:///tmp/rex-fixture-cwd")
    );
    assert!(observation.program_status.is_empty());
    assert_eq!(
        observation.sources,
        vec![
            "com.superlogical.terminal.process".to_string(),
            "com.superlogical.terminal.pwd".to_string(),
            "com.superlogical.terminal.program_status".to_string()
        ]
    );
    assert!(observation.source_errors.is_empty());
    assert!(!observation.gap_possible);
    assert_eq!(env.calls().len(), 4);
}

#[tokio::test]
async fn observe_current_decodes_program_status_owner_without_loss() {
    let _fixture = fixture_guard().await;
    let _env = TestEnv::new("status_working");
    let observation = observe_current(&endpoint())
        .await
        .expect("observe current Rex target");
    let record = observation
        .program_status
        .first()
        .expect("one program_status record");
    assert_eq!(record.state.as_deref(), Some("working"));
    let owner = record.owner.as_ref().expect("owner decoded");
    assert_eq!(owner.pid, 4242);
    assert_eq!(owner.name, "python3");
    let process = observation.process.as_ref().expect("foreground process");
    assert_eq!(process.pid, Some(4242));
    assert_eq!(process.name.as_deref(), Some("python3"));
}

#[tokio::test]
async fn observe_current_drops_incomplete_owner_identity() {
    let _fixture = fixture_guard().await;
    let incomplete = [
        json!({"pid": 4242}),
        json!({"pid": 4242, "name": ""}),
        json!({"pid": 4242, "name": "   "}),
        json!({"pid": 0, "name": "python3"}),
        json!({"name": "python3"}),
    ];
    for owner in incomplete {
        let value = json!({
            "records": [
                {"state": "working", "owner": owner, "updated_at": "2026-11-08T00:00:00Z"}
            ]
        });
        assert!(
            extract_status_records(&value)[0].owner.is_none(),
            "incomplete owner must be dropped: {value}"
        );
    }
    let complete = json!({
        "records": [
            {"state": "working", "owner": {"pid": 4242, "name": "python3"}, "updated_at": "2026-11-08T00:00:00Z"}
        ]
    });
    assert!(extract_status_records(&complete)[0].owner.is_some());
}

#[tokio::test]
async fn event_stream_cancel_reaps_owned_child() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("events_stream");
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let stream = spawn_events(endpoint(), sender);

    let event = receiver.recv().await.expect("first matched Rex event");
    assert_eq!(event.name, "program_status_changed");
    assert_eq!(event.session_id, endpoint().session_id);
    assert_eq!(event.block_id, endpoint().block_id);
    assert_eq!(event.creator_name, TERMINAL_CREATOR);

    stream
        .cancel
        .send(true)
        .expect("send event stream cancellation");
    let batch = tokio::time::timeout(Duration::from_secs(2), stream.task)
        .await
        .expect("cancelled event stream finishes promptly")
        .expect("event stream task joins");
    assert!(
        batch.events.is_empty(),
        "streaming must not retain an event history"
    );
    assert!(!batch.gap_possible);
    assert!(batch.detail.is_none());
    assert_eq!(env.calls().len(), 2);
}

#[tokio::test]
async fn incomplete_target_is_rejected_before_any_cli_call() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("normal");
    for (server, session, block) in [
        (String::new(), endpoint().session_id, endpoint().block_id),
        (
            endpoint().server,
            "session:1111".into(),
            endpoint().block_id,
        ),
        (endpoint().server, endpoint().session_id, "label".into()),
    ] {
        let mut target = endpoint();
        target.server = server.into();
        target.session_id = session;
        target.block_id = block;
        assert_eq!(
            write_text(&target, "hello").await.unwrap_err().class,
            NotificationDeliveryClass::KnownNotDelivered
        );
    }
    assert!(env.calls().is_empty());
}

#[tokio::test]
async fn missing_service_generation_is_rejected_at_admission() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("missing_instance");
    let error = verify_candidate(&candidate(), "agent-session")
        .await
        .unwrap_err();
    assert_eq!(env.calls().len(), 1, "{error}");
}

#[tokio::test]
async fn timeout_covers_a_child_that_does_not_read_stdin() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("no_stdin");
    let mut operation = tokio::spawn(async {
        let target = endpoint();
        let body = json!({"session_id": target.session_id, "block_id": target.block_id,
            "args": {"data": "x".repeat(1_048_576)}})
        .to_string();
        run_cli(
            "write_text",
            Path::new(&official_cli_path()),
            &target.server,
            tokio::time::Instant::now() + Duration::from_millis(150),
            &terminal_write_argv(&target),
            Some(body),
        )
        .await
    });
    let result = tokio::time::timeout(Duration::from_secs(2), &mut operation).await;
    operation.abort();
    if result.is_err() {
        if let Some(pid) = env
            .calls()
            .iter()
            .find(|call| method(call) == "com.superlogical.terminal.write")
            .and_then(|call| call["pid"].as_u64())
        {
            // This is the exact PID recorded by the fixture owned by this case.
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .output();
        }
    }
    assert!(
        result.is_ok(),
        "the process deadline must include stdin writes"
    );
    assert_eq!(
        result.unwrap().unwrap().unwrap_err().class,
        NotificationDeliveryClass::Unknown
    );
}

#[test]
fn macos_cli_discovery_uses_official_user_application_installation() {
    let root = std::env::temp_dir().join(format!("rex-cli-discovery-{}", std::process::id()));
    let system = root.join("system");
    let home = root.join("home");
    let relative = "Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex";
    let user_cli = home.join("Applications").join(relative);
    fs::create_dir_all(user_cli.parent().unwrap()).unwrap();
    fs::write(&user_cli, b"fixture").unwrap();
    let observed = installed_macos_cli(&system, Some(&home));
    let system_cli = system.join(relative);
    fs::create_dir_all(system_cli.parent().unwrap()).unwrap();
    fs::write(&system_cli, b"fixture").unwrap();
    assert_eq!(installed_macos_cli(&system, Some(&home)), system_cli);
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(observed, user_cli);
}

fn owned_pid_is_alive(pid: u64) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

#[tokio::test]
async fn event_cleanup_after_setup_expiry_does_not_wait_for_inherited_stderr() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("events_descendant");
    set_test_timeout(Some(Duration::from_secs(2)));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let stream = spawn_events(endpoint(), sender);
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .expect("fixture event");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    stream.cancel.send(true).unwrap();
    let mut task = stream.task;
    let result = tokio::time::timeout(Duration::from_millis(300), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    let pid = fs::read_to_string(env.dir.join("descendant.pid")).unwrap();
    let stopped = std::process::Command::new("/bin/kill")
        .args(["-KILL", pid.trim()])
        .output()
        .unwrap();
    assert!(
        stopped.status.success(),
        "stop the exact owned stderr-holder PID"
    );
    let parent_pid = env.calls().last().unwrap()["pid"].as_u64().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while owned_pid_is_alive(parent_pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owned events CLI terminates after cancellation");
    assert!(
        result.is_ok(),
        "expired setup budget must not wait for inherited stderr"
    );
    let batch = result.unwrap().unwrap();
    assert!(batch.gap_possible);
    assert!(batch
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("unconfirmed"));
}

#[tokio::test]
async fn cancelling_an_operation_terminates_its_owned_cli_child() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("hang");
    // CI can briefly delay fixture startup; allow the full Rex budget while
    // still requiring the exact owned PID to be logged before cancellation.
    set_test_timeout(Some(Duration::from_secs(10)));
    let operation = tokio::spawn(async { write_text(&endpoint(), "hello").await });
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pid) = env
                .calls()
                .iter()
                .find(|call| method(call) == "com.superlogical.terminal.write")
                .and_then(|call| call["pid"].as_u64())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let exit = tokio::time::timeout(Duration::from_secs(2), async {
        while owned_pid_is_alive(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if exit.is_err() {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .output();
    }
    assert!(
        exit.is_ok(),
        "cancellation must terminate the owned CLI child"
    );
}

#[tokio::test]
async fn event_stream_can_cancel_while_receiver_is_backpressured() {
    let _fixture = fixture_guard().await;
    let env = TestEnv::new("events_burst");
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let capacity = sender.clone();
    let stream = spawn_events(endpoint(), sender);
    tokio::time::timeout(Duration::from_secs(2), async {
        while capacity.capacity() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    stream.cancel.send(true).unwrap();
    let mut task = stream.task;
    let stopped = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    if stopped.is_err() {
        task.abort();
        if let Some(pid) = env.calls().first().and_then(|call| call["pid"].as_u64()) {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .output();
        }
    }
    assert!(
        stopped.is_ok(),
        "a full event queue must not block cancellation"
    );
}

/// Test-only `whoami` CLI. It records each invocation and emits one scenario's
/// reply, so environment discovery can be proven without a live Rex server.
struct WhoamiEnv {
    dir: PathBuf,
    log: PathBuf,
}

impl WhoamiEnv {
    fn new(scenario: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rex-env-discovery-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create whoami fixture dir");
        let log = dir.join("invocations.log");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{}",
            log.display(),
            match scenario {
                "ok" => "printf '%s\\n' '{\"endpoint\":\"unix:///tmp/rex-env-server.sock\"}'\n",
                "bad_json" => "printf '%s\\n' 'not-json'\n",
                "missing_endpoint" => "printf '%s\\n' '{}'\n",
                "fail" => "printf '%s\\n' 'whoami rejected' >&2\nexit 9\n",
                other => panic!("unknown whoami scenario {other}"),
            }
        );
        let fixture = dir.join("rex-whoami");
        fs::write(&fixture, script).expect("write whoami fixture");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700))
            .expect("make whoami fixture executable");
        set_test_cli(Some(fixture));
        Self { dir, log }
    }

    fn invocations(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

impl Drop for WhoamiEnv {
    fn drop(&mut self) {
        set_test_cli(None);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn with_rex_env<T>(session: Option<&str>, block: Option<&str>, run: impl FnOnce() -> T) -> T {
    let _lock = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let previous: Vec<_> = ["REX_SESSION", "REX_BLOCK"]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect();
    for (key, value) in [("REX_SESSION", session), ("REX_BLOCK", block)] {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    let result = run();
    for (key, value) in previous {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    result
}

const ENV_SESSION: &str = "session:01a1c47f-7e66-7df6-b484-e727bc9e1811";
const ENV_BLOCK: &str = "block:01a1c47f-7e66-7e9a-bdf1-84a9bd310276";

#[tokio::test]
async fn env_pair_forms_typed_candidate_from_whoami_endpoint() {
    let _guard = fixture_guard().await;
    let probe = WhoamiEnv::new("ok");
    let candidate = with_rex_env(Some(ENV_SESSION), Some(ENV_BLOCK), candidate_from_env)
        .expect("whoami discovery succeeds")
        .expect("both block variables form a candidate");
    assert_eq!(candidate.server, "unix:///tmp/rex-env-server.sock");
    assert_eq!(candidate.session_id, ENV_SESSION);
    assert_eq!(candidate.block_id, ENV_BLOCK);
    assert_eq!(
        candidate.cwd,
        std::env::current_dir()
            .expect("test cwd")
            .to_string_lossy()
            .into_owned()
    );
    let calls = probe.invocations();
    assert_eq!(calls.len(), 1, "whoami is called once: {calls:?}");
    assert!(calls[0].contains("whoami"), "call is whoami: {calls:?}");
}

#[tokio::test]
async fn env_partial_pair_stays_absent_without_calling_cli() {
    let _guard = fixture_guard().await;
    for (session, block) in [
        (Some(ENV_SESSION), None),
        (None, Some(ENV_BLOCK)),
        (None, None),
    ] {
        let probe = WhoamiEnv::new("ok");
        let result = with_rex_env(session, block, candidate_from_env);
        assert_eq!(result.expect("partial pair is not an error"), None);
        assert!(
            probe.invocations().is_empty(),
            "a partial pair must not run whoami"
        );
    }
}

#[tokio::test]
async fn env_partial_pair_is_still_a_rex_environment_hint() {
    let _guard = fixture_guard().await;
    for (session, block) in [
        (Some(ENV_SESSION), None),
        (None, Some(ENV_BLOCK)),
        (None, None),
    ] {
        assert_eq!(
            with_rex_env(session, block, env_block_hint),
            session.is_some() || block.is_some()
        );
    }
}

#[tokio::test]
async fn whoami_failures_keep_typed_error_and_never_fabricate() {
    let _guard = fixture_guard().await;
    for scenario in ["fail", "bad_json", "missing_endpoint"] {
        let _probe = WhoamiEnv::new(scenario);
        let error = with_rex_env(Some(ENV_SESSION), Some(ENV_BLOCK), candidate_from_env)
            .expect_err("unverified whoami output stays a typed error");
        let text = error.to_string();
        assert!(
            text.starts_with("REX_UNKNOWN:") || text.starts_with("REX_NOT_DELIVERED:"),
            "unexpected error for {scenario}: {text}"
        );
        assert!(
            text.contains("discover_env"),
            "error names the discovery operation for {scenario}: {text}"
        );
    }
}

#[tokio::test]
async fn env_pair_rejects_non_canonical_rex_ids_after_whoami() {
    let _guard = fixture_guard().await;
    let _probe = WhoamiEnv::new("ok");
    let error = with_rex_env(
        Some("session:not-a-uuid"),
        Some(ENV_BLOCK),
        candidate_from_env,
    )
    .expect_err("non-canonical Rex ids are not a candidate");
    assert!(
        error.to_string().contains("canonical lowercase UUID"),
        "{error}"
    );
}
