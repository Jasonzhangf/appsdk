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

fn subscribe_patrol_on(fixture: &AppFixture, thread: &str) -> Value {
    fixture.run_public(&PATROL_SUBSCRIBE, thread)
}

fn record_project_progress(fixture: &AppFixture) {
    let promoted = fixture.run_public(
        &[
            "ceo",
            "promote",
            "--approval",
            "fixture user promotes the current master to record project progress",
        ],
        THREAD_A,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let progress = fixture.run_public(
        &[
            "ceo",
            "reply",
            "--subject",
            "project-patrol-progress",
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
}

fn cancel_patrol(fixture: &AppFixture, thread: &str) -> Value {
    fixture.run_public(
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
        thread,
    )
}

fn cancel_patrol_attempt(fixture: &AppFixture, thread: &str) -> Output {
    fixture.command_public(
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
        thread,
    )
}

fn rejection_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn project_patrol_subscriptions(status: &Value) -> Vec<Value> {
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
}

fn project_patrol_suppressions(fixture: &AppFixture, subscription_id: &str) -> usize {
    std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["ev"] == "NotificationSuppressed" && event["subscription_id"] == subscription_id
        })
        .count()
}

fn has_direct_message_subscription(status: &Value) -> bool {
    status["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|subscription| subscription["event"] == "direct-message")
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

#[test]
fn current_master_cancel_projects_status_repeats_idempotently_and_exact_subscribe_rearms() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    record_project_progress(&fixture);

    let cancelled = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(cancelled["event"], "master-idle", "{cancelled}");
    assert_eq!(cancelled["subject"], "project-patrol", "{cancelled}");
    assert_eq!(
        cancelled["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled}"
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["changed"], true, "{cancelled}");
    let reason = cancelled["reason"]
        .as_str()
        .expect("cancel receipt carries the durable reason")
        .to_owned();
    assert!(
        reason.contains("fixture no actionable project work")
            && reason.contains("resume_condition="),
        "{cancelled}"
    );

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    let projection = &status["project_patrol"];
    assert_eq!(projection["status"], "cancelled", "{status}");
    assert_eq!(
        projection["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{status}"
    );
    assert_eq!(projection["reason"], reason, "{status}");
    assert_eq!(projection["event"], "master-idle", "{status}");
    assert_eq!(projection["subject"], "project-patrol", "{status}");
    assert_eq!(projection["interval_ms"], 900_000, "{status}");
    assert_eq!(projection["eligibility"]["eligible"], true, "{status}");
    assert!(
        projection["eligibility"]["progress_revision"]
            .as_u64()
            .is_some_and(|revision| revision >= 1),
        "{status}"
    );
    let cancelled_ms = projection["updated_ms"].as_i64().unwrap();
    let suppressions_before = project_patrol_suppressions(&fixture, &patrol_id);

    let repeated = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(
        repeated["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{repeated}"
    );
    assert_eq!(repeated["status"], "cancelled", "{repeated}");
    assert_eq!(repeated["changed"], false, "{repeated}");
    assert_eq!(repeated["reason"], reason, "{repeated}");
    let repeated_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        repeated_status["project_patrol"]["updated_ms"].as_i64(),
        Some(cancelled_ms),
        "a repeated cancel must not refresh the cancellation time: {repeated_status}"
    );
    assert_eq!(
        repeated_status["project_patrol"]["reason"], reason,
        "a repeated cancel must not overwrite the original reason: {repeated_status}"
    );
    assert_eq!(
        project_patrol_suppressions(&fixture, &patrol_id),
        suppressions_before,
        "a repeated cancel must not write another suppression event"
    );

    let rearmed = subscribe_patrol(&fixture);
    assert_eq!(
        rearmed["subscription"]["id"].as_str(),
        Some(patrol_id.as_str()),
        "{rearmed}"
    );
    assert_eq!(rearmed["status"], "rearmed", "{rearmed}");
    assert_eq!(rearmed["changed"], true, "{rearmed}");
    assert_eq!(rearmed["deduplicated"], false, "{rearmed}");
    assert!(
        rearmed["subscription"]["status_reason"].is_null(),
        "explicit recovery clears the cancellation reason: {rearmed}"
    );
    let recovered_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(recovered_status["project_patrol"]["status"], "armed");
    assert_eq!(
        recovered_status["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str())
    );
    assert_eq!(
        project_patrol_subscriptions(&recovered_status).len(),
        1,
        "explicit recovery must not create a second record: {recovered_status}"
    );
}

#[test]
fn cancel_without_recorded_ceo_progress_is_rejected() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    subscribe_patrol(&fixture);
    fixture.run_public(
        &[
            "ceo",
            "promote",
            "--approval",
            "fixture user promotes the current master",
        ],
        THREAD_A,
    );

    let rejected = cancel_patrol_attempt(&fixture, THREAD_A);
    assert!(!rejected.status.success(), "{rejected:?}");
    let text = rejection_text(&rejected);
    assert!(
        text.contains("PROJECT_PATROL_CANCEL_NOT_ELIGIBLE"),
        "{text}"
    );
    assert!(
        text.contains("PROJECT_PATROL_PROGRESS_NOT_RECORDED"),
        "{text}"
    );
    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(status["project_patrol"]["status"], "armed");
    assert_eq!(
        status["project_patrol"]["eligibility"]["blocker"], "PROJECT_PATROL_PROGRESS_NOT_RECORDED",
        "{status}"
    );
}

#[test]
fn cancel_with_actionable_master_or_peer_project_task_is_rejected() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    record_project_progress(&fixture);
    subscribe_patrol(&fixture);
    fixture.run_public(
        &[
            "task",
            "register",
            "MASTER-WORK",
            "--feature",
            "master-work-resource",
        ],
        THREAD_A,
    );
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "task",
            "register",
            "PEER-WORK",
            "--feature",
            "peer-work-resource",
        ],
        THREAD_B,
    );

    let rejected = cancel_patrol_attempt(&fixture, THREAD_A);
    assert!(!rejected.status.success(), "{rejected:?}");
    let text = rejection_text(&rejected);
    assert!(
        text.contains("PROJECT_PATROL_CANCEL_NOT_ELIGIBLE"),
        "{text}"
    );
    assert!(text.contains("PROJECT_PATROL_WORK_REMAINS"), "{text}");

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(status["project_patrol"]["status"], "armed");
    let blockers = status["project_patrol"]["eligibility"]["blocking_tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("eligibility must list blocking tasks: {status}"));
    for task_id in ["MASTER-WORK", "PEER-WORK"] {
        assert!(
            blockers
                .iter()
                .any(|task| task["task_id"].as_str() == Some(task_id)),
            "master or peer project work must block cancellation: {status}"
        );
    }
}

#[test]
fn new_project_task_and_wait_release_rearm_the_same_cancelled_patrol() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    record_project_progress(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    cancel_patrol(&fixture, THREAD_A);
    assert_eq!(
        fixture.run_public(&["notify", "status"], THREAD_A)["project_patrol"]["status"],
        "cancelled"
    );

    fixture.run_public(
        &[
            "task",
            "register",
            "NEW-WORK",
            "--feature",
            "new-work-resource",
        ],
        THREAD_A,
    );
    let after_new_task = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        after_new_task["project_patrol"]["status"], "armed",
        "a new project task must rearm without waiting for the next tick: {after_new_task}"
    );
    assert_eq!(
        after_new_task["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "automatic recovery keeps the same subscription id: {after_new_task}"
    );

    // The new task now blocks cancellation. Close it through the public task
    // owner path and cancel again with only a waiting task left.
    fixture.run_public(
        &[
            "task",
            "close",
            "NEW-WORK",
            "--force",
            "--reason",
            "fixture finished new project work",
        ],
        THREAD_A,
    );
    let cancelled_again = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(cancelled_again["changed"], true, "{cancelled_again}");

    // Build a real durable wait through the public task operations. The peer
    // owns the blocking feature; the master's task waits on it.
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "task",
            "register",
            "BLOCKER",
            "--feature",
            "patrol-shared-resource",
        ],
        THREAD_B,
    );
    let conflict = fixture.command_public(
        &[
            "task",
            "register",
            "WAITER",
            "--feature",
            "patrol-shared-resource",
        ],
        THREAD_A,
    );
    assert!(!conflict.status.success(), "{conflict:?}");
    let conflict_text = rejection_text(&conflict);
    assert!(
        conflict_text.contains("TASK_RESOURCE_CONFLICT"),
        "the waiter task must be created through the resource-conflict path: {conflict_text}"
    );
    fixture.run_public(&["task", "wait", "WAITER", "--for", "BLOCKER"], THREAD_A);

    // A live blocking task is still actionable project work, so cancel stays
    // refused until the blocker leaves the project board.
    let refused = cancel_patrol_attempt(&fixture, THREAD_A);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        rejection_text(&refused).contains("PROJECT_PATROL_WORK_REMAINS"),
        "{}",
        rejection_text(&refused)
    );

    // Force close records the manual termination but deliberately does not
    // release dependents. Finalize cleanup is the public completion owner that
    // clears the waiter's TaskRec.wait after the resource contract is verified.
    fixture.run_public(
        &[
            "task",
            "close",
            "BLOCKER",
            "--force",
            "--reason",
            "fixture releases the blocking feature",
        ],
        THREAD_A,
    );

    // With only the durable wait left, cancellation is eligible again. The
    // waiter's TaskRec.wait is the exemption, not any projected busy flag.
    let cancelled_waiting = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(
        cancelled_waiting["changed"], true,
        "a durable wait is the only remaining work: {cancelled_waiting}"
    );
    assert_eq!(
        fixture.run_public(&["notify", "status"], THREAD_A)["project_patrol"]["status"],
        "cancelled"
    );

    fixture.run_public(&["task", "finalize-cleanup", "BLOCKER"], THREAD_A);
    let released = fixture.run_public(&["task", "status", "WAITER"], THREAD_A);
    assert_eq!(released["status"], "blocked", "{released}");
    assert!(released["wait"].is_null(), "{released}");

    // The public wait release re-arms the same patrol id immediately.
    let after_wait_release = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        after_wait_release["project_patrol"]["status"], "armed",
        "the public wait release must rearm the patrol: {after_wait_release}"
    );
    assert_eq!(
        after_wait_release["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{after_wait_release}"
    );
    assert_eq!(
        project_patrol_subscriptions(&after_wait_release).len(),
        1,
        "wait release must not create a second patrol record: {after_wait_release}"
    );
}

