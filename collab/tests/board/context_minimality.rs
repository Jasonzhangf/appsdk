use super::*;

#[test]
fn context_effective_template_text_occurs_once() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &["master", "promote", "--approval", "context minimality test"],
        THREAD_A,
    );
    let template = json!({
        "role_task": "UNIQUE_CONTEXT_TASK_MARKER",
        "responsibilities": ["UNIQUE_CONTEXT_DUTY_MARKER"],
        "next_action": "UNIQUE_CONTEXT_NEXT_MARKER",
        "completion_action": "UNIQUE_CONTEXT_DONE_MARKER"
    })
    .to_string();
    let context = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "master",
            "--workflow-template",
            &template,
        ],
        THREAD_A,
    );
    let raw = context.to_string();
    for marker in [
        "UNIQUE_CONTEXT_TASK_MARKER",
        "UNIQUE_CONTEXT_DUTY_MARKER",
        "UNIQUE_CONTEXT_NEXT_MARKER",
        "UNIQUE_CONTEXT_DONE_MARKER",
    ] {
        assert_eq!(
            raw.matches(marker).count(),
            1,
            "duplicated effective workflow: {context}"
        );
    }
    assert_eq!(
        context["role_brief"]["workflow_template"]["identity"]["revision"],
        1
    );
}

