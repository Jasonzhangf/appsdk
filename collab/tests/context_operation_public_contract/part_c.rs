/// Stale the identity produced by an already-completed seed so an approved
/// recovery can be staged against the exact committed incumbent fence.
fn stale_seeded_identity(fixture: &Fixture, seed: &Value) -> IdentityFence {
    let worker_id = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("seed snapshot has worker id")
        .to_owned();
    let binding = &seed["result"]["snapshot"]["binding"];
    let binding_id = binding["binding_id"]
        .as_str()
        .expect("seed snapshot has binding id")
        .to_owned();
    let endpoint_generation = binding["endpoint_generation"]
        .as_u64()
        .expect("seed snapshot has endpoint generation");
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let mut identity: Value = serde_json::from_slice(
        &fs::read(&identity_path).expect("read isolated fixture identity cache"),
    )
    .expect("isolated fixture identity cache is JSON");
    let token = identity["token"]
        .as_str()
        .expect("isolated fixture identity has token")
        .to_owned();
    identity["token"] = json!(format!("stale-{token}"));
    let stale_identity_bytes =
        serde_json::to_vec_pretty(&identity).expect("serialize isolated fixture identity cache");
    fs::write(&identity_path, &stale_identity_bytes)
        .expect("write isolated fixture identity cache");
    IdentityFence {
        worker_id,
        binding_id,
        endpoint_generation,
        identity_path,
        stale_identity_bytes,
    }
}

/// Cancel one CLI invocation at the pre-send boundary. The fixture holds the
/// process before the first mutation byte, sends SIGINT, waits until the
/// process acknowledges it consumed the signal, and only then releases the
/// barrier so the local decision is never raced by the release.
fn cli_pre_send_cancel(
    fixture: &Fixture,
    pane: &Pane,
    harness: &mut CancellationHarness,
    operation_id: &str,
    approval: Option<&Value>,
    provide: Option<&Value>,
) -> std::process::Output {
    harness.arm("client_pre_send", operation_id);
    harness.arm("client_pre_send_signalled", operation_id);
    let mut child = spawn_context_cli(
        fixture,
        Some(pane),
        Some(operation_id),
        None,
        None,
        approval,
        None,
        provide,
        false,
    );
    let cli_pid = child.id() as i32;
    let barrier =
        match harness.try_wait_for("client_pre_send", operation_id, Duration::from_secs(30)) {
            Ok(barrier) => barrier,
            Err(error) => {
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .expect("collect failed context child");
                panic!(
                    "{error}; context child status={:?}: {}",
                    output.status.code(),
                    text(&output)
                );
            }
        };
    assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
    harness
        .wait_for(
            "client_pre_send_signalled",
            operation_id,
            Duration::from_secs(30),
        )
        .release();
    barrier.release();
    wait_child_output(&mut child, Duration::from_secs(30))
}

/// Cancel one already-admitted CLI invocation exactly at a daemon-owned safe
/// boundary and return its drained public output. The durable cancellation is
/// observed before execution resumes, so the daemon cannot race past the
/// boundary with an un-cancelled owner start.
fn cli_cancel_at(
    fixture: &Fixture,
    pane: &Pane,
    harness: &mut CancellationHarness,
    boundary: &str,
    operation_id: &str,
    approval: Option<&Value>,
    provide: Option<&Value>,
) -> std::process::Output {
    harness.arm(boundary, operation_id);
    let mut child = spawn_context_cli(
        fixture,
        Some(pane),
        Some(operation_id),
        None,
        None,
        approval,
        None,
        provide,
        false,
    );
    let cli_pid = child.id() as i32;
    let barrier = harness.wait_for(boundary, operation_id, Duration::from_secs(30));
    assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
    assert!(
        wait_until(Duration::from_secs(30), || {
            operation_records(&fixture.host_journal(), operation_id)
                .iter()
                .any(|record| record["phase"] == "cancelled")
        }),
        "the daemon must durably cancel operation {operation_id} before the {boundary} boundary releases"
    );
    barrier.release();
    wait_child_output(&mut child, Duration::from_secs(30))
}