#[test]
fn peer_board_idle_cancellation_survives_master_project_patrol_rearm() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    record_project_progress(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    cancel_patrol(&fixture, THREAD_A);

    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["task", "register", "PEER-PATROL"], THREAD_B);
    let rearmed = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        rearmed["project_patrol"]["status"], "armed",
        "the peer project task must rearm the master patrol: {rearmed}"
    );
    assert_eq!(
        rearmed["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str())
    );

    let cancelled = fixture.run_public(&["notify", "cancel", "--task-id", "PEER-PATROL"], THREAD_B);
    assert_eq!(cancelled["cancelled_task_ids"], json!(["PEER-PATROL"]));
    let peer_status = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(
        peer_status["board_patrol"]["cancelled_tasks"],
        json!(["PEER-PATROL"]),
        "{peer_status}"
    );

    // The master patrol machinery uses the exact project-patrol selector and
    // must not clear the peer's separate board-idle cancellation record.
    let master_again = subscribe_patrol(&fixture);
    assert_eq!(
        master_again["subscription"]["id"].as_str(),
        Some(patrol_id.as_str())
    );
    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(master_status["project_patrol"]["status"], "armed");
    let peer_after = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(
        peer_after["board_patrol"]["cancelled_tasks"],
        json!(["PEER-PATROL"]),
        "{peer_after}"
    );
    assert!(
        master_status["project_board_patrol"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|record| {
                record["cancelled_tasks"].as_array().is_some_and(|tasks| {
                    tasks
                        .iter()
                        .any(|task| task["task_id"].as_str() == Some("PEER-PATROL"))
                })
            }),
        "the master projection must still show the peer cancellation: {master_status}"
    );
}