#[test]
fn context_hides_terminal_history_and_keeps_obligations() {
    let mut fixture = AppFixture::new();
    let master = fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let master_id = master["identity"]["worker_id"].as_str().unwrap();
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "context minimality test"],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "publish",
            "context-current-task",
            "--title",
            "Current work",
            "--description",
            "Keep current obligations",
            "--delivery-condition",
            "Current context",
            "--test-condition",
            "Public context test",
            "--priority",
            "p1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "context-current-task",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    let request = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            peer_id,
            "--subject",
            "context-required-reply",
            "--reply-required",
            "Return actual result",
        ],
        THREAD_A,
    );
    let now = 1_900_000_000_000_i64;
    let mut events = Vec::new();
    let terminal = [
        ("hidden-consumed", "consumed"),
        ("hidden-expired", "expired"),
        ("hidden-cancelled", "cancelled"),
        ("hidden-rebound", "rebound"),
    ];
    for (id, status) in terminal.into_iter().chain([("visible-active", "armed")]) {
        events.push(json!({"ev":"NotificationSubscribed", "subscription":{
            "id":id, "worker_id":peer_id, "event":"deadline", "subject":id,
            "target":THREAD_B, "method":"appserver", "trigger_ms":null,
            "trigger_times_ms":[], "interval_ms":null, "repeat_count":1,
            "fired_count":1, "expires_ms":now+600_000, "status":status,
            "created_ms":now, "updated_ms":now
        }}));
    }
    for (task, owner) in [
        ("context-current-task", peer_id),
        ("unrelated-merge-task", master_id),
    ] {
        events.push(json!({"ev":"MergeRequested", "request":{
            "task_id":task, "owner":owner, "requested_by":master_id,
            "requested_ms":now, "candidate_commit":null
        }}));
    }
    fixture.append_name_events(&events);
    let output = fixture.command_public(&["context"], THREAD_B);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = &receipt["result"]["snapshot"];
    let raw = receipt.to_string();
    for (id, _) in terminal {
        assert!(!raw.contains(id), "terminal subscription leaked: {id}");
    }
    assert!(raw.contains("visible-active"));
    assert!(context["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == "context-current-task"));
    assert!(context["pending_merges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["task_id"] == "context-current-task"));
    assert!(context["pending_reply_obligations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["message_id"] == request["msg_id"]));
    let peer_merge_tasks: Vec<&str> = context["pending_merges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|merge| merge["task_id"].as_str())
        .collect();
    assert!(
        !peer_merge_tasks.contains(&"unrelated-merge-task"),
        "peer context leaked an unrelated merge: {peer_merge_tasks:?}"
    );
    assert!(
        !context["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["kind"] == "merge_pending"),
        "peer received a master merge instruction: {}",
        context["operations"]
    );
    assert_eq!(context["peers"].as_array().unwrap().len(), 2);
    let master_context = fixture.run_public(&["context"], THREAD_A);
    let master_merge_tasks: Vec<&str> = master_context["pending_merges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|merge| merge["task_id"].as_str())
        .collect();
    assert!(
        master_merge_tasks.contains(&"context-current-task"),
        "{master_merge_tasks:?}"
    );
    assert!(
        master_merge_tasks.contains(&"unrelated-merge-task"),
        "{master_merge_tasks:?}"
    );
    let master_merge_operations: Vec<&Value> = master_context["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["kind"] == "merge_pending")
        .collect();
    assert_eq!(
        master_merge_operations.len(),
        1,
        "{}",
        master_context["operations"]
    );
    assert_eq!(
        master_merge_operations[0]["pending_merges_ref"],
        "pending_merges"
    );
    assert!(
        master_merge_operations[0].get("pending_merges").is_none(),
        "merge operation copied the pending array"
    );
    let history = fixture.command_public(&["notify", "status"], THREAD_B);
    assert!(history.status.success());
    let history: Value = serde_json::from_slice(&history.stdout).unwrap();
    for (id, status) in terminal {
        assert!(
            history.to_string().contains(id),
            "terminal history lost: {id}"
        );
        assert!(history.to_string().contains(status));
    }
}

#[test]
fn context_pending_merges_keep_requester_relevant_merge_for_peer() {
    let mut fixture = AppFixture::new();
    let master = fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let master_id = master["identity"]["worker_id"].as_str().unwrap();
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "context minimality test"],
        THREAD_A,
    );
    let now = 1_900_000_000_000_i64;
    let events = [
        json!({"ev":"MergeRequested", "request":{
            "task_id":"requester-merge-task", "owner":master_id, "requested_by":peer_id,
            "requested_ms":now, "candidate_commit":null
        }}),
        json!({"ev":"MergeRequested", "request":{
            "task_id":"unrelated-merge-task", "owner":master_id, "requested_by":master_id,
            "requested_ms":now, "candidate_commit":null
        }}),
    ];
    fixture.append_name_events(&events);
    let context = fixture.run_public(&["context"], THREAD_B);
    let peer_merge_tasks: Vec<&str> = context["pending_merges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|merge| merge["task_id"].as_str())
        .collect();
    assert!(
        peer_merge_tasks.contains(&"requester-merge-task"),
        "{peer_merge_tasks:?}"
    );
    assert!(
        !peer_merge_tasks.contains(&"unrelated-merge-task"),
        "{peer_merge_tasks:?}"
    );
    assert!(
        !context["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["kind"] == "merge_pending"),
        "peer received a master merge instruction: {}",
        context["operations"]
    );
}

#[test]
fn ordinary_peer_context_hides_unrelated_closed_history_and_keeps_obligations() {
    let mut fixture = AppFixture::new();
    let master = fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let master_id = master["identity"]["worker_id"].as_str().unwrap();
    let peer_id = peer["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(
        &["master", "promote", "--approval", "context minimality test"],
        THREAD_A,
    );

    let now = 1_900_000_000_000_i64;
    let mut events: Vec<Value> = vec![
        // Owned closed code task still needs review and integration.
        task_event("owned-needs-review", peer_id, "closed"),
        // Owned closed code task is delivered, reviewed and integrated but its
        // worktree cleanup has no verified receipt, so a duty still remains.
        json!({"ev":"TaskCreated", "task":{
            "id":"owned-cleanup-open", "owner":peer_id, "created_by":peer_id,
            "status":"closed", "priority":"p2", "branch":"task/owned-cleanup-open",
            "created_ms":now, "updated_ms":now
        }}),
        // Owned closed code task is fully closed and carries no duty.
        task_event("owned-closed-clean", peer_id, "closed"),
        // Another caller's closed history.
        json!({"ev":"TaskCreated", "task":{
            "id":"unrelated-closed-task", "owner":master_id, "created_by":master_id,
            "status":"closed", "priority":"p2", "created_ms":now, "updated_ms":now
        }}),
    ];
    events.extend([
        json!({"ev":"TaskLifecycleUpdated", "task_id":"owned-needs-review", "record":{
            "delivery_kind":"code", "delivery_evidence":"candidate evidence",
            "delivered_ms":now, "delivery_commit":"candidate-needs-review"
        }}),
        // Delivery, review and integration are complete; only cleanup is open.
        json!({"ev":"TaskLifecycleUpdated", "task_id":"owned-cleanup-open", "record":{
            "delivery_kind":"code", "delivery_evidence":"candidate evidence",
            "delivered_ms":now, "delivery_commit":"candidate-cleanup-open",
            "review_evidence":"independent review pass", "reviewer":master_id,
            "reviewed_ms":now, "integration_commit":"main-cleanup-open", "integrated_ms":now
        }}),
        json!({"ev":"TaskLifecycleUpdated", "task_id":"owned-closed-clean", "record":{
            "delivery_kind":"code", "delivery_evidence":"candidate evidence",
            "delivered_ms":now, "delivery_commit":"candidate-clean",
            "review_evidence":"independent review pass", "reviewer":master_id,
            "reviewed_ms":now, "integration_commit":"main-clean", "integrated_ms":now
        }}),
        json!({"ev":"CleanupVerified", "receipt":{
            "id":"cleanup-clean", "task_id":"owned-closed-clean", "verification":"verified",
            "verified_ms":now
        }}),
        // A peer-owned merge obligation is a real current fact, not history.
        json!({"ev":"MergeRequested", "request":{
            "task_id":"owned-needs-review", "owner":peer_id, "requested_by":peer_id,
            "requested_ms":now, "candidate_commit":null
        }}),
        // Caller-as-parent records: the peer orchestrates these managed
        // subagents, so they never change the caller's own identity.
        subagent_event("hidden-closed-parent", "closed", peer_id, master_id),
        subagent_event("visible-failed-parent", "failed", peer_id, master_id),
        // Truly foreign records: neither parent nor peer is the caller. A
        // foreign failure never transfers responsibility and stays hidden.
        subagent_event(
            "unrelated-closed-subagent",
            "closed",
            master_id,
            "other-parent",
        ),
        subagent_event(
            "unrelated-failed-subagent",
            "failed",
            master_id,
            "other-parent",
        ),
    ]);
    fixture.append_name_events(&events);

    // A peer-owned reply obligation is a real current fact.
    let request = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            peer_id,
            "--subject",
            "context-reply",
            "--reply-required",
            "Return result",
        ],
        THREAD_A,
    );
    // Baseline the peer message counter before the master-only traffic arrives.
    let baseline = fixture.run_public(&["context"], THREAD_B);
    let baseline_messages = baseline["summary"]["messages"].as_u64().unwrap();
    fixture.append_name_events(&[
        // Caller-related message history (peer is sender) still counts.
        json!({"ev":"Sent", "msg":{
            "id":"peer-authored-history", "from":peer_id, "to":master_id,
            "type":"notify", "subject":"peer history", "body":"history",
            "created_ms":now, "state":"read", "wake_attempt_count":0,
            "last_wake_attempt_ms":0, "retry_attempted":false
        }}),
        // Master-only history must not grow the peer message counter or detail.
        json!({"ev":"Sent", "msg":{
            "id":"unrelated-history-message", "from":master_id, "to":master_id,
            "type":"notify", "subject":"unrelated history", "body":"history",
            "created_ms":now, "state":"read", "wake_attempt_count":0,
            "last_wake_attempt_ms":0, "retry_attempted":false
        }}),
    ]);

    let context = fixture.run_public(&["context"], THREAD_B);
    let raw = context.to_string();
    assert!(
        !raw.contains("owned-closed-clean"),
        "clean owned closed task leaked: {context}"
    );
    assert!(
        raw.contains("owned-needs-review"),
        "owned unresolved closed task was hidden: {context}"
    );
    assert!(
        raw.contains("owned-cleanup-open"),
        "owned unresolved cleanup task was hidden: {context}"
    );
    assert!(
        !raw.contains("unrelated-closed-task"),
        "unrelated closed task leaked: {context}"
    );
    assert!(
        !raw.contains("unrelated-history-message"),
        "master-only history leaked: {context}"
    );
    assert!(
        context["master_wake"].is_null(),
        "ordinary peer received master_wake ledger: {context}"
    );
    assert_eq!(
        context["summary"]["messages"].as_u64().unwrap(),
        baseline_messages + 1,
        "master-only history grew the peer message counter: {context}"
    );

    let task_ids: Vec<&str> = context["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(
        task_ids,
        vec!["owned-cleanup-open", "owned-needs-review"],
        "{task_ids:?}"
    );
    assert_eq!(context["summary"]["tasks"], 2, "{context}");
    let cleanup_open = context["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "owned-cleanup-open")
        .expect("cleanup-open task missing from peer context");
    assert_eq!(
        cleanup_open["completion_missing"],
        json!(["cleanup"]),
        "cleanup-open task must still owe cleanup: {cleanup_open}"
    );
    let merge_tasks: Vec<&str> = context["pending_merges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|merge| merge["task_id"].as_str())
        .collect();
    assert!(
        merge_tasks.contains(&"owned-needs-review"),
        "{merge_tasks:?}"
    );
    assert!(
        !context["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["kind"] == "merge_pending"),
        "peer received a master merge instruction: {}",
        context["operations"]
    );
    assert!(
        context["pending_reply_obligations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|obligation| obligation["message_id"] == request["msg_id"]),
        "peer lost its reply obligation: {context}"
    );
    assert_eq!(
        context["peers"].as_array().unwrap().len(),
        2,
        "peer directory changed: {context}"
    );

    // The managed records above are the caller's children, not the caller's
    // own identity. The peer must stay a wire worker with peer derivation and
    // peer authority, never a managed-subagent.
    assert_eq!(
        context["identity"]["kind"], "peer",
        "caller identity kind flipped: {context}"
    );
    assert_eq!(
        context["identity"]["role"], "worker",
        "caller identity role flipped to managed-subagent: {context}"
    );
    assert_eq!(
        context["role_brief"]["role"], "worker",
        "caller role brief flipped to managed-subagent: {context}"
    );
    assert_eq!(
        context["role_brief"]["derivation"]["kind"], "peer",
        "caller derivation flipped: {context}"
    );
    assert!(
        context["role_brief"]["derivation"]["parent"].is_null(),
        "caller derivation gained a parent: {context}"
    );
    assert_eq!(
        context["role_brief"]["authority"]["managed_subagent"], false,
        "{context}"
    );
    assert_eq!(
        context["role_brief"]["authority"]["must_obey_master"], false,
        "{context}"
    );
    assert_eq!(
        context["role_brief"]["authority"]["may_decline_master_invite"], true,
        "{context}"
    );

    let subagent_ids: Vec<&str> = context["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|record| record["id"].as_str())
        .collect();
    assert_eq!(
        subagent_ids,
        vec!["visible-failed-parent"],
        "{subagent_ids:?}"
    );
    assert_eq!(context["summary"]["subagents"], 1, "{context}");
    for hidden in [
        "hidden-closed-parent",
        "unrelated-closed-subagent",
        "unrelated-failed-subagent",
    ] {
        assert!(
            !raw.contains(hidden),
            "default-filtered managed history leaked: {hidden}: {context}"
        );
    }

    // The master keeps the original project-wide stats and managed-child view.
    let master_context = fixture.run_public(&["context"], THREAD_A);
    let master_subagent_ids: Vec<&str> = master_context["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|record| record["id"].as_str())
        .collect();
    for visible in [
        "hidden-closed-parent",
        "unrelated-closed-subagent",
        "unrelated-failed-subagent",
        "visible-failed-parent",
    ] {
        assert!(
            master_subagent_ids.contains(&visible),
            "master lost project child view: {visible}: {master_context}"
        );
    }
    assert!(
        master_context["summary"]["tasks"].as_u64().unwrap() >= 4,
        "master lost project task stats: {master_context}"
    );
    let master_task_ids: Vec<&str> = master_context["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(
        master_task_ids.contains(&"unrelated-closed-task"),
        "{master_task_ids:?}"
    );

    let task_diagnostic =
        fixture.command_public(&["task", "status", "owned-closed-clean"], THREAD_B);
    assert!(
        task_diagnostic.status.success(),
        "{}",
        String::from_utf8_lossy(&task_diagnostic.stderr)
    );
    let task_diagnostic: Value = serde_json::from_slice(&task_diagnostic.stdout).unwrap();
    assert_eq!(
        task_diagnostic["id"], "owned-closed-clean",
        "task diagnostic lost clean task: {task_diagnostic}"
    );
    let unreachable =
        fixture.command_public(&["task", "status", "unrelated-closed-task"], THREAD_B);
    assert!(
        unreachable.status.success(),
        "{}",
        String::from_utf8_lossy(&unreachable.stderr)
    );
    let unreachable: Value = serde_json::from_slice(&unreachable.stdout).unwrap();
    assert_eq!(
        unreachable["id"], "unrelated-closed-task",
        "task diagnostic lost foreign task: {unreachable}"
    );

    // Explicit subagent diagnostics still expose default-filtered managed records.
    let subagent_diagnostic = fixture.command_public(&["subagent", "list"], THREAD_B);
    assert!(
        subagent_diagnostic.status.success(),
        "{}",
        String::from_utf8_lossy(&subagent_diagnostic.stderr)
    );
    let subagent_diagnostic: Value = serde_json::from_slice(&subagent_diagnostic.stdout).unwrap();
    let subagent_diagnostic_ids: Vec<&str> = subagent_diagnostic["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|record| record["id"].as_str())
        .collect();
    assert!(
        subagent_diagnostic_ids.contains(&"hidden-closed-parent"),
        "subagent diagnostic lost default-filtered record: {subagent_diagnostic_ids:?}"
    );
}

fn task_event(id: &str, owner: &str, status: &str) -> Value {
    json!({"ev":"TaskCreated", "task":{
        "id":id, "owner":owner, "created_by":owner, "status":status,
        "priority":"p2", "created_ms":1_900_000_000_000_i64, "updated_ms":1_900_000_000_001_i64
    }})
}

fn subagent_event(id: &str, status: &str, parent: &str, peer: &str) -> Value {
    json!({"ev":"SubagentUpdated", "subagent":{
        "id":id, "parent":parent, "peer":peer, "status":status,
        "thread_id":null, "profile":null, "created_ms":1_900_000_000_100_i64,
        "ready_deadline_ms":1_900_000_001_100_i64, "last_message":null, "error":null
    }})
}