#[test]
fn c12_sigint_before_admission_returns_local_cancelled_without_daemon_operation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let mut harness = CancellationHarness::start(&unique_root("c12-hook"));
    let (mut fixture, pane) = Fixture::isolated("collab-v23-c12");
    fixture.enable_cancellation_hooks(harness.socket());
    let seed = fixture.context(Some("ctxop-v23-c12-seed"), Some(&pane));
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let supplement = json!({"session_id": pane.session_anchor});

    // 1) Pre-admission local cancellation before the first mutation byte for
    //    automatic, supplement and approved recovery through the public CLI.
    // An approved-recovery invocation only reaches the pre-send decision when
    // it carries a well-formed approval object; the content is never validated
    // because the local cancellation wins before any mutation byte is sent.
    let placeholder_approval = json!({
        "decision": "approved",
        "decided_by": "user",
        "target_identity": "peer-fixture",
        "project_scope": canonical_root(&fixture),
        "app_scope_id": "appserver-cli",
        "action": "replace_binding",
        "expected_incumbent": {"binding_id": "bind-placeholder", "endpoint_generation": 1},
        "intent_digest": "sha256:placeholder",
        "approved_at_ms": 1770000000000_i64
    });
    let variants = [
        ("automatic", "ctxop-v23-c12-auto", None, None),
        (
            "supplement",
            "ctxop-v23-c12-supp",
            None,
            Some(supplement.clone()),
        ),
        (
            "approved_recovery",
            "ctxop-v23-c12-approved",
            Some(placeholder_approval.clone()),
            None,
        ),
    ];
    for (label, operation_id, approval, provide) in variants {
        let output = cli_pre_send_cancel(
            &fixture,
            &pane,
            &mut harness,
            operation_id,
            approval.as_ref(),
            provide.as_ref(),
        );
        assert_eq!(output.status.code(), Some(2), "{label}: {}", text(&output));
        let payload = typed_failure(&output);
        assert_eq!(
            payload["result"]["outcome"], "cancelled",
            "{label}: {payload}"
        );
        assert_eq!(
            payload["result"]["phase"],
            Value::Null,
            "{label}: {payload}"
        );
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([]),
            "{label}: {payload}"
        );
        assert!(
            operation_records(&fixture.host_journal(), operation_id).is_empty(),
            "{label}: a pre-admission cancellation must not create a daemon operation record"
        );
    }

    // 2) MCP standard cancellation notification before admission: the targeted
    //    tool result is suppressed and no daemon operation is created.
    let mcp_operation = "ctxop-v23-c12-mcp";
    harness.arm("client_pre_send", mcp_operation);
    harness.arm("client_pre_send_signalled", mcp_operation);
    let mut mcp = McpReader::start(&fixture, Some(&pane));
    let request_id = 41_u64;
    mcp.send_raw(&json!({
        "jsonrpc":"2.0",
        "id":request_id,
        "method":"tools/call",
        "params":{"name":"collab_context","arguments":{"operation_id":mcp_operation}}
    }));
    let barrier = harness.wait_for("client_pre_send", mcp_operation, Duration::from_secs(30));
    mcp.cancel(request_id);
    harness
        .wait_for(
            "client_pre_send_signalled",
            mcp_operation,
            Duration::from_secs(30),
        )
        .release();
    barrier.release();
    assert!(
        mcp.recv(Duration::from_secs(5)).is_err(),
        "a local pre-send MCP cancellation must not produce a tool result"
    );
    assert!(
        operation_records(&fixture.host_journal(), mcp_operation).is_empty(),
        "a local pre-send MCP cancellation must not create a daemon operation"
    );
    mcp.finish();

    // 3) After durable admission, before the first owner: the daemon wins the
    //    arbitration and returns the durable cancelled projection with no
    //    committed phases.
    let admitted_operation = "ctxop-v23-c12-admitted";
    let admitted = cli_cancel_at(
        &fixture,
        &pane,
        &mut harness,
        "admitted_before_owner",
        admitted_operation,
        None,
        None,
    );
    assert_eq!(admitted.status.code(), Some(2), "{}", text(&admitted));
    let admitted_payload = typed_failure(&admitted);
    assert_eq!(
        admitted_payload["result"]["outcome"], "cancelled",
        "{admitted_payload}"
    );
    assert_eq!(
        admitted_payload["result"]["committed_phases"],
        json!([]),
        "{admitted_payload}"
    );
    let admitted_records = operation_records(&fixture.host_journal(), admitted_operation);
    assert_eq!(
        admitted_records
            .iter()
            .filter(|record| record["phase"] == "cancelled")
            .count(),
        1,
        "admission-then-cancellation must be durable exactly once: {admitted_records:?}"
    );

    // 4) After proven committed owner work, before the next owner: cancellation
    //    preserves exactly the committed receipts and performs no next effect.
    let between_operation = "ctxop-v23-c12-between";
    let between = cli_cancel_at(
        &fixture,
        &pane,
        &mut harness,
        "between_owners",
        between_operation,
        None,
        None,
    );
    assert_eq!(between.status.code(), Some(2), "{}", text(&between));
    let between_payload = typed_failure(&between);
    assert_eq!(
        between_payload["result"]["outcome"], "cancelled",
        "{between_payload}"
    );
    assert_eq!(
        between_payload["result"]["committed_phases"],
        json!(["nested_register", "route", "credential", "lease"]),
        "{between_payload}"
    );
    let between_records = operation_records(&fixture.host_journal(), between_operation);
    assert_eq!(
        between_records
            .iter()
            .filter(|record| record["phase"] == "cancelled")
            .count(),
        1,
        "{between_records:?}"
    );

    // 5) Restart + query: both admitted cancellations survive exactly, while
    //    the pre-admission cancellations remain unknown.
    fixture.stop_daemon();
    fixture.restart_daemon();
    for (operation_id, expected_receipts) in [
        (admitted_operation, json!([])),
        (
            between_operation,
            json!(["nested_register", "route", "credential", "lease"]),
        ),
    ] {
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "cancelled",
            "{operation_id}: {query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"], expected_receipts,
            "{operation_id}: {query}"
        );
    }
    for operation_id in [
        "ctxop-v23-c12-auto",
        "ctxop-v23-c12-supp",
        "ctxop-v23-c12-approved",
        mcp_operation,
    ] {
        let query = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
        let query_text = text(&query);
        assert!(
            !query.status.success(),
            "{operation_id} must not become queryable: {query_text}"
        );
        assert!(
            query_text.contains("IDENTITY_OPERATION_UNKNOWN"),
            "{operation_id}: {query_text}"
        );
    }
}