#[test]
fn same_stable_name_new_session_retains_the_cancelled_project_patrol() {
    let mut fixture = AppFixture::new();
    let original = restore_master(&fixture);
    let old_worker = original["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    record_project_progress(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    cancel_patrol(&fixture, THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.append_name_events(&[json!({
        "ev": "AgentPresenceObserved",
        "presence": {
            "project_scope": std::fs::canonicalize(&fixture.project).unwrap(),
            "worker_id": old_worker.as_str(),
            "session_id": THREAD_A,
            "presence": "unknown",
            "reason": "fixture old Jenny carrier stopped responding",
            "observed_ms": 1
        }
    })]);

    let restored = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user confirms replacing Jenny","replace_confirmed":true}"#,
        ],
        THREAD_B,
    );
    assert_eq!(restored["name"], "Jenny", "{restored}");
    let new_worker = restored["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_worker, old_worker, "{restored}");
    assert_eq!(restored["master"]["worker_id"], new_worker, "{restored}");

    let carried = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(
        carried["project_patrol"]["status"], "cancelled",
        "a same-name new session must inherit the cancelled patrol: {carried}"
    );
    assert_eq!(
        carried["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str())
    );
    assert_eq!(carried["project_patrol"]["worker_id"], new_worker);
    let records = project_patrol_subscriptions(&carried);
    assert_eq!(records.len(), 1, "{carried}");
    assert_eq!(records[0]["worker_id"], new_worker);
    assert_eq!(records[0]["status"], "cancelled");

    let old_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert!(
        !project_patrol_subscriptions(&old_status)
            .iter()
            .any(|record| record["worker_id"] == old_worker),
        "the old carrier must not keep the patrol: {old_status}"
    );
    let old_rejected = cancel_patrol_attempt(&fixture, THREAD_A);
    assert!(!old_rejected.status.success(), "{old_rejected:?}");
    assert!(
        rejection_text(&old_rejected).contains("PROJECT_PATROL_MASTER_REQUIRED"),
        "{}",
        rejection_text(&old_rejected)
    );

    let rearmed = subscribe_patrol_on(&fixture, THREAD_B);
    assert_eq!(
        rearmed["subscription"]["id"].as_str(),
        Some(patrol_id.as_str()),
        "the new carrier must recover the same id: {rearmed}"
    );
    assert_eq!(rearmed["status"], "rearmed", "{rearmed}");
    let recancelled = cancel_patrol(&fixture, THREAD_B);
    assert_eq!(recancelled["status"], "cancelled", "{recancelled}");
    assert_eq!(
        recancelled["subscription_id"].as_str(),
        Some(patrol_id.as_str())
    );
}

#[test]
fn peer_and_other_project_cannot_cancel_this_project_patrol() {
    let fixture = AppFixture::new();
    restore_master(&fixture);
    record_project_progress(&fixture);
    subscribe_patrol(&fixture);
    fixture.run_public(&["context"], THREAD_B);

    let peer = cancel_patrol_attempt(&fixture, THREAD_B);
    assert!(!peer.status.success(), "{peer:?}");
    assert!(
        rejection_text(&peer).contains("PROJECT_PATROL_MASTER_REQUIRED"),
        "{}",
        rejection_text(&peer)
    );

    // A separate project has its own route and no current master grant.
    let other = AppFixture::new();
    other.run_public(&["context"], THREAD_A);
    let other_project = cancel_patrol_attempt(&other, THREAD_A);
    assert!(!other_project.status.success(), "{other_project:?}");
    assert!(
        rejection_text(&other_project).contains("PROJECT_PATROL_MASTER_REQUIRED"),
        "{}",
        rejection_text(&other_project)
    );
    assert_eq!(
        fixture.run_public(&["notify", "status"], THREAD_A)["project_patrol"]["status"],
        "armed",
        "rejected non-master requests must not change this project's patrol"
    );
}

#[test]
fn direct_message_send_and_receive_survive_a_cancelled_project_patrol() {
    let fixture = AppFixture::new();
    let master = restore_master(&fixture);
    let master_id = master["identity"]["worker_id"].as_str().unwrap().to_owned();
    record_project_progress(&fixture);
    subscribe_patrol(&fixture);
    cancel_patrol(&fixture, THREAD_A);

    let after_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(after_cancel["project_patrol"]["status"], "cancelled");
    assert!(
        has_direct_message_subscription(&after_cancel),
        "direct-message reception must survive project patrol cancellation: {after_cancel}"
    );

    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &master_id,
            "--subject",
            "during-cancelled-patrol",
            "direct delivery remains available",
        ],
        THREAD_B,
    );
    let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_A);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "during-cancelled-patrol")
        .unwrap_or_else(|| panic!("the master must receive direct mail: {received}"));
    let message_id = message["id"].as_str().unwrap().to_owned();
    let consumed = fixture.run_public(&["msg", &message_id], THREAD_A);
    assert_eq!(consumed["state"], "read", "{consumed}");
}

