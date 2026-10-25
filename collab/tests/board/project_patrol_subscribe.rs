use super::*;

const PATROL_SUBSCRIBE: [&str; 12] = [
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
];

const LEGACY_SUBSCRIBE: [&str; 12] = [
    "notify",
    "subscribe",
    "--event",
    "master-idle",
    "--subject",
    "master-idle:legacy",
    "--every-ms",
    "3600000",
    "--repeat-count",
    "1",
    "--ttl-seconds",
    "600",
];

fn restore_master(fixture: &AppFixture) -> Value {
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user promotes Jenny for patrol subscribe"}"#,
        ],
        THREAD_A,
    )
}

fn subscribe_patrol(fixture: &AppFixture) -> Value {
    fixture.run_public(&PATROL_SUBSCRIBE, THREAD_A)
}

#[test]
fn current_master_exact_project_patrol_subscribe_is_idempotent() {
    let fixture = AppFixture::new();
    let master = restore_master(&fixture);
    let master_id = master["identity"]["worker_id"].as_str().unwrap().to_owned();

    let first = subscribe_patrol(&fixture);
    let first_id = first["subscription"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first["subscription"]["worker_id"], master_id);
    assert_eq!(first["subscription"]["event"], "master-idle");
    assert_eq!(first["subscription"]["subject"], "project-patrol");
    assert_eq!(first["subscription"]["interval_ms"], 900000);
    assert_eq!(first["subscription"]["status"], "armed");
    assert_ne!(first["deduplicated"], true, "{first}");

    let second = subscribe_patrol(&fixture);
    assert_eq!(second["subscription"]["id"], first_id, "{second}");
    assert_eq!(second["deduplicated"], true, "{second}");

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    let patrol: Vec<_> = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|subscription| {
            subscription["event"] == "master-idle"
                && subscription["subject"] == "project-patrol"
                && subscription["interval_ms"] == 900000
        })
        .collect();
    assert_eq!(patrol.len(), 1, "one armed patrol resource: {status}");
    assert_eq!(patrol[0]["id"], first_id);
    assert_eq!(patrol[0]["status"], "armed");
}

#[test]
fn unrelated_finite_master_idle_schedule_stays_independent() {
    let fixture = AppFixture::new();
    restore_master(&fixture);

    let legacy = fixture.run_public(&LEGACY_SUBSCRIBE, THREAD_A);
    let legacy_id = legacy["subscription"]["id"].as_str().unwrap().to_owned();
    let patrol = subscribe_patrol(&fixture);
    let patrol_id = patrol["subscription"]["id"].as_str().unwrap().to_owned();
    assert_ne!(patrol_id, legacy_id, "{patrol}");

    // The daemon-owned direct-message lease plus these two schedules fill the
    // three-active cap. The exact patrol ensure must still reuse its own armed
    // resource instead of failing at the cap.
    let patrol_again = subscribe_patrol(&fixture);
    assert_eq!(
        patrol_again["subscription"]["id"], patrol_id,
        "{patrol_again}"
    );
    assert_eq!(patrol_again["deduplicated"], true, "{patrol_again}");

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    let subscriptions = status["subscriptions"].as_array().unwrap();
    let legacy_record = subscriptions
        .iter()
        .find(|subscription| subscription["id"] == legacy_id)
        .unwrap_or_else(|| panic!("unrelated legacy schedule must stay: {status}"));
    assert_eq!(legacy_record["event"], "master-idle");
    assert_eq!(legacy_record["subject"], "master-idle:legacy");
    assert_eq!(legacy_record["interval_ms"], 3600000);
    assert_eq!(legacy_record["status"], "armed");
    assert_eq!(
        subscriptions
            .iter()
            .filter(|subscription| subscription["subject"] == "project-patrol")
            .count(),
        1,
        "{status}"
    );
    assert_eq!(
        subscriptions
            .iter()
            .filter(|subscription| subscription["subject"] == "master-idle:legacy")
            .count(),
        1,
        "{status}"
    );
}

#[test]
fn peer_cannot_arm_project_patrol() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    fixture.run_public(&["context"], THREAD_B);

    let output = fixture.command_public(&PATROL_SUBSCRIBE, THREAD_B);
    assert!(
        !output.status.success(),
        "peer must not arm the master patrol: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let rejection = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rejection.contains("requires the current registered master"),
        "master authority error must stay unchanged: {rejection}"
    );

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        !status["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|subscription| subscription["subject"] == "project-patrol"),
        "rejected peer request must not arm a patrol: {status}"
    );
}