#[test]
fn c13_cancellation_after_owner_begins_never_claims_cancelled() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();

    // 1) Register owner already started: the exact nested `CommandStarted` has
    //    synced before the boundary is acknowledged. Cancellation is refused
    //    for this invocation, so the operation must finish truthfully instead
    //    of ever claiming cancellation.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-register-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-register");
        fixture.enable_cancellation_hooks(harness.socket());
        let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-register-seed", false);
        let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
        let operation_id = "ctxop-v23-c13-register";
        harness.arm("register_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&approval),
            None,
            None,
            false,
        );
        let cli_pid = child.id() as i32;
        let barrier = harness.wait_for(
            "register_start_synced",
            operation_id,
            Duration::from_secs(30),
        );
        // The outer Validating record already carries the exact nested IDs
        // bound before consume; the owner transaction is in flight.
        let bound = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            bound.iter().any(|record| {
                record["phase"] == "validating"
                    && record["nested_command_id"].as_str().is_some()
                    && record["nested_operation_id"].as_str().is_some()
            }),
            "Register owner must persist its nested IDs before the Start barrier: {bound:?}"
        );
        let nested_command_id = barrier
            .nested_command_id
            .clone()
            .expect("Start barrier identifies the exact Register command");
        let nested_operation_id = barrier
            .nested_operation_id
            .clone()
            .expect("Start barrier identifies the exact Register operation");
        let validating = bound
            .iter()
            .find(|record| record["phase"] == "validating")
            .expect("durable validating projection");
        assert_eq!(validating["nested_command_id"], nested_command_id);
        assert_eq!(validating["nested_operation_id"], nested_operation_id);
        assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
        barrier.release();
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        let payload = json_stdout(&output);
        assert_eq!(
            payload["result"]["outcome"], "completed",
            "cancellation after Register Start must not win: {payload}"
        );
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "{payload}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| record["phase"] != "cancelled"),
            "a started owner must never record a cancelled phase: {records:?}"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "admitted")
                .count(),
            1,
            "admission must be durable exactly once: {records:?}"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "the outer promotion must be appended exactly once: {records:?}"
        );
        let admission = records
            .iter()
            .find(|record| record["phase"] == "admitted")
            .expect("admitted outer record");
        assert!(
            admission["approval_evidence"].is_object(),
            "the durable admission must retain immutable approval evidence: {admission}"
        );
        let completed = records
            .iter()
            .find(|record| record["phase"] == "completed")
            .expect("completed outer record");
        assert_eq!(completed["nested_command_id"], nested_command_id);
        assert_eq!(completed["nested_operation_id"], nested_operation_id);
        assert_eq!(
            completed["business_receipts"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "approval_decision must be promoted only after Start sync: {completed}"
        );
        let new_binding = &payload["result"]["snapshot"]["binding"];
        let new_binding_id = new_binding["binding_id"]
            .as_str()
            .expect("recovery returns a binding id");
        let new_endpoint_generation = new_binding["endpoint_generation"]
            .as_u64()
            .expect("recovery returns an endpoint generation");
        assert_eq!(
            binding_event_count(
                &fixture.project_journal(),
                &fence.binding_id,
                fence.endpoint_generation
            ),
            1,
            "the incumbent must be retired exactly once"
        );
        assert_eq!(
            binding_event_count(
                &fixture.project_journal(),
                new_binding_id,
                new_endpoint_generation
            ),
            1,
            "the recovered binding must commit exactly once"
        );

        fixture.stop_daemon();
        fixture.restart_daemon();
        let journal_before = fs::read(fixture.host_journal()).expect("host journal before query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "completed",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "completed",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], nested_command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "{query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).expect("host journal after query"),
            journal_before,
            "restart query must not append to the operation journal"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).expect("project journal after query"),
            project_before,
            "restart query must not replay the Register owner"
        );
    }

    // 2) Register is proven committed, then the required grant owner fails
    //    before its Start. Preserve only the proven Register receipt prefix.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-partial-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-partial");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence =
            seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-partial-seed", true);
        let status = master_status(&fixture, &pane);
        let expected_grant = grant_fence(&status);
        let identity_approval =
            identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let grant_approval = grant_approval(&fixture, &pane, &identity_fence, &expected_grant);
        let operation_id = "ctxop-v23-c13-partial";
        harness.arm("before_grant_owner", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&identity_approval),
            Some(&grant_approval),
            None,
            false,
        );
        let barrier = harness.wait_for("before_grant_owner", operation_id, Duration::from_secs(30));
        let before_release = operation_records(&fixture.host_journal(), operation_id);
        assert_eq!(
            before_release
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "Register Start promotes the outer operation exactly once before later owners: {before_release:?}"
        );
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            0,
            "the grant owner has not started at this boundary"
        );
        barrier.respond("fail_owner");
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_ne!(output.status.code(), Some(0), "{}", text(&output));
        let payload = typed_failure(&output);
        let proven_receipts = json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "lease"
        ]);
        assert_eq!(payload["result"]["outcome"], "partial", "{payload}");
        assert_eq!(
            payload["result"]["committed_phases"], proven_receipts,
            "{payload}"
        );
        assert!(!payload["result"]["committed_phases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|receipt| receipt == "context_complete"));
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            0,
            "failed next owner must have no Start or effect"
        );
        assert_eq!(
            grant_replacement_completed_count(&fixture.project_journal(), operation_id),
            0,
            "failed next owner must have no completion receipt"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "{records:?}"
        );
        assert_eq!(
            records.last().unwrap()["business_receipts"],
            proven_receipts,
            "{records:?}"
        );
        assert_eq!(
            records.last().unwrap()["committed_phases"],
            json!([
                "validating",
                "inner_dispatched",
                "effect_observed",
                "partial"
            ]),
            "{records:?}"
        );
        fixture.stop_daemon();
        fixture.restart_daemon();
        let host_before =
            fs::read(fixture.host_journal()).expect("host journal before partial query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before partial query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "partial",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"], proven_receipts,
            "{query}"
        );
        assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
        assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_before);
    }

    // 3) Register Start syncs but outer promotion fails. No Register business
    //    event or owner completion may follow; query and restart stay read-only.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-promotion-fault"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-promotion-fault");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence =
            seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-promotion-seed", false);
        let approval = identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let operation_id = "ctxop-v23-c13-promotion-fault";
        harness.arm("register_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&approval),
            None,
            None,
            false,
        );
        let barrier = harness.wait_for(
            "register_start_synced",
            operation_id,
            Duration::from_secs(30),
        );
        let command_id = barrier
            .nested_command_id
            .clone()
            .expect("Register Start reports nested command id");
        let nested_operation_id = barrier
            .nested_operation_id
            .clone()
            .expect("Register Start reports nested operation id");
        barrier.respond("fail_promotion");
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_ne!(output.status.code(), Some(0), "{}", text(&output));
        let payload = typed_failure(&output);
        assert_eq!(payload["result"]["outcome"], "unknown", "{payload}");
        assert!(payload["result"]["phase"].is_null(), "{payload}");
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([]),
            "{payload}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| {
                !matches!(
                    record["phase"].as_str(),
                    Some(
                        "inner_dispatched"
                            | "effect_observed"
                            | "completed"
                            | "partial"
                            | "cancelled"
                    )
                )
            }),
            "promotion failure cannot claim owner work or cancellation: {records:?}"
        );
        let latest = records.last().expect("durable validating record");
        assert_eq!(latest["nested_command_id"], command_id);
        assert_eq!(latest["nested_operation_id"], nested_operation_id);
        let project_records = read_lines(&fixture.project_journal());
        assert_eq!(
            project_records
                .iter()
                .filter(|record| {
                    record["ev"] == "CommandStarted"
                        && record["command_id"] == command_id
                        && record["operation_id"] == nested_operation_id
                })
                .count(),
            1,
            "the exact nested Register Start must be observable: {project_records:?}"
        );
        assert_eq!(
            project_records
                .iter()
                .filter(|record| {
                    record["ev"] == "CommandCompleted"
                        && record["command_id"] == command_id
                        && record["operation_id"] == nested_operation_id
                })
                .count(),
            0,
            "promotion failure precedes Register business and completion"
        );
        let host_before =
            fs::read(fixture.host_journal()).expect("host journal before unknown query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before unknown query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "unknown",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "validating",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
        assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_before);
        fixture.stop_degraded_daemon();
        fixture.restart_daemon();
        let host_after_restart =
            fs::read(fixture.host_journal()).expect("host journal before restart query");
        let project_after_restart =
            fs::read(fixture.project_journal()).expect("project journal before restart query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "unknown",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "validating",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_receipt"]["state"], "unavailable",
            "restart query must retain the explicit Register owner readback failure: {query}"
        );
        assert!(
            query["result"]["queried_operation"]["nested_receipt"]["error"]
                .as_str()
                .is_some_and(|error| error.contains("PROJECT_OWNER_READBACK_UNAVAILABLE")),
            "restart query must explain why the nested receipt cannot be read: {query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).unwrap(),
            host_after_restart,
            "query must not repair the outer operation"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).unwrap(),
            project_after_restart,
            "query must not replay or repeat Register"
        );
        fixture.stop_degraded_daemon();
    }

    // 4) Grant owner already started: the grant Start is durable in the
    //    resident authority journal before the boundary is acknowledged, so
    //    cancellation is refused and the replacement completes exactly once.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-grant-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-grant");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-grant-seed", true);
        let status = master_status(&fixture, &pane);
        let expected_grant = grant_fence(&status);
        let identity_approval =
            identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let grant_approval = grant_approval(&fixture, &pane, &identity_fence, &expected_grant);
        let operation_id = "ctxop-v23-c13-grant";
        harness.arm("grant_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&identity_approval),
            Some(&grant_approval),
            None,
            false,
        );
        let cli_pid = child.id() as i32;
        let barrier = harness.wait_for("grant_start_synced", operation_id, Duration::from_secs(30));
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            1,
            "the grant owner must commit its durable Start before the barrier"
        );
        assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
        barrier.release();
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        let payload = json_stdout(&output);
        assert_eq!(
            payload["result"]["outcome"], "completed",
            "cancellation after the grant Start must not win: {payload}"
        );
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            1,
            "the grant Start must be durable exactly once"
        );
        assert_eq!(
            grant_replacement_completed_count(&fixture.project_journal(), operation_id),
            1,
            "the grant completion must be durable exactly once"
        );
        let grant_after = master_status(&fixture, &pane);
        assert_eq!(
            grant_after["grant_id"], expected_grant["grant_id"],
            "the replacement must retain the stable grant resource: {grant_after}"
        );
        assert_eq!(
            grant_after["grant_generation"],
            json!(
                expected_grant["generation"]
                    .as_u64()
                    .expect("grant generation")
                    + 1
            ),
            "the grant owner must advance the resource version exactly once: {grant_after}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| record["phase"] != "cancelled"),
            "a started grant owner must never record a cancelled phase: {records:?}"
        );

        fixture.stop_daemon();
        fixture.restart_daemon();
        let journal_before = fs::read(fixture.host_journal()).expect("host journal before query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "completed",
            "{query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).expect("host journal after query"),
            journal_before,
            "restart query must not append to the operation journal"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).expect("project journal after query"),
            project_before,
            "restart query must not replay the grant owner"
        );
    }
}