#[test]
fn cancel_supersedes_a_pending_project_patrol_wake_without_delivery() {
    let mut fixture = AppFixture::new();
    let master = restore_master(&fixture);
    let master_id = master["identity"]["worker_id"].as_str().unwrap().to_owned();
    record_project_progress(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();

    let message_id = "msg-fixture-pending-project-patrol";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    fixture.append_name_events(&[
        json!({
            "ev": "Sent",
            "msg": {
                "id": message_id,
                "from": "collab-server",
                "to": master_id,
                "type": "notify",
                "subject": "master-idle:project-patrol",
                "body": "stale project patrol wake",
                "created_ms": now,
                "state": "pending",
                "wake_attempt_count": 1,
                "last_wake_attempt_ms": 0,
                "retry_attempted": true
            }
        }),
        json!({
            "ev": "DeliveryMode",
            "msg_id": message_id,
            "mode": "explicit-notification",
            "source_thread_id": null
        }),
        json!({
            "ev": "WakeBound",
            "message_id": message_id,
            "subscription_id": patrol_id
        }),
    ]);
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_A)["state"],
        "pending",
        "the fixture must seed one undelivered pending patrol wake"
    );

    let cancelled = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(cancelled["changed"], true, "{cancelled}");
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_A)["state"],
        "superseded",
        "cancel must end the pending wake's delivery eligibility"
    );
    let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        events.iter().any(|event| {
            event["ev"] == "Superseded"
                && event["ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == message_id))
        }),
        "cancel must commit the existing supersede event"
    );
    assert!(
        !events.iter().any(|event| {
            event["ev"] == "NotificationDeliveryAccepted" && event["message_id"] == message_id
        }),
        "a superseded pending wake must never be accepted as a fresh patrol"
    );

    // A daemon restart must replay the same terminal state, not a fresh wake.
    fixture.restart_daemon();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_A)["state"],
        "superseded"
    );
    let replay: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        !replay.iter().any(|event| {
            event["ev"] == "NotificationDeliveryAccepted" && event["message_id"] == message_id
        }),
        "replay must not deliver the superseded patrol wake"
    );
}

