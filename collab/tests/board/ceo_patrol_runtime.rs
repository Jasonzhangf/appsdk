use super::*;

const CEO_NOTICE_TIMEOUT: Duration = Duration::from_secs(10);

fn journal_events(fixture: &AppFixture) -> Vec<Value> {
    std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn input_has_message(input: &Value, message_id: &str) -> bool {
    input["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .chain(input["toolOutput"]["output"].as_str())
        .any(|text| {
            text.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .any(|token| token == message_id)
        })
}

fn set_native_idle(fixture: &AppFixture) {
    let mut states = fixture.state.lock().unwrap();
    let state = states
        .get_mut(THREAD_A)
        .unwrap_or_else(|| panic!("native thread {THREAD_A} must exist"));
    state.status = "idle".into();
    state.active_turn = None;
    for turn in &mut state.turns {
        if let Some(turn) = turn.as_object_mut() {
            turn.insert("status".into(), json!("completed"));
        }
    }
}

fn host_has_prepared_batch(fixture: &AppFixture) -> bool {
    std::fs::read_to_string(fixture.host_state.join("journal.jsonl"))
        .expect("host journal exists")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| event["ev"] == "CeoReportBatchPrepared")
}

fn wait_for_ceo_notice(fixture: &AppFixture, worker_id: &str) -> String {
    let deadline = Instant::now() + CEO_NOTICE_TIMEOUT;
    loop {
        let events = journal_events(fixture);
        for event in events.iter().filter(|event| {
            event["ev"] == "Sent"
                && event["msg"]["subject"] == "ceo-report-due"
                && event["msg"]["to"] == worker_id
        }) {
            let message_id = event["msg"]["id"]
                .as_str()
                .expect("CEO notice has a message id");
            let bound = events
                .iter()
                .any(|event| event["ev"] == "WakeBound" && event["message_id"] == message_id);
            let accepted = events.iter().any(|event| {
                event["ev"] == "NotificationDeliveryAccepted"
                    && event["message_id"] == message_id
                    && event["evidence"].is_object()
            });
            let native = fixture
                .state
                .lock()
                .unwrap()
                .get(THREAD_A)
                .is_some_and(|state| {
                    state
                        .inputs
                        .iter()
                        .any(|input| input_has_message(input, message_id))
                });
            if bound && accepted && native {
                return message_id.to_owned();
            }
        }
        assert!(
            Instant::now() < deadline,
            "automatic CEO notice did not reach {THREAD_A}/{worker_id}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn ceo_notice_runtime_native_master_is_global_ceo() {
    let fixture = AppFixture::new();
    let registered = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(registered["identity"]["transport"]["kind"], "appserver");
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the daemon worker")
        .to_owned();

    let master = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture requests Jenny"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(master["name"], "Jenny");
    assert_eq!(master["master"]["worker_id"], worker_id);

    set_native_idle(&fixture);
    let promoted = fixture.run_public(
        &["ceo", "promote", "--approval", "fixtureuserapproved"],
        THREAD_A,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let authorized = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(authorized["ceo"]["is_current"], true, "{authorized}");
    assert_eq!(authorized["master"]["worker_id"], worker_id);
    assert_eq!(authorized["identity"]["worker_id"], worker_id);

    set_native_idle(&fixture);
    let progress = fixture.run_public(
        &[
            "ceo",
            "reply",
            "--subject",
            "native-progress",
            "--urgent",
            "stageA",
        ],
        THREAD_A,
    );
    assert_eq!(progress["recorded"], true, "{progress}");
    assert_eq!(progress["urgent"], true, "{progress}");
    assert_eq!(progress["notified"], false, "{progress}");
    assert_eq!(progress["reported"], false, "{progress}");

    set_native_idle(&fixture);
    let message_id = wait_for_ceo_notice(&fixture, &worker_id);
    assert!(!message_id.is_empty());
    assert!(
        !host_has_prepared_batch(&fixture),
        "the automatic gate-only notice must not prebind report content"
    );

    let authorized = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(authorized["ceo"]["is_current"], true, "{authorized}");
    let project = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let prepared = fixture.run_public(&["ceo", "progress", "--project", &project], THREAD_A);
    let report = &prepared["report"];
    assert!(
        report["batch_id"]
            .as_str()
            .is_some_and(|batch_id| !batch_id.is_empty()),
        "progress prepared a batch: {prepared}"
    );
    let coverage = report["coverage"]
        .as_object()
        .unwrap_or_else(|| panic!("progress report has coverage: {prepared}"));
    assert!(!coverage.is_empty(), "{prepared}");
    assert_eq!(
        coverage.get(&project).and_then(Value::as_u64),
        progress["revision"].as_u64(),
        "{prepared}"
    );
    assert!(
        report["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["project"].as_str() == Some(project.as_str())),
        "{prepared}"
    );
}