#[test]
fn c14_lost_first_response_queries_same_local_key_after_restart_without_remutation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-c14");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c14-seed", false);
    let operation_id = "ctxop-v23-c14-lost";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let mut first = spawn_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let lost = wait_child_output(&mut first, Duration::from_secs(30));
    let committed_payload = success_payload(&lost);
    let committed_binding = committed_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("lost-response recovery committed a binding")
        .to_owned();
    let committed_binding_generation = committed_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("lost-response recovery committed an endpoint generation");
    let journal_after_commit = fs::read(fixture.host_journal()).expect("host journal after commit");
    let proof_before_restart = fixture.proof(operation_id);
    let binding_events = binding_event_count(
        &fixture.project_journal(),
        &committed_binding,
        committed_binding_generation,
    );
    let operation_events = operation_records(&fixture.host_journal(), operation_id);

    fixture.restart_daemon();
    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after query"),
        journal_after_commit,
        "query after adapter restart must be a pure read"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &committed_binding,
            committed_binding_generation
        ),
        binding_events,
        "query must not repeat the committed binding effect"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        operation_events,
        "query must not append a second operation"
    );
    assert_eq!(
        fixture.proof(operation_id)["operation_id"],
        proof_before_restart["operation_id"],
        "the key must come from the retained local proof record, not a lost response"
    );
}