#[test]
fn expired_historical_patrol_does_not_shadow_the_current_control_record() {
    let mut fixture = AppFixture::new();
    let master = restore_master(&fixture);
    let master_id = master["identity"]["worker_id"].as_str().unwrap().to_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let expired_id = "project-patrol-expired";
    fixture.append_name_events(&[json!({
        "ev": "NotificationSubscribed",
        "subscription": {
            "id": expired_id,
            "worker_id": master_id,
            "event": "master-idle",
            "subject": "project-patrol",
            "target": THREAD_A,
            "method": "appserver",
            "trigger_ms": now - 7_200_000,
            "trigger_times_ms": [],
            "interval_ms": 900_000,
            "repeat_count": 100,
            "fired_count": 100,
            "expires_ms": now - 3_600_000,
            "status": "armed",
            "created_ms": now - 86_400_000,
            "updated_ms": now - 7_200_000,
            "status_reason": null
        }
    })]);

    // The expired record is older, so an oldest-first selector without an
    // expiry filter would reuse it instead of ensuring the live record.
    let subscribed = subscribe_patrol(&fixture);
    let current_id = subscribed["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(
        current_id, expired_id,
        "the expired historical record must never be reused: {subscribed}"
    );
    let armed_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        armed_status["project_patrol"]["subscription_id"].as_str(),
        Some(current_id.as_str()),
        "the projection must name the live record: {armed_status}"
    );
    assert_eq!(
        armed_status["project_patrol"]["status"], "armed",
        "{armed_status}"
    );

    record_project_progress(&fixture);
    let cancelled = cancel_patrol(&fixture, THREAD_A);
    assert_eq!(
        cancelled["subscription_id"].as_str(),
        Some(current_id.as_str()),
        "cancel must target the current live record: {cancelled}"
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");

    let after_cancel = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        after_cancel["project_patrol"]["status"], "cancelled",
        "{after_cancel}"
    );
    assert_eq!(
        after_cancel["project_patrol"]["subscription_id"].as_str(),
        Some(current_id.as_str()),
        "{after_cancel}"
    );
    let expired_record = project_patrol_subscriptions(&after_cancel)
        .into_iter()
        .find(|record| record["id"] == expired_id)
        .unwrap_or_else(|| {
            panic!("the expired historical record must stay observable: {after_cancel}")
        });
    assert_eq!(
        expired_record["status"], "expired",
        "the expired historical record stays retired from control: {after_cancel}"
    );
    assert_eq!(
        expired_record["expires_ms"].as_i64(),
        Some(now - 3_600_000),
        "cancel must never renew the expired historical record: {after_cancel}"
    );
}