fn identity_fence_from_status(fixture: &Fixture, pane: &Pane) -> IdentityFence {
    let status = master_status(fixture, pane);
    IdentityFence {
        worker_id: status["worker_id"]
            .as_str()
            .expect("master status has worker id")
            .to_owned(),
        binding_id: status["binding_id"]
            .as_str()
            .expect("master status has binding id")
            .to_owned(),
        endpoint_generation: status["endpoint_generation"]
            .as_u64()
            .expect("master status has endpoint generation"),
        identity_path: PathBuf::new(),
        stale_identity_bytes: Vec::new(),
    }
}

fn grant_fence(status: &Value) -> Value {
    json!({
        "grant_id": status["grant_id"],
        "generation": status["grant_generation"]
    })
}

fn wait_child_output(child: &mut std::process::Child, timeout: Duration) -> std::process::Output {
    assert!(
        wait_until(timeout, || {
            child
                .try_wait()
                .expect("poll exact fixture CLI child")
                .is_some()
        }),
        "fixture CLI child did not exit within {timeout:?}"
    );
    let stdout = child
        .stdout
        .take()
        .expect("fixture CLI child stdout is piped");
    let stderr = child
        .stderr
        .take()
        .expect("fixture CLI child stderr is piped");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::BufReader::new(stdout), &mut stdout_bytes)
        .expect("read fixture CLI stdout");
    std::io::Read::read_to_end(&mut std::io::BufReader::new(stderr), &mut stderr_bytes)
        .expect("read fixture CLI stderr");
    let status = child.wait().expect("wait for exact fixture CLI child");
    std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    }
}

// C1: an unfinished task owned by the peer blocks Close; the typed refusal
// names the exact task id and no archive effect reaches the host.