#[test]
fn project_patrol_rearm_survives_replay_without_refreshing_schedule() {
    let mut fixture = AppFixture::new();
    restore_master(&fixture);
    record_project_progress(&fixture);
    let armed = subscribe_patrol(&fixture);
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    let armed_record =
        project_patrol_subscriptions(&fixture.run_public(&["notify", "status"], THREAD_A))
            .into_iter()
            .find(|record| record["id"] == patrol_id)
            .unwrap();
    assert!(
        armed_record["trigger_ms"].is_null(),
        "the initial standing patrol has no wake until its first renewal: {armed_record}"
    );

    cancel_patrol(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "task",
            "register",
            "REPLAY-WORK",
            "--feature",
            "replay-work-resource",
        ],
        THREAD_A,
    );
    let rearmed_status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(
        rearmed_status["project_patrol"]["status"], "armed",
        "the new project task must rearm the patrol: {rearmed_status}"
    );
    assert_eq!(
        rearmed_status["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{rearmed_status}"
    );
    let rearmed_record = project_patrol_subscriptions(&rearmed_status)
        .into_iter()
        .find(|record| record["id"] == patrol_id)
        .unwrap();
    assert!(
        rearmed_record["trigger_ms"].as_i64().is_some(),
        "the rearm must schedule the next wake: {rearmed_status}"
    );

    // Replay must reapply the persisted task event time, not the restart time.
    fixture.restart_daemon();
    let replayed_status = fixture.run_public(&["notify", "status"], THREAD_A);
    let replayed_record = project_patrol_subscriptions(&replayed_status)
        .into_iter()
        .find(|record| record["id"] == patrol_id)
        .unwrap_or_else(|| panic!("replay must keep the rearmed patrol: {replayed_status}"));
    for field in ["status", "trigger_ms", "expires_ms", "updated_ms"] {
        assert_eq!(
            replayed_record[field], rearmed_record[field],
            "replay must reproduce {field} from the persisted task event: {replayed_status}"
        );
    }
}
